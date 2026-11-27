//! A question asked of a run or of one of its communities, answered by the model in statements every one of which
//! is backed by quotes found in the papers (docs/evidence.md).
//!
//! ```text
//! question ──retrieval (text/retrieval.rs, no model)──> the best passages, with their addresses
//!          ──prepare──> one Ask: the passages labelled by paper and place (at most PROMPT_TOKENS), the question
//!          ──estimate_batch──> what it costs, shown before anything is sent
//!          ──ask_all (one item, budgeted, kept)──> Said: statements with quotes, or "the papers do not say"
//!          ──checked──> Answer: the statements whose quotes were found, the count of those left out
//! ```
//!
//! # The rules an answer is shown by
//!
//! * Every statement says whether it is about **one paper** or about **several**. One about one paper is shown
//!   when a quote of it was found in the paper it names ([`Rules::ONE_PAPER`]); one about several papers only when
//!   quotes of at least [`SHARED_BY`] distinct papers were found ([`Rules::pattern`]), so that "papers agree that"
//!   cannot rest on one paper. Two, not the overview's three (docs/evidence.md): an answer is given a few dozen
//!   passages, not a community's every paper, and "two papers find" is already a claim about more than one. The
//!   papers a quote may name are those whose passages were in the prompt.
//! * A statement whose quotes were not found is left out and counted, as is one that is not of the shape asked
//!   for (each statement is read on its own: one malformed statement fails only itself).
//! * If fewer than half of the statements written are left ([`shown_enough`]), the answer is not shown at all:
//!   it is [`Outcome::Unverified`], and the interface shows the passages found instead of a thin answer.
//! * The model is told to answer only from the passages, and to say so when they do not answer the question:
//!   [`Outcome::NotInPapers`] is an answer of its own, shown as such ("the papers in scope do not say").
//!
//! # Follow-up questions
//!
//! Each question stands alone: it is answered from the passages retrieved for it. Carrying a conversation's
//! earlier passages or answers into the next prompt would let a statement rest on what the model said before
//! rather than on a quote, so it is not done.
//!
//! # What it costs
//!
//! One call: the instructions (about 400 tokens), the passages (at most [`PROMPT_TOKENS`], 24,000 tokens at the
//! assumed 3 characters a token), the question, and an answer of at most [`MAX_TOKENS`] (likely
//! [`LIKELY_TOKENS`]). With the default model that is about USD 0.15 likely and USD 0.25 at most, whatever the
//! number of papers in scope, since the passages are capped (an estimate, shown as one).

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::batch::{self, Asking, BatchEstimate, Failure, Item, Outcome as Asked, Stopped};
use super::chosen::Chosen;
use super::language::{Ask, Budget, CHARS_PER_TOKEN, Form, Provider};
use crate::evidence::{self, Cited, Claim, Established, Evidence, PaperText, Papers, Rules};
use crate::jobs::Tracker;
use crate::library::{AnswerKey, Feature, Library, LibraryError, Slot, Subject};
use crate::text::retrieval::{Hit, Unit};

/// The version of the prompt and the answer's shape: raised when either changes, so kept answers are asked again.
pub const PROMPT: u32 = 1;
/// The most the passages may take in the prompt, in tokens reckoned at [`CHARS_PER_TOKEN`].
pub const PROMPT_TOKENS: u64 = 24_000;
/// The most of one passage put in the prompt, in characters: a long one is cut (its quotes are still checked
/// against the whole paper).
pub const LONGEST_PASSAGE: usize = 4_000;
/// The longest question, in characters.
pub const LONGEST_QUESTION: usize = 1_000;
/// The most the answer may take, thinking included.
pub const MAX_TOKENS: u32 = 4_000;
/// What an answer is likely to take, for the estimate.
pub const LIKELY_TOKENS: u32 = 1_500;
/// Distinct papers whose quotes a statement about several papers needs.
pub const SHARED_BY: usize = 2;
/// What the ledger calls these calls.
pub const PURPOSE: &str = "questions about papers";

/// The instructions. They name no vendor's features: any model that can answer in JSON can follow them.
pub const SYSTEM: &str = "You answer a researcher's question from passages of research papers, and only from them.

You are given numbered passages. Each is labelled with the id of the paper it is from and where it is in that paper.

Rules:
- Use only what the passages say. Do not use anything you know from elsewhere, and do not guess.
- Answer as a list of short statements. Each statement says one thing.
- Support every statement with one or more quotes. A quote is copied word for word from one passage (at least five words, at most two sentences) and names the id of the paper that passage is from. Never change, join or shorten the words of a quote.
- Mark a statement \"one_paper\" when it reports what one paper says, and \"several_papers\" when it says that more than one paper does something or agrees on something; a \"several_papers\" statement needs quotes from at least two different papers.
- If the passages do not answer the question, set outcome to \"not_in_papers\" and give no statements. Do not answer a different question instead.
- Write the statements in plain English, without the passage numbers.";

/// What a statement is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum About {
    /// What one paper says.
    OnePaper,
    /// What several papers share.
    SeveralPapers,
}

/// One statement of an answer: `AnswerStatement` in the contracts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    pub sentence: String,
    pub about: About,
}

/// Whether the model answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaidOutcome {
    Answered,
    /// The passages do not answer the question.
    NotInPapers,
}

/// The model's answer as kept: each statement as it came (each read on its own, see the module).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Said {
    pub outcome: SaidOutcome,
    pub statements: Vec<Value>,
}

/// The JSON schema of [`Said`].
pub fn schema() -> Value {
    let strict = |properties: Value, required: &[&str]| json!({"type": "object", "additionalProperties": false, "required": required, "properties": properties});
    let quote = strict(json!({"paper": {"type": "string"}, "quote": {"type": "string"}}), &["paper", "quote"]);
    let statement = strict(
        json!({"sentence": {"type": "string"}, "about": {"type": "string", "enum": ["one_paper", "several_papers"]}}),
        &["sentence", "about"],
    );
    let claim =
        strict(json!({"statement": statement, "quotes": {"type": "array", "items": quote}}), &["statement", "quotes"]);
    strict(
        json!({
            "outcome": {"type": "string", "enum": ["answered", "not_in_papers"]},
            "statements": {"type": "array", "items": claim}
        }),
        &["outcome", "statements"],
    )
}

/// What a question is asked of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub run: String,
    /// A community's tree path; none for the whole run.
    pub community: Option<String>,
}

impl Scope {
    /// What a kept answer is about.
    pub fn subject(&self) -> Subject {
        match &self.community {
            Some(community) => Subject::Community { run: self.run.clone(), community: community.clone() },
            None => Subject::Run(self.run.clone()),
        }
    }
}

/// A question ready to be estimated and asked: the prompt, and what its answer is kept under.
#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    /// The model it is asked of, of its provider.
    pub chosen: Chosen,
    pub question: String,
    /// The passages in the prompt, best first (those that did not fit are left out).
    pub given: Vec<Hit>,
    /// The papers they are from, in byte order: the only papers a quote may name.
    pub papers: Vec<String>,
    pub key: AnswerKey,
    pub subject: Subject,
    shared: String,
    asked: String,
    schema: Value,
}

/// The text of one hit for the prompt, if the paper's text is the one the index was built from.
fn passage_text(hit: &Hit, texts: &Papers) -> Option<(String, String)> {
    match (hit.unit, texts.get(&hit.paper)?) {
        (Unit::Passage { index }, PaperText::Read(passages)) => {
            let passage = passages.passages.get(usize::try_from(index).ok()?)?;
            let address = &passage.address;
            let section =
                if address.section.is_empty() { String::new() } else { format!(", section \"{}\"", address.section) };
            Some((format!("paper {}, page {}{section}", hit.paper, address.page), passage.text.clone()))
        }
        (Unit::Record, PaperText::Unread { title, abstract_text }) => Some((
            format!("paper {}, its title and abstract (its full text has not been read)", hit.paper),
            format!("{title}\n{abstract_text}"),
        )),
        _ => None,
    }
}

/// The first `most` characters of `text`, at a word's end when there is one near.
fn cut(text: &str, most: usize) -> String {
    match text.char_indices().nth(most) {
        None => text.to_owned(),
        Some((end, _)) => {
            let kept = &text[..end];
            let at_word = kept.rfind(char::is_whitespace).filter(|space| *space > end / 2).unwrap_or(end);
            format!("{} [...]", &kept[..at_word])
        }
    }
}

/// The key the answer to `question` about the passages `given` is kept under, and the papers they are from (in
/// byte order). A question kept with an older [`PROMPT`] gets another key: it is not found, and is asked again.
pub fn key_of(
    provider: Provider,
    model: &str,
    scope: &Scope,
    question: &str,
    given: &[Hit],
) -> (AnswerKey, Vec<String>) {
    let papers: BTreeSet<&str> = given.iter().map(|hit| hit.paper.as_str()).collect();
    let papers: Vec<String> = papers.into_iter().map(str::to_owned).collect();
    let ids: Vec<&str> = papers.iter().map(String::as_str).collect();
    // everything the answer depends on besides the papers: the question, what it is asked of, which passages
    let units: Vec<(&str, Unit)> = given.iter().map(|hit| (hit.paper.as_str(), hit.unit)).collect();
    let depends = json!({"question": question, "scope": scope, "passages": units}).to_string();
    (AnswerKey::new(Feature::Answers, provider, model, PROMPT, &ids, &depends), papers)
}

/// The question about `hits` (best first), asked of `chosen`: the passages that fit in [`PROMPT_TOKENS`], labelled
/// with their paper and place, in the order found; `texts` holds the papers' texts (as the library hands them).
pub fn prepare(chosen: &Chosen, scope: &Scope, question: &str, hits: &[Hit], texts: &Papers) -> Prepared {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    let room = (PROMPT_TOKENS as f64 * CHARS_PER_TOKEN) as usize;
    let mut shared = String::from("Passages from the papers in scope:\n");
    let mut given = Vec::new();
    for hit in hits {
        let Some((label, text)) = passage_text(hit, texts) else { continue };
        let block = format!("\n[{}] {label}\n{}\n", given.len() + 1, cut(text.trim(), LONGEST_PASSAGE));
        if shared.chars().count() + block.chars().count() > room {
            continue;
        }
        shared.push_str(&block);
        given.push(hit.clone());
    }
    let (key, papers) = key_of(chosen.provider(), chosen.model(), scope, question, &given);
    Prepared {
        chosen: chosen.clone(),
        question: question.to_owned(),
        key,
        subject: scope.subject(),
        papers,
        given,
        shared,
        asked: format!("Question: {question}"),
        schema: schema(),
    }
}

impl Prepared {
    /// The one item of the batch it is asked as.
    pub fn item(&self) -> Item<'_> {
        Item {
            slot: Slot { feature: Feature::Answers, key: self.key.clone(), subject: self.subject.clone() },
            prompt: PROMPT,
            ask: Ask {
                purpose: PURPOSE,
                model: self.chosen.model(),
                system: SYSTEM,
                shared: Some(&self.shared),
                question: &self.asked,
                form: Form::Json(&self.schema),
                effort: self.chosen.effort(),
                max_tokens: MAX_TOKENS,
            },
            likely_tokens: LIKELY_TOKENS,
        }
    }

    /// What asking it would cost (nothing when its answer is kept). Nothing is sent.
    pub fn estimate(&self, library: &Library) -> Result<BatchEstimate, LibraryError> {
        batch::estimate_batch::<Said>(library, &[self.item()], &self.chosen)
    }

    /// The prompt's passages, as sent: for the tests.
    pub fn shared(&self) -> &str {
        &self.shared
    }
}

/// Why a question has no answer.
#[derive(Debug, thiserror::Error)]
pub enum NotAnswered {
    #[error(transparent)]
    Library(#[from] LibraryError),
    #[error(transparent)]
    Failed(Failure),
    #[error(transparent)]
    Stopped(Stopped),
}

/// Ask it (one call, within `budget`, its answer kept as it arrives), or hand back the answer kept before.
pub fn ask(
    asking: Asking<'_>,
    prepared: &Prepared,
    budget: &Budget,
    tracker: &Tracker<'_>,
) -> Result<Said, NotAnswered> {
    let outcome = batch::ask_all::<Said>(asking, &[prepared.item()], budget, tracker)?.pop();
    match outcome {
        Some(Asked::Kept(kept)) => Ok(kept.answer),
        Some(Asked::Answered(answered)) => Ok(answered.answer),
        Some(Asked::Failed(why)) => Err(NotAnswered::Failed(why)),
        Some(Asked::NotStarted(why)) => Err(NotAnswered::Stopped(why)),
        None => Err(NotAnswered::Stopped(Stopped::Cancelled)),
    }
}

/// What came of an answer once checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Its statements are shown, each with its evidence.
    Answered,
    /// The model said the passages do not answer the question.
    NotInPapers,
    /// Too few of its statements had their quotes found: nothing of it is shown.
    Unverified,
}

/// How many statements were written, shown and left out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AnswerCounts {
    /// Statements the model wrote, well formed or not.
    pub written: usize,
    /// Whose quotes were found: shown when the answer is, none of them when it is [`Outcome::Unverified`].
    pub shown: usize,
    /// Left out because their quotes were not found in the papers they name (or too few papers for a statement
    /// about several).
    pub not_found: usize,
    /// Left out because they were not of the shape asked for.
    pub malformed: usize,
}

/// An answer, checked: what the route shows. Each outcome holds only what it has, so that no statement of an
/// unverified answer, and no count of one that says the papers do not say, can be shown.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// Its statements whose quotes were found: at least one, and at least half of those written
    /// ([`shown_enough`]).
    Answered { statements: Vec<Cited<Statement>>, counts: AnswerCounts },
    /// The model said the passages do not answer the question.
    NotInPapers,
    /// Too few of its statements had their quotes found: none is shown, only how many there were.
    Unverified { counts: AnswerCounts },
}

impl Answer {
    pub fn outcome(&self) -> Outcome {
        match self {
            Self::Answered { .. } => Outcome::Answered,
            Self::NotInPapers => Outcome::NotInPapers,
            Self::Unverified { .. } => Outcome::Unverified,
        }
    }

    /// The statements shown: none unless answered.
    pub fn statements(&self) -> &[Cited<Statement>] {
        match self {
            Self::Answered { statements, .. } => statements,
            Self::NotInPapers | Self::Unverified { .. } => &[],
        }
    }

    /// How many statements were written, shown and left out (none written when the papers do not say).
    pub fn counts(&self) -> AnswerCounts {
        match self {
            Self::Answered { counts, .. } | Self::Unverified { counts } => *counts,
            Self::NotInPapers => AnswerCounts::default(),
        }
    }

    /// Its statements, taken: none unless answered.
    pub fn into_statements(self) -> Vec<Cited<Statement>> {
        match self {
            Self::Answered { statements, .. } => statements,
            Self::NotInPapers | Self::Unverified { .. } => Vec::new(),
        }
    }
}

/// Is `shown` of `written` enough to show? At least one, and at least half.
pub fn shown_enough(shown: usize, written: usize) -> bool {
    shown > 0 && shown * 2 >= written
}

/// Check the model's answer against `papers` (the texts of the papers whose passages it was given), as the
/// module says.
pub fn checked(said: Said, papers: &Papers) -> Answer {
    if said.outcome == SaidOutcome::NotInPapers {
        return Answer::NotInPapers;
    }
    let written = said.statements.len();
    let mut one: Vec<Claim<(usize, Statement)>> = Vec::new();
    let mut several: Vec<Claim<(usize, Statement)>> = Vec::new();
    let mut malformed = 0;
    for (place, value) in said.statements.into_iter().enumerate() {
        match serde_json::from_value::<Claim<Statement>>(value) {
            Ok(claim) if !claim.statement.sentence.trim().is_empty() => {
                let about = claim.statement.about;
                let claim = Claim { statement: (place, claim.statement), quotes: claim.quotes };
                match about {
                    About::OnePaper => one.push(claim),
                    About::SeveralPapers => several.push(claim),
                }
            }
            Ok(_) | Err(_) => malformed += 1,
        }
    }
    let mut cited: Vec<Cited<(usize, Statement)>> = evidence::check(one, papers, Rules::ONE_PAPER).cited;
    cited.extend(evidence::check(several, papers, Rules::pattern(SHARED_BY)).cited);
    cited.sort_by_key(|cited| cited.statement.0);
    let statements: Vec<Cited<Statement>> =
        cited.into_iter().map(|cited| Cited { statement: cited.statement.1, evidence: cited.evidence }).collect();
    let counts =
        AnswerCounts { written, shown: statements.len(), not_found: written - malformed - statements.len(), malformed };
    if shown_enough(counts.shown, written) {
        Answer::Answered { statements, counts }
    } else {
        Answer::Unverified { counts }
    }
}

/// A passage found for a question: the paper's own words there, as evidence one can click, and whether the
/// answer cites it. `PassageFound` in the contracts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Found {
    pub evidence: Evidence,
    /// Does a statement shown cite this passage?
    pub cited: bool,
}

/// The evidence of a hit: the passage read from the paper's pages, or its abstract (or title) found in its record.
fn evidence_of(hit: &Hit, texts: &Papers) -> Option<Evidence> {
    match (hit.unit, texts.get(&hit.paper)?) {
        (Unit::Passage { index }, PaperText::Read(passages)) => {
            let passage = passages.passages.get(usize::try_from(index).ok()?)?;
            Evidence::at(&hit.paper, passages, &passage.address)
        }
        (Unit::Record, text @ PaperText::Unread { title, abstract_text }) => {
            Evidence::locate(&hit.paper, text, abstract_text.trim())
                .or_else(|_| Evidence::locate(&hit.paper, text, title.trim()))
                .ok()
        }
        _ => None,
    }
}

/// Is `evidence` inside the place of `found`?
fn within(evidence: &Evidence, found: &Evidence) -> bool {
    if evidence.paper() != found.paper() {
        return false;
    }
    match (evidence.established(), found.established()) {
        (Established::Verified { address: quote, .. }, Established::Verified { address: place, .. }) => {
            let after_start = (quote.page, quote.start) >= (place.page, place.start);
            let before_end = (quote.page, quote.start) < (place.end_page, place.end);
            after_start && before_end
        }
        (Established::Abstract { .. }, Established::Abstract { .. }) => true,
        _ => false,
    }
}

/// The passages found, best first, each as evidence (those whose paper's text changed since are left out), with
/// whether `statements` cite it.
pub fn found(hits: &[Hit], texts: &Papers, statements: &[Cited<Statement>]) -> Vec<Found> {
    let cites: Vec<&Evidence> = statements.iter().flat_map(|cited| &cited.evidence).collect();
    hits.iter()
        .filter_map(|hit| evidence_of(hit, texts))
        .map(|evidence| Found { cited: cites.iter().any(|quote| within(quote, &evidence)), evidence })
        .collect()
}

/// The papers `statements` cite, in byte order: what the interface can light up on the graph.
pub fn cited_papers(statements: &[Cited<Statement>]) -> Vec<String> {
    let papers: BTreeSet<&str> =
        statements.iter().flat_map(|cited| cited.evidence.iter().map(Evidence::paper)).collect();
    papers.into_iter().map(str::to_owned).collect()
}

/// The texts of `papers` among `texts`: what an answer's quotes may be checked against.
pub fn citable(texts: &Papers, papers: &[String]) -> Papers {
    papers.iter().filter_map(|paper| Some((paper.clone(), texts.get(paper)?.clone()))).collect::<HashMap<_, _>>()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Mutex;

    use super::super::language::fake::{Fake, message, ok};
    use super::super::language::{Endpoint, KEY_VARIABLE, Limit};
    use super::*;
    use crate::jobs::Cancel;
    use crate::text::passages::read_passages;
    use crate::text::retrieval::{Builder, Document, Ranking, Unread};
    use crate::usage::{Call, Usd};

    const MODEL: &str = "claude-opus-5-5";

    fn texts() -> Papers {
        let read = |pages: &str| PaperText::Read(read_passages(&[pages.to_owned()]));
        Papers::from([
            (
                "2601.00001".to_owned(),
                read(
                    "1. Introduction\n\nWe show that merging task vectors counts shared directions twice. The cure is to rescale the inflated singular values.\n",
                ),
            ),
            (
                "2601.00002".to_owned(),
                read(
                    "1. Method\n\nOur method distils a large teacher into a small student with no labelled data at all.\n\n2. Results\n\nMerging the students also counts shared directions twice, as others found.\n",
                ),
            ),
            (
                "icml-2026-3".to_owned(),
                PaperText::Unread {
                    title: "Calibrated Merging of Task Vectors".to_owned(),
                    abstract_text: "Merging fine-tuned models inflates the spectrum of shared directions.".to_owned(),
                },
            ),
        ])
    }

    fn scope() -> Scope {
        Scope { run: "r1".to_owned(), community: Some("c0".to_owned()) }
    }

    /// The hits for `query` over the papers of [`texts`], as the route finds them.
    fn hits(query: &str) -> Vec<Hit> {
        let texts = texts();
        let mut ids: Vec<&String> = texts.keys().collect();
        ids.sort();
        let documents: Vec<Document<'_>> = ids
            .iter()
            .map(|id| match &texts[*id] {
                PaperText::Read(passages) => Document::Read { paper: id, passages },
                PaperText::Unread { title, abstract_text } => {
                    Document::Record { paper: id, title, abstract_text, why: Unread::NoPdf }
                }
            })
            .collect();
        let mut builder = Builder::default();
        builder.add(&documents);
        builder.finish().search(query, Ranking::Lexical)
    }

    fn prepared() -> Prepared {
        prepare(
            &Chosen::claude(MODEL),
            &scope(),
            "Does merging count shared directions twice?",
            &hits("merging shared directions"),
            &texts(),
        )
    }

    fn said(statements: Value) -> Said {
        serde_json::from_value(json!({"outcome": "answered", "statements": statements})).unwrap()
    }

    fn claim(sentence: &str, about: &str, quotes: &[(&str, &str)]) -> Value {
        let quotes: Vec<Value> = quotes.iter().map(|(paper, quote)| json!({"paper": paper, "quote": quote})).collect();
        json!({"statement": {"sentence": sentence, "about": about}, "quotes": quotes})
    }

    #[test]
    fn the_prompt_labels_each_passage_with_its_paper_and_place_and_the_key_has_everything_it_depends_on() {
        let prepared = prepared();
        let shared = prepared.shared();
        assert!(shared.starts_with("Passages from the papers in scope:\n\n[1] paper "), "{shared}");
        assert!(shared.contains("paper 2601.00001, page 1, section \"1. Introduction\"\nWe show that merging"));
        assert!(shared.contains("paper icml-2026-3, its title and abstract (its full text has not been read)"));
        assert_eq!(prepared.papers, ["2601.00001", "2601.00002", "icml-2026-3"]);
        let item = prepared.item();
        assert_eq!(item.ask.question, "Question: Does merging count shared directions twice?");
        assert_eq!((item.ask.model, item.ask.system, item.ask.max_tokens), (MODEL, SYSTEM, MAX_TOKENS));
        assert!(!SYSTEM.contains("Claude"), "the instructions name no vendor");
        assert_eq!(item.slot.subject, Subject::Community { run: "r1".to_owned(), community: "c0".to_owned() });
        // another model, another question or another scope is another answer
        let texts = texts();
        let found = hits("merging shared directions");
        let other = |model: &str, question: &str, scope: &Scope| {
            prepare(&Chosen::claude(model), scope, question, &found, &texts).key
        };
        let question = "Does merging count shared directions twice?";
        assert_eq!(other(MODEL, question, &scope()), prepared.key);
        assert_ne!(other("claude-sonnet-5-5", question, &scope()), prepared.key);
        assert_ne!(other(MODEL, "Why?", &scope()), prepared.key);
        assert_ne!(other(MODEL, question, &Scope { community: None, ..scope() }), prepared.key);
    }

    #[test]
    fn the_schema_asks_for_statements_with_quotes_and_allows_saying_the_papers_do_not_say() {
        let schema = schema();
        assert!(crate::model::language::strict_ready(&schema), "every object closed and every property required");
        assert_eq!(schema["properties"]["outcome"]["enum"], json!(["answered", "not_in_papers"]));
        let claim = &schema["properties"]["statements"]["items"];
        assert_eq!(claim["required"], json!(["statement", "quotes"]));
        assert_eq!(claim["properties"]["quotes"]["items"]["required"], json!(["paper", "quote"]));
        assert_eq!(
            claim["properties"]["statement"]["properties"]["about"]["enum"],
            json!(["one_paper", "several_papers"])
        );
        fn strict(value: &Value) -> bool {
            match value {
                Value::Object(map) => {
                    (map.get("type") != Some(&json!("object"))
                        || map.get("additionalProperties") == Some(&json!(false)))
                        && map.values().all(strict)
                }
                Value::Array(items) => items.iter().all(strict),
                _ => true,
            }
        }
        assert!(strict(&schema), "every object is closed, as structured outputs require");
    }

    #[test]
    fn the_prompt_holds_no_more_than_its_cap_of_passages() {
        let long = "word ".repeat(5_000);
        let texts: Papers = (0..60)
            .map(|n| {
                let title = format!("Paper {n}");
                (format!("p{n:02}"), PaperText::Unread { title, abstract_text: long.clone() })
            })
            .collect();
        let hits: Vec<Hit> =
            (0..60).map(|n| Hit { paper: format!("p{n:02}"), unit: Unit::Record, score: 1.0 }).collect();
        let prepared = prepare(&Chosen::claude(MODEL), &scope(), "What?", &hits, &texts);
        #[allow(clippy::cast_precision_loss)]
        let tokens = prepared.shared().chars().count() as f64 / CHARS_PER_TOKEN;
        assert!(tokens <= PROMPT_TOKENS as f64, "{tokens}");
        assert!(prepared.given.len() < 60 && !prepared.given.is_empty());
        assert!(prepared.shared().contains("[...]"), "a long passage is cut");
    }

    #[test]
    fn statements_whose_quotes_are_found_are_shown_and_the_rest_left_out_and_counted() {
        let prepared = prepared();
        let papers = citable(&texts(), &prepared.papers);
        let answer = checked(
            said(json!([
                claim(
                    "Merging counts shared directions twice.",
                    "one_paper",
                    &[("2601.00001", "merging task vectors counts shared directions twice")]
                ),
                claim(
                    "Merging always helps.",
                    "one_paper",
                    &[("2601.00001", "merging always improves every task by ten points")]
                ),
                claim(
                    "Two papers find the double count.",
                    "several_papers",
                    &[
                        ("2601.00002", "Merging the students also counts shared directions twice"),
                        ("2601.00001", "counts shared directions twice. The cure is"),
                    ]
                ),
                claim(
                    "The spectrum inflates.",
                    "one_paper",
                    &[("icml-2026-3", "inflates the spectrum of shared directions")]
                ),
            ])),
            &papers,
        );
        assert_eq!(answer.outcome(), Outcome::Answered);
        let sentences: Vec<&str> = answer.statements().iter().map(|cited| cited.statement.sentence.as_str()).collect();
        assert_eq!(
            sentences,
            ["Merging counts shared directions twice.", "Two papers find the double count.", "The spectrum inflates."]
        );
        assert_eq!(answer.counts(), AnswerCounts { written: 4, shown: 3, not_found: 1, malformed: 0 });
        // a paper with no PDF: its quote is found in its abstract, and says so
        assert!(matches!(answer.statements()[2].evidence[0].established(), Established::Abstract { .. }));
        assert_eq!(cited_papers(answer.statements()), ["2601.00001", "2601.00002", "icml-2026-3"]);
    }

    #[test]
    fn a_quote_from_the_wrong_paper_or_one_not_given_is_refused_and_a_shared_statement_needs_two_papers() {
        let prepared = prepare(&Chosen::claude(MODEL), &scope(), "q", &hits("distils teacher student"), &texts());
        assert_eq!(prepared.papers, ["2601.00002"]);
        let papers = citable(&texts(), &prepared.papers);
        let answer = checked(
            said(json!([
                claim(
                    "Distillation needs no labels.",
                    "one_paper",
                    &[("2601.00002", "distils a large teacher into a small student")]
                ),
                // the words are in 2601.00002, not in the paper named
                claim("Wrong paper.", "one_paper", &[("2601.00001", "distils a large teacher into a small student")]),
                // a real quote, but of a paper whose passages were not given
                claim(
                    "Not given.",
                    "one_paper",
                    &[("2601.00001", "merging task vectors counts shared directions twice")]
                ),
                // "several papers" resting on one
                claim(
                    "Everyone distils.",
                    "several_papers",
                    &[("2601.00002", "distils a large teacher into a small student")]
                ),
            ])),
            &papers,
        );
        assert_eq!(answer.outcome(), Outcome::Unverified, "one of four is not enough to show");
        assert!(answer.statements().is_empty());
        assert_eq!(answer.counts(), AnswerCounts { written: 4, shown: 1, not_found: 3, malformed: 0 });
    }

    #[test]
    fn a_malformed_statement_fails_only_itself() {
        let papers = citable(&texts(), &prepared().papers);
        let answer = checked(
            said(json!([
                claim("Merging counts shared directions twice.", "one_paper", &[("2601.00001", "merging task vectors counts shared directions twice")]),
                {"statement": "no object", "quotes": []},
                claim("", "one_paper", &[("2601.00001", "merging task vectors counts shared directions twice")]),
            ])),
            &papers,
        );
        assert_eq!(answer.outcome(), Outcome::Unverified, "one of three");
        assert_eq!(answer.counts().malformed, 2);
        let answer = checked(
            said(json!([
                claim("Merging counts shared directions twice.", "one_paper", &[("2601.00001", "merging task vectors counts shared directions twice")]),
                {"statement": {"sentence": "x", "about": "a guess"}, "quotes": []},
            ])),
            &papers,
        );
        assert_eq!((answer.outcome(), answer.counts().shown, answer.counts().malformed), (Outcome::Answered, 1, 1));
    }

    #[test]
    fn the_papers_do_not_say_is_an_answer_of_its_own() {
        let said: Said = serde_json::from_value(json!({"outcome": "not_in_papers", "statements": []})).unwrap();
        let answer = checked(said, &texts());
        assert_eq!((answer.outcome(), answer.statements().len()), (Outcome::NotInPapers, 0));
        assert!(serde_json::from_value::<Said>(json!({"outcome": "maybe", "statements": []})).is_err());
    }

    #[test]
    fn the_passages_found_are_evidence_and_say_which_the_answer_cites() {
        let prepared = prepared();
        let texts = texts();
        let answer = checked(
            said(json!([claim(
                "Merging counts shared directions twice.",
                "one_paper",
                &[("2601.00001", "merging task vectors counts shared directions twice")]
            )])),
            &citable(&texts, &prepared.papers),
        );
        let found = found(&prepared.given, &texts, answer.statements());
        assert_eq!(found.len(), prepared.given.len());
        let cited: Vec<&str> = found.iter().filter(|found| found.cited).map(|found| found.evidence.paper()).collect();
        assert_eq!(cited, ["2601.00001"]);
        let record = found.iter().find(|found| found.evidence.paper() == "icml-2026-3").unwrap();
        assert!(matches!(record.evidence.established(), Established::Abstract { .. }));
        assert_eq!(record.evidence.quote(), "Merging fine-tuned models inflates the spectrum of shared directions");
        // a passage of a paper whose text has changed since is left out, not shown wrong
        let stale = [Hit { paper: "2601.00001".to_owned(), unit: Unit::Passage { index: 99 }, score: 1.0 }];
        assert!(super::found(&stale, &texts, &[]).is_empty());
    }

    struct Run {
        cancel: Cancel,
        calls: Mutex<Vec<Call>>,
    }

    fn asked(library: &Library, transport: &Fake, prepared: &Prepared, budget: &Budget) -> Result<Said, NotAnswered> {
        let run = Run { cancel: Cancel::new(), calls: Mutex::new(Vec::new()) };
        let (log, progress) = (|_: &str| {}, |_| {});
        let tracker = Tracker::new(&log, &progress, &run.cancel);
        let endpoint =
            Endpoint::from_lookup(|name| (name == KEY_VARIABLE).then(|| "test-key-not-real".to_owned())).unwrap();
        let asking = Asking { library, transport, endpoint, recorder: &run.calls, in_flight: 1 };
        ask(asking, prepared, budget, &tracker)
    }

    fn answered(text: &str) -> Fake {
        let usage = json!({"input_tokens": 3_000, "output_tokens": 400});
        Fake::new(vec![Ok(ok(&message(MODEL, text, "end_turn", usage)))])
    }

    fn plenty() -> Budget {
        Budget::new(Limit::Dollars(Usd::from_picodollars(1_000_000_000_000)))
    }

    #[test]
    fn a_question_is_asked_once_and_its_kept_answer_is_not_asked_again() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let prepared = prepared();
        let estimate = prepared.estimate(&library).unwrap();
        assert_eq!((estimate.to_ask, estimate.kept), (1, 0));
        assert!(estimate.cost.usd.unwrap() > Usd::ZERO && estimate.cost.most_usd.unwrap() > estimate.cost.usd.unwrap());
        let reply = json!({"outcome": "answered", "statements": [claim("Merging counts shared directions twice.", "one_paper", &[("2601.00001", "merging task vectors counts shared directions twice")])]});
        let transport = answered(&reply.to_string());
        let said = asked(&library, &transport, &prepared, &plenty()).unwrap();
        assert_eq!(said.statements.len(), 1);
        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].body["model"], MODEL);
        assert_eq!(sent[0].body["output_config"]["format"]["schema"], schema());
        assert_eq!(sent[0].body["system"][0]["text"], SYSTEM);
        assert_eq!(sent[0].body["messages"][0]["content"][0]["text"], prepared.shared());

        // asked again: the kept answer, nothing sent, and the estimate says it costs nothing
        let again = asked(&library, &transport, &prepared, &plenty()).unwrap();
        assert_eq!(again, said);
        assert_eq!(transport.sent().len(), 1);
        let estimate = prepared.estimate(&library).unwrap();
        assert_eq!((estimate.to_ask, estimate.kept, estimate.cost.usd), (0, 1, Some(Usd::ZERO)));
    }

    #[test]
    fn the_estimate_sends_nothing_and_an_answer_that_is_not_json_fails_the_question_not_the_engine() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let prepared = prepared();
        let transport = answered("this is not JSON");
        prepared.estimate(&library).unwrap();
        assert!(transport.sent().is_empty(), "an estimate sends nothing");
        let failed = asked(&library, &transport, &prepared, &plenty());
        assert!(matches!(failed, Err(NotAnswered::Failed(Failure::NoAnswer(_)))), "{failed:?}");
        assert_eq!(library.kept::<Said>(Feature::Answers, &prepared.key).unwrap(), None);
    }

    #[test]
    fn a_question_the_budget_cannot_cover_is_not_sent() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let prepared = prepared();
        let transport = answered("{}");
        let estimate = prepared.estimate(&library).unwrap();
        let small = Budget::new(Limit::Tokens(estimate.cost.most_tokens - 100));
        let refused = asked(&library, &transport, &prepared, &small);
        assert!(matches!(refused, Err(NotAnswered::Stopped(Stopped::OverBudget(_)))), "{refused:?}");
        assert!(transport.sent().is_empty());
        // exactly its worst case fits
        let reply = json!({"outcome": "not_in_papers", "statements": []}).to_string();
        let transport = answered(&reply);
        let enough = Budget::new(Limit::Tokens(estimate.cost.most_tokens));
        assert!(asked(&library, &transport, &prepared, &enough).is_ok());
    }
}
