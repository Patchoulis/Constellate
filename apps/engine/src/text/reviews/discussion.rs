//! The whole thread of a paper's reviews page, read from its text: every note as what it is (a review, a
//! meta-review, the decision, a rebuttal, a comment), who signed it, each review's sections as the conference's
//! form names them, its scores, and which review a reply answers.
//!
//! As for the scores ([`crate::text::reviews`]), nothing here fetches anything. OpenReview serves a bot challenge to
//! programs, and Constellate does not work around it: the interface hands over the text of the page the user
//! opened in the reader, and only that text is read.
//!
//! # The page
//!
//! An OpenReview forum page shows the paper, then its notes, each under a heading and a signature line:
//!
//! ```text
//! Official Review of Submission4321 by Reviewer aBcD      the note's title
//! Official Review by Reviewer aBcD                        its kind and signature
//! 04 Jul 2023, 10:12 (modified: 12 Dec 2023, 18:01)       when; who may read it
//! Everyone
//! Summary:                                                its fields, as the venue's form names them
//! The submission proposes ...
//! Soundness: 3 good
//! ```
//!
//! A comment may carry a title of its own written above its signature line ("Response to Reviewer aBcD"); a
//! line of at most [`TITLE_LENGTH`] characters, with no colon and not ending a sentence, standing right above a
//! signature line that carries a date, is read as such a title. A title and signature line of the same kind are
//! one note, not two.
//!
//! The fields differ by venue and year (NeurIPS 2023: Strengths, Weaknesses, Soundness...; NeurIPS 2025:
//! Strengths And Weaknesses, Quality, Clarity...; ICML 2025: Claims And Evidence, Questions For Authors, Overall
//! Recommendation; ICLR 2021: one Review), so sections are kept with the names the page gives them, in order,
//! never mapped onto a common list. OpenReview writes every word of a field's name capitalised, which is how a
//! section's start is told from a line of prose that happens to have a colon in it.
//!
//! # Positions
//!
//! Every note and every section keeps its [`Span`] in the page text as it was handed over (cut to
//! [`crate::text::reviews::MAX_TEXT`] characters), counted in characters (Unicode scalar values, as everywhere in
//! [`crate::text`]), not bytes. A section's `text` is exactly that stretch of the page, unchanged, so that something
//! shown later (a point mapped onto a paper's claims) can point back at the reviewer's own words.
//!
//! # Replies
//!
//! The page's text keeps the order of the thread but loses its indentation, so which review a comment answers
//! is read from what the page does say, best first: a title that names the reviewer ([`Reply::Named`]); else, as
//! guesses that say they are ([`Reply::Guessed`]), the commenting reviewer's own review, a reviewer named at
//! the start of the comment, or the review it follows in the thread.

use std::collections::HashMap;
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::text::reviews::{Field, MAX_TEXT, NOT_WHOLE, scores};
use crate::text::{CharIndex, first_chars, floor_boundary, pattern, squash};

/// The version of this reader. Raise it when what is read changes: a page kept from an older version is read
/// again ([`crate::library::Library::read_stored_pages_again`]).
pub const PARSER: u32 = 1;
/// More notes than this is not one paper's discussion.
pub const MAX_NOTES: usize = 400;
/// A note's own title above its signature line is at most this many characters.
pub const TITLE_LENGTH: usize = 150;
/// How far into a comment (in bytes) a reviewer it addresses is looked for.
const MENTION_REACH: usize = 400;
/// Characters of a decision kept.
const DECISION_LENGTH: usize = 200;

/// A stretch of the page text, from `start` up to `end`, in characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// One field of a note, as the venue's form names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    /// As the page writes it ("Strengths And Weaknesses"); empty for text that stands before any named field.
    pub name: String,
    /// Exactly the page's text at `span`.
    pub text: String,
    pub span: Span,
}

/// One score a review gives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Score {
    pub field: Field,
    pub value: u8,
    /// What the score means, as the review puts it ("good"); empty when it says nothing.
    pub meaning: String,
}

/// What a note is, with what belongs to that kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Kind {
    /// A reviewer's review: its scores in [`Field::ALL`] order, and its confidence apart from them, since it is
    /// a judgement of the review and not of the paper.
    OfficialReview {
        scores: Vec<Score>,
        confidence: Option<Score>,
    },
    MetaReview,
    /// The venue's decision, as written ("Accept (poster)").
    Decision {
        decision: String,
    },
    /// A rebuttal or a comment by the paper's authors.
    AuthorResponse,
    ReviewerComment,
    /// A comment by an area chair, a senior area chair or the programme chairs.
    ChairComment,
    /// An official comment whose signer the page does not show.
    Comment,
    PublicComment,
    EthicsReview,
    Withdrawal,
}

impl Kind {
    fn is_comment(&self) -> bool {
        matches!(
            self,
            Self::AuthorResponse | Self::ReviewerComment | Self::ChairComment | Self::Comment | Self::PublicComment
        )
    }
}

/// Who signed a note, as the page shows it. Reviewers are anonymous ("Reviewer aBcD", "AnonReviewer2"); the
/// name of a public commenter is not kept.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Signer {
    Reviewer { id: String },
    Authors,
    AreaChair,
    SeniorAreaChair,
    ProgramChairs,
    EthicsReviewer,
    Public,
    NotShown,
}

/// Why a reply is guessed to answer the review it is attached to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// It is by the reviewer who wrote that review.
    SameReviewer,
    /// Its first lines address that review's reviewer.
    MentionsReviewer,
    /// It comes after that review in the thread, with no meta-review or decision between.
    FollowsReview,
}

/// What a note answers. `note` is the index of a review among [`Discussion::notes`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "answers", rename_all = "snake_case")]
pub enum Reply {
    /// The paper itself: a review, a meta-review, a decision.
    Submission,
    /// The review whose reviewer the note's own title names.
    Named { note: usize },
    /// A guess, and why.
    Guessed { note: usize, basis: Basis },
    /// A comment the page gives nothing to attach by.
    NotKnown,
}

/// One note of the thread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    #[serde(flatten)]
    pub kind: Kind,
    pub signer: Signer,
    /// The heading as written, with the note's own title above it when it has one.
    pub heading: String,
    /// The whole note, heading included.
    pub span: Span,
    pub sections: Vec<Section>,
    pub reply: Reply,
}

impl Note {
    /// The reviewer's id, for a note a reviewer signed.
    pub fn reviewer(&self) -> Option<&str> {
        match &self.signer {
            Signer::Reviewer { id } => Some(id),
            _ => None,
        }
    }
}

/// A conference and year, which decide what fields a review has and on what scales.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Venue {
    /// As the conference writes itself: "NeurIPS", "ICLR", "ICML"; others in capitals.
    pub conference: String,
    pub year: u16,
}

impl Venue {
    pub fn new(conference: &str, year: u16) -> Self {
        let conference = match conference.to_lowercase().as_str() {
            "neurips" | "nips" => "NeurIPS".to_owned(),
            "iclr" => "ICLR".to_owned(),
            "icml" => "ICML".to_owned(),
            _ => conference.to_uppercase(),
        };
        Self { conference, year }
    }

    /// The venue a conference paper's id names ("neurips-2025-123"); none for an arXiv id.
    pub fn of_paper(paper_id: &str) -> Option<Self> {
        let found = CONFERENCE_PAPER.captures(paper_id)?;
        Some(Self::new(found.get(1)?.as_str(), found.get(2)?.as_str().parse().ok()?))
    }
}

impl fmt::Display for Venue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.conference, self.year)
    }
}

/// What a reviews page says, note by note.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discussion {
    /// The version of the reader that read it ([`PARSER`]).
    pub parser: u32,
    /// The venue the page names, if it names one.
    pub venue: Option<Venue>,
    /// In the page's order.
    pub notes: Vec<Note>,
}

impl Discussion {
    /// The official reviews, with their index among the notes.
    pub fn reviews(&self) -> impl Iterator<Item = (usize, &Note)> {
        self.notes.iter().enumerate().filter(|(_, note)| matches!(note.kind, Kind::OfficialReview { .. }))
    }

    pub fn review_count(&self) -> usize {
        self.reviews().count()
    }
}

/// A heading's word, by what it starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Opening {
    Review,
    Comment,
    Rebuttal,
    Meta,
    Decision,
    Public,
    Ethics,
    Withdrawal,
}

impl Opening {
    fn of(word: &str) -> Self {
        let word = word.to_lowercase();
        match word.as_str() {
            "official review" | "official blind review" => Self::Review,
            "author rebuttal" | "rebuttal" => Self::Rebuttal,
            "public comment" => Self::Public,
            "paper decision" | "decision" => Self::Decision,
            "ethics review" => Self::Ethics,
            "withdrawal" => Self::Withdrawal,
            _ if word.starts_with("meta") => Self::Meta,
            _ if word.starts_with("author") => Self::Rebuttal,
            _ => Self::Comment, // official comment, rebuttal comments, acknowledgements: by whoever signed
        }
    }
}

/// Where a note's heading is, before it is read.
struct Heading {
    /// Where the note starts: its own title's line, or the heading's.
    start: usize,
    /// Where the heading's lines end and the note's fields begin.
    end: usize,
    opening: Opening,
    /// The note's own title, when it has one above its signature line.
    title: Option<String>,
}

static HEADING: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?mi)^[^\S\n]*(Official Review|Official Blind Review|Official Comment|Public Comment|Meta[- ]?Review|Reply Rebuttal Comment|Rebuttal Comment|Author Rebuttal|Rebuttal|Paper Decision|Decision|Author[^\n]{0,30}Comment|Mandatory Acknowledgement|Ethics Review|Withdrawal)\b(?:[^\n]{0,120}\b(?:by|of)\s+(?:Reviewer|AnonReviewer\d+|Authors?|Area Chairs?|Senior Area Chairs?|Program Chairs?|Ethics Reviewer|Submission\s?\d+|Paper\s?\d+)\b[^\n]{0,60}|[ \t]*(?:#\d+)?[ \t]*)$",
    )
});
static SIGNER: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)\bby\s+(?:(?:Paper|Submission)\s?\d+\s+)?(?:Reviewer[ _]([A-Za-z0-9]{1,12})\b|(AnonReviewer\d+)|(Authors?)\b|(Senior Area Chairs?)\b|(Area Chairs?)\b|(Program Chairs?)\b|(Ethics Reviewer))",
    )
});
/// A reviewer named in a comment's title or first lines. Case matters: "the reviewer said" names nobody.
static MENTION: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"\b(?:Reviewer[ _]([A-Za-z0-9]{2,12})\b|(AnonReviewer\d+))"));
static DATE: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"(?i)\b\d{1,2}\s+(?:jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?,?\s+\d{4}\b")
});
/// A line of the page that is about the note rather than in it: when, who may read it, the venue's own label.
static BOILERPLATE: LazyLock<Regex> = LazyLock::new(|| {
    let role =
        r"(?:everyone|program chairs|senior area chairs|area chairs|reviewers(?: submitted)?|authors|ethics reviewers)";
    pattern(&format!(
        r"(?i)\A[^\S\n]*(?:|{role}(?:,\s*{role})*|revisions|show revisions|show details|hide details|edit|reply|(?:readers|writers|signatures):[^\n]*|[^\n]*\b\d{{1,2}}\s+(?:jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?,?\s+\d{{4}}\b[^\n]*|[^\n]*\bConference\s+(?:Paper|Submission)\s?\d+\b[^\n]*)[^\S\n]*\z"
    ))
});
/// Where the thread ends: the site's footer.
static FOOTER: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?m)^[^\S\n]*About OpenReview[^\S\n]*$"));
/// A line that may start a section: a name, then a colon. Whether it does is [`is_section`]'s to say.
static SECTION: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?m)^[^\S\n]*([A-Z][^\s:]*(?:[^\S\n]+[^\s:]+){0,9})[^\S\n]*:"));
/// A whole score right after a field's colon (what follows is checked against [`NOT_WHOLE`]).
static SCORE_VALUE: LazyLock<Regex> = LazyLock::new(|| pattern(r"\A[^\S\n]*(?:\n[^\S\n]*){0,2}\d{1,2}"));
static VENUE: LazyLock<Regex> = LazyLock::new(|| pattern(r"\b(NeurIPS|ICLR|ICML)(?:\.cc/|[^\S\n]*)(20\d{2})\b"));
static VENUE_LABEL: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"\b(NeurIPS|ICLR|ICML)(?:\.cc/|[^\S\n]*)(20\d{2})(?:/|[^\S\n]+)Conference\b"));
static CONFERENCE_PAPER: LazyLock<Regex> = LazyLock::new(|| pattern(r"^([a-z]+)-(\d{4})-\d+$"));

/// Field names of one word that are sections wherever they stand; longer names are told by their capitals.
const SECTION_WORDS: &[&str] = &[
    "summary",
    "strengths",
    "weaknesses",
    "questions",
    "limitations",
    "review",
    "comment",
    "comments",
    "metareview",
    "decision",
    "recommendation",
    "justification",
    "rebuttal",
    "response",
    "contributions",
    "reproducibility",
    "correctness",
    "novelty",
    "acknowledgement",
];

fn only_boilerplate(text: &str) -> bool {
    text.lines().all(|line| BOILERPLATE.is_match(line))
}

/// The notes' headings, a title line and its signature line made one.
fn headings(text: &str) -> Vec<Heading> {
    let mut found: Vec<Heading> = Vec::new();
    for heading in HEADING.captures_iter(text) {
        let (Some(whole), Some(word)) = (heading.get(0), heading.get(1)) else {
            continue;
        };
        let opening = Opening::of(word.as_str());
        // two notes of one kind in a row, the first collapsed to its heading, are told apart by their signers
        let signed = |line: &str| SIGNER.find(line).map(|by| by.as_str().to_lowercase());
        if let Some(last) = found.last_mut()
            && last.opening == opening
            && only_boilerplate(&text[last.end..whole.start()])
            && match (signed(&text[last.start..last.end]), signed(whole.as_str())) {
                (Some(before), Some(now)) => before == now,
                _ => true,
            }
        {
            last.end = whole.end(); // the signature line under a title of the same kind
            continue;
        }
        let mut heading = Heading { start: whole.start(), end: whole.end(), opening, title: None };
        let after_previous = found.last().map(|previous| previous.end);
        if let Some(after_previous) = after_previous
            && DATE.is_match(whole.as_str())
            && let Some((start, title)) = title_above(text, whole.start(), after_previous)
        {
            heading.start = start;
            heading.title = Some(title);
        }
        found.push(heading);
    }
    found
}

/// The note's own title on the line right above its signature line at `at`, if that line reads as one.
fn title_above(text: &str, at: usize, not_before: usize) -> Option<(usize, String)> {
    let above = text[..at].strip_suffix('\n')?;
    let start = above.rfind('\n').map_or(0, |end| end + 1);
    let line = above[start..].trim();
    let titled = start >= not_before
        && !line.is_empty()
        && line.chars().count() <= TITLE_LENGTH
        && !line.contains(':')
        && !line.ends_with(['.', '!', '?', ','])
        && !BOILERPLATE.is_match(line);
    titled.then(|| (start, line.to_owned()))
}

fn signer(header: &str, opening: Opening) -> Signer {
    let signed = SIGNER.captures(header).and_then(|found| {
        let role = (1..=7).find(|group| found.get(*group).is_some())?;
        let text = found.get(role)?.as_str();
        Some(match role {
            1 | 2 => Signer::Reviewer { id: text.to_owned() },
            3 => Signer::Authors,
            4 => Signer::SeniorAreaChair,
            5 => Signer::AreaChair,
            6 => Signer::ProgramChairs,
            _ => Signer::EthicsReviewer,
        })
    });
    match (signed, opening) {
        (Some(signer), _) => signer,
        (None, Opening::Public) => Signer::Public,
        (None, Opening::Rebuttal) => Signer::Authors,
        (None, _) => Signer::NotShown,
    }
}

/// Is `name`, at the start of a line and followed by a colon and `after`, the start of a section?
fn is_section(name: &str, after: &str, review: bool, current: Option<&str>, seen: &[String]) -> bool {
    let words: Vec<&str> = name.split_whitespace().collect();
    let lower = words.join(" ").to_lowercase();
    if words.len() > 10 || seen.contains(&lower) {
        return false;
    }
    // "Strengths:" inside a field named "Strengths And Weaknesses" is the reviewer's own heading, not a field
    if current.is_some_and(|current| format!(" {} ", current.to_lowercase()).contains(&format!(" {lower} "))) {
        return false;
    }
    if Field::named(&lower).is_some() {
        let scored = SCORE_VALUE.find(after).is_some_and(|value| !NOT_WHOLE.is_match(&after[value.end()..]));
        // in a review, a field's name followed by anything but a score is the reviewer's prose
        return scored || !review;
    }
    SECTION_WORDS.contains(&lower.as_str())
        || (words.len() >= 2
            && words.iter().all(|word| word.starts_with(|c: char| c.is_uppercase() || c.is_ascii_digit())))
}

/// The sections of a note's fields, which run from `from` to `to` in `text`.
fn sections(text: &str, chars: &CharIndex<'_>, from: usize, to: usize, review: bool) -> Vec<Section> {
    let body = &text[from..to];
    let mut starts: Vec<(String, usize, usize)> = Vec::new(); // name, where its line starts, where its text starts
    let mut seen: Vec<String> = Vec::new();
    for found in SECTION.captures_iter(body) {
        let (Some(whole), Some(name)) = (found.get(0), found.get(1)) else {
            continue;
        };
        let after = &body[whole.end()..];
        let current = starts.last().map(|(name, _, _)| name.as_str());
        if !is_section(name.as_str(), after, review, current, &seen) {
            continue;
        }
        let skipped = after.len() - after.trim_start().len();
        seen.push(name.as_str().split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase());
        starts.push((name.as_str().trim().to_owned(), whole.start(), whole.end() + skipped));
    }
    let span = |start: usize, end: usize| Span { start: chars.at(from + start), end: chars.at(from + end) };
    let mut found = Vec::new();
    let first = starts.first().map_or(body.len(), |(_, line, _)| *line);
    if let Some((start, end)) = preamble(&body[..first]) {
        found.push(Section { name: String::new(), text: body[start..end].to_owned(), span: span(start, end) });
    }
    for (index, (name, _, start)) in starts.iter().enumerate() {
        let next = starts.get(index + 1).map_or(body.len(), |(_, line, _)| *line);
        let start = (*start).min(next);
        let end = start + body[start..next].trim_end().len();
        found.push(Section { name: name.clone(), text: body[start..end].to_owned(), span: span(start, end) });
    }
    found
}

/// Where the text before a note's first field is, without the lines about the note (dates, readers).
fn preamble(text: &str) -> Option<(usize, usize)> {
    let mut kept: Option<(usize, usize)> = None;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        if !BOILERPLATE.is_match(content) {
            let start = at + (content.len() - content.trim_start().len());
            let end = at + content.trim_end().len();
            kept = Some((kept.map_or(start, |(first, _)| first), end));
        }
        at += line.len();
    }
    kept
}

fn kind(opening: Opening, signer: &Signer, body: &str, sections: &[Section]) -> Kind {
    match opening {
        Opening::Review => {
            let mut given = scores(body, true);
            given.sort_by_key(|(field, _, _)| *field);
            let mut confidence = None;
            let mut kept = Vec::new();
            for (field, value, meaning) in given {
                let score = Score { field, value, meaning: meaning.trim_end().to_owned() };
                if field == Field::Confidence { confidence = Some(score) } else { kept.push(score) }
            }
            Kind::OfficialReview { scores: kept, confidence }
        }
        Opening::Meta => Kind::MetaReview,
        Opening::Decision => {
            let said = sections
                .iter()
                .find(|section| section.name.eq_ignore_ascii_case("decision"))
                .or(sections.first())
                .map_or(String::new(), |section| squash(section.text.lines().next().unwrap_or_default()));
            Kind::Decision { decision: first_chars(&said, DECISION_LENGTH).to_owned() }
        }
        Opening::Public => Kind::PublicComment,
        Opening::Ethics => Kind::EthicsReview,
        Opening::Withdrawal => Kind::Withdrawal,
        Opening::Rebuttal | Opening::Comment => match signer {
            Signer::Authors => Kind::AuthorResponse,
            Signer::Reviewer { .. } | Signer::EthicsReviewer => Kind::ReviewerComment,
            Signer::AreaChair | Signer::SeniorAreaChair | Signer::ProgramChairs => Kind::ChairComment,
            Signer::Public => Kind::PublicComment,
            Signer::NotShown => Kind::Comment,
        },
    }
}

/// The first reviewer `text` names who wrote one of the reviews.
fn named_reviewer(text: &str, reviews: &HashMap<String, usize>) -> Option<usize> {
    MENTION.captures_iter(text).find_map(|found| {
        let id = found.get(1).or(found.get(2))?.as_str();
        reviews.get(id).copied()
    })
}

/// Which review each comment answers ([`Reply`]); `titles` and `openings` are the notes' own titles and the
/// text of their first lines.
fn attach(notes: &mut [Note], titles: &[Option<String>], openings: &[&str]) {
    let mut reviews: HashMap<String, usize> = HashMap::new();
    for (index, note) in notes.iter().enumerate() {
        if let (Kind::OfficialReview { .. }, Some(id)) = (&note.kind, note.reviewer()) {
            reviews.entry(id.to_owned()).or_insert(index);
        }
    }
    let mut last_review: Option<usize> = None;
    for index in 0..notes.len() {
        let note = &notes[index];
        let reply = if !note.kind.is_comment() {
            Reply::Submission
        } else if let Some(review) = titles[index].as_deref().and_then(|title| named_reviewer(title, &reviews)) {
            Reply::Named { note: review }
        } else if let Some(&review) = note.reviewer().and_then(|id| reviews.get(id)) {
            Reply::Guessed { note: review, basis: Basis::SameReviewer }
        } else if let Some(review) = named_reviewer(openings[index], &reviews) {
            Reply::Guessed { note: review, basis: Basis::MentionsReviewer }
        } else if let Some(review) = last_review {
            Reply::Guessed { note: review, basis: Basis::FollowsReview }
        } else {
            Reply::NotKnown
        };
        match note.kind {
            Kind::OfficialReview { .. } => last_review = Some(index),
            Kind::MetaReview | Kind::Decision { .. } => last_review = None, // a new branch of the thread
            _ => {}
        }
        notes[index].reply = reply;
    }
}

/// The heading as written: the note's own title and its heading lines, without the dates and readers between.
fn heading_of(header: &str, title: Option<&str>) -> String {
    let lines: Vec<&str> =
        header.lines().map(str::trim).filter(|line| HEADING.is_match(line) || Some(*line) == title).collect();
    squash(&lines.join(" "))
}

/// The discussion on a reviews page, or `None` if the text has no notes (not loaded yet, not a forum page).
pub fn read_discussion(text: &str) -> Option<Discussion> {
    let text = first_chars(text, MAX_TEXT);
    let thread_end = FOOTER.find(text).map_or(text.len(), |footer| footer.start());
    let found = headings(&text[..thread_end]);
    if found.is_empty() {
        return None;
    }
    let chars = CharIndex::new(text);
    let mut notes = Vec::new();
    let mut titles = Vec::new();
    let mut openings = Vec::new();
    for (index, heading) in found.iter().enumerate().take(MAX_NOTES) {
        let end = found.get(index + 1).map_or(thread_end, |next| next.start);
        let header = &text[heading.start..heading.end];
        let signer = signer(header, heading.opening);
        let body = &text[heading.end..end];
        let sections = sections(text, &chars, heading.end, end, heading.opening == Opening::Review);
        let kind = kind(heading.opening, &signer, body, &sections);
        let note_end = heading.end + body.trim_end().len();
        notes.push(Note {
            kind,
            signer,
            heading: heading_of(header, heading.title.as_deref()),
            span: Span { start: chars.at(heading.start), end: chars.at(note_end) },
            sections,
            reply: Reply::Submission,
        });
        titles.push(heading.title.clone());
        openings.push(&body[..floor_boundary(body, MENTION_REACH)]);
    }
    attach(&mut notes, &titles, &openings);
    // the venue's own label ("ICLR 2024 Conference", "ICLR.cc/2024/Conference"), else a name and year above the
    // first note: a review citing another venue's paper does not name this one
    let first_note = found.first().map_or(thread_end, |heading| heading.start);
    let venue = VENUE_LABEL
        .captures(text)
        .or_else(|| VENUE.captures(&text[..first_note]))
        .and_then(|found| Some(Venue::new(found.get(1)?.as_str(), found.get(2)?.as_str().parse().ok()?)));
    Some(Discussion { parser: PARSER, venue, notes })
}

/// Pages shaped like OpenReview forum pages of several venues and years, as the reader's view hands their text
/// over. Every word of them is made up for these tests: no reviewer's text is copied here.
#[cfg(test)]
pub(crate) mod fixtures {
    const PRAISE: [&str; 6] = [
        "The method is novel and simple.",
        "The writing is clear.",
        "The theoretical analysis is sound.",
        "Strong empirical results on many benchmarks.",
        "The code is released.",
        "A well motivated problem.",
    ];
    const CRITICISM: [&str; 6] = [
        "The baselines are weak.",
        "The experiments are limited to small models.",
        "An ablation study is missing.",
        "The related work misses recent papers.",
        "The notation is confusing.",
        "The compute cost is not reported.",
    ];

    /// A made-up ICLR-style page for paper number `paper`, with one review per (rating, confidence) given; what
    /// the reviews praise and criticise is picked from short made-up lists by the paper and review numbers.
    pub(crate) fn synthetic_page(paper: usize, reviews: &[(u8, u8)]) -> String {
        let mut page = format!("Go to ICLR 2024 Conference homepage\nPaper {paper}\n");
        for (review, (rating, confidence)) in reviews.iter().enumerate() {
            let pick = |list: &[&'static str; 6], offset: usize| list[(paper * 7 + review * 3 + offset) % list.len()];
            page.push_str(&format!(
                "Official Review of Submission{paper} by Reviewer R{review}x\nSummary:\nA paper.\nSoundness: 3 good\n\
                 Strengths:\n{} {}\nWeaknesses:\n{} {}\nQuestions:\nNone.\nRating: {rating}: a rating\n\
                 Confidence: {confidence}: a confidence\n",
                pick(&PRAISE, 0),
                pick(&PRAISE, 1),
                pick(&CRITICISM, 0),
                pick(&CRITICISM, 2),
            ));
        }
        page
    }

    /// NeurIPS 2023: separate Strengths and Weaknesses, sub-scores of 1 to 4, a rating of 1 to 10, oldest first,
    /// a rebuttal, a titled reply, a meta-review and the decision.
    pub(crate) const NEURIPS_2023: &str = "Go to NeurIPS 2023 Conference homepage
A Made-Up Routing Rule for Sparse Experts
Anonymous Authors
Published: 21 Sept 2023, Last Modified: 15 Jan 2024
NeurIPS 2023 poster
Readers: Everyone
Keywords: sparse experts, routing
Abstract: We study an invented routing rule. Summary: none of this is real.
Submission Number: 4321
Filter by reply type...
Sort: Oldest First
9 / 9 replies shown
Official Review of Submission4321 by Reviewer aBcD
Official Review by Reviewer aBcD
04 Jul 2023, 10:12 (modified: 12 Dec 2023, 18:01)
Everyone
Revisions
Summary:
The submission proposes a routing rule for sparse experts and tests it on two benchmarks.
Soundness: 3 good
Presentation: 2 fair
Contribution: 3 good
Strengths:
The routing rule is simple and novel. The theoretical analysis is clear and convincing.
Weaknesses:
The experiments are limited to two small benchmarks. The baselines are weak, and an ablation study is missing.
Questions:
How does the rule behave with sixty-four experts?
Limitations:
Discussed adequately.
Flag For Ethics Review: No ethics review needed.
Rating: 6: Weak Accept: Technically solid, moderate-to-high impact paper
Confidence: 4: You are confident in your assessment, but not absolutely certain.
Code Of Conduct: Yes
Official Review of Submission4321 by Reviewer eFgH
Official Review by Reviewer eFgH
06 Jul 2023, 08:00
Everyone
Summary:
An invented method for routing tokens.
Soundness: 2 fair
Presentation: 3 good
Contribution: 2 fair
Strengths:
The writing is clear and the theoretical analysis is sound.
Weaknesses:
The experiments are limited and the baselines are weak. The ablation study does not isolate the rule.
Questions:
Why were larger models not tried?
Limitations:
Yes.
Flag For Ethics Review: No ethics review needed.
Rating: 4: Borderline reject: Technically solid paper where reasons to reject outweigh reasons to accept
Confidence: 3: You are fairly confident in your assessment.
Code Of Conduct: Yes
Rebuttal by Authors
Rebuttal by Authors
09 Aug 2023, 21:00
Everyone
Rebuttal:
We thank Reviewer aBcD for the careful reading and add a third benchmark.
Official Comment by Reviewer aBcD
Official Comment by Reviewer aBcD
12 Aug 2023, 10:00
Everyone
Comment:
Thank you, the new benchmark answers my question. I keep my rating.
Response to Reviewer eFgH
Official Comment by Authors 10 Aug 2023, 09:00
Everyone
Comment:
We ran the larger models as suggested.
Meta Review of Submission4321 by Area Chair XyZw
Meta Review by Area Chair XyZw
05 Sept 2023, 12:00
Everyone
Metareview:
The reviewers agree the rule is simple; the experiments were extended during the discussion.
Recommendation: Accept (poster)
Confidence: 4: The area chair is confident but not absolutely certain
Paper Decision
Decision by Program Chairs
21 Sept 2023, 17:00
Everyone
Decision: Accept (poster)
Comment: Congratulations.
About OpenReview
Hosting a Venue
";

    /// NeurIPS 2025: one field for strengths and weaknesses with the reviewer's own headings inside it, quality,
    /// clarity, significance and originality of 1 to 4, a rating of 1 to 6, newest first.
    pub(crate) const NEURIPS_2025: &str = "Go to NeurIPS 2025 Conference homepage
An Invented Benchmark for Long Contexts
Sort: Newest First
Paper Decision
Decision by Program Chairs 18 Sept 2025, 20:00
Everyone
Decision: Reject
Comment: The paper does not meet the bar this year.
Official Review of Submission987 by Reviewer Qr12
Official Review by Reviewer Qr12 01 Jul 2025, 09:00
Everyone
Summary:
The paper introduces an invented benchmark of long documents.
Strengths And Weaknesses:
Strengths:
- The benchmark is large and carefully documented.
- The evaluation protocol is clear.
Weaknesses:
- The baselines are weak and outdated.
- The evaluation ignores contamination.
Quality: 2: fair
Clarity: 3: good
Significance: 2: fair
Originality: 2: fair
Questions:
How was contamination checked?
Limitations:
No.
Rating: 3: Borderline reject
Confidence: 4: You are confident in your assessment, but not absolutely certain.
Final Justification:
My concerns about contamination remain.
Paper Formatting Concerns: None
Official Comment by Authors
Official Comment by Authors 02 Aug 2025, 10:00
Everyone
Comment:
Dear Reviewer Qr12, we checked contamination with two methods.
Official Review of Submission987 by Reviewer St34
Official Review by Reviewer St34 03 Jul 2025, 11:00
Everyone
Summary:
A benchmark for long contexts.
Strengths And Weaknesses:
Strengths: the benchmark is large and the documentation is excellent.
Weaknesses: the baselines are weak.
Quality: 3: good
Clarity: 4: excellent
Significance: 3: good
Originality: 3: good
Questions:
None.
Limitations:
Yes.
Rating: 5: Accept
Confidence: 2: You are willing to defend your assessment, but it is quite likely that you did not understand central parts.
Mandatory Acknowledgement by Reviewer St34
Mandatory Acknowledgement by Reviewer St34 08 Aug 2025, 10:00
Everyone
Acknowledgement: I have read the rebuttal.
About OpenReview
";

    /// ICLR 2021: the older layout, a title of the reviewer's own above each signature line, anonymous reviewers
    /// by number, one Review field, a rating of 1 to 10, and the decision with the meta-review in it.
    pub(crate) const ICLR_2021: &str = "Go to ICLR 2021 Conference homepage
Made-Up Regularisers for Small Networks
Paper Decision
Decision by Program Chairs 12 Jan 2021 (modified: 13 Jan 2021)
ICLR 2021 Conference Paper1111 Decision
Readers: Everyone
Decision: Accept (Poster)
Comment: The reviewers found the regulariser useful.
Interesting idea, limited evaluation
Official Review by AnonReviewer2 26 Oct 2020 (modified: 10 Nov 2020)
ICLR 2021 Conference Paper1111 Official Review
Readers: Everyone
Review: The paper proposes an invented regulariser. The idea is novel but the evaluation is limited to one dataset and the baselines are weak.
Rating: 5: Marginally below acceptance threshold
Confidence: 4: The reviewer is confident but not absolutely certain that the evaluation is correct
Reply to AnonReviewer2
Official Comment by Paper1111 Authors 20 Nov 2020
ICLR 2021 Conference Paper1111 Official Comment
Readers: Everyone
Comment: We add two datasets, as AnonReviewer2 asked.
Clear and useful
Official Review by AnonReviewer3 28 Oct 2020
ICLR 2021 Conference Paper1111 Official Review
Readers: Everyone
Review: A clear paper. The idea is novel and the writing is excellent.
Rating: 7: Good paper, accept
Confidence: 3: The reviewer is fairly confident that the evaluation is correct
";

    /// ICLR 2024: separate fields, soundness, presentation and contribution of 1 to 4, a rating of 1 to 10, newest
    /// first, with each review's replies under it.
    pub(crate) const ICLR_2024: &str = "Go to ICLR 2024 Conference homepage
An Imaginary Optimiser
Sort: Newest First
Paper Decision
Decision by Program Chairs 16 Jan 2024, 19:55
Everyone
Decision: Accept (spotlight)
Meta Review of Submission2468 by Area Chair Mn56
Meta Review by Area Chair Mn56 10 Dec 2023, 12:00
Everyone
Metareview:
A strong paper about an imaginary optimiser.
Justification For Why Not Higher Score:
Experiments on larger models would help.
Justification For Why Not Lower Score:
All reviewers recommend acceptance.
Official Review of Submission2468 by Reviewer Uv78
Official Review by Reviewer Uv78 30 Oct 2023, 10:00
Everyone
Summary:
An imaginary optimiser with a convergence proof.
Soundness: 4 excellent
Presentation: 3 good
Contribution: 4 excellent
Strengths:
The convergence proof is novel and the writing is clear.
Weaknesses:
The experiments are limited to small models.
Questions:
Does it scale?
Flag For Ethics Review: No ethics review needed.
Rating: 8: accept, good paper
Confidence: 4: You are confident in your assessment, but not absolutely certain.
Code Of Conduct: Yes
Thanks for the review
Official Comment by Authors 20 Nov 2023, 10:00
Everyone
Comment:
We thank the reviewer and add experiments on a larger model.
Official Review of Submission2468 by Reviewer Wx90
Official Review by Reviewer Wx90 01 Nov 2023, 10:00
Everyone
Summary:
An optimiser.
Soundness: 3 good
Presentation: 3 good
Contribution: 3 good
Strengths:
The method is simple and the convergence proof is novel.
Weaknesses:
The baselines are weak.
Questions:
None.
Flag For Ethics Review: No ethics review needed.
Rating: 6: marginally above the acceptance threshold
Confidence: 3: You are fairly confident in your assessment.
Code Of Conduct: Yes
";

    /// ICML 2024: like ICLR, with strengths and weaknesses in one field and a rating of 1 to 10.
    pub(crate) const ICML_2024: &str = "Go to ICML 2024 Conference homepage
A Fictional Study of Calibration
Official Review of Submission555 by Reviewer Ab01
Official Review by Reviewer Ab01 01 Mar 2024, 10:00
Everyone
Summary:
A fictional study of calibration under shift.
Strengths And Weaknesses:
Pros: the study is thorough. Cons: the theory is thin.
Questions:
Which datasets were held out?
Limitations:
Discussed.
Soundness: 3: good
Presentation: 3: good
Contribution: 2: fair
Rating: 5: Borderline accept: Technically solid paper where reasons to accept outweigh reasons to reject
Confidence: 3: You are fairly confident in your assessment.
Code Of Conduct: Yes
";

    /// ICML 2025: the long form (claims, methods, theory, experiments...), an overall recommendation of 1 to 5,
    /// rebuttal comments.
    pub(crate) const ICML_2025: &str = "Go to ICML 2025 Conference homepage
A Pretend Theory of Grokking
Official Review of Submission3141 by Reviewer Cd23
Official Review by Reviewer Cd23 10 Mar 2025, 10:00
Everyone
Summary:
A pretend theory of delayed generalisation.
Claims And Evidence:
The main claim is supported by the toy experiments.
Methods And Evaluation Criteria:
Reasonable.
Theoretical Claims:
I checked the first theorem.
Experimental Designs Or Analyses:
Only toy settings are used.
Supplementary Material:
Code was provided.
Relation To Broader Scientific Literature:
Related work is covered.
Essential References Not Discussed:
None.
Other Strengths And Weaknesses:
The writing is clear; the experiments are limited to toy settings.
Other Comments Or Suggestions:
Typo on page 3.
Questions For Authors:
Does the theory predict the delay?
Overall Recommendation: 3: Weak accept (i.e., leaning towards accept, but could also be rejected)
Rebuttal by Authors
Rebuttal by Authors 01 Apr 2025, 10:00
Everyone
Rebuttal:
We add a larger experiment.
Rebuttal Comment by Reviewer Cd23
Rebuttal Comment by Reviewer Cd23 05 Apr 2025, 10:00
Everyone
Comment:
Thanks, I raise my score.
";
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn read(page: &str) -> Discussion {
        read_discussion(page).unwrap()
    }

    fn kinds(discussion: &Discussion) -> Vec<&'static str> {
        discussion
            .notes
            .iter()
            .map(|note| match note.kind {
                Kind::OfficialReview { .. } => "review",
                Kind::MetaReview => "meta",
                Kind::Decision { .. } => "decision",
                Kind::AuthorResponse => "authors",
                Kind::ReviewerComment => "reviewer",
                Kind::ChairComment => "chair",
                Kind::Comment => "comment",
                Kind::PublicComment => "public",
                Kind::EthicsReview => "ethics",
                Kind::Withdrawal => "withdrawal",
            })
            .collect()
    }

    fn names(note: &Note) -> Vec<&str> {
        note.sections.iter().map(|section| section.name.as_str()).collect()
    }

    fn scores_of(note: &Note) -> Vec<(Field, u8)> {
        match &note.kind {
            Kind::OfficialReview { scores, .. } => scores.iter().map(|score| (score.field, score.value)).collect(),
            _ => Vec::new(),
        }
    }

    fn confidence_of(note: &Note) -> Option<u8> {
        match &note.kind {
            Kind::OfficialReview { confidence, .. } => confidence.as_ref().map(|score| score.value),
            _ => None,
        }
    }

    /// The page's text at a span, cut by characters.
    fn at(page: &str, span: Span) -> String {
        page.chars().skip(span.start).take(span.end - span.start).collect()
    }

    #[test]
    fn every_note_of_a_neurips_2023_thread_is_read_as_what_it_is() {
        let discussion = read(NEURIPS_2023);
        assert_eq!(discussion.venue, Some(Venue::new("neurips", 2023)));
        assert_eq!(kinds(&discussion), ["review", "review", "authors", "reviewer", "authors", "meta", "decision"]);
        let signers: Vec<&Signer> = discussion.notes.iter().map(|note| &note.signer).collect();
        assert_eq!(signers[0], &Signer::Reviewer { id: "aBcD".into() });
        assert_eq!(signers[2], &Signer::Authors);
        assert_eq!(signers[5], &Signer::AreaChair);
        assert_eq!(signers[6], &Signer::ProgramChairs);
        let first = &discussion.notes[0];
        assert_eq!(
            names(first),
            [
                "Summary",
                "Soundness",
                "Presentation",
                "Contribution",
                "Strengths",
                "Weaknesses",
                "Questions",
                "Limitations",
                "Flag For Ethics Review",
                "Rating",
                "Confidence",
                "Code Of Conduct"
            ]
        );
        assert_eq!(
            scores_of(first),
            [(Field::Rating, 6), (Field::Soundness, 3), (Field::Presentation, 2), (Field::Contribution, 3)]
        );
        assert_eq!(confidence_of(first), Some(4));
        assert_eq!(discussion.notes[6].kind, Kind::Decision { decision: "Accept (poster)".into() });
        // the meta-review's confidence is not a review's, and its recommendation is a section of its own
        assert_eq!(names(&discussion.notes[5]), ["Metareview", "Recommendation", "Confidence"]);
        assert_eq!(discussion.review_count(), 2);
    }

    #[test]
    fn a_sections_text_is_exactly_the_page_at_its_span() {
        for page in [NEURIPS_2023, NEURIPS_2025, ICLR_2021, ICLR_2024, ICML_2024, ICML_2025] {
            let discussion = read(page);
            for note in &discussion.notes {
                assert!(at(page, note.span).contains(note.heading.split(' ').next().unwrap()), "{}", note.heading);
                for section in &note.sections {
                    assert_eq!(at(page, section.span), section.text, "{}", section.name);
                    assert!(note.span.start <= section.span.start && section.span.end <= note.span.end);
                }
            }
        }
        let weaknesses = &read(NEURIPS_2023).notes[0].sections[5];
        assert_eq!(weaknesses.name, "Weaknesses");
        assert!(weaknesses.text.starts_with("The experiments are limited to two small benchmarks."));
        assert!(weaknesses.text.ends_with("an ablation study is missing."));
    }

    #[test]
    fn spans_count_characters_not_bytes() {
        let page = "Résumé — ünïcödé\nOfficial Review of Submission1 by Reviewer Zz99\nSummary:\nÉtude très claire.\nRating: 8: accept\n";
        let discussion = read(page);
        let summary = &discussion.notes[0].sections[0];
        assert_eq!(summary.text, "Étude très claire.");
        assert_eq!(at(page, summary.span), summary.text);
        assert_ne!(summary.span.start, page.find("Étude").unwrap()); // a byte offset would be further on
    }

    #[test]
    fn replies_are_attached_by_what_the_page_says_and_guesses_say_so() {
        let discussion = read(NEURIPS_2023);
        let replies: Vec<&Reply> = discussion.notes.iter().map(|note| &note.reply).collect();
        assert_eq!(replies[0], &Reply::Submission);
        // the global rebuttal comes after the second review: a guess from the order
        assert_eq!(replies[2], &Reply::Guessed { note: 0, basis: Basis::MentionsReviewer });
        assert_eq!(replies[3], &Reply::Guessed { note: 0, basis: Basis::SameReviewer });
        assert_eq!(replies[4], &Reply::Named { note: 1 }); // "Response to Reviewer eFgH"
        assert_eq!(
            discussion.notes[4].heading,
            "Response to Reviewer eFgH Official Comment by Authors 10 Aug 2023, 09:00"
        );
        assert_eq!(replies[5], &Reply::Submission);

        let newest_first = read(ICLR_2024);
        assert_eq!(kinds(&newest_first), ["decision", "meta", "review", "authors", "review"]);
        assert_eq!(newest_first.notes[3].reply, Reply::Guessed { note: 2, basis: Basis::FollowsReview });
        let mentioned = read(NEURIPS_2025);
        assert_eq!(mentioned.notes[2].reply, Reply::Guessed { note: 1, basis: Basis::MentionsReviewer });
    }

    #[test]
    fn neurips_2025_keeps_one_field_for_strengths_and_weaknesses_and_its_scales_fields() {
        let discussion = read(NEURIPS_2025);
        assert_eq!(kinds(&discussion), ["decision", "review", "authors", "review", "reviewer"]);
        assert_eq!(discussion.notes[0].kind, Kind::Decision { decision: "Reject".into() });
        let review = &discussion.notes[1];
        assert_eq!(
            names(review),
            [
                "Summary",
                "Strengths And Weaknesses",
                "Quality",
                "Clarity",
                "Significance",
                "Originality",
                "Questions",
                "Limitations",
                "Rating",
                "Confidence",
                "Final Justification",
                "Paper Formatting Concerns"
            ]
        );
        assert!(review.sections[1].text.contains("Weaknesses:\n- The baselines are weak"));
        assert_eq!(
            scores_of(review),
            [
                (Field::Rating, 3),
                (Field::Quality, 2),
                (Field::Clarity, 3),
                (Field::Significance, 2),
                (Field::Originality, 2)
            ]
        );
        assert_eq!(discussion.notes[4].reply, Reply::Guessed { note: 3, basis: Basis::SameReviewer });
    }

    #[test]
    fn the_older_iclr_layout_with_titles_of_the_reviewers_own_is_read() {
        let discussion = read(ICLR_2021);
        assert_eq!(discussion.venue, Some(Venue::new("ICLR", 2021)));
        assert_eq!(kinds(&discussion), ["decision", "review", "authors", "review"]);
        let review = &discussion.notes[1];
        assert_eq!(review.signer, Signer::Reviewer { id: "AnonReviewer2".into() });
        assert!(review.heading.starts_with("Interesting idea, limited evaluation Official Review by AnonReviewer2"));
        assert_eq!(names(review), ["Review", "Rating", "Confidence"]);
        assert_eq!(scores_of(review), [(Field::Rating, 5)]);
        assert_eq!(discussion.notes[2].reply, Reply::Named { note: 1 }); // "Reply to AnonReviewer2"
        assert_eq!(discussion.notes[0].kind, Kind::Decision { decision: "Accept (Poster)".into() });
    }

    #[test]
    fn icml_forms_of_two_years_are_kept_with_their_own_names() {
        let earlier = read(ICML_2024);
        assert_eq!(earlier.venue, Some(Venue::new("ICML", 2024)));
        assert_eq!(names(&earlier.notes[0])[1], "Strengths And Weaknesses");
        let later = read(ICML_2025);
        assert_eq!(kinds(&later), ["review", "authors", "reviewer"]);
        assert_eq!(
            names(&later.notes[0]),
            [
                "Summary",
                "Claims And Evidence",
                "Methods And Evaluation Criteria",
                "Theoretical Claims",
                "Experimental Designs Or Analyses",
                "Supplementary Material",
                "Relation To Broader Scientific Literature",
                "Essential References Not Discussed",
                "Other Strengths And Weaknesses",
                "Other Comments Or Suggestions",
                "Questions For Authors",
                "Overall Recommendation"
            ]
        );
        assert_eq!(scores_of(&later.notes[0]), [(Field::OverallRecommendation, 3)]);
        assert_eq!(confidence_of(&later.notes[0]), None);
        assert_eq!(later.notes[2].reply, Reply::Guessed { note: 0, basis: Basis::SameReviewer });
    }

    #[test]
    fn prose_with_a_colon_does_not_start_a_section() {
        let page = "Official Review of Submission1 by Reviewer Ab12\nSummary:\nThe idea: route tokens.\nNote: the \
                    proofs are sketched.\nQuality: 2 of the 3 proofs are sketched only.\nWeaknesses: few.\nQuality: 2: fair\nRating: 4: weak reject\n";
        let discussion = read(page);
        assert_eq!(names(&discussion.notes[0]), ["Summary", "Weaknesses", "Quality", "Rating"]);
        assert!(discussion.notes[0].sections[0].text.contains("Quality: 2 of the 3 proofs"));
    }

    #[test]
    fn two_collapsed_reviews_in_a_row_stay_two_and_a_cited_venue_is_not_the_pages() {
        let page = "Go to ICML 2024 Conference homepage\nOfficial Review of Submission1 by Reviewer Aa11\nEveryone\n\
                    Official Review of Submission1 by Reviewer Bb22\nSummary:\nAs in a paper at ICLR 2023, fine.\nRating: 6: ok\n";
        let discussion = read(page);
        assert_eq!(discussion.review_count(), 2);
        assert_eq!(discussion.venue, Some(Venue::new("ICML", 2024)));
        let unlabelled =
            "Official Review of Submission1 by Reviewer Aa11\nSummary:\nLike ICLR 2023 work.\nRating: 6: ok\n";
        assert_eq!(read(unlabelled).venue, None);
    }

    #[test]
    fn a_page_without_notes_is_no_discussion() {
        assert_eq!(read_discussion("Just a moment... checking your browser."), None);
        assert_eq!(read_discussion(""), None);
        let decisions_in_prose = "Decision boundaries are discussed.\nRebuttal acknowledged, I keep my score.\n";
        assert_eq!(read_discussion(decisions_in_prose), None);
    }

    #[test]
    fn the_reading_is_written_as_the_interface_reads_it() {
        let discussion = read(ICML_2024);
        let json = serde_json::to_value(&discussion).unwrap();
        assert_eq!(json["parser"], PARSER);
        assert_eq!(json["venue"], serde_json::json!({"conference": "ICML", "year": 2024}));
        let note = &json["notes"][0];
        assert_eq!(note["kind"], "official_review");
        assert_eq!(note["signer"], serde_json::json!({"role": "reviewer", "id": "Ab01"}));
        assert_eq!(note["reply"], serde_json::json!({"answers": "submission"}));
        assert_eq!(
            note["scores"][0],
            serde_json::json!({"field": "Rating", "value": 5, "meaning": "Borderline accept: Technically solid paper where reasons to"})
        );
        assert_eq!(note["confidence"]["value"], 3);
        assert_eq!(note["sections"][0]["span"], serde_json::json!({"start": 189, "end": 234}));
        let back: Discussion = serde_json::from_value(json).unwrap();
        assert_eq!(back, discussion);
    }

    #[test]
    fn a_conference_papers_id_names_its_venue() {
        assert_eq!(Venue::of_paper("neurips-2025-12"), Some(Venue::new("NeurIPS", 2025)));
        assert_eq!(Venue::of_paper("iclr-2024-7").unwrap().to_string(), "ICLR 2024");
        assert_eq!(Venue::of_paper("2601.00001"), None);
    }
}
