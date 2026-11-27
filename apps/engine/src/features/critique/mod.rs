//! A paper's weaknesses: the reviewers' own points first, read from the reviews page the user opened, and a
//! model's reading of the paper second, each tied to the exact place it is about.
//!
//! # The reviewers' part
//!
//! Needs no model. Every review's Weaknesses, Questions and Limitations, split into points
//! ([`crate::text::reviews::review_points`]), shown per reviewer with the rating and confidence the reviewer gave (each on
//! its venue's scale), each point with its place in the kept page, and the authors' replies attached to the review
//! with how they were attached (named by the reply's title, or guessed). Whether a reply settles a point is not
//! said: nothing here can know it.
//!
//! # The model's part
//!
//! One call per paper, on the user's click, with its estimate shown first ([`Plan::estimate`]). The paper's text
//! is the shared, cached part of the prompt; the reviewers' points, numbered, are the question. The model is
//! asked for two things, and nothing else (no score, no verdict, no accept or reject):
//!
//! 1. For each reviewer's point, where in the paper it applies: a quote of the passage it is about, or "the paper
//!    has no passage on this" as an answer of its own; and, if the paper's text appears to respond to it, a quote
//!    of that response, shown only as "the paper says: ..." beside the point, never as a judgement.
//! 2. Weaknesses the reviewers did not raise, each of a [`WeaknessKind`], with a quote of the claim or passage it
//!    is about. These are the model's reading and are shown apart from the reviewers', under a heading that says
//!    so.
//!
//! Every quote is looked for in the paper ([`crate::evidence::check`]): a statement whose quote is not found is
//! left out and counted ([`LeftOut`]), never shown bare. An entry of the answer that is not in the shape asked for
//! is left out and counted too; the rest of the answer stands.
//!
//! # A weakness that says something is absent
//!
//! "No ablation of the router" cannot be shown by a quote: a quote shows what is there. Such a weakness (one that
//! names words to look for, or whose kind or wording says something is missing) is marked as an absence to check
//! oneself ([`Absence`]). The engine also looks for the words the model said are absent, in every sentence of
//! the paper's text, whole words, in any case and also with an "s" added. If they are there, the weakness is still
//! shown, with "but see:" and the first [`MOST_BUT_SEE`] sentences that have them, each a link into the paper: the
//! words may be used in passing (future work, related work) and the reader is better placed to judge than a
//! word search; dropping the weakness would hide that the model raised it. If none is found, it is shown with the
//! words searched for, so the reader knows what "not found" covered.
//!
//! The search runs over the paper's passages on every core (rayon); what it finds is put back in the passages'
//! order, so the answer is the same on one thread or many.

pub mod routes;

use std::collections::HashSet;
use std::sync::LazyLock;

use rayon::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::evidence::{self, Claim, Evidence, PaperText, Papers, Quote, Rules};
use crate::jobs::Tracker;
use crate::library::{Feature, Kept, KeptReviews, Library, LibraryError, ReviewPage, Slot, Subject};
use crate::model::batch::{self, Asking, BatchEstimate, Item, Outcome};
use crate::model::chosen::Chosen;
use crate::model::language::{Ask, Budget, Form};
use crate::text::data_section;
use crate::text::pdf::passages::{Address, Kind as PassageKind, Passages, chars_of};
use crate::text::reviews::Field;
use crate::text::reviews::discussion::{Discussion, Kind, Score, Span};
use crate::text::reviews::review_points::{Attachment, Concern, review_points};

use crate::analysis::reviews::{Scale, scale, venue_of};

/// The version of the prompt and the answer's shape: raise it when either changes, and kept answers are asked for
/// again.
pub const PROMPT: u32 = 2;
/// The most the answer may take, thinking included: a few dozen short entries with quotes.
pub const MAX_TOKENS: u32 = 12_000;
/// What an answer is likely to take, for the estimate.
pub const LIKELY_TOKENS: u32 = 3_500;
/// The paper's text sent, at most, in characters (about 120,000 tokens): a long paper with its appendix.
pub const LONGEST_PAPER: usize = 360_000;
/// Reviewers' points sent, at most; the rest are shown but not asked about.
pub const MOST_POINTS_ASKED: usize = 120;
/// Characters of one reviewer's point sent: enough to say what it is about.
const POINT_CHARS: usize = 1_200;
/// The model's own weaknesses kept, at most: the reviewers' points come first.
pub const MOST_WEAKNESSES: usize = 8;
/// Words looked for, at most, for one weakness that says something is absent.
const MOST_TERMS: usize = 6;
/// Sentences shown under "but see:", at most.
pub const MOST_BUT_SEE: usize = 3;
/// The longest statement of a weakness kept, in characters: one sentence or two.
const LONGEST_STATEMENT: usize = 600;
/// The longest quote looked for, in characters.
const LONGEST_QUOTE: usize = 2_000;
/// Characters of an authors' reply shown beside a review.
const REPLY_CHARS: usize = 4_000;

/// What a weakness the model raises is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeaknessKind {
    MissingBaseline,
    FewDatasetsOrSeeds,
    NoSignificanceOrVariance,
    ClaimBroaderThanEvidence,
    MissingAblation,
    ReproducibilityDetailMissing,
    LimitationNotDiscussed,
    Other,
}

impl WeaknessKind {
    pub const ALL: [Self; 8] = [
        Self::MissingBaseline,
        Self::FewDatasetsOrSeeds,
        Self::NoSignificanceOrVariance,
        Self::ClaimBroaderThanEvidence,
        Self::MissingAblation,
        Self::ReproducibilityDetailMissing,
        Self::LimitationNotDiscussed,
        Self::Other,
    ];

    /// As the answer names it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::MissingBaseline => "missing_baseline",
            Self::FewDatasetsOrSeeds => "few_datasets_or_seeds",
            Self::NoSignificanceOrVariance => "no_significance_or_variance",
            Self::ClaimBroaderThanEvidence => "claim_broader_than_evidence",
            Self::MissingAblation => "missing_ablation",
            Self::ReproducibilityDetailMissing => "reproducibility_detail_missing",
            Self::LimitationNotDiscussed => "limitation_not_discussed",
            Self::Other => "other",
        }
    }

    fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// Does a weakness of this kind say, by its kind, that something is missing from the paper?
    fn is_absence(self) -> bool {
        matches!(
            self,
            Self::MissingBaseline
                | Self::NoSignificanceOrVariance
                | Self::MissingAblation
                | Self::ReproducibilityDetailMissing
                | Self::LimitationNotDiscussed
        )
    }
}

// ------------------------------------------------------------------------------------------------ the prompt

/// The instructions: the same for every paper, so cached with it. Written for any capable model, not one vendor's.
pub const SYSTEM: &str = concat!(
    "You read one research paper. Its text is given paragraph by paragraph, each marked with \
its page and section. You answer with one JSON object in exactly the shape asked for, and nothing else.

Rules for every quote: copy it word for word from the paper's text given (not from the reviewers' points), at \
least eight words, from a single paragraph. Never paraphrase inside a quote and never join pieces of text.

You do not judge the paper or its reviewers. You give no score, no rating, no ranking, no overall verdict and no \
recommendation to accept or reject. You do not say whether a reviewer was right or wrong, or whether a point is \
resolved.

Part 1, \"points\": one entry per reviewer's point given, with its id. \"located\" is \"passage\" with \"quote\" the \
passage of the paper the point is about, or \"no_passage\" (and \"quote\" empty) when the paper has no passage on \
what the point is about. \"paper_says\" is a quote of a passage of the paper that responds to the point (an \
experiment, a result, an explanation, a stated limitation), or empty when there is none; it is shown only as what \
the paper says.

Part 2, \"weaknesses\": at most eight weaknesses of the paper that the reviewers' points do not already raise. Prefer \
few and well founded; leave the list empty if nothing stands out. Each has a \"kind\" from the list allowed, \
\"weakness\" (one plain sentence, without exaggeration), \"quote\" (the claim or passage of the paper it is about) \
and \"absent_terms\": when the weakness says something is missing from the paper (no ablation, no baseline, no \
variance, a detail not given), the words or short phrases that would appear in the paper if it were there, such as \
\"ablation\" or \"standard deviation\"; otherwise an empty list.

",
    crate::text::data_rule!()
);

/// A reviewer's point as it is asked about: its id in the prompt and what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    /// "p1", "p2", ... in the reviews' order.
    pub id: String,
    /// "Reviewer aBcD, Weaknesses".
    pub label: String,
    pub text: String,
}

/// The reviewers' points of a discussion as they are asked about, at most [`MOST_POINTS_ASKED`].
pub fn asked_points(discussion: &Discussion) -> Vec<Asked> {
    let mut asked = Vec::new();
    for review in review_points(discussion) {
        let note = &discussion.notes[review.note];
        let who = note.reviewer().map_or_else(|| "A reviewer".to_owned(), |id| format!("Reviewer {id}"));
        for point in review.points {
            let section = note.sections.get(point.section).map_or("", |section| section.name.as_str());
            asked.push(Asked {
                id: format!("p{}", asked.len() + 1),
                label: format!("{who}, {section}"),
                text: point.text,
            });
        }
    }
    asked.truncate(MOST_POINTS_ASKED);
    asked
}

/// The paper's text as the prompt gives it: one line per passage, with its page and section; fragments (text
/// inside figures, author lists) left out. Cut after [`LONGEST_PAPER`] characters: the second value says where,
/// when it was.
pub fn paper_prompt(passages: &Passages) -> (String, Option<usize>) {
    let mut text = String::from("The paper's text:\n");
    let mut length = text.chars().count();
    for passage in passages.passages.iter().filter(|passage| passage.kind != PassageKind::Fragment) {
        let words = passage.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let line = if passage.address.section.is_empty() {
            format!("[p. {}] {}\n", passage.address.page, words)
        } else {
            format!("[p. {} | {}] {}\n", passage.address.page, passage.address.section, words)
        };
        let added = line.chars().count();
        if length + added > LONGEST_PAPER {
            return (data_section("PAPER", &text), Some(length));
        }
        length += added;
        text.push_str(&line);
    }
    (data_section("PAPER", &text), None)
}

/// The question: the reviewers' points, numbered, or that there are none.
pub fn question(points: &[Asked]) -> String {
    if points.is_empty() {
        return "No reviews of this paper are available: answer \"points\" with an empty list, and give part 2."
            .to_owned();
    }
    let mut listed = String::from("The reviewers' points, each with its id:\n");
    for point in points {
        let words = point.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let cut: String = words.chars().take(POINT_CHARS).collect();
        listed.push_str(&format!("[{}] ({}) {}\n", point.id, point.label, cut));
    }
    let mut question = data_section("POINTS", &listed);
    question.push_str("\nAnswer part 1 for every id above, and give part 2.");
    question
}

/// The answer's shape, as structured outputs take it (every object closed).
pub static SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    let kinds: Vec<&str> = WeaknessKind::ALL.iter().map(|kind| kind.name()).collect();
    json!({
        "type": "object", "additionalProperties": false, "required": ["points", "weaknesses"],
        "properties": {
            "points": {"type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "required": ["point", "located", "quote", "paper_says"],
                "properties": {
                    "point": {"type": "string"},
                    "located": {"type": "string", "enum": ["passage", "no_passage"]},
                    "quote": {"type": "string"},
                    "paper_says": {"type": "string"}
                }}},
            "weaknesses": {"type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "required": ["kind", "weakness", "quote", "absent_terms"],
                "properties": {
                    "kind": {"type": "string", "enum": kinds},
                    "weakness": {"type": "string"},
                    "quote": {"type": "string"},
                    "absent_terms": {"type": "array", "items": {"type": "string"}}
                }}}
        }
    })
});

/// The model's answer as it came, and as it is kept: its entries are read one by one ([`read`]), so that one
/// entry not in the shape asked for costs only itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Said {
    #[serde(default)]
    pub points: Vec<Value>,
    #[serde(default)]
    pub weaknesses: Vec<Value>,
}

/// Everything one paper's question is made of.
#[derive(Clone, Debug)]
pub struct Plan {
    pub paper: String,
    /// The model chosen in the settings, of its provider.
    pub chosen: Chosen,
    /// The paper's text, as sent.
    pub shared: String,
    /// Where the paper's text was cut, in characters, when it was.
    pub cut: Option<usize>,
    pub points: Vec<Asked>,
    pub question: String,
}

impl Plan {
    pub fn new(paper: &str, chosen: &Chosen, passages: &Passages, discussion: Option<&Discussion>) -> Self {
        let (shared, cut) = paper_prompt(passages);
        let points = discussion.map(asked_points).unwrap_or_default();
        let question = question(&points);
        Self { paper: paper.to_owned(), chosen: chosen.clone(), shared, cut, points, question }
    }

    /// Where the answer is kept: by provider and model, prompt version, paper and question (which holds the
    /// reviewers' points).
    pub fn slot(&self) -> Slot {
        Slot {
            feature: Feature::Critique,
            key: self.chosen.key(Feature::Critique, PROMPT, &[&self.paper], &self.question),
            subject: Subject::Paper(self.paper.clone()),
        }
    }

    pub fn item(&self) -> Item<'_> {
        Item {
            slot: self.slot(),
            prompt: PROMPT,
            ask: Ask {
                purpose: "a paper's weaknesses",
                model: self.chosen.model(),
                system: SYSTEM,
                shared: Some(&self.shared),
                question: &self.question,
                form: Form::Json(&SCHEMA),
                effort: self.chosen.effort(),
                max_tokens: MAX_TOKENS,
            },
            likely_tokens: LIKELY_TOKENS,
        }
    }

    /// What asking would cost, if it is not kept. Nothing is sent.
    pub fn estimate(&self, library: &Library) -> Result<BatchEstimate, LibraryError> {
        batch::estimate_batch::<Said>(library, &[self.item()], &self.chosen)
    }

    /// The answer kept for this question, if there is one.
    pub fn kept(&self, library: &Library) -> Result<Option<Kept<Said>>, LibraryError> {
        library.kept::<Said>(Feature::Critique, &self.slot().key)
    }

    /// Ask, unless the answer is kept; the answer is kept as it arrives.
    pub fn ask(
        &self,
        asking: Asking<'_>,
        budget: &Budget,
        tracker: &Tracker<'_>,
    ) -> Result<Outcome<Said>, LibraryError> {
        let mut outcomes = batch::ask_all::<Said>(asking, &[self.item()], budget, tracker)?;
        Ok(outcomes.pop().unwrap_or(Outcome::NotStarted(batch::Stopped::Cancelled)))
    }
}

// ------------------------------------------------------------------------------------------------ the answer read

/// Where a reviewer's point applies in the paper, as the model reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Location {
    /// The passage the point is about, found in the paper.
    Passage { evidence: Evidence },
    /// The model's answer that the paper has no passage on what the point is about.
    NoPassage,
}

/// What the model's reading of one reviewer's point holds: a place for it, the paper's response to it, or both,
/// but never neither (a reading with nothing found in the paper is no reading, and is not shown).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PointFound {
    /// Where the point applies (a passage found in the paper, or the answer that there is none), and the paper's
    /// response when one was found.
    Placed { location: Location, paper_says: Option<Evidence> },
    /// The model's place for the point was not found in the paper (or contradicted its own response): only the
    /// paper's response, found, is shown.
    Responded { paper_says: Evidence },
}

/// The model's reading of one reviewer's point. `PointReading` in the contracts: `location` is null exactly when
/// `paper_says` is not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PointReading {
    /// The point's id ("p3").
    pub point: String,
    pub found: PointFound,
}

impl PointReading {
    /// A reading of `point` from what was found of it, if anything was.
    pub fn of(point: String, location: Option<Location>, paper_says: Option<Evidence>) -> Option<Self> {
        let found = match (location, paper_says) {
            (Some(location), paper_says) => PointFound::Placed { location, paper_says },
            (None, Some(paper_says)) => PointFound::Responded { paper_says },
            (None, None) => return None,
        };
        Some(Self { point, found })
    }

    /// Where the point applies; none when the model's place for it was not found in the paper.
    pub fn location(&self) -> Option<&Location> {
        match &self.found {
            PointFound::Placed { location, .. } => Some(location),
            PointFound::Responded { .. } => None,
        }
    }

    /// What the paper says that responds to the point, found in the paper; shown as the paper's words only.
    pub fn paper_says(&self) -> Option<&Evidence> {
        match &self.found {
            PointFound::Placed { paper_says, .. } => paper_says.as_ref(),
            PointFound::Responded { paper_says } => Some(paper_says),
        }
    }
}

/// As the interface reads it: `{point, location, paper_says}`, either of the two null but not both.
impl Serialize for PointReading {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Wire<'a> {
            point: &'a str,
            location: Option<&'a Location>,
            paper_says: Option<&'a Evidence>,
        }
        Wire { point: &self.point, location: self.location(), paper_says: self.paper_says() }.serialize(serializer)
    }
}

/// Whether a weakness says something is absent, and what a search of the paper's text found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Absence {
    /// It says what is there: its quote shows it.
    NotClaimed,
    /// It says something is missing, and none of the words searched for is in the paper's text: check yourself.
    NoneFound { searched: Vec<String> },
    /// It says something is missing, and the model named no words to search for: check yourself.
    NotSearched,
    /// It says something is missing, but the paper's text has the words: these sentences.
    ButSee { searched: Vec<String>, found: Vec<Evidence> },
}

/// A weakness the model raises, which the reviewers did not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModelWeakness {
    pub kind: WeaknessKind,
    /// The model's sentence: its reading, not the paper's statement.
    pub weakness: String,
    /// The claim or passage it is about, found in the paper.
    pub about: Evidence,
    pub absence: Absence,
}

/// What of the answer is not shown, counted: never what it said.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LeftOut {
    /// Statements whose quote was not found in the paper.
    pub not_in_paper: usize,
    /// Entries not in the shape asked for.
    pub malformed: usize,
    /// Entries about a point that was not asked about, or a second time about one.
    pub unknown_point: usize,
    /// Weaknesses past [`MOST_WEAKNESSES`].
    pub beyond_limit: usize,
    /// Weaknesses worded as a judgement of the paper (accept, reject, a score): this section gives none.
    pub verdict: usize,
    /// Points asked about that the answer has no entry for, in the shape asked: they have no reading at all.
    pub not_answered: usize,
}

/// The model's answer, checked: what is shown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Reading {
    /// In the order of the points asked.
    pub points: Vec<PointReading>,
    pub weaknesses: Vec<ModelWeakness>,
    pub left_out: LeftOut,
}

/// One quote of the answer, by which entry and which part of it it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Location(usize),
    Says(usize),
    About(usize),
}

/// An entry of part 1 in the shape asked for.
struct PointEntry {
    point: usize,
    located: Option<String>,
    says: Option<String>,
}

/// An entry of part 2 in the shape asked for.
struct WeaknessEntry {
    kind: WeaknessKind,
    weakness: String,
    quote: String,
    terms: Vec<String>,
}

fn text_of<'a>(entry: &'a Value, field: &str) -> Option<&'a str> {
    entry.get(field)?.as_str()
}

/// A quote as the answer gives it: none when empty, refused when too long.
fn quote_of(entry: &Value, field: &str) -> Result<Option<String>, ()> {
    let quote = text_of(entry, field).ok_or(())?.trim();
    match quote.chars().count() {
        0 => Ok(None),
        length if length > LONGEST_QUOTE => Err(()),
        _ => Ok(Some(quote.to_owned())),
    }
}

fn point_entry(entry: &Value, ids: &[String]) -> Result<Result<PointEntry, ()>, ()> {
    let id = text_of(entry, "point").ok_or(())?;
    let located = quote_of(entry, "quote")?;
    let located = match text_of(entry, "located").ok_or(())? {
        "passage" => Some(located.ok_or(())?),
        "no_passage" => None,
        _ => return Err(()),
    };
    let says = quote_of(entry, "paper_says")?;
    // in the shape asked for, but about no point asked: told apart from a malformed entry
    Ok(ids.iter().position(|known| known == id).map(|point| PointEntry { point, located, says }).ok_or(()))
}

fn weakness_entry(entry: &Value) -> Option<WeaknessEntry> {
    let kind = WeaknessKind::named(text_of(entry, "kind")?)?;
    let weakness = text_of(entry, "weakness")?.trim();
    if weakness.is_empty() || weakness.chars().count() > LONGEST_STATEMENT {
        return None;
    }
    let quote = quote_of(entry, "quote").ok()??;
    let terms = match entry.get("absent_terms") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(terms)) => {
            terms.iter().map(|term| term.as_str().map(str::to_owned)).collect::<Option<_>>()?
        }
        Some(_) => return None,
    };
    Some(WeaknessEntry { kind, weakness: weakness.to_owned(), quote, terms })
}

/// Wording that says something is missing.
static ABSENT: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
    Regex::new(r"(?i)\b(?:no|not|missing|lacks?|lacking|absent|absence|without|omits?|omitted|never|nor)\b").unwrap()
});

/// Wording that judges the paper as a whole: the prompt forbids it, and the reviewers' text in the prompt (from a
/// page anyone could write on) might ask for it, so it is refused here too. A score or a rating counts only with a
/// number, and "accepted" only as "be accepted": a paper's own scores (accuracy, F1) and an accepted practice are
/// what weaknesses are about.
static VERDICT: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
    Regex::new(
        r"(?i)\b(?:accept(?:ance)?|reject(?:ed|ion)?|verdict|recommend(?:s|ed|ation)?)\b|\b(?:be|been)\s+accepted\b|\b(?:scores?|scored|rating|rated)\s*(?:of|:|=|at)?\s*\d|\b\d+(?:\.\d+)?\s*(?:/|out of)\s*10\b",
    )
    .unwrap()
});

/// Check `said` against the paper `paper`, whose text is in `papers`, for the points asked (`ids`, in order).
pub fn read(said: &Said, paper: &str, papers: &Papers, ids: &[String]) -> Reading {
    let mut left_out = LeftOut::default();
    let mut points: Vec<Option<PointEntry>> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    for entry in &said.points {
        match point_entry(entry, ids) {
            Err(()) => left_out.malformed += 1,
            Ok(Err(())) => left_out.unknown_point += 1,
            Ok(Ok(entry)) if !seen.insert(entry.point) => left_out.unknown_point += 1,
            Ok(Ok(entry)) => points.push(Some(entry)),
        }
    }
    left_out.not_answered = ids.len().saturating_sub(seen.len());
    let mut weaknesses: Vec<WeaknessEntry> = Vec::new();
    for entry in &said.weaknesses {
        match weakness_entry(entry) {
            None => left_out.malformed += 1,
            Some(entry) if VERDICT.is_match(&entry.weakness) => left_out.verdict += 1,
            Some(_) if weaknesses.len() >= MOST_WEAKNESSES => left_out.beyond_limit += 1,
            Some(entry) => weaknesses.push(entry),
        }
    }

    // every quote, as a statement of one quote, looked for in the paper asked about (and only there)
    let quote = |quote: &str| vec![Quote { paper: paper.to_owned(), quote: quote.to_owned() }];
    let mut claims: Vec<Claim<Role>> = Vec::new();
    for (index, entry) in points.iter().flatten().enumerate() {
        if let Some(located) = &entry.located {
            claims.push(Claim { statement: Role::Location(index), quotes: quote(located) });
        }
        if let Some(says) = &entry.says {
            claims.push(Claim { statement: Role::Says(index), quotes: quote(says) });
        }
    }
    for (index, entry) in weaknesses.iter().enumerate() {
        claims.push(Claim { statement: Role::About(index), quotes: quote(&entry.quote) });
    }
    let checked = evidence::check(claims, papers, Rules::ONE_PAPER);
    left_out.not_in_paper += checked.counts.dropped;
    let found = |role: Role| -> Option<Evidence> {
        checked.cited.iter().find(|cited| cited.statement == role).and_then(|cited| cited.evidence.first().cloned())
    };

    let mut readings: Vec<(usize, PointReading)> = Vec::new();
    for (index, entry) in points.iter().flatten().enumerate() {
        let location = match &entry.located {
            Some(_) => found(Role::Location(index)).map(|evidence| Location::Passage { evidence }),
            None => Some(Location::NoPassage),
        };
        let paper_says = entry.says.as_ref().and_then(|_| found(Role::Says(index)));
        // "no passage on this" beside a passage that responds to it contradicts itself: only the quote is kept
        let location = match location {
            Some(Location::NoPassage) if paper_says.is_some() => {
                left_out.malformed += 1;
                None
            }
            location => location,
        };
        if let Some(reading) = PointReading::of(ids[entry.point].clone(), location, paper_says) {
            readings.push((entry.point, reading));
        }
    }
    readings.sort_by_key(|(point, _)| *point);

    let passages = match papers.get(paper) {
        Some(PaperText::Read(passages)) => Some(passages),
        _ => None,
    };
    let weaknesses = weaknesses
        .into_iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let about = found(Role::About(index))?;
            let claims_absence = !entry.terms.is_empty() || entry.kind.is_absence() || ABSENT.is_match(&entry.weakness);
            let absence = if !claims_absence {
                Absence::NotClaimed
            } else {
                let searched = searchable(&entry.terms);
                match passages {
                    _ if searched.is_empty() => Absence::NotSearched,
                    None => Absence::NoneFound { searched },
                    Some(passages) => {
                        let found = sentences_with(paper, passages, &searched);
                        if found.is_empty() {
                            Absence::NoneFound { searched }
                        } else {
                            Absence::ButSee { searched, found }
                        }
                    }
                }
            };
            Some(ModelWeakness { kind: entry.kind, weakness: entry.weakness, about, absence })
        })
        .collect();
    Reading { points: readings.into_iter().map(|(_, reading)| reading).collect(), weaknesses, left_out }
}

/// The words of `text` in lower case, one space apart and one at each end, so that " word " finds a whole word.
fn words(text: &str) -> String {
    let mut found = String::from(" ");
    for word in text.split(|c: char| !c.is_alphanumeric()).filter(|word| !word.is_empty()) {
        found.push_str(&word.to_lowercase());
        found.push(' ');
    }
    found
}

/// The terms worth searching for: at least three letters, a few words at most, each once, at most
/// [`MOST_TERMS`].
fn searchable(terms: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for term in terms {
        let normal = words(term).trim().to_owned();
        let letters = normal.chars().filter(|c| c.is_alphanumeric()).count();
        if letters >= 3 && normal.split(' ').count() <= 5 && !kept.contains(&normal) {
            kept.push(normal);
        }
    }
    kept.truncate(MOST_TERMS);
    kept
}

/// The first [`MOST_BUT_SEE`] sentences of the paper, in reading order, that have one of `terms` (as made by
/// [`searchable`]) as whole words, or with an "s" added. The passages are searched on every core; the order of
/// what is found is the passages' own.
pub fn sentences_with(paper: &str, passages: &Passages, terms: &[String]) -> Vec<Evidence> {
    let needles: Vec<(String, String)> = terms.iter().map(|term| (format!(" {term} "), format!(" {term}s "))).collect();
    let found: Vec<Vec<Address>> = passages
        .passages
        .par_iter()
        .map(|passage| {
            let mut places = Vec::new();
            for sentence in &passage.sentences {
                let start = sentence.start.saturating_sub(passage.address.start);
                let end = sentence.end.saturating_sub(passage.address.start);
                let said = words(&chars_of(&passage.text, start, end));
                if needles.iter().any(|(word, plural)| said.contains(word.as_str()) || said.contains(plural.as_str())) {
                    places.push(Address {
                        start: sentence.start,
                        end: sentence.end,
                        end_page: passage.address.page,
                        ..passage.address.clone()
                    });
                }
            }
            places
        })
        .collect();
    found
        .into_iter()
        .flatten()
        .filter_map(|address| Evidence::at(paper, passages, &address))
        .take(MOST_BUT_SEE)
        .collect()
}

// ------------------------------------------------------------------------------------------------ the reviewers

/// A score a reviewer gave, on its venue's scale.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ScoreGiven {
    pub field: Field,
    pub value: u8,
    /// What it means, as the review puts it ("Weak Accept"); empty when it says nothing.
    pub meaning: String,
    pub scale: Scale,
}

/// A section a point is in: the reviewer's words around the point.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SectionShown {
    /// Its index among the review's sections.
    pub index: usize,
    pub name: String,
    /// Exactly the page's text at `span`.
    pub text: String,
    pub span: Span,
}

/// One point of a reviewer's, as shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PointShown {
    /// "p3": the id the model's reading of it has; none past [`MOST_POINTS_ASKED`].
    pub id: Option<String>,
    pub concern: Concern,
    /// Its section's index among the review's sections.
    pub section: usize,
    /// Exactly the page's text at `span`.
    pub text: String,
    pub span: Span,
}

/// A reply of the authors attached to a review.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReplyShown {
    pub note: usize,
    pub heading: String,
    pub attachment: Attachment,
    /// The reply's text from the kept page, its first [`REPLY_CHARS`] characters; `None` when the page is not kept
    /// (or no longer holds the reply's place), so that the interface says so rather than show an empty reply.
    pub text: Option<String>,
    pub cut: bool,
    pub span: Span,
}

/// One review, as the weaknesses section shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Reviewer {
    pub note: usize,
    /// The reviewer's anonymous id ("aBcD"), when the page shows one.
    pub reviewer: Option<String>,
    pub heading: String,
    pub rating: Option<ScoreGiven>,
    pub confidence: Option<ScoreGiven>,
    /// The sections the points are in.
    pub sections: Vec<SectionShown>,
    pub points: Vec<PointShown>,
    /// Sections of strengths and weaknesses together that could not be split: their names.
    pub not_split: Vec<String>,
    pub replies: Vec<ReplyShown>,
}

/// The reviewers' part: the reviews kept for the paper, or that there are none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ReviewersPart {
    /// No reviews page was kept for the paper: the reader's Reviews tab reads one.
    NoneKept,
    Read {
        /// The page the reviews were read from, and when.
        address: String,
        read_at: String,
        reviewers: Vec<Reviewer>,
        /// Points of all reviews; those asked about are the first [`MOST_POINTS_ASKED`].
        points: usize,
    },
}

/// The overall score a review gave (rating, overall recommendation...), and its confidence, on their scales.
fn scores_of(
    kind: &Kind,
    venue: Option<&crate::text::reviews::discussion::Venue>,
) -> (Option<ScoreGiven>, Option<ScoreGiven>) {
    let given = |score: &Score| ScoreGiven {
        field: score.field,
        value: score.value,
        meaning: score.meaning.clone(),
        scale: scale(venue, score.field),
    };
    match kind {
        Kind::OfficialReview { scores, confidence } => {
            let overall = [Field::Rating, Field::OverallRecommendation, Field::Overall, Field::Recommendation]
                .iter()
                .find_map(|field| scores.iter().find(|score| score.field == *field));
            (overall.map(given), confidence.as_ref().map(given))
        }
        _ => (None, None),
    }
}

/// The reviewers' part from the reviews kept for a paper and the text of their page.
pub fn reviewers(kept: &KeptReviews, page: Option<&ReviewPage>) -> ReviewersPart {
    let discussion = &kept.discussion;
    let venue = venue_of(&kept.paper_id, discussion);
    let page_text: Option<Vec<char>> = page.map(|page| page.text.chars().collect());
    // the page's text at `span`; none without the page, or when the span is not inside it
    let at = |span: Span| -> Option<String> {
        let text = page_text.as_ref()?;
        text.get(span.start..span.end).map(|chars| chars.iter().collect())
    };
    let mut asked = 0;
    let mut all = 0;
    let shown = review_points(discussion)
        .into_iter()
        .map(|review| {
            let note = &discussion.notes[review.note];
            let (rating, confidence) = scores_of(&note.kind, venue.as_ref());
            let mut sections: Vec<SectionShown> = Vec::new();
            let points = review
                .points
                .into_iter()
                .map(|point| {
                    all += 1;
                    let id = (asked < MOST_POINTS_ASKED).then(|| {
                        asked += 1;
                        format!("p{asked}")
                    });
                    if !sections.iter().any(|section| section.index == point.section)
                        && let Some(section) = note.sections.get(point.section)
                    {
                        sections.push(SectionShown {
                            index: point.section,
                            name: section.name.clone(),
                            text: section.text.clone(),
                            span: section.span,
                        });
                    }
                    PointShown {
                        id,
                        concern: point.concern,
                        section: point.section,
                        text: point.text,
                        span: point.span,
                    }
                })
                .collect();
            let replies = review
                .replies
                .iter()
                .filter_map(|reply| {
                    let replied = discussion.notes.get(reply.note)?;
                    let text = at(replied.span);
                    let cut = text.as_ref().is_some_and(|text| text.chars().count() > REPLY_CHARS);
                    Some(ReplyShown {
                        note: reply.note,
                        heading: replied.heading.clone(),
                        attachment: reply.attachment,
                        text: text.map(|text| text.chars().take(REPLY_CHARS).collect()),
                        cut,
                        span: replied.span,
                    })
                })
                .collect();
            Reviewer {
                note: review.note,
                reviewer: note.reviewer().map(str::to_owned),
                heading: note.heading.clone(),
                rating,
                confidence,
                sections,
                points,
                not_split: review
                    .not_split
                    .iter()
                    .filter_map(|index| note.sections.get(*index).map(|section| section.name.clone()))
                    .collect(),
                replies,
            }
        })
        .collect();
    ReviewersPart::Read { address: kept.address.clone(), read_at: kept.read_at.clone(), reviewers: shown, points: all }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::jobs::{Cancel, Progress};
    use crate::library::Known;
    use crate::library::papers::tests::paper;
    use crate::model::language::fake::{Fake, error, message, ok};
    use crate::model::language::{Endpoint, KEY_VARIABLE, Limit};
    use crate::text::pdf::passages::read_passages;
    use crate::text::reviews::discussion::fixtures::{ICLR_2024, NEURIPS_2023};
    use crate::text::reviews::discussion::read_discussion;
    use crate::usage::{Call, Usd};

    /// A made-up paper: every word of it invented for these tests.
    pub(crate) const PAPER: &str = "1. Introduction\n\nWe propose a routing rule for sparse experts that sends each \
        token to the expert whose key is closest to it. We claim that the rule improves every mixture of experts \
        model on every task.\n\n4. Experiments\n\nWe evaluate the rule on two small benchmarks and report the \
        accuracy of a single run for each model.\n\nIn response to the reviews we add a third benchmark with longer \
        documents and the same models.\n\n5. Limitations\n\nThe rule has only been tried with up to eight experts \
        per layer.\n";
    /// Another made-up paper, whose words must not count as the first one's.
    const OTHER: &str =
        "1. Method\n\nOur method distils a large teacher into a small student with no labelled data at all.\n";

    fn passages(text: &str) -> Passages {
        read_passages(&[text.to_owned()])
    }

    fn papers() -> Papers {
        Papers::from([("p".to_owned(), PaperText::Read(passages(PAPER)))])
    }

    fn ids(count: usize) -> Vec<String> {
        (1..=count).map(|n| format!("p{n}")).collect()
    }

    fn said(value: Value) -> Said {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn the_prompt_gives_the_papers_text_by_page_and_section_and_the_reviewers_points_by_id() {
        let discussion = read_discussion(NEURIPS_2023).unwrap();
        let plan =
            Plan::new("neurips-2023-1", &Chosen::claude("claude-sonnet-5-5"), &passages(PAPER), Some(&discussion));
        assert!(
            plan.shared.starts_with("<<<BEGIN PAPER>>>\nThe paper's text:\n[p. 1 | 1. Introduction] 1. Introduction\n"),
            "{}",
            plan.shared
        );
        assert!(plan.shared.contains("[p. 1 | 4. Experiments] We evaluate the rule on two small benchmarks"));
        assert_eq!(plan.cut, None);
        assert_eq!(
            plan.question,
            "<<<BEGIN POINTS>>>\nThe reviewers' points, each with its id:\n\
             [p1] (Reviewer aBcD, Weaknesses) The experiments are limited to two small benchmarks. The baselines are weak, and an ablation study is missing.\n\
             [p2] (Reviewer aBcD, Questions) How does the rule behave with sixty-four experts?\n\
             [p3] (Reviewer eFgH, Weaknesses) The experiments are limited and the baselines are weak. The ablation study does not isolate the rule.\n\
             [p4] (Reviewer eFgH, Questions) Why were larger models not tried?\n\
             <<<END POINTS>>>\n\
             \nAnswer part 1 for every id above, and give part 2."
        );
        let item = plan.item();
        assert_eq!((item.ask.model, item.ask.max_tokens, item.prompt), ("claude-sonnet-5-5", MAX_TOKENS, PROMPT));
        assert_eq!(item.ask.shared, Some(plan.shared.as_str()));
        assert!(!SYSTEM.contains("Claude"), "the prompt names no vendor's model");
        // the schema asks for both parts and allows only the kinds there are
        assert_eq!(SCHEMA["required"], json!(["points", "weaknesses"]));
        assert!(crate::model::language::strict_ready(&SCHEMA), "every object closed and every property required");
        let kinds = &SCHEMA["properties"]["weaknesses"]["items"]["properties"]["kind"]["enum"];
        assert_eq!(kinds.as_array().unwrap().len(), WeaknessKind::ALL.len());
        assert_eq!(kinds[0], "missing_baseline");
        // no reviews: only part 2 is asked for
        let alone = Plan::new("2601.00001", &Chosen::claude("claude-sonnet-5-5"), &passages(PAPER), None);
        assert!(alone.points.is_empty() && alone.question.starts_with("No reviews of this paper are available"));
        assert_ne!(alone.slot().key, plan.slot().key);
        // another model is another answer
        let other =
            Plan::new("neurips-2023-1", &Chosen::claude("claude-opus-5-5"), &passages(PAPER), Some(&discussion));
        assert_ne!(other.slot().key, plan.slot().key);
    }

    #[test]
    fn a_long_paper_is_cut_and_says_where() {
        let long: String = (0..12_000).map(|n| format!("Paragraph {n} says something about the rule.\n\n")).collect();
        let (text, cut) = paper_prompt(&passages(&long));
        let inside = text.strip_prefix("<<<BEGIN PAPER>>>\n").and_then(|rest| rest.strip_suffix("<<<END PAPER>>>\n"));
        let inside = inside.unwrap();
        assert!(inside.chars().count() <= LONGEST_PAPER);
        assert_eq!(cut, Some(inside.chars().count()), "the paper's text, as cut, is all its section holds");
        assert!(SYSTEM.ends_with(crate::text::data_rule!()), "the system prompt says the sections are data");
    }

    #[test]
    fn found_quotes_become_links_and_the_rest_is_left_out_and_counted() {
        let answer = said(json!({
            "points": [
                {"point": "p1", "located": "passage", "quote": "We evaluate the rule on two small benchmarks and report the accuracy", "paper_says": "In response to the reviews we add a third benchmark with longer documents"},
                {"point": "p2", "located": "no_passage", "quote": "", "paper_says": ""},
                // a fabricated quote: its statement is left out, and so is the point's reading
                {"point": "p3", "located": "passage", "quote": "We run every experiment with ten seeds and report the variance", "paper_says": ""},
                // a real quote of another paper: not this paper's words
                {"point": "p4", "located": "passage", "quote": "Our method distils a large teacher into a small student", "paper_says": ""},
                {"point": "p9", "located": "no_passage", "quote": "", "paper_says": ""},
                {"point": "p2", "located": "no_passage", "quote": "", "paper_says": ""},
                {"point": "p1", "located": "somewhere", "quote": "", "paper_says": ""},
                {"located": "passage", "quote": 7},
                // "no passage" beside a passage that responds: the contradiction is left out, the quote kept
                {"point": "p5", "located": "no_passage", "quote": "", "paper_says": "The rule has only been tried with up to eight experts per layer"}
            ],
            "weaknesses": [
                {"kind": "claim_broader_than_evidence", "weakness": "The claim covers every model and task, the experiments two benchmarks.", "quote": "We claim that the rule improves every mixture of experts model on every task", "absent_terms": []},
                {"kind": "a_verdict", "weakness": "Reject.", "quote": "We propose a routing rule for sparse experts", "absent_terms": []},
                {"kind": "other", "weakness": "Invented.", "quote": "the rule was proven optimal for all architectures in Theorem 9", "absent_terms": []},
                // a verdict, even with a real quote: this section gives none
                {"kind": "other", "weakness": "The paper should be rejected, 3/10.", "quote": "We propose a routing rule for sparse experts", "absent_terms": []}
            ]
        }));
        let mut cite = papers();
        cite.insert("q".to_owned(), PaperText::Read(passages(OTHER)));
        let reading = read(&answer, "p", &cite, &ids(5));
        let read_points: Vec<&str> = reading.points.iter().map(|point| point.point.as_str()).collect();
        assert_eq!(read_points, ["p1", "p2", "p5"]);
        assert_eq!(reading.points[2].location(), None);
        assert!(reading.points[2].paper_says().is_some());
        let Some(Location::Passage { evidence }) = reading.points[0].location() else { panic!() };
        assert_eq!(evidence.paper(), "p");
        assert!(evidence.address().is_some());
        assert!(reading.points[0].paper_says().unwrap().quote().starts_with("In response to the reviews"));
        assert_eq!(reading.points[1].location(), Some(&Location::NoPassage));
        // "no passage" is told as such, never as a place
        let told = serde_json::to_value(&reading.points[1]).unwrap();
        assert_eq!(told, json!({"point": "p2", "location": {"kind": "no_passage"}, "paper_says": null}));
        assert_eq!(reading.weaknesses.len(), 1);
        assert_eq!(reading.weaknesses[0].absence, Absence::NotClaimed);
        assert_eq!(
            reading.left_out,
            LeftOut { not_in_paper: 3, malformed: 4, unknown_point: 2, beyond_limit: 0, verdict: 1, not_answered: 0 }
        );
    }

    #[test]
    fn points_asked_about_that_the_answer_leaves_out_are_counted_as_not_answered() {
        let answer = said(json!({
            "points": [{"point": "p2", "located": "no_passage", "quote": "", "paper_says": ""},
                       {"point": "p2", "located": "no_passage", "quote": "", "paper_says": ""}],
            "weaknesses": []
        }));
        let reading = read(&answer, "p", &papers(), &ids(4));
        assert_eq!(reading.points.len(), 1);
        assert_eq!((reading.left_out.not_answered, reading.left_out.unknown_point), (3, 1), "p1, p3 and p4");
    }

    #[test]
    fn a_weakness_about_scores_or_an_accepted_practice_is_kept_and_only_a_verdict_is_left_out() {
        for kept in [
            "The accuracy scores come from a single run, with no variance.",
            "A widely accepted baseline, hash routing, is missing.",
            "The rating of each expert by the router is not explained.",
            "Results are rated by one annotator only.",
        ] {
            assert!(!VERDICT.is_match(kept), "{kept}");
        }
        for verdict in [
            "The paper should be rejected, 3/10.",
            "It should be accepted.",
            "I recommend acceptance.",
            "Reject.",
            "A score of 4 is fair.",
            "My rating: 3.",
            "The overall verdict is negative.",
        ] {
            assert!(VERDICT.is_match(verdict), "{verdict}");
        }
    }

    #[test]
    fn a_points_reading_has_a_place_or_the_papers_response_and_is_none_with_neither() {
        let response = Evidence::at("p", &passages(PAPER), &passages(PAPER).passages[1].address).unwrap();
        assert_eq!(PointReading::of("p1".into(), None, None), None, "nothing found: no reading");
        let responded = PointReading::of("p1".into(), None, Some(response.clone())).unwrap();
        assert_eq!((responded.location(), responded.paper_says()), (None, Some(&response)));
        let told = serde_json::to_value(&responded).unwrap();
        assert_eq!((told["location"].clone(), told["paper_says"]["paper"].clone()), (Value::Null, json!("p")));
        let placed = PointReading::of("p2".into(), Some(Location::NoPassage), None).unwrap();
        assert_eq!((placed.location(), placed.paper_says()), (Some(&Location::NoPassage), None));
        let both =
            PointReading::of("p3".into(), Some(Location::Passage { evidence: response.clone() }), Some(response));
        assert!(matches!(both.unwrap().found, PointFound::Placed { paper_says: Some(_), .. }));
    }

    #[test]
    fn an_absence_is_searched_for_and_shown_with_but_see_or_as_one_to_check() {
        let answer = said(json!({
            "points": [],
            "weaknesses": [
                {"kind": "few_datasets_or_seeds", "weakness": "Only a single run per model is reported, so there is no variance.", "quote": "report the accuracy of a single run for each model", "absent_terms": ["variance", "standard deviation", "seed"]},
                {"kind": "missing_baseline", "weakness": "No comparison with hash routing.", "quote": "We propose a routing rule for sparse experts", "absent_terms": ["hash routing"]},
                {"kind": "limitation_not_discussed", "weakness": "The number of experts tried is small.", "quote": "The rule has only been tried with up to eight experts per layer", "absent_terms": ["expert"]},
                {"kind": "missing_ablation", "weakness": "No ablation of the key distance.", "quote": "the expert whose key is closest to it", "absent_terms": []}
            ]
        }));
        let reading = read(&answer, "p", &papers(), &[]);
        let absences: Vec<&Absence> = reading.weaknesses.iter().map(|weakness| &weakness.absence).collect();
        assert_eq!(
            absences[0],
            &Absence::NoneFound { searched: vec!["variance".into(), "standard deviation".into(), "seed".into()] }
        );
        assert_eq!(absences[1], &Absence::NoneFound { searched: vec!["hash routing".into()] });
        let Absence::ButSee { searched, found } = absences[2] else { panic!("{:?}", absences[2]) };
        assert_eq!(searched, &["expert"]);
        // "experts" is "expert" with an "s": the first sentences that have it, in reading order
        assert_eq!(found.len(), MOST_BUT_SEE);
        assert!(found[0].quote().starts_with("We propose a routing rule for sparse experts"));
        assert!(found.iter().all(|evidence| evidence.address().is_some()));
        assert_eq!(absences[3], &Absence::NotSearched);
    }

    #[test]
    fn the_search_finds_the_same_sentences_on_one_thread_and_on_many() {
        let long: String = (0..400)
            .map(|n| {
                format!(
                    "{n}. Part\n\nThe rule is tried on dataset {n}. An ablation of part {n} is in the appendix.\n\n"
                )
            })
            .collect();
        let paper = passages(&long);
        let terms = searchable(&["ablation".to_owned()]);
        let on = |threads: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| sentences_with("p", &paper, &terms))
        };
        let one = on(1);
        assert_eq!(one.len(), MOST_BUT_SEE);
        assert_eq!(serde_json::to_string(&one).unwrap(), serde_json::to_string(&on(8)).unwrap());
        assert!(one[0].quote().contains("part 0 "));
    }

    #[test]
    fn the_reviewers_part_has_each_reviews_points_scores_on_their_scale_and_attached_replies() {
        let discussion = read_discussion(NEURIPS_2023).unwrap();
        let kept = KeptReviews {
            paper_id: "neurips-2023-1".into(),
            address: "https://openreview.net/forum?id=abc".into(),
            read_at: "2026-11-27T10:00:00Z".into(),
            discussion,
        };
        let page =
            ReviewPage { address: kept.address.clone(), read_at: kept.read_at.clone(), text: NEURIPS_2023.into() };
        let ReviewersPart::Read { reviewers, points, .. } = reviewers(&kept, Some(&page)) else { panic!() };
        assert_eq!((reviewers.len(), points), (2, 4));
        let first = &reviewers[0];
        assert_eq!(first.reviewer.as_deref(), Some("aBcD"));
        let rating = first.rating.as_ref().unwrap();
        assert_eq!((rating.field, rating.value, rating.scale.maximum), (Field::Rating, 6, 10));
        assert_eq!(first.confidence.as_ref().map(|score| (score.value, score.scale.maximum)), Some((4, 5)));
        let ids: Vec<Option<&str>> = first.points.iter().map(|point| point.id.as_deref()).collect();
        assert_eq!(ids, [Some("p1"), Some("p2")]);
        assert_eq!(
            first.sections.iter().map(|section| section.name.as_str()).collect::<Vec<_>>(),
            ["Weaknesses", "Questions"]
        );
        let reply = &first.replies[0];
        assert!(matches!(reply.attachment, Attachment::Guessed { .. }));
        let text = reply.text.as_deref().unwrap();
        assert!(text.starts_with("Rebuttal by Authors") && text.contains("add a third benchmark"));
        assert_eq!(reviewers[1].replies[0].attachment, Attachment::Named);
        let told = serde_json::to_value(&reviewers[1]).unwrap();
        assert_eq!(told["replies"][0]["attachment"], json!({"kind": "named"}));
        assert_eq!(told["rating"]["scale"]["source"], "published");
        // without the page the reply's words are not known: told as such, never as an empty reply
        let ReviewersPart::Read { reviewers, .. } = super::reviewers(&kept, None) else { panic!() };
        assert_eq!((reviewers[0].replies[0].text.as_deref(), reviewers[0].replies[0].cut), (None, false));
        assert_eq!(serde_json::to_value(&reviewers[0]).unwrap()["replies"][0]["text"], Value::Null);
    }

    // ---------------------------------------------------------------------------------- through the library

    fn endpoint() -> Endpoint {
        Endpoint::from_lookup(|name| (name == KEY_VARIABLE).then(|| "test-key-not-real".to_owned())).unwrap()
    }

    fn answer_reply(text: &str) -> crate::model::language::Reply {
        ok(&message("claude-sonnet-5-5", text, "end_turn", json!({"input_tokens": 3000, "output_tokens": 900})))
    }

    struct Run {
        cancel: Cancel,
        calls: Mutex<Vec<Call>>,
    }

    impl Run {
        fn new() -> Self {
            Run { cancel: Cancel::new(), calls: Mutex::new(Vec::new()) }
        }

        fn ask(&self, library: &Library, transport: &Fake, plan: &Plan) -> Outcome<Said> {
            let log = |_: &str| {};
            let progress = |_: Progress| {};
            let tracker = Tracker::new(&log, &progress, &self.cancel);
            let asking = Asking { library, transport, endpoint: endpoint(), recorder: &self.calls, in_flight: 2 };
            let budget = Budget::new(Limit::Dollars(Usd::from_picodollars(2_000_000_000_000)));
            plan.ask(asking, &budget, &tracker).unwrap()
        }
    }

    fn library() -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        library.add_papers(&[paper("p")], None, Known::Keep).unwrap();
        (folder, library)
    }

    #[test]
    fn an_answer_is_asked_once_kept_and_not_asked_again_and_the_estimate_sends_nothing() {
        let (_folder, library) = library();
        let discussion = read_discussion(ICLR_2024).unwrap();
        let plan = Plan::new("p", &Chosen::claude("claude-sonnet-5-5"), &passages(PAPER), Some(&discussion));
        let answer = json!({"points": [{"point": "p1", "located": "no_passage", "quote": "", "paper_says": ""}], "weaknesses": []});
        let fake = Fake::new(vec![Ok(answer_reply(&answer.to_string()))]);
        let estimate = plan.estimate(&library).unwrap();
        assert_eq!((estimate.to_ask, estimate.kept), (1, 0));
        assert!(estimate.cost.usd.unwrap() > Usd::ZERO && estimate.cost.usd.unwrap() < estimate.cost.most_usd.unwrap());
        assert!(fake.sent().is_empty(), "the estimate sends nothing");

        let run = Run::new();
        let Outcome::Answered(answered) = run.ask(&library, &fake, &plan) else { panic!() };
        assert_eq!(answered.answer.points.len(), 1);
        let sent = fake.sent();
        assert_eq!(sent.len(), 1);
        let content = &sent[0].body["messages"][0]["content"];
        assert_eq!(content[0]["text"], plan.shared.as_str());
        assert_eq!(content[0]["cache_control"]["type"], "ephemeral");
        assert_eq!(content[1]["text"], plan.question.as_str());
        assert_eq!(sent[0].body["model"], "claude-sonnet-5-5");
        assert_eq!(sent[0].body["output_config"]["format"]["schema"], *SCHEMA);
        assert_eq!(run.calls.lock().unwrap()[0].purpose, "a paper's weaknesses");

        assert!(matches!(Run::new().ask(&library, &fake, &plan), Outcome::Kept(_)));
        assert_eq!(fake.sent().len(), 1, "kept: not asked again");
        assert_eq!(plan.estimate(&library).unwrap().to_ask, 0);
        assert!(plan.kept(&library).unwrap().is_some());
        // the redo button
        assert_eq!(library.forget(Feature::Critique, &[Subject::Paper("p".into())]).unwrap(), 1);
        assert!(plan.kept(&library).unwrap().is_none());
    }

    #[test]
    fn an_answer_that_is_not_json_fails_this_paper_and_keeps_nothing() {
        let (_folder, library) = library();
        let plan = Plan::new("p", &Chosen::claude("claude-sonnet-5-5"), &passages(PAPER), None);
        let fake = Fake::new(vec![Ok(answer_reply("The paper is weak. Reject.")), Ok(error(500, "api_error", "x"))]);
        assert!(matches!(Run::new().ask(&library, &fake, &plan), Outcome::Failed(_)));
        assert!(plan.kept(&library).unwrap().is_none());
        // an answer of the right shape whose entries are wrong is kept, and read entry by entry
        let odd = json!({"points": "none", "weaknesses": [{"kind": "other"}]}).to_string();
        let fake = Fake::new(vec![Ok(answer_reply(&odd))]);
        assert!(matches!(Run::new().ask(&library, &fake, &plan), Outcome::Failed(_)), "points is not a list");
        let partly = json!({"points": [], "weaknesses": [{"kind": "other"}, {"kind": "missing_ablation", "weakness": "No ablation of the distance.", "quote": "the expert whose key is closest to it", "absent_terms": ["ablation"]}]});
        let fake = Fake::new(vec![Ok(answer_reply(&partly.to_string()))]);
        let Outcome::Answered(answered) = Run::new().ask(&library, &fake, &plan) else { panic!() };
        let reading = read(&answered.answer, "p", &papers(), &[]);
        assert_eq!((reading.weaknesses.len(), reading.left_out.malformed), (1, 1));
        assert_eq!(reading.weaknesses[0].absence, Absence::NoneFound { searched: vec!["ablation".into()] });
    }

    #[test]
    fn a_weakness_kind_is_one_of_the_list_and_said_as_the_answer_names_it() {
        for kind in WeaknessKind::ALL {
            assert_eq!(WeaknessKind::named(kind.name()), Some(kind));
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.name());
        }
        assert_eq!(WeaknessKind::named("score"), None);
    }
}
