//! A paper's text made addressable: pages, and on each page its sections, paragraphs and sentences, so that
//! anything said about a paper can point at the place in it that says so, and a click on that pointer lands there.
//!
//! The text comes from the PDF page by page ([`crate::text::pdf::pdf_pages`]), in the order the page draws it, which
//! for the two-column templates of the conferences read here is column by column with a blank line between
//! paragraphs. What a PDF does to that text, and what is done about it:
//!
//! | in the PDF                                  | here                                                          |
//! |---------------------------------------------|---------------------------------------------------------------|
//! | a running header or footer, a page number   | dropped (a line repeated at the top or bottom of many pages)  |
//! | a word hyphenated at a line end             | joined again ("ana-" "lytic" is "analytic")                   |
//! | a paragraph cut by a column break, with the column's footnotes between its halves | joined, the footnotes after it |
//! | a paragraph that runs on to the next page   | two passages, the second marked [`Flow::Continues`]            |
//! | a figure's or a table's caption             | a passage of its own kind                                      |
//! | an equation                                 | kept as the text the PDF has for it, of kind [`Kind::Equation`] |
//! | text drawn inside a figure or a table       | kept, of kind [`Kind::Fragment`]                               |
//! | the reference list                          | in the page's text but not a passage: it is [`super::references`]'s |
//! | the appendices, the checklist               | passages, marked by their [`Part`]                             |
//!
//! Each page's text is rebuilt from what is kept, paragraph after paragraph with a blank line between them, and
//! every address counts characters in that rebuilt text. The rebuilding is deterministic: the same PDF read by the
//! same [`READER`] gives the same pages and so the same addresses, which is what makes an address stored with a
//! piece of evidence still point at the same words later. When the reading changes, [`READER`] goes up and what
//! was read by an older reader is read again.
//!
//! Two more things are read from the passages: where a quote is in the paper ([`Passages::locate`]: what the
//! evidence a feature shows is checked with, `crate::evidence`), and which sentences cite each entry of the
//! reference list ([`citing_sentences`]).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use super::references::{
    APPENDIX_HEADING, Citing, END_HEADING, REFERENCES_HEADING, Reference, ReferenceList, accents_composed,
};
use crate::text::declarations::{DANGLING, FURNITURE, RUNNING};
use crate::text::pattern;

/// The version of this reader. Raise it when what it reads changes: addresses are only stable for one version.
pub const READER: u32 = 1;

/// Top and bottom lines of a page looked at for running headers, footers and page numbers.
const EDGE_LINES: usize = 3;
/// A quote's letters and digits this many or more may be found with a few differences; a shorter one only exactly.
const SHORTEST_TOLERANT: usize = 20;
/// Most differences a tolerant match allows, whatever the quote's length.
const MOST_DIFFERENCES: usize = 8;
/// A quote longer than this (in letters and digits) is only looked for exactly: the tolerant search is quadratic.
const LONGEST_TOLERANT: usize = 2_000;

/// Where something is in a paper: a page, the section and paragraph on it, and the characters of the page's text.
///
/// `page` and `end_page` are the PDF's own page numbers, from 1 (what `#page=N` opens), not the numbers the paper
/// prints. `start` counts characters (Unicode scalar values, not bytes and not UTF-16 units) into the text of
/// `page`, `end` into the text of `end_page`, which is `page` unless a quote runs on to the next page.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Address {
    pub page: u32,
    /// The heading of the section as the paper writes it ("3.2. Results"); "" before the first heading.
    pub section: String,
    /// Within the section, from 0, the heading itself being 0 when there is one.
    pub paragraph: u32,
    pub start: u32,
    pub end_page: u32,
    pub end: u32,
}

/// What a passage is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The paper's title, on its first page.
    Title,
    Heading,
    /// Running prose.
    Text,
    FigureCaption,
    TableCaption,
    /// An equation set apart from the text, as the PDF has it (math is rarely kept well).
    Equation,
    /// A footnote, an affiliation, the venue's notice.
    Footnote,
    /// Text drawn inside a figure or a table, author lists, anything that is not running prose.
    Fragment,
}

/// Which part of the paper a passage is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    /// Before the first heading: title, authors, affiliations.
    Front,
    Body,
    Appendix,
    /// The NeurIPS paper checklist.
    Checklist,
}

/// Does a passage start a paragraph, or carry on one from the page before?
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flow {
    Starts,
    Continues,
}

/// A stretch of a page's text, in characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

/// One paragraph (or caption, equation, heading) of a page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passage {
    pub kind: Kind,
    pub part: Part,
    pub flow: Flow,
    /// Its own place: `start..end` of its page's text.
    pub address: Address,
    /// The same characters as its place in the page's text.
    pub text: String,
    /// Its sentences, as places in the page's text.
    pub sentences: Vec<Span>,
}

/// A page's text as rebuilt from what is kept of it; addresses count characters in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    pub number: u32,
    pub text: String,
}

/// A paper's text, addressable: `PaperPassages` in the contracts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passages {
    /// The [`READER`] that read it.
    pub reader: u32,
    pub pages: Vec<Page>,
    /// In reading order; the reference list is not among them.
    pub passages: Vec<Passage>,
}

/// How a quote was found in a paper.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Match {
    /// The same letters and digits in the same order: only spacing, line breaks, hyphenation, ligatures, accents,
    /// quote marks, punctuation and case differ.
    Exact,
    /// A few letters differ inside words (a typo, an OCR slip), word for word the same otherwise: never a word added
    /// or left out, a number or a negation changed (`close_enough`).
    Close { differences: u32 },
}

/// A quote found in a paper: where, and how the paper itself words it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Located {
    pub address: Address,
    /// The paper's own words at that place, whole words, as its text has them.
    pub wording: String,
    pub matched: Match,
}

/// What `POST /api/papers/<id>/locate` answers: `{"found": true, "address": ..., "wording": ..., "matched": ...}`
/// or `{"found": false}` (`LocateAnswer` in the contracts).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LocateAnswer {
    found: bool,
    #[serde(flatten)]
    located: Option<Located>,
}

impl From<Option<Located>> for LocateAnswer {
    fn from(located: Option<Located>) -> Self {
        Self { found: located.is_some(), located }
    }
}

/// How a sentence points at an entry of the reference list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Marker {
    /// "[12]", "[3, 7-9]".
    Number,
    /// A number written small after a word, which comes out of the PDF as "models.12": a reading, less sure.
    Superscript,
    /// "(Vaswani et al., 2017)", "Vaswani and Shazeer (2017)".
    AuthorYear,
}

/// A sentence of the paper that cites a reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitingSentence {
    pub address: Address,
    pub sentence: String,
    pub marker: Marker,
    /// The citation as the sentence writes it: "[3, 7-9]", "Vaswani et al., 2017".
    pub written: String,
}

/// An entry of the reference list and the sentences that cite it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cited {
    /// Its position in the reference list, from 0.
    pub reference: usize,
    pub sentences: Vec<CitingSentence>,
}

// ------------------------------------------------------------------------------------------------ reading pages

/// Lines that are a page's number, as printed: "7", "vii", "Page 7", "7 of 30".
static PAGE_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?i)^(?:page\s+)?(?:\d{1,4}|[ivxlc]{1,7})(?:\s*(?:of|/)\s*\d{1,4})?$"));
/// The stamp arXiv puts in the margin of a paper's first page.
static ARXIV_STAMP: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^arXiv:\d{4}\.\d{4,5}v\d+\s+\[[^\]]+\]\s+\d{1,2}\s+\w{3}\s+\d{4}$"));
/// A numbered heading: "3 Method", "2.1. Problem Statement", "A. Proofs", "B.2 More results".
static NUMBERED_HEADING: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^(?:[1-9]\d?(?:\.\d{1,2})*\.?|[A-H](?:\.\d{1,2})*\.)\s+\p{Lu}"));
/// A subsection's heading in capitals that runs into its paragraph, as ICML sets them: "4.3.5. ROBUSTNESS AND
/// SCOPE We finally test". The heading ends where a capitalised word in lower case begins.
static RUN_IN_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"^([1-9]\d?(?:\.\d{1,2})+\.?\s+[\p{Lu}\d][\p{Lu}\d&:\-]+(?:\s+[\p{Lu}\d&:\-]+)*)\s+\p{Lu}\p{Ll}")
});
/// An appendix heading in capitals with no full stop after its letter: "A PROOFS".
static CAPITAL_APPENDIX: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^[A-H](?:\.\d{1,2})*\s+[\p{Lu}\d][\p{Lu}\d\s\-:]{2,}$"));
/// Headings that are known by their words, numbered or not.
static KNOWN_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)^(?:(?:\d{1,2}|[A-H])\.?\s+)?(?:abstract|introduction|related works?|background|preliminaries|methods?|methodology|approach|experiments|experimental (?:setup|results)|results|evaluation|discussion|conclusions?(?: and future work)?|limitations(?: and future work)?|future work|acknowledge?ments?|impact statement|broader impacts?|ethics statement|reproducibility statement|appendix(?:\s+[A-Z](?:\.\d+)*)?(?:[.:]?\s+\p{Lu}[^.,;]{0,60})?|appendices|supplementary materials?|contents|(?:neurips )?paper checklist)$",
    )
});
static CAPTION: LazyLock<Regex> = LazyLock::new(|| pattern(r"^(?:(Figure|Fig\.)|(Table|Tab\.))\s*[A-Z]?\d{1,3}[.:|]"));
static EQUATION_NUMBER: LazyLock<Regex> = LazyLock::new(|| pattern(r"\(\d{1,3}[a-z]?\)$"));
/// What a footnote, an affiliation or the venue's notice begins and is about.
static FOOTNOTE_START: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"^(?:[*†‡§¶]|\d{1,2}\s?\p{Lu}|Correspondence|Proceedings of the|Copyright|Preprint|Work done|Equal contribution|Code (?:is )?available)",
    )
});
static FOOTNOTE_ABOUT: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)universit|institut|laborator|school of|department|@|proceedings|copyright|correspondence|contribution|https?://|work done",
    )
});
/// The characters equations are written with.
const MATH: &str = "=∑∫∏≈≤≥∈∉∼→←↦±×·√∂∇∞≠≡⊂⊆∪∩∀∃αβγδεθλμνπρστφχψωΣΠΩ";

/// A line seen at the top or bottom of this share of pages (and of at least two) is a running header or footer.
fn repeats_enough(times: usize, pages: usize) -> bool {
    times >= 2 && times * 10 >= pages * 3
}

/// What a line is compared as when looking for running headers: numbers in it count for any number ("Page 3").
fn furniture_key(line: &str) -> String {
    line.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .map(|c| if c.is_ascii_digit() { '#' } else { c })
        .collect()
}

/// The indices of a page's non-blank lines that are at its top or bottom.
fn edges(lines: &[&str]) -> Vec<usize> {
    let present: Vec<usize> = (0..lines.len()).filter(|&at| !lines[at].trim().is_empty()).collect();
    let mut edge: BTreeSet<usize> = present.iter().take(EDGE_LINES).copied().collect();
    edge.extend(present.iter().rev().take(EDGE_LINES));
    edge.into_iter().collect()
}

/// Keys of lines that repeat at the top or bottom of many pages.
fn running_lines(pages: &[Vec<&str>]) -> HashSet<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    for lines in pages {
        let keys: HashSet<String> = edges(lines).into_iter().map(|at| furniture_key(lines[at])).collect();
        for key in keys {
            *seen.entry(key).or_default() += 1;
        }
    }
    seen.into_iter()
        .filter(|(key, times)| key.chars().count() >= 4 && repeats_enough(*times, pages.len()))
        .map(|(key, _)| key)
        .collect()
}

/// A run of lines between blank lines (or column breaks), before it is judged.
struct Block {
    lines: Vec<String>,
}

/// A page's blocks: furniture dropped, a blank line or a column break between two. `pdf-extract` starts a new
/// column's first line with a space, which is how a paragraph cut by a column break is told from one line wrapping
/// into the next. On the first page, the first line is the paper's title even when later pages repeat it as their
/// running header.
fn blocks(lines: &[&str], running: &HashSet<String>, first_page: bool) -> Vec<Block> {
    let edge: HashSet<usize> = edges(lines).into_iter().collect();
    let title = if first_page { lines.iter().position(|line| !line.trim().is_empty()) } else { None };
    let mut blocks: Vec<Block> = Vec::new();
    let mut open = false;
    for (at, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        let repeated = title != Some(at) && running.contains(&furniture_key(line));
        let furniture = ARXIV_STAMP.is_match(line)
            || (edge.contains(&at) && (PAGE_NUMBER.is_match(line) || FURNITURE.is_match(line) || repeated));
        if line.is_empty() || furniture {
            open = false;
            continue;
        }
        let column_break = raw.starts_with(' ') && open;
        match blocks.last_mut() {
            Some(block) if open && !column_break => block.lines.push(accents_composed(line)),
            _ => blocks.push(Block { lines: vec![accents_composed(line)] }),
        }
        open = true;
    }
    blocks
}

/// Does `next` carry on a word that `before` hyphenated at its line end? "ana-" "lytic" does; "plug-and-" "play"
/// keeps its hyphen, since a word that already has one is a compound.
fn hyphenated(before: &str, next: &str) -> bool {
    let mut ending = before.chars().rev();
    let cut = ending.next() == Some('-') && ending.next().is_some_and(char::is_lowercase);
    let last_word = before.rsplit(' ').next().unwrap_or_default();
    cut && next.chars().next().is_some_and(char::is_lowercase) && !last_word.trim_end_matches('-').contains('-')
}

/// Lines as one line: one space between words, hyphenated words joined again.
fn join_lines<S: AsRef<str>>(lines: &[S]) -> String {
    let mut joined = String::new();
    for line in lines {
        let line = line.as_ref().split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            continue;
        }
        if joined.is_empty() {
        } else if hyphenated(&joined, &line) {
            joined.pop();
        } else {
            joined.push(' ');
        }
        joined.push_str(&line);
    }
    joined
}

/// Words of the text that are ordinary lower-case words, as a share of all its words: high for prose, low for
/// math, tables, figure labels and lists of names.
fn prose_share(text: &str) -> f64 {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return 0.0;
    }
    let prose = words
        .iter()
        .filter(|word| {
            let word = word.trim_matches(|c: char| !c.is_alphanumeric());
            word.chars().filter(|c| c.is_alphabetic()).count() >= 2
                && word.chars().all(|c| c.is_lowercase() || c == '-' || c == '\'' || c == '’')
        })
        .count();
    prose as f64 / words.len() as f64
}

fn ends_sentence(text: &str) -> bool {
    text.trim_end_matches(['"', '”', '’', ')', ']']).ends_with(['.', '?', '!', ':'])
}

fn is_heading(block: &Block, text: &str) -> bool {
    let chars = text.chars().count();
    if block.lines.len() > 2 || chars > 100 || (block.lines.len() > 1 && chars > 60) || text.contains('=') {
        return false;
    }
    if text.ends_with(['.', ',', ';', ':', '-']) {
        return false;
    }
    let words: Vec<String> = text.split_whitespace().map(|word| word.to_lowercase()).collect();
    let after_number = words.iter().skip_while(|word| word.chars().all(|c| c.is_ascii_digit() || c == '.')).count();
    if after_number == 0 || after_number > 12 {
        return false;
    }
    let last = words.last().map(String::as_str).unwrap_or_default();
    if words.iter().any(|word| RUNNING.contains(&word.as_str())) || DANGLING.contains(&last) {
        return false;
    }
    KNOWN_HEADING.is_match(text)
        || REFERENCES_HEADING.is_match(text)
        || NUMBERED_HEADING.is_match(text)
        || CAPITAL_APPENDIX.is_match(text)
}

fn is_equation(text: &str) -> bool {
    let words = text.split_whitespace().count();
    let share = prose_share(text);
    let symbols = text.chars().filter(|c| MATH.contains(*c)).count();
    (EQUATION_NUMBER.is_match(text) && share < 0.5 && words <= 80) || (symbols >= 2 && share < 0.3 && words <= 60)
}

fn is_footnote(text: &str) -> bool {
    FOOTNOTE_START.is_match(text) && FOOTNOTE_ABOUT.is_match(text)
}

fn is_fragment(block: &Block, text: &str) -> bool {
    let words = text.split_whitespace().count();
    let average = block.lines.iter().map(|line| line.chars().count()).sum::<usize>() / block.lines.len().max(1);
    let finished = ends_sentence(text);
    (words <= 3 && !finished) || (prose_share(text) < 0.35 && !finished) || (block.lines.len() >= 3 && average < 15)
}

/// Where the reading is in the paper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Front,
    Body,
    References,
    Appendix,
    Checklist,
}

impl Stage {
    fn part(self) -> Part {
        match self {
            Self::Front => Part::Front,
            Self::Body | Self::References => Part::Body,
            Self::Appendix => Part::Appendix,
            Self::Checklist => Part::Checklist,
        }
    }
}

/// What a block was judged to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Passage(Kind, Part),
    /// The reference list (and its heading): in the page's text, not a passage.
    Reference,
}

/// A judged block of a page.
struct Piece {
    role: Role,
    section: String,
    text: String,
}

/// The section and part a heading opens, and where the reading goes from it.
fn after_heading(stage: Stage, text: &str) -> Stage {
    let lower = text.to_lowercase();
    if REFERENCES_HEADING.is_match(text) {
        Stage::References
    } else if lower.contains("checklist") {
        Stage::Checklist
    } else if END_HEADING.is_match(text) || (stage == Stage::References && APPENDIX_HEADING.is_match(text)) {
        Stage::Appendix
    } else if matches!(stage, Stage::Front | Stage::References) {
        // a numbered or lettered heading after the reference list with no "Appendix" before it
        if stage == Stage::References { Stage::Appendix } else { Stage::Body }
    } else {
        stage
    }
}

/// Judge every block of every page, in order: the section a block is in depends on the headings before it.
fn judged(pages: &[Vec<Block>]) -> Vec<Vec<Piece>> {
    let mut stage = Stage::Front;
    let mut section = String::new();
    let mut previous_reference = String::new();
    let mut judged = Vec::with_capacity(pages.len());
    for (number, blocks) in pages.iter().enumerate() {
        let mut pieces = Vec::with_capacity(blocks.len());
        for (at, block) in blocks.iter().enumerate() {
            let text = join_lines(&block.lines);
            let heading = is_heading(block, &text);
            let role = if stage == Stage::References && !(heading && ends_sentence(&previous_reference)) {
                previous_reference.clone_from(&text);
                Role::Reference
            } else if heading {
                stage = after_heading(stage, &text);
                if stage == Stage::References {
                    Role::Reference
                } else {
                    section.clone_from(&text);
                    Role::Passage(Kind::Heading, stage.part())
                }
            } else {
                // "4.3.5. ROBUSTNESS AND SCOPE We finally test...": a run-in heading opens its paragraph
                if let Some(run_in) = RUN_IN_HEADING.captures(&text).and_then(|found| found.get(1)) {
                    let heading = run_in.as_str().to_owned();
                    let rest = text[run_in.end()..].trim_start().to_owned();
                    section.clone_from(&heading);
                    pieces.push(Piece {
                        role: Role::Passage(Kind::Heading, stage.part()),
                        section: section.clone(),
                        text: heading,
                    });
                    pieces.push(Piece {
                        role: Role::Passage(Kind::Text, stage.part()),
                        section: section.clone(),
                        text: rest,
                    });
                    continue;
                }
                let kind = if stage == Stage::Front && number == 0 && at == 0 {
                    Kind::Title
                } else if let Some(caption) = CAPTION.captures(&text) {
                    if caption.get(1).is_some() { Kind::FigureCaption } else { Kind::TableCaption }
                } else if is_footnote(&text) {
                    Kind::Footnote
                } else if is_equation(&text) {
                    Kind::Equation
                } else if is_fragment(block, &text) {
                    Kind::Fragment
                } else {
                    Kind::Text
                };
                Role::Passage(kind, stage.part())
            };
            pieces.push(Piece { role, section: section.clone(), text });
        }
        judged.push(pieces);
    }
    judged
}

fn starts_lower(text: &str) -> bool {
    text.chars().next().is_some_and(char::is_lowercase)
}

/// Join a paragraph a column break cut in two: the second half starts in lower case, the first did not finish its
/// sentence, and between them is only what a column's bottom holds (footnotes, a caption, a figure's text). The
/// second half moves up to the first; what was between stays after it.
fn rejoined(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::with_capacity(pieces.len());
    for piece in pieces {
        if matches!(piece.role, Role::Passage(Kind::Text, _)) && starts_lower(&piece.text) {
            let between = |role: Role| {
                matches!(
                    role,
                    Role::Passage(Kind::Footnote | Kind::Fragment | Kind::FigureCaption | Kind::TableCaption, _)
                )
            };
            let open = out
                .iter()
                .rposition(|earlier| !between(earlier.role))
                .filter(|&at| matches!(out[at].role, Role::Passage(Kind::Text, _)) && !ends_sentence(&out[at].text));
            if let Some(at) = open {
                let first = &mut out[at].text;
                if hyphenated(first, &piece.text) {
                    first.pop();
                } else {
                    first.push(' ');
                }
                first.push_str(&piece.text);
                continue;
            }
        }
        out.push(piece);
    }
    out
}

/// The places of the sentences of `text`, in characters from its start. A full stop ends a sentence when a space
/// and a capital, digit or opening bracket follow it, and it does not end an abbreviation or an initial.
pub fn sentence_spans(text: &str) -> Vec<(usize, usize)> {
    const ABBREVIATIONS: [&str; 34] = [
        "al", "e.g", "i.e", "etc", "fig", "figs", "eq", "eqs", "sec", "secs", "tab", "vs", "cf", "resp", "approx",
        "ref", "refs", "no", "dr", "prof", "mr", "ms", "st", "jr", "vol", "pp", "ch", "thm", "lem", "def", "prop",
        "cor", "alg", "app",
    ];
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut from = 0;
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        if matches!(c, '.' | '?' | '!') {
            let mut end = at + 1;
            while end < chars.len() && matches!(chars[end], '"' | '”' | '’' | ')' | ']') {
                end += 1;
            }
            let next = chars.get(end + 1).copied();
            let spaced = chars.get(end).is_some_and(|c| c.is_whitespace());
            let opens =
                next.is_some_and(|n| n.is_uppercase() || n.is_ascii_digit() || matches!(n, '(' | '[' | '"' | '“'));
            // The word before the stop, looked at only as far back as an abbreviation is long: a run of thousands of
            // characters without a space (dot leaders, a line of a table) would otherwise be read back to its start
            // at every one of its full stops.
            const LONGEST_ABBREVIATION: usize = 8;
            let word: String = chars[from..at]
                .iter()
                .rev()
                .take(LONGEST_ABBREVIATION + 1)
                .take_while(|c| !c.is_whitespace() && **c != '(')
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let word = word.to_lowercase();
            let abbreviation = c == '.'
                && (ABBREVIATIONS.contains(&word.as_str())
                    || (word.chars().count() == 1 && word.chars().all(char::is_alphabetic)));
            if spaced && opens && !abbreviation {
                spans.push((from, end));
                from = end + 1;
                at = end;
            }
        }
        at += 1;
    }
    let rest: String = chars[from.min(chars.len())..].iter().collect();
    if !rest.trim().is_empty() {
        spans.push((from, chars.len()));
    }
    spans
}

/// A paper's passages, from its PDF: what `GET /api/papers/<id>/passages` answers (kept by paper id and
/// [`READER`], since reading a PDF takes a moment and its addresses must not change under stored evidence).
pub fn passages_of_pdf(bytes: &[u8]) -> Result<Passages, crate::text::pdf::Unreadable> {
    crate::text::pdf::pdf_pages(bytes).map(|pages| read_passages(&pages))
}

/// Read a paper's passages from the text of its pages, as [`crate::text::pdf::pdf_pages`] gives it.
pub fn read_passages(pages: &[String]) -> Passages {
    let lines: Vec<Vec<&str>> = pages.iter().map(|page| page.lines().collect()).collect();
    let running = running_lines(&lines);
    let blocks: Vec<Vec<Block>> = lines.iter().enumerate().map(|(at, page)| blocks(page, &running, at == 0)).collect();
    let judged = judged(&blocks);

    let mut built_pages = Vec::with_capacity(pages.len());
    let mut passages: Vec<Passage> = Vec::new();
    let mut counter: (String, u32) = (String::new(), 0);
    let mut first_in_section = true;
    for (index, pieces) in judged.into_iter().enumerate() {
        let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let pieces = rejoined(pieces);
        let mut text = String::new();
        let mut length = 0usize;
        let mut first_text_on_page = true;
        for piece in &pieces {
            if !text.is_empty() {
                text.push_str("\n\n");
                length += 2;
            }
            let start = length;
            let piece_length = piece.text.chars().count();
            text.push_str(&piece.text);
            length += piece_length;
            let Role::Passage(kind, part) = piece.role else { continue };
            // a paragraph that runs on from the page before: same paragraph, marked as its continuation
            let continues = kind == Kind::Text && first_text_on_page && starts_lower(&piece.text) && {
                let last_text = passages.iter().rev().find(|passage| {
                    !matches!(passage.kind, Kind::Footnote | Kind::Fragment | Kind::FigureCaption | Kind::TableCaption)
                });
                last_text.is_some_and(|last| {
                    last.kind == Kind::Text && !ends_sentence(&last.text) && last.address.page + 1 == number
                })
            };
            if kind == Kind::Text {
                first_text_on_page = false;
            }
            if counter.0 != piece.section || kind == Kind::Heading {
                counter = (piece.section.clone(), 0);
                first_in_section = true;
            }
            if !(continues || first_in_section) {
                counter.1 += 1;
            }
            first_in_section = false;
            let to_u32 = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
            let sentences = if matches!(kind, Kind::Text | Kind::FigureCaption | Kind::TableCaption | Kind::Footnote) {
                sentence_spans(&piece.text)
                    .into_iter()
                    .map(|(from, to)| Span { start: to_u32(start + from), end: to_u32(start + to) })
                    .collect()
            } else {
                vec![Span { start: to_u32(start), end: to_u32(start + piece_length) }]
            };
            passages.push(Passage {
                kind,
                part,
                flow: if continues { Flow::Continues } else { Flow::Starts },
                address: Address {
                    page: number,
                    section: piece.section.clone(),
                    paragraph: counter.1,
                    start: to_u32(start),
                    end_page: number,
                    end: to_u32(start + piece_length),
                },
                text: piece.text.clone(),
                sentences,
            });
        }
        built_pages.push(Page { number, text });
    }
    Passages { reader: READER, pages: built_pages, passages }
}

/// The characters `start..end` of `text`.
pub fn chars_of(text: &str, start: u32, end: u32) -> String {
    text.chars().skip(start as usize).take(end.saturating_sub(start) as usize).collect()
}

// ------------------------------------------------------------------------------------------------ finding quotes

/// Text as it is compared: only letters and digits, lower case, accents and ligatures undone, each remembering
/// where it came from (which segment, which character of it). Spacing, line breaks, hyphenation, quote marks and
/// punctuation then make no difference, which is what text from a PDF and a model's copy of it differ by.
struct Skeleton {
    letters: Vec<char>,
    from: Vec<(usize, usize)>,
}

impl Skeleton {
    fn of<'a>(segments: impl IntoIterator<Item = &'a str>) -> Self {
        let mut letters = Vec::new();
        let mut from = Vec::new();
        for (segment, text) in segments.into_iter().enumerate() {
            for (at, c) in text.chars().enumerate() {
                for plain in std::iter::once(c).nfkd().filter(|c| !is_combining_mark(*c)) {
                    for lower in plain.to_lowercase().filter(|c| c.is_alphanumeric()) {
                        letters.push(lower);
                        from.push((segment, at));
                    }
                }
            }
        }
        Self { letters, from }
    }
}

/// Where `needle` first occurs in `haystack`, exactly.
fn find_exact(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    let first = needle[0];
    (0..=haystack.len() - needle.len()).find(|&at| haystack[at] == first && haystack[at..at + needle.len()] == *needle)
}

/// Every place `needle` occurs in `haystack`, exactly.
fn find_all(haystack: &[char], needle: &[char]) -> Vec<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return Vec::new();
    }
    (0..=haystack.len() - needle.len())
        .filter(|&at| haystack[at] == needle[0] && haystack[at..at + needle.len()] == *needle)
        .collect()
}

/// The end of the best match of `pattern` anywhere in `text` with at most `most` edits (insertions, deletions,
/// substitutions), and its edits: Sellers' algorithm, one column of the edit-distance table at a time.
fn best_end(text: &[char], pattern: &[char], most: usize) -> Option<(usize, usize)> {
    let mut column: Vec<usize> = (0..=pattern.len()).collect();
    let mut best: Option<(usize, usize)> = None;
    for (at, &letter) in text.iter().enumerate() {
        let mut diagonal = column[0];
        column[0] = 0;
        for row in 1..=pattern.len() {
            let above = column[row];
            let substituted = diagonal + usize::from(pattern[row - 1] != letter);
            let value = substituted.min(above + 1).min(column[row - 1] + 1);
            diagonal = above;
            column[row] = value;
        }
        let edits = column[pattern.len()];
        if edits <= most && best.is_none_or(|(_, fewest)| edits < fewest) {
            best = Some((at + 1, edits));
        }
    }
    best
}

/// The span `start..end` (in skeleton letters) where `quote` is in `text`, and how it matched.
fn find_quote(text: &[char], quote: &[char]) -> Option<(usize, usize, Match)> {
    if quote.is_empty() {
        return None;
    }
    if let Some(at) = find_exact(text, quote) {
        return Some((at, at + quote.len(), Match::Exact));
    }
    if quote.len() < SHORTEST_TOLERANT || quote.len() > LONGEST_TOLERANT {
        return None;
    }
    let most = (quote.len() / 20).clamp(1, MOST_DIFFERENCES);
    // with at most `most` edits, one of `most + 1` pieces of the quote is in the text unchanged: only the
    // neighbourhoods of those pieces are searched
    let piece = quote.len() / (most + 1);
    let mut windows: Vec<(usize, usize)> = Vec::new();
    for index in 0..=most {
        let offset = index * piece;
        for found in find_all(text, &quote[offset..offset + piece]) {
            let from = found.saturating_sub(offset + most);
            let to = (found - offset.min(found) + quote.len() + most).min(text.len());
            windows.push((from, to));
        }
    }
    windows.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (from, to) in windows {
        match merged.last_mut() {
            Some(last) if from <= last.1 => last.1 = last.1.max(to),
            _ => merged.push((from, to)),
        }
    }
    let reversed: Vec<char> = quote.iter().rev().copied().collect();
    let mut best: Option<(usize, usize, usize)> = None;
    for (from, to) in merged {
        let Some((end, edits)) = best_end(&text[from..to], quote, most) else { continue };
        let end = from + end;
        let back_from = end.saturating_sub(quote.len() + most).max(from);
        let backwards: Vec<char> = text[back_from..end].iter().rev().copied().collect();
        let Some((length, _)) = best_end(&backwards, &reversed, most) else { continue };
        if best.is_none_or(|(_, _, fewest)| edits < fewest) {
            best = Some((end - length, end, edits));
        }
    }
    best.map(|(start, end, edits)| (start, end, Match::Close { differences: u32::try_from(edits).unwrap_or(u32::MAX) }))
}

/// `start..end` (characters of `text`) widened to whole words. An accent written as a mark of its own after its
/// letter ("e" then U+0301) is part of the word, so the wording shown keeps it.
fn whole_words(text: &[char], start: usize, end: usize) -> (usize, usize) {
    let in_word = |c: char| c.is_alphanumeric() || is_combining_mark(c);
    let mut start = start.min(text.len());
    let mut end = end.min(text.len());
    while start > 0 && in_word(text[start - 1]) {
        start -= 1;
    }
    while end < text.len() && in_word(text[end]) {
        end += 1;
    }
    (start, end)
}

/// Words that turn a statement into its opposite: a quote may never differ from the paper by one of them.
const NEGATIONS: [&str; 12] =
    ["not", "no", "nor", "never", "none", "nothing", "neither", "nobody", "nowhere", "cannot", "without", "non"];
/// What, put before a word, makes it mean the opposite ("likely", "unlikely").
const NEGATING_PREFIXES: [&str; 8] = ["un", "in", "im", "il", "ir", "non", "dis", "a"];

/// A word as the close match compares it: its letters and digits as the search sees them, and whether it negates.
struct Word {
    letters: Vec<char>,
    negates: bool,
}

/// The words of `text`, split at spaces (so "multi-task" is one word), those with no letter or digit left out.
fn words(text: &str) -> Vec<Word> {
    text.split_whitespace()
        .filter_map(|token| {
            let letters = Skeleton::of([token]).letters;
            let plain: String = letters.iter().collect();
            let lower = token.to_lowercase();
            let negates = NEGATIONS.contains(&plain.as_str()) || lower.contains("n't") || lower.contains("n\u{2019}t");
            (!letters.is_empty()).then_some(Word { letters, negates })
        })
        .collect()
}

/// The fewest insertions, deletions and substitutions that make `one` into `other`.
fn edit_distance(one: &[char], other: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=other.len()).collect();
    for (at, &letter) in one.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = at + 1;
        for column in 1..=other.len() {
            let above = row[column];
            row[column] = (diagonal + usize::from(other[column - 1] != letter)).min(above + 1).min(row[column - 1] + 1);
            diagonal = above;
        }
    }
    row[other.len()]
}

/// Whether a close match of `quote` to the paper's `wording` differs only by slips inside words (a letter the PDF
/// reader or the model got wrong, hyphenation, a ligature), and so says what the paper says. Refused: a word added or
/// left out (the model dropping "not"), a number changed, a negation changed ("not" for "now", "likely" for
/// "unlikely"), or a word more than a slip away from the paper's (more than a third of its letters).
fn close_enough(wording: &str, quote: &str) -> bool {
    let (paper, model) = (words(wording), words(quote));
    if paper.len() != model.len() {
        return false;
    }
    paper.iter().zip(&model).all(|(paper, model)| {
        if paper.letters == model.letters {
            return true;
        }
        let digits = |word: &Word| word.letters.iter().any(char::is_ascii_digit);
        if digits(paper) || digits(model) || paper.negates || model.negates {
            return false;
        }
        let (short, long) = if paper.letters.len() <= model.letters.len() { (paper, model) } else { (model, paper) };
        let prefix: String = long.letters[..long.letters.len() - short.letters.len()].iter().collect();
        if long.letters.ends_with(&short.letters) && NEGATING_PREFIXES.contains(&prefix.as_str()) {
            return false;
        }
        edit_distance(&paper.letters, &model.letters) * 3 <= long.letters.len()
    })
}

/// Where `quote` is in `text` (a title or an abstract, say): its span in characters of `text`, the words of `text`
/// there, and how it matched. A quote with no letters or digits is never found.
pub fn locate_in(text: &str, quote: &str) -> Option<(Span, String, Match)> {
    let haystack = Skeleton::of([text]);
    let needle = Skeleton::of([quote]);
    let (start, end, matched) = find_quote(&haystack.letters, &needle.letters)?;
    let chars: Vec<char> = text.chars().collect();
    let (start, end) = whole_words(&chars, haystack.from[start].1, haystack.from[end - 1].1 + 1);
    let to_u32 = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
    let wording: String = chars[start..end].iter().collect();
    if matches!(matched, Match::Close { .. }) && !close_enough(&wording, quote) {
        return None;
    }
    Some((Span { start: to_u32(start), end: to_u32(end) }, wording, matched))
}

/// How many letters and digits a quote has, as the search counts them: what a minimum length is measured in.
pub fn letters_in(quote: &str) -> usize {
    Skeleton::of([quote]).letters.len()
}

impl Passages {
    /// The passages a quote may be found in: all of them (the reference list is not among them).
    fn skeleton(&self) -> Skeleton {
        Skeleton::of(self.passages.iter().map(|passage| passage.text.as_str()))
    }

    /// Where `quote` is in the paper, and how the paper words it there; `None` when it is not in the paper's
    /// passages, exactly or with a few differences.
    pub fn locate(&self, quote: &str) -> Option<Located> {
        self.locate_all(&[quote]).pop().flatten()
    }

    /// [`Passages::locate`] for many quotes of the same paper, reading the paper once.
    pub fn locate_all(&self, quotes: &[&str]) -> Vec<Option<Located>> {
        let skeleton = self.skeleton();
        quotes.iter().map(|quote| self.located(&skeleton, quote)).collect()
    }

    fn located(&self, skeleton: &Skeleton, quote: &str) -> Option<Located> {
        let needle = Skeleton::of([quote]);
        let (start, end, matched) = find_quote(&skeleton.letters, &needle.letters)?;
        let (first, first_at) = skeleton.from[start];
        let (last, last_at) = skeleton.from[end - 1];
        let first_text: Vec<char> = self.passages[first].text.chars().collect();
        let last_text: Vec<char> = self.passages[last].text.chars().collect();
        let (from, _) = whole_words(&first_text, first_at, first_at + 1);
        let (_, to) = whole_words(&last_text, last_at, last_at + 1);
        let wording = if first == last {
            first_text[from..to].iter().collect()
        } else {
            let mut wording: String = first_text[from..].iter().collect();
            for passage in &self.passages[first + 1..last] {
                wording.push(' ');
                wording.push_str(&passage.text);
            }
            wording.push(' ');
            wording.extend(&last_text[..to]);
            wording
        };
        if matches!(matched, Match::Close { .. }) && !close_enough(&wording, quote) {
            return None;
        }
        let to_u32 = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
        let (begin, finish) = (&self.passages[first].address, &self.passages[last].address);
        Some(Located {
            address: Address {
                page: begin.page,
                section: begin.section.clone(),
                paragraph: begin.paragraph,
                start: begin.start + to_u32(from),
                end_page: finish.page,
                end: finish.start + to_u32(to),
            },
            wording,
            matched,
        })
    }

    /// The share of the pages' text that is in passages, in letters and digits: the rest is the reference list
    /// (what dropped furniture was is not in the pages' text at all).
    pub fn letters(&self) -> (usize, usize) {
        let passages = self.passages.iter().map(|passage| letters_in(&passage.text)).sum();
        let pages = self.pages.iter().map(|page| letters_in(&page.text)).sum();
        (passages, pages)
    }
}

// ------------------------------------------------------------------------------------------------ citing sentences

static BRACKETED: LazyLock<Regex> = LazyLock::new(|| pattern(r"\[(\s*\d{1,3}(?:\s*[-–,;]\s*\d{1,3})*\s*)\]"));
/// A number written small after a word: "models.12 ", "work,3,4 ", "(Fig. 2)7 ".
static SUPERSCRIPT: LazyLock<Regex> = LazyLock::new(|| pattern(r"[a-z\)][.,;:]?(\d{1,3}(?:[,–-]\d{1,3})*)(?:\s|$)"));

/// The numbers a marker lists: "3, 7-9" is 3, 7, 8, 9.
fn numbers(listed: &str) -> Vec<usize> {
    let mut numbers = Vec::new();
    for part in listed.split([',', ';']) {
        let bounds: Vec<usize> = part.split(['-', '–']).filter_map(|n| n.trim().parse().ok()).collect();
        match bounds.as_slice() {
            [one] => numbers.push(*one),
            [from, to] if from <= to && to - from <= 50 => numbers.extend(*from..=*to),
            _ => {}
        }
    }
    numbers
}

/// The surname an author is cited by: "Vaswani, A." and "Ashish Vaswani" are both "Vaswani"; an organisation
/// ("Qwen Team") is cited by its whole name.
fn surname(author: &str) -> Option<String> {
    let author = author.trim();
    let name = match author.split_once(',') {
        Some((surname, _)) => surname.trim().to_owned(),
        None => {
            let words: Vec<&str> = author.split_whitespace().collect();
            match words.as_slice() {
                [] => return None,
                // "Qwen Team" is cited as "Qwen Team", not as "Team"
                [.., "Team" | "Group" | "Consortium"] => author.to_owned(),
                [.., last] => (*last).to_owned(),
            }
        }
    };
    (name.chars().filter(|c| c.is_alphabetic()).count() >= 2).then_some(name)
}

/// The pattern of an author-year citation of `reference`: its first author's surname, "et al." or a second
/// author, and its year with the letter that tells two of one year apart ("2024a").
fn author_year(reference: &Reference) -> Option<Regex> {
    let year = reference.year?;
    let first = regex::escape(&surname(reference.authors.first()?)?);
    let second = reference.authors.get(1).and_then(|author| surname(author)).map(|name| regex::escape(&name));
    let letter = Regex::new(&format!(r"\b{year}([a-z])\b"))
        .ok()?
        .captures_iter(&reference.raw)
        .last()
        .map(|found| found[1].to_owned());
    let others = match (&second, reference.authors.len()) {
        (Some(second), 2) => format!(r"(?:\s*,?\s*(?:and|&)\s+{second})"),
        (_, 1) => String::new(),
        _ => r"(?:\s+et\s+al\.?)".to_owned(),
    };
    let others = if others.is_empty() { String::new() } else { format!("{others}?") };
    let year = match letter {
        Some(letter) => format!("{year}{letter}"),
        None => format!(r"{year}(?:[^a-z0-9]|$)"),
    };
    Regex::new(&format!(r"\b{first}{others}[\s,]*\(?\s*{year}")).ok()
}

/// For each entry of the reference list that the paper's text cites, the sentences that cite it, in the order
/// they come. A numbered list is cited by "[12]" markers, or, where the PDF wrote numbers small after words and has
/// no bracketed markers at all, by those numbers (marked [`Marker::Superscript`], a less sure reading); an
/// author-year list by its entries' first authors and years.
pub fn citing_sentences(passages: &Passages, list: &ReferenceList) -> Vec<Cited> {
    let mut found: Vec<Vec<CitingSentence>> = vec![Vec::new(); list.references.len()];
    let sentences: Vec<(Address, String)> = passages
        .passages
        .iter()
        .filter(|passage| {
            matches!(passage.kind, Kind::Text | Kind::FigureCaption | Kind::TableCaption | Kind::Footnote)
        })
        .flat_map(|passage| {
            passage.sentences.iter().map(move |span| {
                let sentence =
                    chars_of(&passage.text, span.start - passage.address.start, span.end - passage.address.start);
                (
                    Address {
                        start: span.start,
                        end: span.end,
                        end_page: passage.address.page,
                        ..passage.address.clone()
                    },
                    sentence,
                )
            })
        })
        .collect();
    let mut note = |reference: usize, address: &Address, sentence: &str, marker: Marker, written: &str| {
        let Some(sentences) = found.get_mut(reference) else { return };
        if !sentences.iter().any(|known| known.address == *address) {
            sentences.push(CitingSentence {
                address: address.clone(),
                sentence: sentence.to_owned(),
                marker,
                written: written.to_owned(),
            });
        }
    };
    match list.citing {
        Citing::ByNumber => {
            let bracketed = sentences.iter().any(|(_, sentence)| BRACKETED.is_match(sentence));
            for (address, sentence) in &sentences {
                let (marker, pattern) =
                    if bracketed { (Marker::Number, &*BRACKETED) } else { (Marker::Superscript, &*SUPERSCRIPT) };
                for marked in pattern.captures_iter(sentence) {
                    let written = if bracketed { marked[0].to_owned() } else { marked[1].to_owned() };
                    for number in numbers(&marked[1]).into_iter().filter(|&number| number >= 1) {
                        note(number - 1, address, sentence, marker, &written);
                    }
                }
            }
        }
        Citing::ByAuthorAndYear => {
            for (index, reference) in list.references.iter().enumerate() {
                let Some(pattern) = author_year(reference) else { continue };
                for (address, sentence) in &sentences {
                    if let Some(written) = pattern.find(sentence) {
                        let written = written.as_str().trim_end_matches(|c: char| !c.is_alphanumeric());
                        note(index, address, sentence, Marker::AuthorYear, written);
                    }
                }
            }
        }
    }
    found
        .into_iter()
        .enumerate()
        .filter(|(_, sentences)| !sentences.is_empty())
        .map(|(reference, sentences)| Cited { reference, sentences })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::super::references::read_reference_list;
    use super::*;

    /// Three pages of a two-column ICML paper as `pdf-extract` gives them: the running header on every page, the
    /// page number at the bottom, words hyphenated at line ends, the first column's footnotes between the halves of
    /// a paragraph the column break cut, a paragraph running on to the next page, a caption, an equation, a
    /// figure's text, the reference list and an appendix.
    fn icml() -> Vec<String> {
        [
            "\n\nSpectral Calibration for Model Merging\n\nAda Lovelace 1 Alan Turing 1 2\n\nAbstract\n\n\
Model merging combines fine-tuned models into one by adding their weight updates. We show that shared\n\
directions are counted twice.\n\n1. Introduction\n\nModel merging combines trained models into a single model\n\
by operating directly in weight space (Ruan et al., 2025). Com-\npared with retraining, it is cheap and the\n\
merged model keeps most of the accuracy of its parts\n\n1Department of Computing, University of Nowhere.\n\
Correspondence to: Ada Lovelace <ada@example.org>.\n\nProceedings of the 43 rd International Conference on Machine\n\
Learning, Seoul, South Korea. PMLR 306, 2026.\n and costs nothing to train (Ilharco et al., 2022; Ruan et al.,\n\
2025). Task arithmetic adds the updates [3, 7-9].\n\nRecent work studies when merging fails. The failure we study\n\
comes from shared knowledge, which is\n\n1\n",
            "\n\nSpectral Calibration for Model Merging\n\ncounted once per task and so too often. Lovelace and Turing\n\
(2024a) saw this first.\n\nTask A Task B Task C\n\nShared Knowledge\n\nFigure 1. Shared directions are added once per \
task.\n\n2. Method\n\nThe merged update is the sum of the task updates:\n\n∆W = ∑ i ∆Wi (1)\n\nwhere ∆Wi is the update \
of task i. We rescale the inflated singular values. The ﬁnal model is “calibrated”.\n\n2\n",
            "\n\nSpectral Calibration for Model Merging\n\nReferences\n\nIlharco, G., Ribeiro, M. T., and Wortsman, M. Editing \
models with task arithmetic. In ICLR, 2022.\n\nLovelace, A. and Turing, A. Counting twice. In NeurIPS, 2024a.\n\n\
Ruan, W., Chen, Y., and Li, X. Model merging survey. arXiv preprint arXiv:2501.00001, 2025.\n\nA. Proofs\n\n\
We prove that the bound holds for every task.\n\n3\n",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    fn of_kind(passages: &Passages, kind: Kind) -> Vec<&str> {
        passages.passages.iter().filter(|passage| passage.kind == kind).map(|passage| passage.text.as_str()).collect()
    }

    #[test]
    fn running_headers_and_page_numbers_are_dropped_but_pages_are_kept() {
        let read = read_passages(&icml());
        assert_eq!(read.pages.len(), 3);
        assert_eq!(read.pages.iter().map(|page| page.number).collect::<Vec<_>>(), [1, 2, 3]);
        for page in &read.pages[1..] {
            assert!(!page.text.contains("Spectral Calibration for Model Merging"), "{}", page.text);
        }
        for page in &read.pages {
            assert!(!page.text.ends_with(char::is_numeric), "{}", page.text);
        }
        // the title is not a running header on the first page, where it is the title
        assert_eq!(of_kind(&read, Kind::Title), ["Spectral Calibration for Model Merging"]);
    }

    #[test]
    fn a_paragraph_cut_by_a_column_break_is_joined_and_its_footnotes_follow_it() {
        let read = read_passages(&icml());
        let text = of_kind(&read, Kind::Text);
        let joined = text.iter().find(|text| text.starts_with("Model merging combines trained")).unwrap();
        assert!(joined.contains("Compared with retraining"), "hyphenation undone: {joined}");
        assert!(joined.contains("parts and costs nothing to train"), "{joined}");
        assert!(joined.ends_with("[3, 7-9]."));
        let footnotes = of_kind(&read, Kind::Footnote);
        assert_eq!(footnotes.len(), 2, "{footnotes:?}");
        let page = &read.pages[0].text;
        assert!(page.find("costs nothing").unwrap() < page.find("Department of Computing").unwrap());
    }

    #[test]
    fn a_paragraph_that_runs_on_to_the_next_page_is_marked_as_continuing() {
        let read = read_passages(&icml());
        let next =
            read.passages.iter().find(|passage| passage.address.page == 2 && passage.kind == Kind::Text).unwrap();
        assert_eq!(next.flow, Flow::Continues);
        assert!(next.text.starts_with("counted once per task"));
        let before =
            read.passages.iter().rfind(|passage| passage.address.page == 1 && passage.kind == Kind::Text).unwrap();
        assert_eq!(before.address.paragraph, next.address.paragraph);
        assert_eq!(before.address.section, next.address.section);
    }

    #[test]
    fn captions_equations_figure_text_and_headings_are_labelled() {
        let read = read_passages(&icml());
        assert_eq!(of_kind(&read, Kind::FigureCaption), ["Figure 1. Shared directions are added once per task."]);
        assert_eq!(of_kind(&read, Kind::Equation), ["∆W = ∑ i ∆Wi (1)"]);
        assert_eq!(of_kind(&read, Kind::Fragment).len(), 3, "{:?}", of_kind(&read, Kind::Fragment)); // authors, figure text
        assert_eq!(of_kind(&read, Kind::Heading), ["Abstract", "1. Introduction", "2. Method", "A. Proofs"]);
    }

    #[test]
    fn the_reference_list_is_left_out_and_the_appendix_is_labelled() {
        let read = read_passages(&icml());
        assert!(read.passages.iter().all(|passage| !passage.text.contains("Editing models with task arithmetic")));
        assert!(read.pages[2].text.contains("Editing models with task arithmetic"), "kept in the page's text");
        let proof = read.passages.iter().find(|passage| passage.text.starts_with("We prove")).unwrap();
        assert_eq!((proof.part, proof.address.section.as_str()), (Part::Appendix, "A. Proofs"));
        let abstract_text =
            read.passages.iter().find(|passage| passage.text.starts_with("Model merging combines fine")).unwrap();
        assert_eq!((abstract_text.part, abstract_text.address.section.as_str()), (Part::Body, "Abstract"));
        let authors = read.passages.iter().find(|passage| passage.text.starts_with("Ada Lovelace")).unwrap();
        assert_eq!(authors.part, Part::Front);
    }

    #[test]
    fn every_address_is_its_passage_s_place_in_its_page_s_text() {
        let read = read_passages(&icml());
        for passage in &read.passages {
            let page = &read.pages[passage.address.page as usize - 1];
            assert_eq!(chars_of(&page.text, passage.address.start, passage.address.end), passage.text);
            for sentence in &passage.sentences {
                assert!(sentence.start >= passage.address.start && sentence.end <= passage.address.end);
            }
        }
    }

    #[test]
    fn reading_the_same_pages_again_gives_the_same_addresses() {
        assert_eq!(read_passages(&icml()), read_passages(&icml()));
        assert_eq!(read_passages(&icml()).reader, READER);
    }

    #[test]
    fn sentences_end_at_full_stops_but_not_after_abbreviations_or_initials() {
        let text = "Smith et al. (2024) show it. See Fig. 2 and Eq. 3, i.e. the bound. A. Turing agreed. Is it? Yes.";
        let sentences: Vec<String> = sentence_spans(text)
            .into_iter()
            .map(|(from, to)| text.chars().skip(from).take(to - from).collect())
            .collect();
        assert_eq!(
            sentences,
            [
                "Smith et al. (2024) show it.",
                "See Fig. 2 and Eq. 3, i.e. the bound.",
                "A. Turing agreed.",
                "Is it?",
                "Yes."
            ]
        );
    }

    #[test]
    fn a_quote_is_found_whatever_its_spacing_hyphenation_ligatures_and_quote_marks() {
        let read = read_passages(&icml());
        for quote in [
            "The final model is \"calibrated\".",
            "The ﬁnal  model is “calibrated”",
            "the FINAL model\nis calibrated",
            "We rescale the in-\nflated singular values.",
        ] {
            let found = read.locate(quote).unwrap_or_else(|| panic!("{quote}"));
            assert_eq!(found.matched, Match::Exact, "{quote}");
            assert_eq!(found.address.page, 2);
            assert_eq!(found.address.section, "2. Method");
        }
        let found = read.locate("The final model is \"calibrated\"").unwrap();
        assert_eq!(found.wording, "The ﬁnal model is “calibrated");
        let page = &read.pages[1].text;
        assert_eq!(chars_of(page, found.address.start, found.address.end), found.wording);
    }

    #[test]
    fn a_quote_with_a_few_differences_is_found_and_says_how_many() {
        let read = read_passages(&icml());
        let found = read.locate("We rescale the inflated singlar values. The final model is").unwrap();
        assert!(matches!(found.matched, Match::Close { differences } if (1..=8).contains(&differences)), "{found:?}");
        assert!(found.wording.starts_with("We rescale the inflated singular values"), "{}", found.wording);
    }

    /// What the paper says, for the close-match rule: long enough that three letters are within what a close match
    /// allows.
    const SAYS: &str = "Merging the fine-tuned models does not improve the accuracy on every task we tried, and it \
                        costs 3 GPU hours. Calibration is unlikely to help and never removes the shared directions.";

    #[test]
    fn a_close_match_may_differ_by_letters_inside_words() {
        let (_, wording, matched) =
            locate_in(SAYS, "the fine-tuned modles does not improve the acuracy on every task we tried").unwrap();
        assert!(matches!(matched, Match::Close { differences: 3 }), "{matched:?}");
        assert_eq!(wording, "the fine-tuned models does not improve the accuracy on every task we tried");
    }

    #[test]
    fn a_close_match_that_drops_or_adds_a_word_or_changes_a_number_or_a_negation_is_refused() {
        for changed in [
            // "not" dropped: three letters, within what a quote this long may differ by
            "Merging the fine-tuned models does improve the accuracy on every task we tried",
            // a word added
            "the fine-tuned models does not improve the accuracy on every one task we tried",
            // "no" for "not"
            "Merging the fine-tuned models does no improve the accuracy on every task we tried",
            // a number changed
            "on every task we tried, and it costs 8 GPU hours. Calibration is unlikely to help",
            // a negating prefix dropped
            "and it costs 3 GPU hours. Calibration is likely to help and never removes the shared",
            // "never" for "ever"
            "Calibration is unlikely to help and ever removes the shared directions.",
            // another word, not a slip
            "Merging the fine-tuned models does not improve the accuracy on every task we tired",
        ] {
            assert_eq!(locate_in(SAYS, changed), None, "{changed}");
            let read = read_passages(&[format!("1. Results\n\n{SAYS}\n")]);
            assert_eq!(read.locate(changed), None, "{changed}");
        }
    }

    #[test]
    fn a_quote_padded_with_spaces_and_punctuation_is_no_longer_for_it() {
        assert!(letters_in("  the  model ,,, ... !!! ---   we  show  ") < 20);
        assert_eq!(letters_in("“ﬁne”"), 4, "a ligature counts as its letters, quote marks as nothing");
    }

    #[test]
    fn a_quote_the_paper_does_not_contain_is_not_found() {
        let read = read_passages(&icml());
        assert_eq!(read.locate("Merging always improves every task by twenty points."), None);
        assert_eq!(read.locate(""), None);
        assert_eq!(read.locate("..."), None);
        // in the reference list only: not a passage, so not evidence
        assert_eq!(read.locate("Editing models with task arithmetic"), None);
    }

    #[test]
    fn the_answer_to_a_locate_request_says_whether_it_was_found() {
        let read = read_passages(&icml());
        let found =
            serde_json::to_value(LocateAnswer::from(read.locate("We rescale the inflated singular values"))).unwrap();
        assert_eq!(found["found"], true);
        assert_eq!(found["address"]["page"], 2);
        assert_eq!(found["matched"]["kind"], "exact");
        assert_eq!(found["wording"], "We rescale the inflated singular values");
        let missing =
            serde_json::to_value(LocateAnswer::from(read.locate("Nothing of the sort is in this paper at all")))
                .unwrap();
        assert_eq!(missing, serde_json::json!({"found": false}));
    }

    #[test]
    fn a_quote_across_a_page_break_spans_both_pages() {
        let read = read_passages(&icml());
        let found = read.locate("comes from shared knowledge, which is counted once per task").unwrap();
        assert_eq!((found.address.page, found.address.end_page), (1, 2));
        assert_eq!(chars_of(&read.pages[1].text, 0, found.address.end), "counted once per task");
    }

    #[test]
    fn a_quote_is_found_in_a_plain_text_such_as_an_abstract() {
        let (span, wording, matched) =
            locate_in("We show that shared directions are counted twice.", "shared directions are counted").unwrap();
        assert_eq!(
            (span.start, span.end, wording.as_str(), matched),
            (13, 42, "shared directions are counted", Match::Exact)
        );
        assert_eq!(locate_in("We show nothing.", "shared directions"), None);
    }

    #[test]
    fn a_quote_ending_on_a_letter_with_an_accent_of_its_own_keeps_the_accent() {
        // "café" written as "cafe" and a combining acute accent, as text from some PDFs and pages is
        let text = "Un cafe\u{301} noir, s'il vous plait.";
        let (span, wording, matched) = locate_in(text, "un café").unwrap();
        assert_eq!((wording.as_str(), matched), ("Un cafe\u{301}", Match::Exact));
        assert_eq!(chars_of(text, span.start, span.end), wording);
    }

    #[test]
    fn a_long_run_without_spaces_is_cut_into_sentences_in_linear_time() {
        // dot leaders, or a table row the PDF ran together: 200,000 characters with a full stop at every other one
        let text = format!("{} End of it. Next one.", "x.".repeat(100_000));
        let started = std::time::Instant::now();
        let spans = sentence_spans(&text);
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
        assert_eq!(spans, [(0, 200_000), (200_001, 200_011), (200_012, 200_021)]);
    }

    #[test]
    fn citing_sentences_are_found_by_author_and_year() {
        let pages = icml();
        let read = read_passages(&pages);
        let list = read_reference_list(&pages.join("\n"));
        assert_eq!(list.citing, Citing::ByAuthorAndYear);
        assert_eq!(list.references.len(), 3);
        let cited = citing_sentences(&read, &list);
        let of = |index: usize| cited.iter().find(|cited| cited.reference == index).map(|cited| &cited.sentences);
        let ilharco = of(0).unwrap();
        assert_eq!(ilharco.len(), 1);
        assert_eq!(ilharco[0].written, "Ilharco et al., 2022");
        let lovelace = of(1).unwrap();
        assert!(lovelace[0].sentence.contains("Lovelace and Turing (2024a) saw this first."), "{lovelace:?}");
        assert_eq!(lovelace[0].address.page, 2);
        let ruan = of(2).unwrap();
        assert_eq!(ruan.len(), 2, "{ruan:?}");
        assert!(ruan.iter().all(|sentence| sentence.marker == Marker::AuthorYear));
        let page = &read.pages[ruan[0].address.page as usize - 1].text;
        assert_eq!(chars_of(page, ruan[0].address.start, ruan[0].address.end), ruan[0].sentence);
    }

    #[test]
    fn citing_sentences_are_found_by_bracketed_numbers_with_ranges() {
        let pages = vec![
            "1 Introduction\n\nAttention replaced recurrence [1]. Later work [2, 3] and [4-5] built on it.\n\n\
Nothing is cited here.\n\nReferences\n\n[1] A. Vaswani. Attention is all you need. In NeurIPS, 2017.\n\
[2] J. Devlin. BERT. In NAACL, 2019.\n[3] T. Brown. Language models are few-shot learners. In NeurIPS, 2020.\n\
[4] A. Radford. GPT-2. 2019.\n[5] C. Raffel. T5. In JMLR, 2020.\n"
                .to_owned(),
        ];
        let read = read_passages(&pages);
        let list = read_reference_list(&pages[0]);
        assert_eq!(list.citing, Citing::ByNumber);
        let cited = citing_sentences(&read, &list);
        assert_eq!(cited.iter().map(|cited| cited.reference).collect::<Vec<_>>(), [0, 1, 2, 3, 4]);
        assert_eq!(cited[0].sentences[0].sentence, "Attention replaced recurrence [1].");
        assert_eq!(cited[0].sentences[0].marker, Marker::Number);
        assert_eq!(cited[3].sentences[0].written, "[4-5]");
        assert_eq!(cited[1].sentences[0].sentence, "Later work [2, 3] and [4-5] built on it.");
    }

    #[test]
    fn superscript_numbers_are_read_only_where_a_numbered_list_has_no_brackets() {
        let pages = vec![
            "1 Introduction\n\nAttention replaced recurrence in translation models.1 It was scaled up later.2,3\n\n\
References\n\n1. Vaswani, A. Attention is all you need. In NeurIPS, 2017.\n2. Devlin, J. BERT. In NAACL, 2019.\n\
3. Brown, T. Language models are few-shot learners. In NeurIPS, 2020.\n"
                .to_owned(),
        ];
        let read = read_passages(&pages);
        let list = read_reference_list(&pages[0]);
        let cited = citing_sentences(&read, &list);
        assert_eq!(cited.len(), 3, "{cited:?}");
        assert!(cited.iter().all(|cited| cited.sentences[0].marker == Marker::Superscript));
        assert_eq!(numbers("3, 7-9"), [3, 7, 8, 9]);
        assert_eq!(numbers("12–14"), [12, 13, 14]);
    }

    #[test]
    fn surnames_are_read_whichever_way_a_list_writes_names() {
        assert_eq!(surname("Vaswani, A.").as_deref(), Some("Vaswani"));
        assert_eq!(surname("Ashish Vaswani").as_deref(), Some("Vaswani"));
        assert_eq!(surname("van Schaik, A.").as_deref(), Some("van Schaik"));
        assert_eq!(surname("Qwen Team").as_deref(), Some("Qwen Team"));
        assert_eq!(surname("OpenAI").as_deref(), Some("OpenAI"));
        assert_eq!(surname(" "), None);
    }

    #[test]
    fn a_heading_that_runs_into_its_paragraph_opens_a_section() {
        let pages = vec![
            "1. Introduction

We begin here.

4.3.5. ROBUSTNESS AND SCOPE We finally test how tightly
the effect is tied to one teacher.

3.1. A common approach is to
add the updates of all tasks.
"
            .to_owned(),
        ];
        let read = read_passages(&pages);
        assert_eq!(of_kind(&read, Kind::Heading), ["1. Introduction", "4.3.5. ROBUSTNESS AND SCOPE"]);
        let rest = read.passages.iter().find(|passage| passage.text.starts_with("We finally")).unwrap();
        assert_eq!(rest.address.section, "4.3.5. ROBUSTNESS AND SCOPE");
        assert!(of_kind(&read, Kind::Text).contains(&"3.1. A common approach is to add the updates of all tasks."));
    }

    #[test]
    fn headings_are_told_from_numbered_sentences_and_figure_labels() {
        let heading = |text: &str| is_heading(&Block { lines: vec![text.to_owned()] }, text);
        for yes in [
            "1. Introduction",
            "2.1. Problem Statement and Notations",
            "A. Proofs",
            "B PROOFS",
            "References",
            "Acknowledgements",
            "Appendix",
            "Appendix B: More Results",
        ] {
            assert!(heading(yes), "{yes}");
        }
        for no in [
            "3. We demonstrate its effectiveness on tasks",
            "Appendix E.4, we show how Mira",
            "n = 14",
            "1. A list item that ends.",
            "2 The effect of",
            "0 Unknown",
        ] {
            assert!(!heading(no), "{no}");
        }
    }
}
