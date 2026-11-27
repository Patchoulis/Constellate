//! The works a paper cites, read from its reference list: what a citation network is built from.
//!
//! The text comes out of a PDF ([`crate::text::pdf`]), so it is the reference list as it was laid out on the page: entries
//! wrap over several lines, words are hyphenated at line ends, addresses break in the middle, and a page break drops
//! a page number and the running header into the middle of an entry. Reading it is three steps:
//!
//! 1. **Find the list**: the lines after a heading "References" or "Bibliography" (any case, numbered or not),
//!    up to the first appendix, checklist or supplementary heading, or the end. When several headings qualify
//!    (a table of contents, a second list in the appendix), the one that yields the most entries is kept.
//! 2. **Split it into entries**. Numbered lists start an entry at each marker ("[12]", or "12." counting up);
//!    unnumbered (author-year) lists at a line that begins with an author's name ("Vaswani, A.", "Ashish
//!    Vaswani,", "DeepSeek-AI.") right after a blank line or a line that ended a sentence. A line that does not
//!    begin like that continues the entry before it, which is what makes two-column pages and page breaks
//!    harmless: a stray line joins an entry instead of starting one. Running headers (a line seen on many pages)
//!    and page numbers are dropped first.
//! 3. **Read each entry**: identifiers first (an arXiv id in any of its written forms, a DOI, an address), then
//!    the year, the authors and the title. The title is the hard part; the layouts read are
//!
//!    ```text
//!    Vaswani, A., Shazeer, N., and Parmar, N. Attention is all you need. In NeurIPS, 2017.        (ICML)
//!    Ashish Vaswani, Noam Shazeer, and Niki Parmar. Attention is all you need. In NeurIPS, 2017.  (NeurIPS)
//!    Vaswani, A., & Shazeer, N. (2017). Attention is all you need. In NeurIPS.                    (APA)
//!    A. Vaswani, N. Shazeer, "Attention is all you need," in NeurIPS, 2017.                      (IEEE)
//!    ```
//!
//! An entry never pretends to more than was read: each keeps its text as written and says how far it was
//! understood ([`Confidence`]), so a title that could not be told apart is shown as the raw entry, not as a guess.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::text::{first_chars, pattern, squash};

/// More than this is not one paper's reference list (the longest surveys list a few hundred).
pub const MAX_REFERENCES: usize = 600;
/// Characters of one entry kept: a longer one is several entries run together.
const MAX_ENTRY: usize = 1_200;
/// A line seen this often in a paper is a running header or footer, not part of an entry.
const HEADER_REPEATS: usize = 3;
/// Names kept from an entry's author list.
const MAX_AUTHORS: usize = 60;
/// Words a title may have; more is a run-on of title and venue.
const MAX_TITLE_WORDS: usize = 45;

/// How far an entry was understood.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// It carries an arXiv id or a DOI: it can be matched for certain.
    Identified,
    /// Its title was told apart from its authors and venue; it can be matched by title.
    TitleParsed,
    /// Only the text as written is known.
    RawOnly,
}

/// One entry of a reference list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    /// The entry as written, on one line.
    pub raw: String,
    /// As written: "Vaswani, A." or "Ashish Vaswani".
    pub authors: Vec<String>,
    pub title: Option<String>,
    pub year: Option<u16>,
    pub venue: Option<String>,
    /// Without its version: "2301.01234", or "cs/0501001" for the old style.
    pub arxiv: Option<String>,
    pub doi: Option<String>,
    pub url: Option<String>,
    pub confidence: Confidence,
}

/// A title as it is compared: letters and digits only, lower case, accents dropped (`_key` in
/// `rp/proceedings.py`). Line breaks, hyphens and spacing from a PDF then make no difference.
pub fn title_key(title: &str) -> String {
    title.nfkd().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// The words of a title as they are compared: lower case, accents dropped.
pub fn title_words(title: &str) -> Vec<String> {
    let plain: String = title
        .nfkd()
        .filter(char::is_ascii)
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { ' ' })
        .collect();
    plain.split_whitespace().map(str::to_owned).collect()
}

/// Every entry of the paper's reference list, in the order listed; empty when no list is found.
pub fn read_references(text: &str) -> Vec<Reference> {
    read_reference_list(text).references
}

/// How the paper's text points at its reference list: by the entries' numbers ("[12]"), or by their authors and
/// years ("Vaswani et al., 2017"). A numbered list's entry `n` is `references[n - 1]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Citing {
    ByNumber,
    ByAuthorAndYear,
}

/// The reference list and how it is cited: what the citing sentences of a paper are found with
/// ([`super::passages::citing_sentences`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReferenceList {
    pub citing: Citing,
    pub references: Vec<Reference>,
}

/// The paper's reference list, with how its entries are marked.
pub fn read_reference_list(text: &str) -> ReferenceList {
    let lines = page_lines(text);
    let (marking, best) = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.as_deref().is_some_and(|line| REFERENCES_HEADING.is_match(line)))
        .map(|(at, _)| entries(list_after(&lines, at + 1)))
        .max_by_key(|(_, entries)| entries.len())
        .unwrap_or((Marking::Authors, Vec::new()));
    let citing = match marking {
        Marking::Brackets | Marking::Numbers => Citing::ByNumber,
        Marking::Authors => Citing::ByAuthorAndYear,
    };
    ReferenceList { citing, references: best.iter().take(MAX_REFERENCES).map(|entry| read_entry(entry)).collect() }
}

pub(super) static REFERENCES_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)^(?:\d{1,2}\.?\s*|[A-Z]\.?\s+)?(?:references|bibliography|references and notes|literature cited|works cited)$",
    )
});
/// What follows a reference list: an appendix or a checklist.
pub(super) static END_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)^(?:appendix|appendices|supplementary material|supplemental material|technical appendices|(?:neurips |paper )*checklist)\b",
    )
});
/// An appendix's lettered heading, "A Proofs" or "B.1. More results": short, and no sentence in it.
pub(super) static APPENDIX_HEADING: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^[A-H](?:\.\d+)*\.?\s+\p{Lu}[^.,:;()\d]{2,60}$"));
static PAGE_NUMBER: LazyLock<Regex> = LazyLock::new(|| pattern(r"^\d{1,4}$"));

/// The text's lines, trimmed; a blank line is `None`. Page numbers and running headers are left out entirely.
fn page_lines(text: &str) -> Vec<Option<String>> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for line in text.lines().map(str::trim) {
        *seen.entry(line).or_default() += 1;
    }
    // a header repeats on every page; an entry's line rarely does, and then it ends with punctuation ("In NeurIPS,")
    let header = |line: &str| {
        line.chars().count() >= 12
            && seen.get(line).is_some_and(|&times| times >= HEADER_REPEATS)
            && !line.ends_with([',', '.', ';', ':', '-'])
            && !line.starts_with("In ")
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !PAGE_NUMBER.is_match(line) && !header(line))
        .map(|line| (!line.is_empty()).then(|| accents_composed(line)))
        .collect()
}

/// The lines of the list that starts at `start`, up to what ends it.
fn list_after(lines: &[Option<String>], start: usize) -> &[Option<String>] {
    let mut previous = "";
    for (at, line) in lines.iter().enumerate().skip(start) {
        let Some(line) = line.as_deref() else { continue };
        // "A. EMNIST: ..." is an initial wrapped from the line before, which ended mid-name: only a heading after
        // a finished entry ends the list
        let finished = previous.is_empty() || previous.ends_with('.');
        let heading = END_HEADING.is_match(line) || REFERENCES_HEADING.is_match(line);
        if heading || (finished && APPENDIX_HEADING.is_match(line)) {
            return &lines[start..at];
        }
        previous = line;
    }
    &lines[start..]
}

static BRACKET_MARKER: LazyLock<Regex> = LazyLock::new(|| pattern(r"^\[(\d{1,3})\]\s*"));
static DOT_MARKER: LazyLock<Regex> = LazyLock::new(|| pattern(r"^(\d{1,3})\.\s+"));

/// Name particles that begin a surname in lower case: "van Schaik", "de Freitas".
const PARTICLE: &str = r"(?:van|von|der|den|de|del|della|da|di|du|dos|le|la|ten|ter)\s";
/// One word of a name: capitalised, with apostrophes and hyphens ("O'Neil", "Ben-David", "Müller").
const WORD: &str = r"\p{Lu}[\p{L}\p{M}'’\-]*";
/// Initials: "A.", "J. M.", "J.-P."; after the first, the particles of a name too ("Oord, A. v. d.", "Geng, Y.-a.").
const INITIALS: &str = r"\p{Lu}\.(?:\s?-?\p{L}\.)*";

/// A surname followed by initials, "Van Der Schaar, M.": the ICML and APA way of writing an author.
fn surname_initials() -> String {
    format!(r"(?:{PARTICLE})*{WORD}(?:\s(?:{PARTICLE})*{WORD})*,\s+{INITIALS}")
}

/// A line that begins an author-year entry.
static AUTHOR_START: LazyLock<Regex> = LazyLock::new(|| {
    pattern(&format!(
        r"^(?:{surname}|{INITIALS}\s+{WORD}(?:,|\s+and\s)|{WORD}(?:\s{INITIALS})?\s(?:{PARTICLE})*{WORD}(?:\s{WORD})?(?:,|\.\s|\s+and\s)|[\p{{Lu}}][\p{{L}}\d\-]+\.\s)",
        surname = surname_initials()
    ))
});
/// A line that begins with a surname and initials.
static SURNAME_START: LazyLock<Regex> = LazyLock::new(|| pattern(&format!("^{}", surname_initials())));
/// A line that begins with an organisation as the author: "OpenAI.", "DeepSeek-AI.", "Qwen Team.".
static ORGANISATION_START: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^\p{Lu}[\p{L}\d\-]+(?:\s\p{Lu}[\p{L}\d\-]+){0,2}\.\s"));

/// Does this line begin an entry with its first author (or the organisation that wrote it)? In a list whose
/// entries begin "Surname, I." (`strict`), only that way of writing a name counts.
fn begins_with_author(line: &str, strict: bool) -> bool {
    // "In Meila, M. and Zhang, T. (eds.), Proceedings of ..." names the editors of the book an entry is in
    if line.starts_with("In ") {
        return false;
    }
    if strict { SURNAME_START.is_match(line) || ORGANISATION_START.is_match(line) } else { AUTHOR_START.is_match(line) }
}

/// Lines of text that follow a finished entry without beginning one: this many, and the list is over (an
/// appendix with no heading); fewer are a stray (a footnote, a caption from the other column) and are dropped.
const STRAY_LINES: usize = 3;

/// How the list's entries are marked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Marking {
    /// "[12] ..."
    Brackets,
    /// "12. ..."
    Numbers,
    /// Not numbered: an entry begins with its first author.
    Authors,
}

fn marking(lines: &[&str]) -> Marking {
    let brackets = lines.iter().filter(|line| BRACKET_MARKER.is_match(line)).count();
    if brackets >= 3 {
        return Marking::Brackets;
    }
    // numbers counting up from 1, not the odd line that begins with a figure
    let mut expected = 1;
    for line in lines {
        if let Some(number) = DOT_MARKER.captures(line).and_then(|found| found[1].parse::<usize>().ok()) {
            if number == expected {
                expected += 1;
            }
        }
    }
    if expected > 3 { Marking::Numbers } else { Marking::Authors }
}

/// How the list's entries are marked, and its entries, each on one line, without its marker.
fn entries(list: &[Option<String>]) -> (Marking, Vec<String>) {
    let present: Vec<&str> = list.iter().flatten().map(String::as_str).collect();
    let marking = marking(&present);
    let strict = marking == Marking::Authors && mostly_surname_first(list);
    let mut entries: Vec<Vec<&str>> = Vec::new();
    let mut expected = 1;
    let mut after_break = true; // the list's first line, or the line after a blank one
    let mut previous = "";
    let mut strays = 0;
    for line in list {
        let Some(line) = line.as_deref() else {
            after_break = true;
            continue;
        };
        let start = match marking {
            Marking::Brackets => BRACKET_MARKER.find(line).map(|marker| marker.end()),
            Marking::Numbers => DOT_MARKER.captures(line).and_then(|found| {
                let number: usize = found[1].parse().ok()?;
                // a missed marker should not lose the rest of the list, so a skip of one is allowed
                (number == expected || number == expected + 1).then(|| {
                    expected = number + 1;
                    found[0].len()
                })
            }),
            Marking::Authors => {
                let finished = after_break || previous.ends_with('.');
                // in a list whose entries begin "Surname, I.", a line "Curran Associates, Inc., 2015." after a
                // page break ends the entry before it rather than beginning one
                (entries.is_empty() || (finished && begins_with_author(line, strict))).then_some(0)
            }
        };
        // a line with a year in it is still the list (an entry whose start was missed), so it is never dropped
        let stray = marking == Marking::Authors
            && start.is_none()
            && !YEAR.is_match(line)
            && (after_break || strays > 0)
            && entries.last().is_some_and(|entry| complete(&joined(entry)));
        if stray {
            strays += 1;
            if strays >= STRAY_LINES {
                break;
            }
        } else {
            strays = 0;
            match (start, entries.last_mut()) {
                (Some(marker), _) => entries.push(vec![&line[marker..]]),
                (None, Some(entry)) => entry.push(line),
                (None, None) => {} // before the first marker: the heading's own line, a note
            }
        }
        after_break = false;
        previous = line;
    }
    (marking, entries.iter().map(|lines| joined(lines)).filter(|entry| !entry.is_empty()).collect())
}

/// Do most of the list's entries begin with a surname and initials, "Vaswani, A."?
fn mostly_surname_first(list: &[Option<String>]) -> bool {
    let (mut starts, mut surname_first) = (0, 0);
    let mut previous: Option<&str> = None;
    for line in list {
        let line = line.as_deref();
        let finished = previous.is_none_or(|previous| previous.ends_with('.'));
        if let Some(line) = line.filter(|line| finished && begins_with_author(line, false)) {
            starts += 1;
            surname_first += usize::from(SURNAME_START.is_match(line));
        }
        previous = line;
    }
    starts > 0 && surname_first * 10 >= starts * 6
}

/// Has this entry said all an entry says (authors, title, venue and year) and come to a full stop?
fn complete(entry: &str) -> bool {
    entry.ends_with('.') && YEAR.is_match(entry) && sentences(entry).len() >= 2
}

/// The last word of `text`, if it is part of an address or a DOI: those break anywhere and rejoin without a space.
fn ends_in_address(text: &str) -> bool {
    let last = text.rsplit(' ').next().unwrap_or_default();
    let address =
        last.contains("://") || last.starts_with("http") || last.starts_with("10.") || last.starts_with("doi.");
    address && last.ends_with(['/', ':', '.', '-', '_', '='])
}

/// An entry's lines as one line: a word hyphenated at a line end is joined again, an address too.
fn joined(lines: &[&str]) -> String {
    let mut entry = String::new();
    for line in lines {
        let next_lower = line.chars().next().is_some_and(char::is_lowercase);
        let mut ending = entry.chars().rev();
        let hyphenated = ending.next() == Some('-') && ending.next().is_some_and(char::is_lowercase);
        // "plug-and-" "play" keeps its hyphen: a word that already has one is a compound
        let last_word = entry.rsplit(' ').next().unwrap_or_default();
        let compound = last_word.trim_end_matches('-').contains('-');
        if hyphenated && next_lower && !compound {
            entry.pop(); // "poli-" "cies" was one word
        } else if !(entry.is_empty() || entry.ends_with(['-', '–']) || ends_in_address(&entry)) {
            entry.push(' ');
        }
        entry.push_str(line);
    }
    first_chars(&squash(&entry), MAX_ENTRY).to_owned()
}

/// A PDF often writes an accent as its own character before the letter ("M¨oller"); put it on the letter.
pub(super) fn accents_composed(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending = None;
    for c in text.chars() {
        let mark = match c {
            '\u{a8}' => Some('\u{308}'),  // diaeresis
            '\u{b4}' => Some('\u{301}'),  // acute
            '`' => Some('\u{300}'),       // grave
            '\u{2c6}' => Some('\u{302}'), // circumflex
            '\u{2dc}' => Some('\u{303}'), // tilde
            '\u{b8}' => Some('\u{327}'),  // cedilla
            '\u{2c7}' => Some('\u{30c}'), // caron
            _ => None,
        };
        match (mark, pending.take()) {
            (Some(mark), _) => pending = Some((c, mark)),
            (None, Some((_, mark))) if c.is_alphabetic() => {
                out.push(c);
                out.push(mark);
            }
            (None, Some((spacing, _))) => {
                out.push(spacing);
                out.push(c);
            }
            (None, None) => out.push(c),
        }
    }
    if let Some((spacing, _)) = pending {
        out.push(spacing);
    }
    out.nfc().collect()
}

static ARXIV_NEW: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)(?:arxiv(?:\s+preprint)?\s*[:.]?\s*(?:arxiv\s*:\s*)?|arxiv\.org/(?:abs|pdf)/|\babs/|10\.48550/arxiv\.)(\d{4}\.\d{4,5})(?:v\d+)?",
    )
});
static ARXIV_OLD: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)(?:arxiv(?:\s+preprint)?\s*:?\s*|arxiv\.org/(?:abs|pdf)/|\babs/)([a-z][a-z\-]+)(?:\.[a-z]{2})?/(\d{7})(?:v\d+)?",
    )
});
static DOI: LazyLock<Regex> = LazyLock::new(|| pattern(r"\b(10\.\d{4,9}/[^\s\x22<>]+)"));
static URL: LazyLock<Regex> = LazyLock::new(|| pattern(r"https?://[^\s\x22<>]+"));
/// Where the bibliographic part of an entry ends and its addresses begin.
static ADDRESSES: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)\s(?:url\s|doi\s*:|https?://|issn\s|isbn\s)"));
static YEAR_IN_PARENTHESES: LazyLock<Regex> = LazyLock::new(|| pattern(r"\(((?:19[5-9]|20[0-4])\d)[a-z]?\)"));
static YEAR: LazyLock<Regex> = LazyLock::new(|| pattern(r"\b((?:19[5-9]|20[0-4])\d)[a-z]?\b"));
static QUOTED: LazyLock<Regex> = LazyLock::new(|| pattern(r#"[“"]([^”"]{6,}?)[,.]?\s*[”"]"#));
static SURNAME_LIST: LazyLock<Regex> = LazyLock::new(|| {
    let one = surname_initials();
    pattern(&format!(r"^{one}(?:(?:,\s*(?:and\s+|&\s*)?|,?\s+and\s+|\s*&\s*){one})*(?:,?\s+et\s+al\.)?"))
});
/// "..., April 2025" or ", 2024c" at the end of a title's sentence: when it was published, not the title.
static TRAILING_YEAR: LazyLock<Regex> =
    LazyLock::new(|| pattern(&format!(r",\s*(?:{MONTH}\s+)?(?:19|20)\d{{2}}[a-z]?$")));
/// A month at the end of a venue whose year was taken out: "ICLR, September".
static TRAILING_MONTH: LazyLock<Regex> = LazyLock::new(|| pattern(&format!(r",?\s*{MONTH}$")));
const MONTH: &str = r"(?:January|February|March|April|May|June|July|August|September|October|November|December|Jan\.|Feb\.|Mar\.|Apr\.|Jun\.|Jul\.|Aug\.|Sep\.|Sept\.|Oct\.|Nov\.|Dec\.)";
static LEADING_YEAR: LazyLock<Regex> = LazyLock::new(|| pattern(r"^\(?(?:19|20)\d{2}[a-z]?\)?[.,]?\s+"));
static NOT_A_TITLE: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)^(?:in\s|proceedings|arxiv|advances in|journal|transactions|url\b|doi\b|https?:|preprint|available|retrieved|accessed|pp\.|vol\.|volume\b)",
    )
});
static AUTHOR_SEPARATOR: LazyLock<Regex> = LazyLock::new(|| pattern(r",\s*(?:and\s+|&\s*)?|\s+and\s+|\s*&\s*"));
static ONLY_INITIALS: LazyLock<Regex> = LazyLock::new(|| pattern(&format!(r"^{INITIALS}$")));
static ET_AL: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i),?\s*et\.?\s+al\.?$"));

/// The arXiv id an entry cites, without its version: "2301.01234", "cs/0501001".
fn arxiv_id(entry: &str) -> Option<String> {
    if let Some(found) = ARXIV_NEW.captures(entry) {
        return Some(found[1].to_owned());
    }
    ARXIV_OLD.captures(entry).map(|found| format!("{}/{}", found[1].to_lowercase(), &found[2]))
}

fn trimmed_address(address: &str) -> String {
    address.trim_end_matches(['.', ',', ';', ')', ']']).to_owned()
}

fn read_entry(raw: &str) -> Reference {
    let arxiv = arxiv_id(raw);
    let doi = DOI
        .captures(raw)
        .map(|found| trimmed_address(&found[1]))
        .filter(|doi| !doi.to_lowercase().starts_with("10.48550/"));
    let url = URL.find(raw).map(|found| trimmed_address(found.as_str()));
    // the authors, title and venue come before any address; ids are blanked so that "2012.01234" is no year
    let cut = ADDRESSES.find(raw).map_or(raw.len(), |found| found.start());
    let body = ARXIV_OLD.replace_all(&ARXIV_NEW.replace_all(&raw[..cut], "arXiv"), "arXiv").into_owned();
    let year = YEAR_IN_PARENTHESES
        .captures(&body)
        .or_else(|| YEAR.captures_iter(&body).last())
        .and_then(|found| found[1].parse().ok());
    let (authors, title, venue) = authors_title_venue(&body);
    let title = title.filter(|title| plausible_title(title));
    let confidence = match (&arxiv, &doi, &title) {
        (Some(_), _, _) | (_, Some(_), _) => Confidence::Identified,
        (None, None, Some(_)) => Confidence::TitleParsed,
        (None, None, None) => Confidence::RawOnly,
    };
    Reference { raw: raw.to_owned(), authors, title, year, venue, arxiv, doi, url, confidence }
}

/// The places a sentence of an entry ends: ". " (not after an initial), "? " and "! ".
fn sentences(text: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut pieces = Vec::new();
    let mut from = 0;
    for (index, &(at, c)) in chars.iter().enumerate() {
        let followed = chars.get(index + 1).is_some_and(|&(_, next)| next == ' ');
        if !followed || !matches!(c, '.' | '?' | '!') {
            continue;
        }
        if c == '.' {
            // "J." or "J.-P." is an initial; "al." ends an author list and so ends a sentence
            let before = index.checked_sub(1).map(|i| chars[i].1);
            let two_before = index.checked_sub(2).map(|i| chars[i].1);
            let initial =
                before.is_some_and(char::is_uppercase) && two_before.is_none_or(|c| matches!(c, ' ' | '-' | '.'));
            if initial {
                continue;
            }
        }
        let end = at + c.len_utf8();
        pieces.push(text[from..end].trim());
        from = end;
    }
    pieces.push(text[from..].trim());
    pieces.retain(|piece| !piece.is_empty());
    pieces
}

/// Split an entry's bibliographic part into its authors, its title and its venue.
fn authors_title_venue(body: &str) -> (Vec<String>, Option<String>, Option<String>) {
    // IEEE: the title is the quoted part
    if let Some(quoted) = QUOTED.captures(body) {
        let whole = quoted.get(0).map_or(0..0, |found| found.range());
        let authors = body[..whole.start].trim().trim_end_matches([',', ' ']);
        let venue = body[whole.end..].trim().trim_start_matches([',', '.', ' ']);
        return (names(authors), Some(quoted[1].trim().to_owned()), venue_of(venue));
    }
    // ICML and APA: names written "Surname, I." end in an initial, so the author list is read as names
    let (authors, rest) = match SURNAME_LIST.find(body) {
        Some(list) if body[list.end()..].starts_with(' ') => (list.as_str(), &body[list.end()..]),
        _ => {
            // the author list is the first sentence, its full stop not part of the last name
            let pieces = sentences(body);
            let first = pieces.first().copied().unwrap_or_default();
            (first.trim_end_matches('.'), body.get(first.len()..).unwrap_or_default())
        }
    };
    let rest = LEADING_YEAR.replace(rest.trim(), "");
    let pieces = sentences(&rest);
    // "Appa: Bending weather dynamics, 2025." has its year in the title's sentence
    let title = pieces.first().map(|title| TRAILING_YEAR.replace(title.trim_end_matches('.').trim(), "").into_owned());
    let venue = pieces.get(1).and_then(|venue| venue_of(venue));
    (names(authors), title, venue)
}

/// The names of an author list, as written; "Vaswani, A." stays one name.
fn names(list: &str) -> Vec<String> {
    let list = ET_AL.replace(list.trim(), "");
    let mut names: Vec<String> = Vec::new();
    for piece in AUTHOR_SEPARATOR.split(&list).map(str::trim).filter(|piece| !piece.is_empty()) {
        match names.last_mut() {
            // "Vaswani" "A." was split at the comma between surname and initials
            Some(last) if ONLY_INITIALS.is_match(piece) && !last.contains(',') && !ONLY_INITIALS.is_match(last) => {
                last.push_str(", ");
                last.push_str(piece);
            }
            _ => names.push(piece.to_owned()),
        }
        if names.len() >= MAX_AUTHORS {
            break;
        }
    }
    names
}

/// Where the work appeared, from the sentence after its title: "In NeurIPS, 2017." is "NeurIPS".
fn venue_of(sentence: &str) -> Option<String> {
    let venue = sentence.trim().trim_start_matches("In ").trim_start_matches("in ");
    // "Guyon, I., ... (eds.), Advances in ..." names the editors first
    let venue = venue.rsplit_once("(eds.),").map_or(venue, |(_, after)| after).trim();
    let venue = venue.split(", pp.").next().unwrap_or(venue);
    let venue = squash(&YEAR.replace_all(venue, ""));
    let venue = venue.trim_end_matches(['.', ',', ' ', ':']);
    let venue = TRAILING_MONTH.replace(venue, "");
    let venue = venue.trim_end_matches(['.', ',', ' ', ':']);
    let venue = if venue.to_lowercase().starts_with("arxiv") { "arXiv" } else { venue };
    (venue.chars().any(char::is_alphabetic) && venue.chars().count() <= 200).then(|| venue.to_owned())
}

/// Could this be a title, rather than a venue, an address or a fragment?
fn plausible_title(title: &str) -> bool {
    let words = title.split_whitespace().count();
    title.chars().filter(|c| c.is_alphabetic()).count() >= 6
        && (1..=MAX_TITLE_WORDS).contains(&words)
        && !NOT_A_TITLE.is_match(title)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// The way a two-column ICML page comes out of the PDF: blank lines between entries, hyphenated words, an
    /// address broken over two lines, and a page break (number and running header) in the middle of an entry.
    const ICML: &str = "\
Some text of the paper's conclusion.

References

Acero, F. and Li, Z. Distilling reinforcement learning poli-
cies for interpretable robot locomotion: Gradient boosting
machines and symbolic regression. In IEEE/RSJ Inter-
national Conference on Intelligent Robots and Systems,
IROS 2024, pp. 6840–6847. IEEE, 2024. doi: 10.1109/
IROS58592.2024.10802433. URL https://doi.
org/10.1109/IROS58592.2024.10802433.

Achiam, J., Adler, S., Agarwal, S., Ahmad, L., Akkaya, I.,
Anadkat, S., et al. Gpt-4 technical report. arXiv preprint
arXiv:2303.08774, 2023.

Chen, Y., Xia, J., Shao, S., Ge, D., and Ye, Y.

9

CALM Before the STORM: Reasoning for Optimization

Solver-informed rl: Grounding large language models
for authentic optimization modeling. arXiv preprint
arXiv:2505.11792, 2025.
 Cohen, G., Afshar, S., Tapson, J., and van Schaik,
A. EMNIST: Extending MNIST to handwritten letters. In
2017 International Joint Conference on Neural Networks, 2017.

Antonelo, E., Couto, G., and M¨oller, C. Exploring multi-
modal implicit behavior learning. In Anais do ENIAC, 2025.

A. Theoretical Explanation

We prove that the bound holds.

CALM Before the STORM: Reasoning for Optimization
CALM Before the STORM: Reasoning for Optimization
";

    fn titles(text: &str) -> Vec<Option<String>> {
        read_references(text).into_iter().map(|reference| reference.title).collect()
    }

    #[test]
    fn an_icml_list_is_split_into_its_entries_and_ends_at_the_appendix() {
        let read = read_references(ICML);
        assert_eq!(
            titles(ICML),
            [
                "Distilling reinforcement learning policies for interpretable robot locomotion: Gradient boosting machines and symbolic regression",
                "Gpt-4 technical report",
                "Solver-informed rl: Grounding large language models for authentic optimization modeling",
                "EMNIST: Extending MNIST to handwritten letters",
                "Exploring multimodal implicit behavior learning",
            ]
            .map(|title| Some(title.to_owned()))
        );
        assert_eq!(read[0].authors, ["Acero, F.", "Li, Z."]);
        assert_eq!(read[1].authors.len(), 6);
        assert_eq!(read[3].authors.last().unwrap(), "van Schaik, A.");
        assert_eq!(read[4].authors.last().unwrap(), "Möller, C.");
        assert_eq!(
            read.iter().map(|reference| reference.year).collect::<Vec<_>>(),
            [2024, 2023, 2025, 2017, 2025].map(Some)
        );
    }

    /// What was once read wrongly in real ICML 2026 papers: a venue's last line after a page break taken for an
    /// entry, an editor list taken for an entry, names with lower-case parts, an organisation as the author, a
    /// compound word hyphenated at a line end, a date in the title's sentence, and an appendix with no heading.
    const ONCE_READ_WRONGLY: &str = "\
References

Coates, A., Ng, A., and Lee, H. An analysis of single-layer networks in unsupervised feature learning. In
Proceedings of AISTATS, pp. 215–223.

JMLR Workshop and Conference Proceedings, June 2011.

Ho, J. and Ermon, S. Generative adversarial imitation learning.
In Lee, D. D., Sugiyama, M., and Garnett, R. (eds.), Advances in Neural Information Processing Systems 29, pp. 4565–
4573, 2016.

Sun, W., Li, Q., Geng, Y.-a., and Li, B. Cat merging: A training-free approach. arXiv preprint arXiv:2505.06977, 2025a.

Theis, L., Oord, A. v. d., and Bethge, M. A note on the evaluation of generative models. In ICLR, 2016.

Qwen Team. Qwen3 technical report. arXiv preprint arXiv:2505.09388, 2025.

Zhu, Y. and Gool, L. V. Denoising diffusion models for plug-and-
play image restoration, April 2023. URL https://openreview.net/forum?id=abc.

Organization In this appendix, we describe the materials
that are not covered in the main paper, and report more
visualisations and experiments.
";

    #[test]
    fn layouts_once_read_wrongly_are_read_right() {
        let read = read_references(ONCE_READ_WRONGLY);
        assert_eq!(
            titles(ONCE_READ_WRONGLY),
            [
                "An analysis of single-layer networks in unsupervised feature learning",
                "Generative adversarial imitation learning",
                "Cat merging: A training-free approach",
                "A note on the evaluation of generative models",
                "Qwen3 technical report",
                "Denoising diffusion models for plug-and-play image restoration",
            ]
            .map(|title| Some(title.to_owned()))
        );
        assert!(read[0].raw.ends_with("pp. 215–223. JMLR Workshop and Conference Proceedings, June 2011."));
        assert_eq!(read[0].year, Some(2011));
        assert_eq!(read[1].venue.as_deref(), Some("Advances in Neural Information Processing Systems 29"));
        assert!(read[1].raw.contains("pp. 4565–4573"));
        assert_eq!(read[2].authors, ["Sun, W.", "Li, Q.", "Geng, Y.-a.", "Li, B."]);
        assert_eq!(read[3].authors, ["Theis, L.", "Oord, A. v. d.", "Bethge, M."]);
        assert_eq!(read[4].authors, ["Qwen Team"]);
        assert_eq!(read[5].year, Some(2023));
        assert!(read[5].raw.ends_with("?id=abc."), "the appendix is not part of the last entry");
    }

    #[test]
    fn identifiers_are_read_whatever_way_they_are_written() {
        let read = read_references(ICML);
        assert_eq!(read[0].doi.as_deref(), Some("10.1109/IROS58592.2024.10802433"));
        assert_eq!(read[0].url.as_deref(), Some("https://doi.org/10.1109/IROS58592.2024.10802433"));
        assert_eq!(
            read[0].venue.as_deref(),
            Some("IEEE/RSJ International Conference on Intelligent Robots and Systems, IROS")
        );
        assert_eq!(read[1].arxiv.as_deref(), Some("2303.08774"));
        assert_eq!(read[1].venue.as_deref(), Some("arXiv"));
        assert_eq!(read[3].confidence, Confidence::TitleParsed);
        for (written, id) in [
            ("arXiv:2301.01234", "2301.01234"),
            ("arXiv preprint arXiv:2301.01234v2", "2301.01234"),
            ("CoRR, abs/1706.03762, 2017", "1706.03762"),
            ("https://arxiv.org/pdf/2504.18720v1", "2504.18720"),
            ("doi:10.48550/arXiv.2211.03812", "2211.03812"),
            ("arXiv preprint cs.LG/0501001", "cs/0501001"),
            ("arXiv:hep-th/9901001v3", "hep-th/9901001"),
        ] {
            assert_eq!(arxiv_id(written).as_deref(), Some(id), "{written}");
        }
        assert_eq!(arxiv_id("Proceedings of ICML 2017, pp. 1234-1240"), None);
    }

    #[test]
    fn a_neurips_list_writes_full_names_and_its_titles_are_still_found() {
        let text = "REFERENCES\n\n\
[1] Ashish Vaswani, Noam Shazeer, Niki Parmar, Jakob Uszkoreit, Llion Jones, Aidan N. Gomez, Łukasz\n\
Kaiser, and Illia Polosukhin. Attention is all you need. In Advances in Neural Information Processing\n\
Systems, volume 30, 2017.\n\
[2] Jacob Devlin, Ming-Wei Chang, Kenton Lee, and Kristina Toutanova. BERT: Pre-training of deep\n\
bidirectional transformers for language understanding. In NAACL, 2019.\n\
[3] OpenAI. GPT-4 technical report, 2023.\n";
        let read = read_references(text);
        assert_eq!(
            titles(text),
            [
                "Attention is all you need",
                "BERT: Pre-training of deep bidirectional transformers for language understanding",
                "GPT-4 technical report"
            ]
            .map(|title| Some(title.to_owned()))
        );
        assert_eq!(read[0].authors.len(), 8);
        assert_eq!(read[0].authors[5], "Aidan N. Gomez");
        assert_eq!(read[0].venue.as_deref(), Some("Advances in Neural Information Processing Systems, volume 30"));
        assert_eq!(read[1].year, Some(2019));
        assert_eq!(read[2].authors, ["OpenAI"]);
    }

    #[test]
    fn numbered_apa_and_ieee_entries_are_read() {
        let text = "7 References\n\
1. Vaswani, A., Shazeer, N., & Parmar, N. (2017). Attention is all you need. In NeurIPS.\n\
2. A. Krizhevsky, I. Sutskever, and G. E. Hinton, \u{201c}ImageNet classification with deep convolutional\n\
neural networks,\u{201d} in Advances in Neural Information Processing Systems, 2012.\n\
3. He, K., Zhang, X., Ren, S., & Sun, J. (2016). Deep residual learning for image recognition. CVPR.\n";
        let read = read_references(text);
        assert_eq!(read.len(), 3);
        assert_eq!(read[0].title.as_deref(), Some("Attention is all you need"));
        assert_eq!(read[0].authors, ["Vaswani, A.", "Shazeer, N.", "Parmar, N."]);
        assert_eq!((read[0].year, read[0].venue.as_deref()), (Some(2017), Some("NeurIPS")));
        assert_eq!(read[1].title.as_deref(), Some("ImageNet classification with deep convolutional neural networks"));
        assert_eq!(read[1].authors, ["A. Krizhevsky", "I. Sutskever", "G. E. Hinton"]);
        assert_eq!(read[1].venue.as_deref(), Some("Advances in Neural Information Processing Systems"));
        assert_eq!(read[2].title.as_deref(), Some("Deep residual learning for image recognition"));
    }

    #[test]
    fn an_entry_that_cannot_be_told_apart_keeps_only_its_text_and_says_so() {
        let text = "References\n\nMNIST handwritten digit database, Yann LeCun, Corinna Cortes and Chris Burges.\n\
https://yann.lecun.com/exdb/mnist/.\n";
        let read = read_references(text);
        assert_eq!(read.len(), 1);
        assert_eq!((read[0].title.as_deref(), read[0].confidence), (None, Confidence::RawOnly));
        assert_eq!(read[0].url.as_deref(), Some("https://yann.lecun.com/exdb/mnist/"));
        assert!(read[0].raw.starts_with("MNIST handwritten digit database"));
    }

    #[test]
    fn a_paper_without_a_reference_list_has_no_references() {
        assert!(read_references("Introduction\nWe study references in general.\n").is_empty());
        assert!(read_references("").is_empty());
    }

    #[test]
    fn the_list_with_the_most_entries_wins_over_a_heading_in_the_contents() {
        let text = format!("Contents\nReferences\n1 Introduction\n\n{ICML}");
        assert_eq!(read_references(&text).len(), 5);
    }

    #[test]
    fn titles_compare_by_their_letters_and_digits_only() {
        assert_eq!(title_key("Solver-Informed RL: Grounding"), "solverinformedrlgrounding");
        assert_eq!(title_key("Mo\u{308}ller’s ﬁeld"), "mollersfield");
        assert_eq!(title_words("Self-supervised, Learning!"), ["self", "supervised", "learning"]);
    }
}
