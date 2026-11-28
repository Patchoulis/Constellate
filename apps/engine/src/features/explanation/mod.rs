//! "Explain this": a passage, an equation or a caption of a paper explained by the model, every statement about
//! the paper resting on the paper's own words, and passages of nearby papers about the same thing found for free.
//!
//! | module            | what it does                                                                   |
//! |-------------------|--------------------------------------------------------------------------------|
//! | [`definitions`]   | where the paper introduces the symbols and terms a passage uses (no model)     |
//! | [`related`]       | passages of other papers sharing the passage's words (no model)                |
//! | this one          | the question asked about one passage, the answer's shape, and how it is checked |
//!
//! What is explained is a place in a paper (one passage, found by its address), never free text: the reader
//! resolves a selection to its passage through the engine's locate route first. One explanation is one call:
//! the passage, the passages around it in its section ([`CONTEXT_CHARS`]), the paper's title and abstract, and
//! the definitions [`definitions`] found. The answer is a list of statements, each of one [`Source`]:
//!
//! - [`Source::Paper`]: what the paper says. It carries quotes, checked with [`evidence::check`]; one whose
//!   quotes are not found in the paper is dropped and counted, never shown.
//! - [`Source::Background`]: general knowledge, shown as "background, not from this paper", with no link to it.
//!
//! The model is asked for the extracted text, not an image: the client ([`crate::model::language::Ask`]) sends
//! text only. An equation's or a table's text as a PDF gives it is often garbled, and the answer says so
//! ([`Question::garbled`]).
//!
//! What the model answered is kept ([`Library::keep`] under [`Feature::Explanation`]) by paper, passage address,
//! provider and model, prompt version and question, so reopening it is free; its quotes are checked again each time
//! it is shown. A redo forgets that one answer by its key ([`Library::forget_key`]: [`Library::forget`], which
//! clears by paper, would take every other explanation of the paper with it), so the next request asks again.
//!
//! # This folder
//!
//! Its routes are in [`routes`]; the parts of its logic are [`definitions`] and [`related`].

pub mod definitions;
pub mod related;
pub mod routes;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::evidence::{self, Claim, Evidence, PaperText, Papers, Rules};
use crate::jobs::Tracker;
use crate::library::{Feature, Library, LibraryError, Slot, Subject};
use crate::model::batch::{self, Asking, BatchEstimate, Failure, Item, Outcome, Stopped};
use crate::model::chosen::{self, Accepted, Chosen, Terms};
use crate::model::language::{Ask, Budget, Form, Limit};
use crate::text::pdf::passages::{Address, Kind, Passages};
use crate::text::{data_section, first_chars};
use definitions::Definition;

/// The version of the prompt and of the answer's shape: raise it when either changes, and kept explanations are
/// asked for again.
pub const PROMPT: u32 = 2;
/// Characters of the section around the passage sent with it: about 4,000 tokens at three characters a token.
pub const CONTEXT_CHARS: usize = 12_000;
/// Characters of the abstract sent.
const ABSTRACT_CHARS: usize = 3_000;
/// The most the answer may take, thinking included: a dozen statements with their quotes, and room to think.
pub const MAX_TOKENS: u32 = 4_000;
/// What an answer usually takes, for the estimate.
pub const LIKELY_TOKENS: u32 = 1_500;
/// Statements of an answer read; the rest are counted as left out.
pub const MOST_STATEMENTS: usize = 40;
/// The longest statement or symbol read, in characters.
const LONGEST_STATEMENT: usize = 1_500;
const LONGEST_SYMBOL: usize = 120;
/// Quotes of one statement looked for.
const MOST_QUOTES: usize = 6;
/// Papers searched for related passages: the paper's neighbours in the run's graph first, then its community.
pub const NEARBY_PAPERS: usize = 30;
/// What the ledger says the calls were for.
pub const PURPOSE: &str = "explaining a passage";

/// What is asked about a passage, from its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Question {
    /// A paragraph, a footnote, the text inside a figure.
    Passage,
    Equation,
    /// A table's caption: what it compares and how to read it.
    Table,
    /// A figure's caption: the figure itself is not sent.
    Figure,
}

impl Question {
    /// The question for a passage of `kind`; none for a title or a heading, which there is nothing to explain in.
    pub fn of(kind: Kind) -> Option<Question> {
        match kind {
            Kind::Text | Kind::Footnote | Kind::Fragment => Some(Question::Passage),
            Kind::Equation => Some(Question::Equation),
            Kind::TableCaption => Some(Question::Table),
            Kind::FigureCaption => Some(Question::Figure),
            Kind::Title | Kind::Heading => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Question::Passage => "passage",
            Question::Equation => "equation",
            Question::Table => "table",
            Question::Figure => "figure",
        }
    }

    /// May the passage's text as the PDF gives it be garbled? An equation's symbols and a table's or figure's
    /// layout rarely survive text extraction, and only the text is sent.
    pub fn garbled(kind: Kind) -> bool {
        matches!(kind, Kind::Equation | Kind::Fragment | Kind::TableCaption | Kind::FigureCaption)
    }

    fn task(self) -> &'static str {
        match self {
            Question::Passage => {
                "Explain what this passage says and why it matters in the paper (part \"explanation\"), then the \
                 ideas a reader needs to follow it (part \"prerequisite\")."
            }
            Question::Equation => {
                "Explain what this equation computes and its role in the paper (part \"explanation\"); then give \
                 each symbol in it with its meaning, one statement per symbol (part \"symbol\", the symbol in \
                 LaTeX in the field \"symbol\"); then the ideas a reader needs (part \"prerequisite\"). Prefer the \
                 paper's own definitions given below; a symbol the paper does not define is background."
            }
            Question::Table => {
                "This is a table's caption; the table's cells, if any, are among the nearby text as extracted, \
                 often garbled. Explain what the table compares and how to read it (part \"reading\"), what the \
                 paper concludes from it (part \"explanation\"), then the ideas a reader needs (part \
                 \"prerequisite\")."
            }
            Question::Figure => {
                "This is a figure's caption; the figure itself is not given. Explain what the figure shows and how \
                 to read it, as far as the caption and the text say (part \"reading\"), what the paper concludes \
                 from it (part \"explanation\"), then the ideas a reader needs (part \"prerequisite\")."
            }
        }
    }
}

/// Which part of an explanation a statement is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    /// What the passage says, in plain language.
    Explanation,
    /// One symbol of an equation and its meaning.
    Symbol,
    /// What a table or figure compares and how to read it.
    Reading,
    /// What a reader needs to know first.
    Prerequisite,
}

/// Where what a statement says comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The paper: shown only with its quotes found in the paper.
    Paper,
    /// General knowledge of the field: shown as background, never with a link to the paper.
    Background,
}

/// What the model answered, as it answered it: kept as it came, read and checked each time it is shown, so that a
/// statement of the wrong shape is one left out, not an answer lost.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Said {
    pub statements: Vec<Value>,
}

/// A statement as the model writes it (`statement` of a [`Claim`]).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct Written {
    part: Part,
    #[serde(default)]
    symbol: String,
    text: String,
    source: Source,
}

/// A statement read from the answer, with its place in it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Read {
    order: usize,
    part: Part,
    symbol: Option<String>,
    text: String,
}

/// A statement as the interface shows it. `ExplanationStatement` in the contracts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Shown {
    /// What the paper says, with the evidence found for it: never empty.
    Paper { part: Part, symbol: Option<String>, text: String, evidence: Vec<Evidence> },
    /// General knowledge, not from this paper: no evidence, by construction.
    Background { part: Part, symbol: Option<String>, text: String },
}

/// Statements of an answer not shown, and why: counted, never shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LeftOut {
    /// Said to be the paper's, but none of their quotes was found in it.
    pub not_found: usize,
    /// Not of the shape asked for (no text, an unknown part, a symbol with no name).
    pub malformed: usize,
    /// Past [`MOST_STATEMENTS`]: not read at all.
    pub past_bound: usize,
}

/// An explanation as shown: its statements in the model's order, and what was left out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Checked {
    pub statements: Vec<Shown>,
    pub left_out: LeftOut,
}

fn bounded(text: &str, most: usize) -> Option<String> {
    let text = text.trim();
    (!text.is_empty() && text.chars().nth(most).is_none()).then(|| text.to_owned())
}

/// One statement of the answer read and its shape checked: a paper's statement with its quotes, or background.
fn read_one(order: usize, value: &Value) -> Option<(Read, Source, Vec<evidence::Quote>)> {
    let Claim { statement, quotes } = serde_json::from_value::<Claim<Written>>(value.clone()).ok()?;
    let text = bounded(&statement.text, LONGEST_STATEMENT)?;
    let symbol = match statement.part {
        Part::Symbol => Some(bounded(&statement.symbol, LONGEST_SYMBOL)?),
        _ => None,
    };
    let quotes = quotes.into_iter().take(MOST_QUOTES).collect();
    Some((Read { order, part: statement.part, symbol, text }, statement.source, quotes))
}

/// The model's answer about `paper`, read and its quotes checked against the paper's passages: what is shown.
pub fn check(said: &Said, paper: &str, passages: &Passages) -> Checked {
    let mut left_out =
        LeftOut { past_bound: said.statements.len().saturating_sub(MOST_STATEMENTS), ..LeftOut::default() };
    let mut claims: Vec<Claim<Read>> = Vec::new();
    let mut background: Vec<Read> = Vec::new();
    for (order, value) in said.statements.iter().take(MOST_STATEMENTS).enumerate() {
        match read_one(order, value) {
            None => left_out.malformed += 1,
            Some((read, Source::Paper, quotes)) => claims.push(Claim { statement: read, quotes }),
            // background never carries a link to the paper, whatever quotes came with it
            Some((read, Source::Background, _)) => background.push(read),
        }
    }
    let papers = Papers::from([(paper.to_owned(), PaperText::Read(passages.clone()))]);
    let checked = evidence::check(claims, &papers, Rules::ONE_PAPER);
    left_out.not_found = checked.counts.dropped;
    let mut shown: Vec<(usize, Shown)> = checked
        .cited
        .into_iter()
        .map(|cited| {
            let Read { order, part, symbol, text } = cited.statement;
            (order, Shown::Paper { part, symbol, text, evidence: cited.evidence })
        })
        .chain(
            background
                .into_iter()
                .map(|Read { order, part, symbol, text }| (order, Shown::Background { part, symbol, text })),
        )
        .collect();
    shown.sort_by_key(|(order, _)| *order);
    Checked { statements: shown.into_iter().map(|(_, shown)| shown).collect(), left_out }
}

/// The JSON schema of the answer: statements, each a claim with its part, source and quotes.
pub fn schema() -> Value {
    let text = json!({"type": "string"});
    json!({
        "type": "object", "additionalProperties": false, "required": ["statements"],
        "properties": {"statements": {"type": "array", "items": {
            "type": "object", "additionalProperties": false, "required": ["statement", "quotes"],
            "properties": {
                "statement": {
                    "type": "object", "additionalProperties": false,
                    "required": ["part", "symbol", "text", "source"],
                    "properties": {
                        "part": {"type": "string", "enum": ["explanation", "symbol", "reading", "prerequisite"]},
                        "symbol": text,
                        "text": text,
                        "source": {"type": "string", "enum": ["paper", "background"]},
                    },
                },
                "quotes": {"type": "array", "items": {
                    "type": "object", "additionalProperties": false, "required": ["paper", "quote"],
                    "properties": {"paper": text, "quote": text},
                }},
            },
        }}},
    })
}

/// The instructions, the same for every explanation, whatever the model.
pub const SYSTEM: &str = concat!(
    "You explain one passage of a research paper to a reader who knows science but not this \
paper's field. Answer only with the JSON asked for: a list of statements, each one or two plain sentences, in the \
order a reader should read them.

Every statement has a source:
- \"paper\": it says what this paper says, does or means. It must carry at least one quote copied word for word \
from the paper's text given to you (twenty letters or more, no ellipsis, no paraphrase, no words added), with the \
paper's id as \"paper\". A statement about the paper whose quotes cannot be found in the paper is not shown.
- \"background\": general knowledge of the field that is not taken from this paper, such as what a standard \
method is or what a symbol usually means. Its \"quotes\" is an empty list. Never present the paper's own content \
as background, and never present background as the paper's.

Write mathematics in LaTeX between dollar signs, for example $\\alpha_t$. The field \"symbol\" is empty except in \
statements of part \"symbol\". The paper's text was extracted from its PDF, so equations and tables may be \
garbled in it: when you cannot read something, say so in a statement instead of guessing.

",
    crate::text::data_rule!()
);

/// One explanation's prompt: the paper's part (title, abstract, the section around the passage) and the question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub shared: String,
    pub question: String,
}

/// The passages of the target's section around it, in reading order, as many as fit in [`CONTEXT_CHARS`] taking
/// one before and one after in turn.
fn context(passages: &Passages, index: usize) -> Vec<usize> {
    let all = &passages.passages;
    let target = &all[index];
    let same = |at: usize| all[at].address.section == target.address.section && all[at].part == target.part;
    let length = |at: usize| all[at].text.chars().count();
    let mut room = CONTEXT_CHARS.saturating_sub(length(index));
    let (mut before, mut after) = (index, index);
    let mut open = (true, true);
    while open.0 || open.1 {
        if open.0 {
            match before.checked_sub(1).filter(|at| same(*at) && length(*at) <= room) {
                Some(at) => {
                    room -= length(at);
                    before = at;
                }
                None => open.0 = false,
            }
        }
        if open.1 {
            match Some(after + 1).filter(|at| *at < all.len() && same(*at) && length(*at) <= room) {
                Some(at) => {
                    room -= length(at);
                    after = at;
                }
                None => open.1 = false,
            }
        }
    }
    (before..=after).collect()
}

/// How the passage to explain is marked among those around it (inside a data section, so no `>>>` in it).
const MARK: &str = "[[THE PASSAGE TO EXPLAIN]]";

/// The prompt that explains passage `index` of `paper` (its title and abstract given), with the definitions found.
pub fn prompt(
    paper: &str,
    title: &str,
    abstract_text: &str,
    passages: &Passages,
    index: usize,
    found: &[Definition],
) -> Prompt {
    let target = &passages.passages[index];
    let question = Question::of(target.kind).unwrap_or(Question::Passage);
    let section = if target.address.section.is_empty() {
        "the paper's opening, before its first heading"
    } else {
        &target.address.section
    };
    // the paper's part only: the same for every passage of the paper, so that a provider's cache of it serves the
    // next passage explained; what depends on the passage (its section, the mark) is the question's
    let shared = data_section(
        "PAPER",
        &format!("Paper id: {paper}\nTitle: {title}\nAbstract: {}\n", first_chars(abstract_text, ABSTRACT_CHARS)),
    );
    let mut asked = format!(
        "The section \"{section}\" around the passage, as extracted from the PDF (page numbers in brackets; the \
         passage itself is marked {MARK}):\n"
    );
    let mut around = String::new();
    for at in context(passages, index) {
        let passage = &passages.passages[at];
        let marked = if at == index { format!(" {MARK}") } else { String::new() };
        around.push_str(&format!("\n[p. {}]{marked} {}\n", passage.address.page, passage.text));
    }
    asked.push_str(&data_section("SECTION", &around));
    asked.push_str(&format!(
        "\nThe passage to explain is a {} on page {} of paper {paper}:\n\n{}\n{}",
        match target.kind {
            Kind::Equation => "displayed equation",
            Kind::TableCaption => "table caption",
            Kind::FigureCaption => "figure caption",
            Kind::Footnote => "footnote",
            Kind::Fragment => "piece of text drawn inside a figure or table",
            Kind::Text | Kind::Title | Kind::Heading => "paragraph",
        },
        target.address.page,
        data_section("PASSAGE", &target.text),
        question.task(),
    ));
    if Question::garbled(target.kind) {
        asked.push_str(" Its text was extracted from the PDF and may be garbled.");
    }
    if around.contains(crate::text::pdf::UNREADABLE) {
        asked.push_str(&format!(
            " Each {} in the text stands for a formula that could not be read from the PDF: do not guess what it \
             says.",
            crate::text::pdf::UNREADABLE
        ));
    }
    if !found.is_empty() {
        asked.push_str(
            "\n\nWhere the paper introduces the symbols and terms the passage uses (found by a program; \
                        quote them when you use them):",
        );
        let mut defined = String::new();
        for definition in found {
            defined.push_str(&format!(
                "- {} (p. {}): {}\n",
                definition.term.text, definition.address.page, definition.sentence
            ));
        }
        asked.push('\n');
        asked.push_str(&data_section("DEFINITIONS", &defined));
    }
    Prompt { shared, question: asked }
}

/// What an explanation's answer depends on besides the paper and the model: the passage's place and the question.
pub fn question_key(address: &Address, question: Question) -> String {
    json!({"passage": address, "question": question.name()}).to_string()
}

/// Where the explanation of a passage of `paper` written by `chosen` is kept.
pub fn slot(paper: &str, chosen: &Chosen, address: &Address, question: Question) -> Slot {
    Slot {
        feature: Feature::Explanation,
        key: chosen.key(Feature::Explanation, PROMPT, &[paper], &question_key(address, question)),
        subject: Subject::Paper(paper.to_owned()),
    }
}

/// The one question of an explanation, as a batch of one.
pub fn item<'a>(slot: Slot, chosen: &'a Chosen, prompt: &'a Prompt, schema: &'a Value) -> Item<'a> {
    Item {
        slot,
        prompt: PROMPT,
        ask: Ask {
            purpose: PURPOSE,
            model: chosen.model(),
            system: SYSTEM,
            shared: Some(&prompt.shared),
            question: &prompt.question,
            form: Form::Json(schema),
            effort: chosen.effort(),
            max_tokens: MAX_TOKENS,
        },
        likely_tokens: LIKELY_TOKENS,
    }
}

/// What explaining would cost when asked of `chosen` (nothing when it is kept), before anything is sent.
pub fn estimate(library: &Library, item: &Item<'_>, chosen: &Chosen) -> Result<BatchEstimate, LibraryError> {
    batch::estimate_batch::<Said>(library, std::slice::from_ref(item), chosen)
}

/// The budget of one explanation: at most what the estimate said it could cost at worst, in dollars when the
/// model's price is known, in tokens when not ([`chosen::budget`] of the worst case shown).
pub fn budget(estimate: &BatchEstimate) -> Budget {
    // the worst case shown, accepted as shown, is never refused
    let accepted = estimate
        .cost
        .most_usd
        .map_or(Accepted::Tokens(estimate.cost.most_tokens), |most| Accepted::Dollars(most.dollars()));
    chosen::budget(&estimate.cost, accepted, Terms::AsShown).unwrap_or_else(|_| Budget::new(Limit::Tokens(0)))
}

/// Why an explanation was not written.
#[derive(Debug, thiserror::Error)]
pub enum NotExplained {
    #[error(transparent)]
    Failed(#[from] Failure),
    #[error(transparent)]
    Stopped(#[from] Stopped),
    #[error("asking gave no outcome for the explanation")]
    NoOutcome,
    #[error("the library could not be read: {0}")]
    Library(#[from] LibraryError),
}

/// Ask for one explanation (unless it is kept), keep it, and hand back what the model said.
pub fn explain(
    asking: Asking<'_>,
    item: &Item<'_>,
    budget: &Budget,
    tracker: &Tracker<'_>,
) -> Result<Said, NotExplained> {
    let outcome = batch::ask_all::<Said>(asking, std::slice::from_ref(item), budget, tracker)?.pop();
    match outcome {
        Some(Outcome::Kept(kept)) => Ok(kept.answer),
        Some(Outcome::Answered(answered)) => Ok(answered.answer),
        Some(Outcome::Failed(failure)) => Err(failure.into()),
        Some(Outcome::NotStarted(why)) => Err(why.into()),
        None => Err(NotExplained::NoOutcome),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::jobs::Cancel;
    use crate::library::Known;
    use crate::library::papers::tests::paper as record;
    use crate::model::language::fake::{Fake, error, message, ok};
    use crate::model::language::{Effort, Endpoint, KEY_VARIABLE, NoAnswer, Provider, Unavailable};
    use crate::text::pdf::passages::read_passages;
    use crate::usage::Call;

    pub(crate) const PAPER: &str = "2601.00001";

    /// A paper of two pages: an introduction, a method with an equation and the paragraph under it, a table's
    /// caption.
    pub(crate) fn passages() -> Passages {
        read_passages(&[
            "Spectral Merging\n\nAbstract\n\nWe study Low-Rank Adaptation (LoRA) for merging fine-tuned models into \
one model.\n\n1. Introduction\n\nModel merging adds the weight updates of several fine-tuned models. Let α be the \
merging coefficient used for every task.\n"
                .to_owned(),
            "2. Method\n\nThe merged update is the sum of the task updates, scaled by the merging coefficient:\n\n\
∆W = α ∑ i ∆Wi (1)\n\nwhere ∆Wi is the update of task i. We rescale the inflated singular values of the merged \
update.\n\nTable 1. Accuracy of merged models on eight vision tasks, with and without rescaling.\n"
                .to_owned(),
        ])
    }

    pub(crate) fn index_of(passages: &Passages, start: &str) -> usize {
        passages.passages.iter().position(|passage| passage.text.starts_with(start)).unwrap()
    }

    fn statement(part: &str, symbol: &str, text: &str, source: &str, quotes: &[(&str, &str)]) -> Value {
        json!({
            "statement": {"part": part, "symbol": symbol, "text": text, "source": source},
            "quotes": quotes.iter().map(|(paper, quote)| json!({"paper": paper, "quote": quote})).collect::<Vec<_>>(),
        })
    }

    /// An answer about the equation: what it does (quoted), a symbol (quoted), a symbol as background, a
    /// fabricated quote, a quote of another paper, and two of the wrong shape.
    pub(crate) fn answer() -> Said {
        Said {
            statements: vec![
                statement(
                    "explanation",
                    "",
                    "The equation adds up the task updates, scaled by $\\alpha$.",
                    "paper",
                    &[(PAPER, "The merged update is the sum of the task updates")],
                ),
                statement(
                    "symbol",
                    "\\Delta W_i",
                    "The update of task $i$.",
                    "paper",
                    &[(PAPER, "where ∆Wi is the update of task i")],
                ),
                statement("symbol", "\\sum", "A sum over the tasks.", "background", &[]),
                statement(
                    "explanation",
                    "",
                    "Merging always improves accuracy.",
                    "paper",
                    &[(PAPER, "merging always improves accuracy on every task by ten points")],
                ),
                statement(
                    "prerequisite",
                    "",
                    "Distillation needs no labels.",
                    "paper",
                    &[("2601.99999", "we rescale the inflated singular values of the merged update")],
                ),
                statement(
                    "symbol",
                    "",
                    "A symbol without its name.",
                    "paper",
                    &[(PAPER, "The merged update is the sum")],
                ),
                json!({"statement": {"part": "opinion", "text": "x", "source": "paper"}, "quotes": []}),
                statement(
                    "prerequisite",
                    "",
                    "Fine-tuning adapts a trained model to one task.",
                    "background",
                    &[(PAPER, "adds the weight updates of several fine-tuned models")],
                ),
            ],
        }
    }

    #[test]
    fn the_prompt_sends_the_passage_its_section_the_abstract_and_the_definitions_and_says_what_to_do() {
        let passages = passages();
        let equation = index_of(&passages, "∆W =");
        let found = definitions::definitions(&passages, equation);
        let prompt = prompt(PAPER, "Spectral Merging", "We study merging.", &passages, equation, &found);
        assert_eq!(
            prompt.shared,
            "<<<BEGIN PAPER>>>\nPaper id: 2601.00001\nTitle: Spectral Merging\nAbstract: We study merging.\n<<<END PAPER>>>\n"
        );
        assert!(SYSTEM.ends_with(crate::text::data_rule!()), "the system prompt says the sections are data");
        assert!(prompt.question.contains("<<<BEGIN SECTION>>>\n") && prompt.question.contains("<<<BEGIN PASSAGE>>>\n"));
        assert!(prompt.question.contains("<<<BEGIN DEFINITIONS>>>\n- α (p. 1)"));
        assert!(prompt.question.starts_with("The section \"2. Method\" around the passage"));
        assert!(prompt.question.contains("[p. 2] The merged update is the sum of the task updates"));
        assert!(prompt.question.contains("[p. 2] [[THE PASSAGE TO EXPLAIN]] ∆W = α ∑ i ∆Wi (1)"));
        assert!(prompt.question.contains("[p. 2] where ∆Wi is the update of task i."));
        assert!(!prompt.question.contains("[p. 1]"), "another section is not sent");
        assert!(
            prompt.question.contains("\nThe passage to explain is a displayed equation on page 2 of paper 2601.00001")
        );
        assert!(prompt.question.contains("part \"symbol\""));
        assert!(prompt.question.contains("may be garbled"));
        assert!(prompt.question.contains("\n- α (p. 1): Let α be the merging coefficient used for every task."));
        assert!(prompt.question.contains("\n- ∆Wi (p. 2): where ∆Wi is the update of task i."));
        // a paragraph is asked as a paragraph, with nothing said of garbling
        let paragraph = index_of(&passages, "Model merging adds");
        let asked = super::prompt(PAPER, "t", "a", &passages, paragraph, &[]);
        assert!(asked.question.contains("a paragraph on page 1") && !asked.question.contains("garbled"));
        assert!(asked.question.contains("part \"prerequisite\"") && !asked.question.contains("Where the paper"));
    }

    #[test]
    fn every_passage_of_a_paper_is_asked_with_the_same_system_and_shared_text_and_its_own_question() {
        let passages = passages();
        let schema = schema();
        let chosen = Chosen::claude("claude-opus-5-5");
        let asked: Vec<Prompt> = ["∆W =", "Model merging adds", "Table 1."]
            .into_iter()
            .map(|start| {
                let index = index_of(&passages, start);
                prompt(
                    PAPER,
                    "Spectral Merging",
                    "We study merging.",
                    &passages,
                    index,
                    &definitions::definitions(&passages, index),
                )
            })
            .collect();
        let items: Vec<Item<'_>> = asked
            .iter()
            .zip([Question::Equation, Question::Passage, Question::Table])
            .map(|(prompt, question)| {
                let address = &passages.passages[0].address;
                item(slot(PAPER, &chosen, address, question), &chosen, prompt, &schema)
            })
            .collect();
        for pair in items.windows(2) {
            assert_eq!(pair[0].ask.system, pair[1].ask.system);
            assert_eq!(pair[0].ask.shared, pair[1].ask.shared, "byte for byte: a provider's cache of it serves both");
            assert_ne!(pair[0].ask.question, pair[1].ask.question);
        }
    }

    #[test]
    fn the_section_sent_is_bounded_and_taken_from_both_sides() {
        let long = "A paragraph of the method that goes on for a while to fill the room. ".repeat(20);
        let page: String = (0..40).map(|n| format!("{long}{n}.\n\n")).collect();
        let passages = read_passages(&[format!("2. Method\n\n{page}")]);
        let middle = passages.passages.len() / 2;
        let taken = context(&passages, middle);
        let length: usize = taken.iter().map(|at| passages.passages[*at].text.chars().count()).sum();
        assert!(length <= CONTEXT_CHARS, "{length}");
        assert!(taken.contains(&middle) && taken.first() < Some(&middle) && taken.last() > Some(&middle));
        assert!(taken.windows(2).all(|pair| pair[1] == pair[0] + 1), "in reading order, with no gaps");
    }

    #[test]
    fn the_schema_asks_for_claims_with_a_part_a_source_and_quotes() {
        let schema = schema();
        let statement = &schema["properties"]["statements"]["items"];
        assert_eq!(statement["required"], json!(["statement", "quotes"]));
        assert_eq!(
            statement["properties"]["statement"]["properties"]["source"]["enum"],
            json!(["paper", "background"])
        );
        assert_eq!(statement["properties"]["quotes"]["items"]["required"], json!(["paper", "quote"]));
        // every object closed, as structured outputs ask
        assert_eq!(statement["properties"]["statement"]["additionalProperties"], false);
        assert!(!SYSTEM.contains("Claude"), "the prompt names no vendor");
    }

    #[test]
    fn only_statements_whose_quotes_are_found_are_shown_as_the_papers_and_the_rest_are_counted() {
        let passages = passages();
        let checked = check(&answer(), PAPER, &passages);
        assert_eq!(
            checked.left_out,
            LeftOut { not_found: 2, malformed: 2, past_bound: 0 },
            "fabricated and other paper; two shapes"
        );
        let texts: Vec<&str> = checked
            .statements
            .iter()
            .map(|shown| match shown {
                Shown::Paper { text, .. } | Shown::Background { text, .. } => text.as_str(),
            })
            .collect();
        assert_eq!(
            texts,
            [
                "The equation adds up the task updates, scaled by $\\alpha$.",
                "The update of task $i$.",
                "A sum over the tasks.",
                "Fine-tuning adapts a trained model to one task.",
            ],
            "in the model's order"
        );
        let Shown::Paper { evidence, symbol, part, .. } = &checked.statements[1] else { panic!() };
        assert_eq!((part, symbol.as_deref()), (&Part::Symbol, Some("\\Delta W_i")));
        assert_eq!(evidence[0].quote(), "where ∆Wi is the update of task i");
        assert_eq!(evidence[0].address().unwrap().page, 2);
        // background is shown as background, and never with a link, even when quotes came with it
        assert!(matches!(&checked.statements[3], Shown::Background { part: Part::Prerequisite, .. }));
        let told = serde_json::to_value(&checked).unwrap();
        assert_eq!(told["statements"][3]["source"], "background");
        assert!(told["statements"][3].get("evidence").is_none());
        assert_eq!(told["statements"][0]["evidence"][0]["established"]["kind"], "verified");
        assert_eq!(told["left_out"], json!({"not_found": 2, "malformed": 2, "past_bound": 0}));
    }

    #[test]
    fn an_answer_with_too_many_statements_is_read_up_to_the_bound() {
        let one = statement("prerequisite", "", "Background.", "background", &[]);
        let checked = check(&Said { statements: vec![one; MOST_STATEMENTS + 5] }, PAPER, &passages());
        assert_eq!((checked.statements.len(), checked.left_out.past_bound), (MOST_STATEMENTS, 5));
    }

    fn endpoint() -> Endpoint {
        Endpoint::from_lookup(|name| (name == KEY_VARIABLE).then(|| "test-key-not-real".to_owned())).unwrap()
    }

    fn library() -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        library.add_papers(&[record(PAPER)], None, Known::Keep).unwrap();
        (folder, library)
    }

    struct Asked {
        said: Result<Said, NotExplained>,
        calls: Vec<Call>,
    }

    fn ask(library: &Library, transport: &Fake, item: &Item<'_>, budget: &Budget) -> Asked {
        let calls: Mutex<Vec<Call>> = Mutex::new(Vec::new());
        let cancel = Cancel::new();
        let (log, progress) = (|_: &str| {}, |_| {});
        let tracker = Tracker::new(&log, &progress, &cancel);
        let asking = Asking { library, transport, endpoint: endpoint(), recorder: &calls, in_flight: 1 };
        let said = explain(asking, item, budget, &tracker);
        Asked { said, calls: calls.into_inner().unwrap() }
    }

    fn answered(said: &Said) -> Result<crate::model::language::Reply, crate::model::language::TransportError> {
        let usage = json!({"input_tokens": 2_000, "output_tokens": 600});
        Ok(ok(&message("claude-opus-5-5", &serde_json::to_string(said).unwrap(), "end_turn", usage)))
    }

    #[test]
    fn an_explanation_is_asked_once_kept_and_not_asked_again_and_the_estimate_sends_nothing() {
        let (_folder, library) = library();
        let passages = passages();
        let equation = index_of(&passages, "∆W =");
        let prompt = prompt(PAPER, "Spectral Merging", "We study merging.", &passages, equation, &[]);
        let schema = schema();
        let model = "claude-opus-5-5";
        let chosen = Chosen::claude(model);
        let address = &passages.passages[equation].address;
        let item = item(slot(PAPER, &chosen, address, Question::Equation), &chosen, &prompt, &schema);

        let transport = Fake::new(vec![answered(&answer())]);
        let before = estimate(&library, &item, &chosen).unwrap();
        assert!(transport.sent().is_empty(), "the estimate sends nothing");
        assert_eq!((before.to_ask, before.kept), (1, 0));
        assert!(before.cost.usd.unwrap() > crate::usage::Usd::ZERO && before.cost.usd <= before.cost.most_usd);
        assert!(crate::model::language::strict_ready(&schema), "every object closed and every property required");

        let budget = budget(&before);
        let first = ask(&library, &transport, &item, &budget);
        assert_eq!(first.said.unwrap(), answer());
        assert_eq!(first.calls.len(), 1, "recorded in the ledger");
        assert_eq!(first.calls[0].purpose, PURPOSE);
        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        let body = &sent[0].body;
        assert_eq!(body["model"], model, "the model chosen, not one written in the request");
        assert_eq!(body["system"][0]["text"], SYSTEM);
        assert_eq!(body["messages"][0]["content"][0]["text"], prompt.shared);
        assert_eq!(body["messages"][0]["content"][1]["text"], prompt.question);
        assert_eq!(body["output_config"]["format"]["schema"], schema);
        assert_eq!(body["max_tokens"], MAX_TOKENS);

        let after = estimate(&library, &item, &chosen).unwrap();
        assert_eq!((after.to_ask, after.kept, after.cost.usd), (0, 1, Some(crate::usage::Usd::ZERO)));
        let again = ask(&library, &transport, &item, &budget);
        assert_eq!(again.said.unwrap(), answer());
        assert_eq!(transport.sent().len(), 1, "kept: not asked again");

        // a redo forgets that one answer: the next request asks again
        assert!(library.forget_key(Feature::Explanation, &item.slot.key).unwrap());
        assert_eq!(estimate(&library, &item, &chosen).unwrap().to_ask, 1);
        assert!(library.kept::<Said>(Feature::Explanation, &item.slot.key).unwrap().is_none());
    }

    #[test]
    fn a_malformed_answer_is_a_failure_of_its_own_and_nothing_is_kept() {
        let (_folder, library) = library();
        let passages = passages();
        let prompt = prompt(PAPER, "t", "a", &passages, 2, &[]);
        let schema = schema();
        let chosen = Chosen::claude("claude-opus-5-5");
        let item =
            item(slot(PAPER, &chosen, &passages.passages[2].address, Question::Passage), &chosen, &prompt, &schema);
        let usage = json!({"input_tokens": 10, "output_tokens": 10});
        let transport = Fake::new(vec![Ok(ok(&message("claude-opus-5-5", "not json at all", "end_turn", usage)))]);
        let budget = budget(&estimate(&library, &item, &chosen).unwrap());
        let asked = ask(&library, &transport, &item, &budget);
        assert!(matches!(asked.said, Err(NotExplained::Failed(Failure::NoAnswer(NoAnswer::NotJson(_))))));
        assert_eq!(asked.calls.len(), 1, "paid for, so recorded");
        assert!(library.kept::<Said>(Feature::Explanation, &item.slot.key).unwrap().is_none());
        // a server failure is told as one, and nothing is kept either
        let failing = Fake::new(vec![
            Ok(error(500, "api_error", "x")),
            Ok(error(500, "api_error", "x")),
            Ok(error(500, "api_error", "x")),
        ]);
        // the budget of one explanation was spent on the first ask: a second is refused, unsent
        assert!(matches!(
            ask(&library, &failing, &item, &budget).said,
            Err(NotExplained::Stopped(Stopped::OverBudget(_)))
        ));
        assert!(failing.sent().is_empty());
        let fresh = super::budget(&estimate(&library, &item, &chosen).unwrap());
        assert!(matches!(ask(&library, &failing, &item, &fresh).said, Err(NotExplained::Failed(Failure::NoAnswer(_)))));
        assert!(library.kept::<Said>(Feature::Explanation, &item.slot.key).unwrap().is_none());
    }

    #[test]
    fn a_budget_too_small_for_the_worst_case_sends_nothing() {
        let (_folder, library) = library();
        let passages = passages();
        let prompt = prompt(PAPER, "t", "a", &passages, 2, &[]);
        let schema = schema();
        let chosen = Chosen::claude("claude-opus-5-5");
        let item =
            item(slot(PAPER, &chosen, &passages.passages[2].address, Question::Passage), &chosen, &prompt, &schema);
        let transport = Fake::new(vec![answered(&answer())]);
        let asked = ask(&library, &transport, &item, &Budget::new(Limit::Tokens(100)));
        assert!(matches!(asked.said, Err(NotExplained::Stopped(Stopped::OverBudget(_)))));
        assert!(transport.sent().is_empty());
        // the budget of an explanation is what its estimate said at most
        let estimate = estimate(&library, &item, &chosen).unwrap();
        let budget = budget(&estimate);
        assert!(ask(&library, &Fake::new(vec![answered(&answer())]), &item, &budget).said.is_ok());
    }

    #[test]
    fn the_question_follows_the_passages_kind_and_headings_are_not_explained() {
        assert_eq!(Question::of(Kind::Equation), Some(Question::Equation));
        assert_eq!(Question::of(Kind::TableCaption), Some(Question::Table));
        assert_eq!(Question::of(Kind::Heading), None);
        assert!(Question::garbled(Kind::Equation) && !Question::garbled(Kind::Text));
        let address = passages().passages[2].address.clone();
        let (m, other) = (Chosen::claude("m"), Chosen::claude("other model"));
        let one = slot(PAPER, &m, &address, Question::Passage);
        assert_ne!(one.key, slot(PAPER, &other, &address, Question::Passage).key);
        let elsewhere = Chosen::of(Provider::OpenRouter, "m", Effort::Medium, Err(Unavailable::NoKey));
        assert_ne!(one.key, slot(PAPER, &elsewhere, &address, Question::Passage).key, "another provider's m");
        assert_ne!(
            one.key,
            slot(PAPER, &m, &Address { start: address.start + 1, ..address.clone() }, Question::Passage).key
        );
        assert_ne!(one.key, slot(PAPER, &m, &address, Question::Equation).key);
        assert_eq!(one.subject, Subject::Paper(PAPER.to_owned()));
    }
}
