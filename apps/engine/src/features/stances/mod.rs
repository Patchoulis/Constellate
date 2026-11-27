//! Where a community's papers stand on a yes/no question: each paper placed as supporting it, mixed (yes under a
//! condition the paper states), contradicting it, or not addressing it, each placement with the paper's own words.
//!
//! What other tools get wrong decides the shape (docs/research-tools-survey.md, item 5). Consensus documents that
//! its meter misclassifies about one paper in ten and counts every paper equally whatever its design; so nothing
//! here comes to one verdict. A placement is the model's reading of one paper; it is shown with the quotes it rests
//! on, and only when those quotes were found in that paper ([`Evidence::locate`]). A placement whose quotes are not
//! found is [`Placed::NotVerified`]: shown apart, never counted for or against.
//!
//! The work, in order:
//!
//! 1. **Which papers could be about it**, with no model ([`relevance`]): the question's distinctive words looked for
//!    in each paper's title and abstract. Papers sharing none are skipped, and how many and why is said; the user
//!    can include them. Embedding the question would need the embedding model on hand, which the engine cannot count
//!    on, so the filter is lexical.
//! 2. **The most central first** ([`central_first`]): the ten papers most linked to the rest of the community are
//!    asked as a sample, the others on a second click.
//! 3. **One call per paper** ([`items`]): the paper's text (its PDF's passages, or its title and abstract when the
//!    library has no PDF) is the shared, cached part of the prompt; the question comes after it, so a second
//!    question about the same papers reads the prompt cache. Answers are kept by paper, model, prompt and question.
//! 4. **Placed** ([`place`]): the answer's shape checked, its quotes looked for in the paper.
//!
//! The questions a community's papers take positions on can be suggested without a model ([`recurring_terms`]: the
//! words its titles and abstracts keep using, and the rest of the run does not), or by the model over the abstracts
//! ([`proposal_items`], [`proposals`]), each proposal kept only when quotes from at least two papers' abstracts
//! were found.

pub mod routes;

use std::collections::{HashMap, HashSet};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::evidence::{self, Cited, Claim, Established, Evidence, PaperText, Papers, Rules};
use crate::library::{AnswerKey, Feature, PaperReading, Slot, Subject};
use crate::model::batch::Item;
use crate::model::chosen::Chosen;
use crate::model::language::{Ask, Form};
use crate::text::passages::{Kind, Match, Part, Passages};
use crate::text::words;
use crate::text::{data_section, first_chars};

/// The version of the per-paper prompt and answer: raised when either changes, so kept answers are asked again.
pub const PROMPT: u32 = 2;
/// The version of the prompt that proposes questions.
pub const PROPOSE_PROMPT: u32 = 2;
/// Papers asked first, before the user decides whether to ask the rest.
pub const SAMPLE: usize = 10;
/// A question's length in characters: one sentence.
pub const SHORTEST_QUESTION: usize = 10;
pub const LONGEST_QUESTION: usize = 300;
/// The most of a paper's text sent, in characters (about 16,000 tokens): its front matter and body in reading
/// order, cut at a passage. Appendices and checklists are left out: a paper's position is in its body.
pub const PAPER_CHARS: usize = 48_000;
/// Quotes looked at per placement; more are ignored.
pub const QUOTES: usize = 3;
/// The longest quote looked for, in characters.
pub const LONGEST_QUOTE: usize = 2_000;
/// What a placement may take, thinking included, and what it is likely to.
pub const MAX_TOKENS: u32 = 3_000;
pub const LIKELY_TOKENS: u32 = 700;
/// Papers whose abstracts are sent to propose questions: the most central, enough to show what a community is
/// about, few enough for one prompt.
pub const PROPOSE_PAPERS: usize = 120;
/// Questions proposed in one call.
pub const PROPOSALS: usize = 5;
pub const PROPOSE_MAX_TOKENS: u32 = 6_000;
pub const PROPOSE_LIKELY_TOKENS: u32 = 2_500;

/// The instructions of every placement: the same for every paper and question, so cached with the prefix.
pub const SYSTEM: &str = concat!(
    "You read one research paper and say where it stands on a yes/no research question. \
Answer only from the paper's text given; never from what you know of other work. A paper supports the question when \
its own findings or claims say yes; it is mixed when it says yes only under a condition, or yes in some settings \
and no in others (then state that condition in one sentence of your own); it contradicts the question when its \
findings say no; and it does not address the question when it takes no position on it, which is the common case \
and the right answer when in doubt. For every position other than not addressed, copy one to three passages word \
for word from the paper's text that show the position: whole clauses, not fragments, never paraphrased. Reply \
with JSON only. ",
    crate::text::data_rule!()
);

/// Where a paper stands, as the model is asked to say it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stance {
    Supports,
    Mixed,
    Contradicts,
    NotAddressed,
}

/// The model's answer about one paper, as it is kept (never `Cited`: it is checked again when it is loaded).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Said {
    pub stance: Stance,
    /// For a mixed stance: the condition under which the answer is yes. Empty otherwise.
    #[serde(default)]
    pub condition: String,
    /// The paper's words that show its stance.
    #[serde(default)]
    pub quotes: Vec<String>,
}

/// What the paper's text was when it was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Read {
    /// Its PDF's passages.
    Pdf,
    /// Its title and abstract only: the library has no readable PDF of it. A weaker reading.
    Abstract,
}

/// Where a paper is placed, after its answer was checked: what the interface groups papers by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "group", rename_all = "snake_case")]
pub enum Placed {
    /// The quotes found, and how many others the model gave were not found in the paper (left out, and said).
    Supports {
        evidence: Vec<Evidence>,
        quotes_not_found: usize,
    },
    Mixed {
        /// The model's words for the condition, beside the paper's own.
        condition: String,
        evidence: Vec<Evidence>,
        quotes_not_found: usize,
    },
    Contradicts {
        evidence: Vec<Evidence>,
        quotes_not_found: usize,
    },
    NotAddressed,
    /// The model placed it for or against, but none of its quotes was found in the paper: not counted either way,
    /// and what it said is not shown.
    NotVerified {
        quotes_not_found: usize,
    },
    /// The kept answer is not one the question allows (a mixed stance with no condition): not counted.
    Unusable,
}

/// Check one answer about `paper` against the paper's text. The quotes of a placement are looked for in this
/// paper only, so a quote of another paper, or one the model made up, is not evidence.
pub fn place(paper: &str, said: &Said, text: &PaperText) -> Placed {
    if said.stance == Stance::NotAddressed {
        return Placed::NotAddressed;
    }
    let condition = said.condition.trim();
    if said.stance == Stance::Mixed && condition.is_empty() {
        return Placed::Unusable;
    }
    let mut evidence: Vec<Evidence> = Vec::new();
    let mut not_found = 0;
    for quote in said.quotes.iter().take(QUOTES) {
        let quote = quote.trim();
        if quote.chars().nth(LONGEST_QUOTE).is_some() {
            not_found += 1;
            continue;
        }
        // only the paper's exact words: a close match may differ by a "not", which turns a yes into a no
        let exact = Evidence::locate(paper, text, quote).ok().filter(|found| match found.established() {
            Established::Verified { matched, .. } | Established::Abstract { matched, .. } => *matched == Match::Exact,
        });
        match exact {
            Some(found) => {
                if !evidence.contains(&found) {
                    evidence.push(found);
                }
            }
            None => not_found += 1,
        }
    }
    if evidence.is_empty() {
        return Placed::NotVerified { quotes_not_found: not_found.max(1) };
    }
    // some quotes found and some not: placed on those found, with the others counted rather than dropped unseen
    let quotes_not_found = not_found;
    match said.stance {
        Stance::Supports => Placed::Supports { evidence, quotes_not_found },
        Stance::Mixed => Placed::Mixed { condition: condition.to_owned(), evidence, quotes_not_found },
        Stance::Contradicts => Placed::Contradicts { evidence, quotes_not_found },
        Stance::NotAddressed => Placed::NotAddressed,
    }
}

/// A question written the one way it is kept: spacing collapsed, ends trimmed. Case is kept: it is the user's.
pub fn normalised(question: &str) -> String {
    question.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Why a question is not asked.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BadQuestion {
    #[error("a question needs at least {SHORTEST_QUESTION} characters")]
    TooShort,
    #[error("a question may have at most {LONGEST_QUESTION} characters")]
    TooLong,
}

/// The question as it is asked and kept, or why it cannot be.
pub fn checked_question(question: &str) -> Result<String, BadQuestion> {
    let question = normalised(question);
    match question.chars().count() {
        length if length < SHORTEST_QUESTION => Err(BadQuestion::TooShort),
        length if length > LONGEST_QUESTION => Err(BadQuestion::TooLong),
        _ => Ok(question),
    }
}

/// One paper's text as it is sent: the shared part of its prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub paper: String,
    pub shared: String,
    pub read: Read,
}

/// The passages a position is read from: running text, headings and captions of the front matter and body.
fn sent(passages: &Passages) -> String {
    let mut text = String::from("The paper's text, read from its PDF (front matter and body; appendices left out):\n");
    let mut sent_any = false;
    for passage in &passages.passages {
        let kept = matches!(passage.part, Part::Front | Part::Body)
            && matches!(
                passage.kind,
                Kind::Title | Kind::Heading | Kind::Text | Kind::FigureCaption | Kind::TableCaption
            );
        if !kept {
            continue;
        }
        let line = format!("[p. {}] {}\n", passage.address.page, passage.text.trim());
        if text.len() + line.len() > PAPER_CHARS {
            // cut at a passage; but a paper whose first passage is longer than the limit (a PDF read as one
            // block) is sent what fits of it, not its heading alone
            if !sent_any {
                let room = crate::text::floor_boundary(&line, PAPER_CHARS.saturating_sub(text.len()));
                text.push_str(&line[..room]);
                text.push('\n');
            }
            text.push_str("[the rest of the paper is left out]\n");
            break;
        }
        text.push_str(&line);
        sent_any = true;
    }
    data_section("PAPER", &text)
}

fn record(title: &str, abstract_text: &str) -> String {
    data_section(
        "PAPER",
        &format!(
            "The paper's title and abstract (its full text is not available):\nTitle: {}\nAbstract: {}\n",
            title.trim(),
            first_chars(abstract_text.trim(), PAPER_CHARS)
        ),
    )
}

/// The text of a paper to send, made the same way every time (so the prompt cache is read): its passages when its
/// PDF was read, its title and abstract when not. None for a paper the library does not have.
pub fn prepare(paper: &str, reading: &PaperReading) -> Option<Prepared> {
    let (shared, read) = match reading {
        PaperReading::Read(passages) => (sent(passages), Read::Pdf),
        PaperReading::NoPdf { title, abstract_text } | PaperReading::Unreadable { title, abstract_text, .. } => {
            (record(title, abstract_text), Read::Abstract)
        }
        PaperReading::NotInLibrary => return None,
    };
    Some(Prepared { paper: paper.to_owned(), shared, read })
}

/// What comes after the paper's text: the question and the form of the answer.
pub fn question_prompt(question: &str) -> String {
    format!(
        "The question: {question}\n\nSay where this paper stands on it: supports, mixed (give the condition), \
         contradicts, or not_addressed. For any stance but not_addressed, give one to three quotes copied word \
         for word from the paper's text above."
    )
}

/// The JSON an answer must be: checked again after parsing ([`place`]), whatever the model was told.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["stance", "condition", "quotes"],
        "properties": {
            "stance": {"type": "string", "enum": ["supports", "mixed", "contradicts", "not_addressed"]},
            "condition": {"type": "string"},
            "quotes": {"type": "array", "items": {"type": "string"}}
        }
    })
}

/// What an answer about a paper depends on besides the paper and the model: the question, and the round (raised by
/// a redo, so that the question is asked again without forgetting other questions' answers).
pub fn key_question(question: &str, round: u32) -> String {
    format!("{question}\nround {round}")
}

/// The calls that place `prepared` papers on `question`, one per paper, in their order.
pub fn items<'a>(
    prepared: &'a [Prepared],
    prompt: &'a str,
    key_question: &str,
    chosen: &'a Chosen,
    schema: &'a Value,
) -> Vec<Item<'a>> {
    prepared
        .iter()
        .map(|paper| Item {
            slot: Slot {
                feature: Feature::Stances,
                key: answer_key(chosen, &paper.paper, paper.read, key_question),
                subject: Subject::Paper(paper.paper.clone()),
            },
            prompt: PROMPT,
            ask: Ask {
                purpose: "where papers stand on a question",
                model: chosen.model(),
                system: SYSTEM,
                shared: Some(&paper.shared),
                question: prompt,
                form: Form::Json(schema),
                effort: chosen.effort(),
                max_tokens: MAX_TOKENS,
            },
            likely_tokens: LIKELY_TOKENS,
        })
        .collect()
}

/// The key an answer about `paper` is kept under: what [`items`] uses, for looking it up without asking.
/// What the paper was read from is in the key: an answer from its abstract is not taken for one from its PDF, so a
/// paper whose PDF arrives later is asked again with its whole text, and its label says what was read.
pub fn answer_key(chosen: &Chosen, paper: &str, read: Read, key_question: &str) -> AnswerKey {
    let from = match read {
        Read::Pdf => "pdf",
        Read::Abstract => "abstract",
    };
    chosen.key(Feature::Stances, PROMPT, &[paper], &format!("{key_question}\nread from {from}"))
}

/// Words that say nothing about what a question or a paper is about, besides [`words::STOP_WORDS`].
const STOPWORDS: &[&str] = &[
    "a",
    "about",
    "above",
    "across",
    "after",
    "again",
    "against",
    "all",
    "also",
    "among",
    "an",
    "and",
    "any",
    "are",
    "as",
    "at",
    "be",
    "been",
    "being",
    "better",
    "between",
    "both",
    "but",
    "by",
    "can",
    "could",
    "do",
    "does",
    "doing",
    "done",
    "during",
    "each",
    "effect",
    "either",
    "for",
    "from",
    "further",
    "generally",
    "get",
    "had",
    "has",
    "have",
    "help",
    "helps",
    "how",
    "however",
    "if",
    "improve",
    "improves",
    "in",
    "into",
    "is",
    "it",
    "its",
    "lead",
    "leads",
    "less",
    "make",
    "makes",
    "may",
    "more",
    "most",
    "much",
    "new",
    "no",
    "not",
    "of",
    "on",
    "one",
    "only",
    "or",
    "other",
    "our",
    "over",
    "paper",
    "papers",
    "propose",
    "proposed",
    "results",
    "same",
    "should",
    "show",
    "shows",
    "so",
    "some",
    "such",
    "than",
    "that",
    "the",
    "their",
    "them",
    "then",
    "there",
    "these",
    "they",
    "this",
    "those",
    "through",
    "to",
    "two",
    "under",
    "use",
    "used",
    "using",
    "very",
    "via",
    "was",
    "we",
    "well",
    "were",
    "what",
    "when",
    "where",
    "whether",
    "which",
    "while",
    "who",
    "why",
    "will",
    "with",
    "without",
    "work",
    "would",
    "yes",
    "you",
];

/// A word folded so that its plural and its common endings meet: "models" and "model", "training" and "train".
/// A plural folded as every lexical matcher folds it ([`crate::text::words::singular`]), and here also "-ing" and
/// "-ed" cut, which a yes/no question's verbs need ("does training on X help" and "trained on X").
fn stem(word: &str) -> String {
    let cut = |suffix: &str, keep: usize| {
        word.strip_suffix(suffix).filter(|stem| stem.chars().count() >= keep).map(str::to_owned)
    };
    if word.ends_with("ies") {
        return words::singular(word);
    }
    cut("ing", 4).or_else(|| cut("ed", 4)).unwrap_or_else(|| words::singular(word))
}

/// The words of a text that can say what it is about, folded, in order (for word pairs).
fn content_words(text: &str) -> Vec<Option<String>> {
    words::split(text)
        .into_iter()
        .map(|lower| {
            let useful = lower.chars().count() >= 3
                && !STOPWORDS.contains(&lower.as_str())
                && !words::is_stop_word(&lower)
                && !lower.chars().all(|c| c.is_ascii_digit());
            useful.then(|| stem(&lower))
        })
        .collect()
}

/// The folded words of a question that can say what it is about, each once, in their order.
pub fn question_terms(question: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    content_words(question).into_iter().flatten().filter(|term| seen.insert(term.clone())).collect()
}

/// A paper's title and abstract, as the filters read them.
#[derive(Clone, Copy, Debug)]
pub struct Record<'a> {
    pub title: &'a str,
    pub abstract_text: &'a str,
}

fn words_of(record: &Record<'_>) -> HashSet<String> {
    content_words(&format!("{} {}", record.title, record.abstract_text)).into_iter().flatten().collect()
}

/// Which papers could be about a question: what the pre-filter found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Relevance {
    /// The question's words the papers were looked at for: those in some of the papers but not in most (a word
    /// every paper uses, or none does, tells none apart); when none is, those some paper uses; when none is
    /// used, all of them.
    pub terms: Vec<String>,
    /// By paper, in the order given: does its title or abstract have one of `terms`? All true when the question
    /// has no word to look for.
    #[serde(skip)]
    pub related: Vec<bool>,
}

/// The pre-filter, with no model: a paper is related when its title or abstract has one of the question's
/// distinctive words. Read on every core; the answer does not depend on how many there are.
pub fn relevance(question: &str, records: &[Record<'_>]) -> Relevance {
    let terms = question_terms(question);
    if terms.is_empty() || records.is_empty() {
        return Relevance { terms: Vec::new(), related: vec![true; records.len()] };
    }
    let words: Vec<HashSet<String>> = records.par_iter().map(words_of).collect();
    let share = |term: &String| words.iter().filter(|paper| paper.contains(term)).count() as f64 / words.len() as f64;
    // a word no paper uses tells none apart either: looked for alone, it would skip every paper
    let distinctive: Vec<String> = terms
        .iter()
        .filter(|term| {
            let share = share(term);
            share > 0.0 && share < 0.5
        })
        .cloned()
        .collect();
    let used: Vec<String> = terms.iter().filter(|term| share(term) > 0.0).cloned().collect();
    let terms = if !distinctive.is_empty() {
        distinctive
    } else if !used.is_empty() {
        used
    } else {
        terms
    };
    let related = words.par_iter().map(|paper| terms.iter().any(|term| paper.contains(term))).collect();
    Relevance { terms, related }
}

/// A word, or pair of words, a community's titles and abstracts keep using: a topic its papers may take positions
/// on. Counted, never a model's reading.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Recurring {
    /// As it is first written in the community's papers ("synthetic data").
    pub term: String,
    /// Of the community's papers, how many use it.
    pub papers: usize,
    /// How many times more often a paper of the community uses it than a paper of the run.
    pub lift: f64,
}

/// A paper's terms, and each term with its wording where the paper first uses it.
type Worded = (HashSet<String>, Vec<(String, String)>);

/// Terms and their first wording in `records`, each paper's set of terms.
fn terms_of(records: &[Record<'_>]) -> (Vec<HashSet<String>>, HashMap<String, String>) {
    let per_paper: Vec<Worded> = records
        .par_iter()
        .map(|record| {
            let text = format!("{} {}", record.title, record.abstract_text);
            let raw: Vec<&str> = text.split(|c: char| !c.is_alphanumeric()).filter(|word| !word.is_empty()).collect();
            let folded = content_words(&text);
            let mut terms = HashSet::new();
            let mut wording = Vec::new();
            for (index, term) in folded.iter().enumerate() {
                let Some(term) = term else { continue };
                if terms.insert(term.clone()) {
                    wording.push((term.clone(), raw[index].to_lowercase()));
                }
                if let Some(Some(next)) = folded.get(index + 1) {
                    let pair = format!("{term} {next}");
                    if terms.insert(pair.clone()) {
                        wording.push((pair, format!("{} {}", raw[index], raw[index + 1]).to_lowercase()));
                    }
                }
            }
            (terms, wording)
        })
        .collect();
    let mut first: HashMap<String, String> = HashMap::new();
    let mut sets = Vec::with_capacity(per_paper.len());
    for (terms, wording) in per_paper {
        for (term, written) in wording {
            first.entry(term).or_insert(written);
        }
        sets.push(terms);
    }
    (sets, first)
}

/// The terms `community`'s titles and abstracts keep using more than the run's (`run`, which holds them too):
/// used by at least three of its papers and a tenth of them, at least twice as often as in the run, a pair of
/// words preferred to the single words in it. The `most` best, by how many papers use them and how much more.
pub fn recurring_terms(community: &[Record<'_>], run: &[Record<'_>], most: usize) -> Vec<Recurring> {
    if community.is_empty() {
        return Vec::new();
    }
    let (inside, wording) = terms_of(community);
    let (outside, _) = terms_of(run);
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for terms in &inside {
        for term in terms {
            *counts.entry(term.as_str()).or_default() += 1;
        }
    }
    let least = 3.max(community.len().div_ceil(10));
    let run_papers = outside.len().max(1) as f64;
    let mut found: Vec<(f64, Recurring, &str)> = counts
        .into_iter()
        .filter(|(_, papers)| *papers >= least)
        .filter_map(|(term, papers)| {
            let in_run = outside.iter().filter(|terms| terms.contains(term)).count().max(papers) as f64;
            let lift = (papers as f64 / community.len() as f64) / (in_run / run_papers);
            let pair = term.contains(' ');
            (lift >= 2.0 || run.len() <= community.len()).then(|| {
                let score = papers as f64 * lift.max(1.0).ln_1p() * if pair { 1.5 } else { 1.0 };
                let shown = wording.get(term).cloned().unwrap_or_else(|| term.to_owned());
                (score, Recurring { term: shown, papers, lift }, term)
            })
        })
        .collect();
    // best first; equal ones in the order of their words, so the answer is the same every time
    found.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.2.cmp(b.2)));
    let mut chosen: Vec<(Recurring, &str)> = Vec::new();
    let pairs: Vec<&str> =
        found.iter().take(4 * most).map(|(_, _, term)| *term).filter(|term| term.contains(' ')).collect();
    for (_, recurring, term) in found {
        // a single word inside a pair that is shown (whichever ranks higher) is said by the pair
        let inside_a_pair = !term.contains(' ') && pairs.iter().any(|pair| pair.split(' ').any(|word| word == term));
        if !inside_a_pair && chosen.len() < most {
            chosen.push((recurring, term));
        }
    }
    chosen.into_iter().map(|(recurring, _)| recurring).collect()
}

/// `members` (indices of a graph's nodes, `ids` its ids) most central first, as
/// [`crate::analysis::links::central_first`] orders the papers of a set, each once.
pub fn central_first(members: &[usize], edges: &[(usize, usize, f64)], ids: &[String]) -> Vec<usize> {
    let index_of: HashMap<&str, usize> =
        members.iter().filter_map(|&member| ids.get(member).map(|id| (id.as_str(), member))).collect();
    let papers: Vec<String> = index_of.keys().map(|id| (*id).to_owned()).collect();
    crate::analysis::links::central_first(&papers, ids, edges)
        .iter()
        .filter_map(|id| index_of.get(id.as_str()).copied())
        .collect()
}

/// The model's proposal of a question, as it is kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposals {
    pub questions: Vec<Claim<String>>,
}

pub const PROPOSE_SYSTEM: &str = concat!(
    "You read the abstracts of a group of research papers and find the yes/no research questions on which several \
     of them take a position. Use only the abstracts given. Reply with JSON only. ",
    crate::text::data_rule!()
);

/// Characters of one paper's abstract sent to propose questions, at most: a record whose abstract runs on (a
/// source that put the paper's text there) does not make one prompt too large to send.
pub const PROPOSAL_ABSTRACT: usize = 3_000;

/// The abstracts of the papers a proposal is asked from, each headed by its id, in the order given.
pub fn proposal_text(papers: &[(&str, Record<'_>)]) -> String {
    let mut text = String::from("The papers' abstracts, each headed by the paper's id in brackets:\n\n");
    for (id, record) in papers {
        let (title, abstract_text) = (record.title.trim(), first_chars(record.abstract_text.trim(), PROPOSAL_ABSTRACT));
        text.push_str(&format!("[{id}] {title}\n{abstract_text}\n\n"));
    }
    data_section("ABSTRACTS", &text)
}

pub fn proposal_prompt() -> String {
    format!(
        "Propose {PROPOSALS} yes/no research questions that at least two of these papers take a position on (for \
         or against, or under a condition): questions a researcher would ask, in one sentence each, ending with a \
         question mark. For each, give quotes copied word for word from the abstracts of at least two different \
         papers that show they address it, each with the id of the paper it is from, exactly as in the brackets."
    )
}

pub fn proposal_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["questions"],
        "properties": {"questions": {"type": "array", "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["statement", "quotes"],
            "properties": {
                "statement": {"type": "string"},
                "quotes": {"type": "array", "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["paper", "quote"],
                    "properties": {"paper": {"type": "string"}, "quote": {"type": "string"}}
                }}
            }
        }}}
    })
}

/// The one call that proposes questions over these papers' abstracts.
pub fn proposal_items<'a>(
    papers: &[&str],
    shared: &'a str,
    prompt: &'a str,
    round: u32,
    chosen: &'a Chosen,
    schema: &'a Value,
    subject: Subject,
) -> Vec<Item<'a>> {
    vec![Item {
        slot: Slot {
            feature: Feature::Stances,
            key: chosen.key(Feature::Stances, PROPOSE_PROMPT, papers, &format!("proposals\nround {round}")),
            subject,
        },
        prompt: PROPOSE_PROMPT,
        ask: Ask {
            purpose: "questions a community's papers take positions on",
            model: chosen.model(),
            system: PROPOSE_SYSTEM,
            shared: Some(shared),
            question: prompt,
            form: Form::Json(schema),
            effort: chosen.effort(),
            max_tokens: PROPOSE_MAX_TOKENS,
        },
        likely_tokens: PROPOSE_LIKELY_TOKENS,
    }]
}

/// Proposed questions that can be shown: one sentence of a question's length, ending with a question mark, with
/// quotes found in the abstracts of at least two of the papers. How many were left out is counted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Shown {
    pub questions: Vec<Cited<String>>,
    /// Proposals left out: their quotes were not found in two papers' abstracts, or the question was malformed.
    pub left_out: usize,
}

/// Check proposals against the abstracts they were asked from (`papers`: each paper's title and abstract only, so a
/// quote is looked for in what was sent).
pub fn proposals(said: Proposals, papers: &Papers) -> Shown {
    let total = said.questions.len();
    let well_formed: Vec<Claim<String>> = said
        .questions
        .into_iter()
        .take(PROPOSALS)
        .filter_map(|claim| {
            let question = checked_question(&claim.statement).ok().filter(|question| question.ends_with('?'))?;
            let quotes = claim.quotes.into_iter().take(2 * QUOTES).collect();
            Some(Claim { statement: question, quotes })
        })
        .collect();
    let checked = evidence::check(well_formed, papers, Rules::pattern(2));
    Shown { left_out: total - checked.cited.len(), questions: checked.cited }
}

/// The text a proposal's quotes are looked for in: a paper's title and abstract, as sent.
pub fn abstract_text(record: &Record<'_>) -> PaperText {
    PaperText::Unread { title: record.title.to_owned(), abstract_text: record.abstract_text.to_owned() }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::evidence::{Established, Quote};
    use crate::jobs::{Cancel, Progress, Tracker};
    use crate::library::{Known, Library};
    use crate::model::batch::{self, Asking, Outcome};
    use crate::model::language::fake::{Fake, message, ok};
    use crate::model::language::{Budget, Endpoint, KEY_VARIABLE, Limit};
    use crate::text::passages::read_passages;
    use crate::usage::{Call, Usd};

    const MODEL: &str = "claude-opus-5-5";

    fn reading(text: &str) -> PaperReading {
        PaperReading::Read(read_passages(&[text.to_owned()]))
    }

    fn pdf_paper() -> PaperReading {
        reading(
            "1. Introduction\n\nWe find that pre-training on synthetic data improves downstream accuracy on every \
             benchmark we tried.\n\n2. Limits\n\nThe gain disappears when the synthetic corpus is smaller than the \
             real one.\n",
        )
    }

    fn no_pdf_paper() -> PaperReading {
        PaperReading::NoPdf {
            title: "Real data is all you need".to_owned(),
            abstract_text: "Synthetic pre-training data brought no gain in downstream accuracy over real data alone."
                .to_owned(),
        }
    }

    fn said(stance: Stance, condition: &str, quotes: &[&str]) -> Said {
        Said {
            stance,
            condition: condition.to_owned(),
            quotes: quotes.iter().map(|quote| (*quote).to_owned()).collect(),
        }
    }

    fn text(reading: &PaperReading) -> PaperText {
        reading.clone().into_text().unwrap()
    }

    #[test]
    fn a_placement_whose_quote_is_in_the_paper_carries_the_paper_s_words() {
        let placed = place(
            "2601.00001",
            &said(Stance::Supports, "", &["pre-training on synthetic data improves downstream accuracy"]),
            &text(&pdf_paper()),
        );
        let Placed::Supports { evidence, quotes_not_found } = placed else { panic!("{placed:?}") };
        assert_eq!(evidence[0].quote(), "pre-training on synthetic data improves downstream accuracy");
        assert!(matches!(evidence[0].established(), Established::Verified { .. }));
        assert_eq!(quotes_not_found, 0);
    }

    #[test]
    fn a_placement_with_some_quotes_not_found_is_placed_on_those_found_and_counts_the_others() {
        let quotes =
            ["pre-training on synthetic data improves downstream accuracy", "a sentence the paper never wrote"];
        let placed = place("2601.00001", &said(Stance::Supports, "", &quotes), &text(&pdf_paper()));
        let Placed::Supports { evidence, quotes_not_found } = placed else { panic!("{placed:?}") };
        assert_eq!((evidence.len(), quotes_not_found), (1, 1), "the one not found is counted, not dropped unseen");
    }

    #[test]
    fn a_fabricated_quote_leaves_the_paper_not_verified_and_never_counted() {
        let placed = place(
            "2601.00001",
            &said(Stance::Contradicts, "", &["synthetic data always hurts accuracy by ten points"]),
            &text(&pdf_paper()),
        );
        assert_eq!(placed, Placed::NotVerified { quotes_not_found: 1 });
        let none = place("2601.00001", &said(Stance::Supports, "", &[]), &text(&pdf_paper()));
        assert_eq!(none, Placed::NotVerified { quotes_not_found: 1 }, "a stance with no quote is not verified");
    }

    #[test]
    fn a_quote_from_another_paper_is_not_evidence_for_this_one() {
        let placed = place(
            "2601.00001",
            &said(Stance::Contradicts, "", &["Synthetic pre-training data brought no gain in downstream accuracy"]),
            &text(&pdf_paper()),
        );
        assert!(matches!(placed, Placed::NotVerified { .. }), "the words are the other paper's");
    }

    #[test]
    fn a_mixed_stance_needs_its_condition_and_keeps_it() {
        let quote = "The gain disappears when the synthetic corpus is smaller than the real one";
        let missing = place("p", &said(Stance::Mixed, "  ", &[quote]), &text(&pdf_paper()));
        assert_eq!(missing, Placed::Unusable);
        let placed = place("p", &said(Stance::Mixed, "only with enough synthetic data", &[quote]), &text(&pdf_paper()));
        assert!(
            matches!(placed, Placed::Mixed { ref condition, .. } if condition == "only with enough synthetic data")
        );
        assert_eq!(
            place("p", &said(Stance::NotAddressed, "", &["anything"]), &text(&pdf_paper())),
            Placed::NotAddressed
        );
    }

    #[test]
    fn a_paper_with_no_pdf_is_read_from_its_abstract_and_says_so() {
        let prepared = prepare("icml-2026-7", &no_pdf_paper()).unwrap();
        assert_eq!(prepared.read, Read::Abstract);
        assert!(prepared.shared.contains("its full text is not available"));
        let placed = place(
            "icml-2026-7",
            &said(Stance::Contradicts, "", &["Synthetic pre-training data brought no gain in downstream accuracy"]),
            &text(&no_pdf_paper()),
        );
        let Placed::Contradicts { evidence, .. } = placed else { panic!() };
        assert!(matches!(evidence[0].established(), Established::Abstract { .. }));
        assert_eq!(prepare("x", &PaperReading::NotInLibrary), None);
    }

    #[test]
    fn a_question_is_kept_one_way_and_refused_when_too_short_or_long() {
        assert_eq!(checked_question("  Does   synthetic data help?\n").unwrap(), "Does synthetic data help?");
        assert_eq!(checked_question("Why?"), Err(BadQuestion::TooShort));
        assert_eq!(checked_question(&"a".repeat(LONGEST_QUESTION + 1)), Err(BadQuestion::TooLong));
    }

    fn records(texts: &[(&'static str, &'static str)]) -> Vec<Record<'static>> {
        texts.iter().map(|(title, abstract_text)| Record { title, abstract_text }).collect()
    }

    #[test]
    fn the_pre_filter_skips_papers_sharing_no_distinctive_word_with_the_question() {
        let papers = records(&[
            ("Synthetic data for pre-training", "We pre-train language models on generated text."),
            ("Graph neural networks", "Message passing models for molecules."),
            ("Scaling language models", "Larger models trained longer."),
            ("Robust vision models", "Adversarial training of image classifiers."),
        ]);
        let found = relevance("Does pre-training on synthetic data help downstream accuracy?", &papers);
        assert_eq!(found.related, [true, false, false, false]);
        assert!(found.terms.contains(&"synthetic".to_owned()));
        // a question of nothing but common words skips nothing
        assert_eq!(relevance("Does it help or not at all?", &papers).related, [true; 4]);
    }

    #[test]
    fn a_question_word_no_paper_uses_does_not_make_the_pre_filter_skip_every_paper() {
        // a community about synthetic data, asked about robustness, which none of its abstracts names
        let papers = records(&[
            ("Synthetic data for pre-training", "Generated text as training data."),
            ("Cheap synthetic corpora", "Synthetic data at scale."),
            ("Filtering synthetic data", "Removing bad synthetic samples."),
        ]);
        let found = relevance("Does synthetic data improve robustness?", &papers);
        assert_eq!(found.related, [true, true, true], "{found:?}");
        assert!(!found.terms.contains(&"robustness".to_owned()), "{found:?}");
    }

    #[test]
    fn the_pre_filter_and_the_terms_are_the_same_whatever_the_number_of_threads() {
        let titles: Vec<String> = (0..300).map(|n| format!("Paper {n} on synthetic data {}", n % 7)).collect();
        let abstracts: Vec<String> =
            (0..300).map(|n| format!("We study topic{} and data augmentation for task{}", n % 5, n % 11)).collect();
        let papers: Vec<Record<'_>> =
            titles.iter().zip(&abstracts).map(|(title, abstract_text)| Record { title, abstract_text }).collect();
        let run = |threads: usize| {
            rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap().install(|| {
                (relevance("Does topic3 augmentation help task4?", &papers), recurring_terms(&papers[..60], &papers, 8))
            })
        };
        let one = run(1);
        assert_eq!(one, run(4));
        assert_eq!(one, run(9));
    }

    #[test]
    fn recurring_terms_are_those_a_community_uses_more_than_the_run_pairs_first() {
        let community = records(&[
            ("Synthetic data at scale", "Synthetic data for pre-training."),
            ("Cheap synthetic data", "We generate synthetic data."),
            ("Synthetic data and safety", "Filtering synthetic data."),
            ("Something else", "Unrelated words."),
        ]);
        let mut run = community.clone();
        run.extend(records(&[("Vision", "Images."); 12]));
        let found = recurring_terms(&community, &run, 5);
        assert_eq!(found[0].term, "synthetic data");
        assert_eq!(found[0].papers, 3);
        assert!(found.iter().all(|recurring| recurring.term != "synthetic"), "the single word is inside the pair");
    }

    #[test]
    fn the_most_central_papers_come_first_and_ties_go_by_id() {
        let ids: Vec<String> = ["d", "c", "b", "a", "outside"].iter().map(|id| (*id).to_owned()).collect();
        let edges = [(0, 1, 0.9), (1, 2, 0.8), (1, 3, 0.1), (2, 4, 5.0)];
        // c: 0.9 + 0.8 + 0.1, d: 0.9, b: 0.8 (the link outside the community is not counted), a: 0.1
        assert_eq!(central_first(&[0, 1, 2, 3], &edges, &ids), [1, 0, 2, 3]);
        assert_eq!(central_first(&[3, 2], &[], &ids), [3, 2], "no links: by id");
    }

    fn endpoint() -> Endpoint {
        Endpoint::from_lookup(|name| (name == KEY_VARIABLE).then(|| "test-key-not-real".to_owned())).unwrap()
    }

    fn answer(said: &Value) -> Result<crate::model::language::Reply, crate::model::language::TransportError> {
        let usage = json!({"input_tokens": 40, "output_tokens": 60, "cache_creation_input_tokens": 900});
        Ok(ok(&message(MODEL, &said.to_string(), "end_turn", usage)))
    }

    struct Asked {
        outcomes: Vec<Outcome<Said>>,
        sent: Vec<crate::model::language::fake::Sent>,
    }

    fn ask(library: &Library, fake: &Fake, items: &[Item<'_>]) -> Asked {
        let calls: Mutex<Vec<Call>> = Mutex::new(Vec::new());
        let cancel = Cancel::new();
        let log = |_: &str| {};
        let progress = |_: Progress| {};
        let tracker = Tracker::new(&log, &progress, &cancel);
        let asking = Asking { library, transport: fake, endpoint: endpoint(), recorder: &calls, in_flight: 1 };
        let budget = Budget::new(Limit::Dollars(Usd::from_picodollars(1_000_000_000_000)));
        let outcomes = batch::ask_all::<Said>(asking, items, &budget, &tracker).unwrap();
        Asked { outcomes, sent: fake.sent() }
    }

    fn library() -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let papers: Vec<_> =
            ["2601.00001", "icml-2026-7"].iter().map(|id| crate::library::papers::tests::paper(id)).collect();
        library.add_papers(&papers, None, Known::Keep).unwrap();
        (folder, library)
    }

    #[test]
    fn each_paper_is_asked_once_with_its_text_as_the_cached_prefix_and_the_question_after_it() {
        let (_folder, library) = library();
        let prepared =
            vec![prepare("2601.00001", &pdf_paper()).unwrap(), prepare("icml-2026-7", &no_pdf_paper()).unwrap()];
        let schema = schema();
        let prompt = question_prompt("Does pre-training on synthetic data help downstream accuracy?");
        let key = key_question("Does pre-training on synthetic data help downstream accuracy?", 0);
        let chosen = Chosen::claude(MODEL);
        let items = items(&prepared, &prompt, &key, &chosen, &schema);
        assert!(
            crate::model::language::strict_ready(&schema) && crate::model::language::strict_ready(&proposal_schema())
        );

        // the estimate sends nothing
        let fake = Fake::new(vec![]);
        let estimate = batch::estimate_batch::<Said>(&library, &items, &chosen).unwrap();
        assert_eq!((estimate.to_ask, fake.sent().len()), (2, 0));
        assert!(estimate.cost.usd.unwrap() > Usd::ZERO);

        let fake = Fake::new(vec![
            answer(
                &json!({"stance": "supports", "condition": "", "quotes": ["synthetic data improves downstream accuracy on every benchmark"]}),
            ),
            answer(&json!({"stance": "maybe", "quotes": []})),
        ]);
        let asked = ask(&library, &fake, &items);
        let body = &asked.sent[0].body;
        assert_eq!(body["model"], MODEL, "the model chosen, as given");
        assert_eq!(body["system"][0]["text"], SYSTEM);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["text"], prepared[0].shared);
        assert_eq!(content[0]["cache_control"]["type"], "ephemeral");
        assert_eq!(content[1]["text"], prompt);
        assert_eq!(body["output_config"]["format"]["schema"], schema);
        assert!(prepared[0].shared.contains("[p. 1] We find that pre-training"));

        assert!(matches!(asked.outcomes[0], Outcome::Answered(_)));
        assert!(matches!(asked.outcomes[1], Outcome::Failed(_)), "a malformed answer fails its own paper only");

        // a second look asks nothing for what is kept; the malformed one is asked again
        let again = Fake::new(vec![answer(&json!({"stance": "not_addressed", "condition": "", "quotes": []}))]);
        let asked = ask(&library, &again, &items);
        assert!(matches!(asked.outcomes[0], Outcome::Kept(_)));
        assert!(matches!(asked.outcomes[1], Outcome::Answered(_)));
        assert_eq!(asked.sent.len(), 1);
        let key = answer_key(&chosen, "2601.00001", Read::Pdf, &key);
        let kept = library.kept::<Said>(Feature::Stances, &key).unwrap().unwrap();
        assert_eq!(kept.answer.stance, Stance::Supports);
    }

    #[test]
    fn a_second_question_sends_the_same_prefix_and_a_redo_asks_again() {
        let one = prepare("2601.00001", &pdf_paper()).unwrap();
        let two = prepare("2601.00001", &pdf_paper()).unwrap();
        assert_eq!(one.shared, two.shared, "byte for byte, so the prompt cache is read");
        let key = |model: &str, read: Read, question: &str| answer_key(&Chosen::claude(model), "p", read, question);
        assert_ne!(key(MODEL, Read::Pdf, &key_question("q?", 0)), key(MODEL, Read::Pdf, &key_question("q?", 1)));
        assert_ne!(key(MODEL, Read::Pdf, "q"), key("another-model", Read::Pdf, "q"), "the model is in the key");
        assert_ne!(key(MODEL, Read::Pdf, "q"), key(MODEL, Read::Abstract, "q"), "an abstract's answer is not a PDF's");
    }

    #[test]
    fn two_questions_about_a_paper_send_the_same_system_and_shared_text() {
        let prepared = vec![prepare("2601.00001", &pdf_paper()).unwrap()];
        let (chosen, schema) = (Chosen::claude(MODEL), schema());
        let ask = |question: &str| {
            let prompt = question_prompt(question);
            let key = key_question(question, 0);
            let items = items(&prepared, &prompt, &key, &chosen, &schema);
            (items[0].ask.system.to_owned(), items[0].ask.shared.map(str::to_owned), items[0].slot.key.clone())
        };
        let one = ask("Does pre-training on synthetic data help downstream accuracy?");
        let two = ask("Does a smaller synthetic corpus remove the gain?");
        assert_eq!((&one.0, &one.1), (&two.0, &two.1), "byte for byte, so the prompt cache is read");
        assert_ne!(one.2, two.2, "each question's answer is kept apart");
    }

    #[test]
    fn a_quote_found_only_with_letters_changed_does_not_place_the_paper() {
        // "no gain" read as "a gain": a close match, which could turn a no into a yes
        let placed = place(
            "icml-2026-7",
            &said(
                Stance::Supports,
                "",
                &["Synthetic pre-training data brought a gain in downstream accuracy over real data"],
            ),
            &text(&no_pdf_paper()),
        );
        assert!(matches!(placed, Placed::NotVerified { .. }), "{placed:?}");
    }

    #[test]
    fn a_long_paper_is_cut_at_a_passage() {
        let long: String = (0..2_000).map(|n| format!("Sentence number {n} of a long paper.\n\n")).collect();
        let prepared = prepare("p", &reading(&format!("1. Introduction\n\n{long}"))).unwrap();
        assert!(prepared.shared.len() <= PAPER_CHARS + 100);
        assert!(prepared.shared.starts_with("<<<BEGIN PAPER>>>\n"), "the paper's text is a data section");
        assert!(prepared.shared.ends_with("[the rest of the paper is left out]\n<<<END PAPER>>>\n"));
        assert!(SYSTEM.ends_with(crate::text::data_rule!()) && PROPOSE_SYSTEM.ends_with(crate::text::data_rule!()));
    }

    #[test]
    fn a_paper_read_as_one_passage_longer_than_the_limit_is_sent_what_fits_of_it() {
        let block: String = (0..4_000).map(|n| format!("Sentence number {n} of a paper read as one block. ")).collect();
        let prepared = prepare("p", &reading(&block)).unwrap();
        assert!(prepared.shared.contains("Sentence number 10 of a paper"), "not the heading alone");
        assert!(prepared.shared.len() <= PAPER_CHARS + 100);
        assert!(prepared.shared.ends_with("[the rest of the paper is left out]\n<<<END PAPER>>>\n"));
    }

    #[test]
    fn a_proposals_abstracts_are_a_data_section_each_cut_to_its_bound() {
        let long = "word ".repeat(PROPOSAL_ABSTRACT);
        let text = proposal_text(&[("p1", Record { title: "A title", abstract_text: &long })]);
        assert!(text.starts_with("<<<BEGIN ABSTRACTS>>>\n") && text.ends_with("<<<END ABSTRACTS>>>\n"));
        assert!(text.len() < PROPOSAL_ABSTRACT + 200, "{}", text.len());
    }

    #[test]
    fn proposals_are_kept_only_with_quotes_found_in_two_papers_abstracts() {
        let records = records(&[
            ("Synthetic data helps", "Pre-training on synthetic data improves downstream accuracy."),
            ("Synthetic data does not help", "Synthetic data brought no gain over real data alone."),
        ]);
        let ids = ["a", "b"];
        let papers: Papers =
            ids.iter().zip(&records).map(|(id, record)| ((*id).to_owned(), abstract_text(record))).collect();
        let quote = |paper: &str, quote: &str| Quote { paper: paper.to_owned(), quote: quote.to_owned() };
        let shown = proposals(
            Proposals {
                questions: vec![
                    Claim {
                        statement: "Does synthetic pre-training data help downstream accuracy?".to_owned(),
                        quotes: vec![
                            quote("a", "Pre-training on synthetic data improves downstream accuracy"),
                            quote("b", "Synthetic data brought no gain over real data alone"),
                        ],
                    },
                    Claim {
                        statement: "Is one paper enough for a question?".to_owned(),
                        quotes: vec![quote("a", "Pre-training on synthetic data improves downstream accuracy")],
                    },
                    Claim {
                        statement: "Does a made-up quote count for anything?".to_owned(),
                        quotes: vec![
                            quote("a", "Pre-training on synthetic data improves downstream accuracy"),
                            quote("b", "synthetic data doubles accuracy on every benchmark"),
                        ],
                    },
                    Claim { statement: "Not a question".to_owned(), quotes: vec![] },
                ],
            },
            &papers,
        );
        assert_eq!(shown.questions.len(), 1);
        assert_eq!(shown.left_out, 3);
        assert_eq!(shown.questions[0].evidence.len(), 2);
        let text = proposal_text(&ids.iter().copied().zip(records.iter().copied()).collect::<Vec<_>>());
        assert!(text.contains("[a] Synthetic data helps\nPre-training"));
    }
}
