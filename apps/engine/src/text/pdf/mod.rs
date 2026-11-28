//! The text of a paper's PDF, page by page, in reading order: what passages ([`crate::text::pdf::passages`]), reference
//! lists ([`crate::text::pdf::references`]) and citing sentences are read from.
//!
//! The text comes from `pdf-extract`, which is pure Rust: it builds on Windows and Linux with no system library,
//! which `pdfium` (a native library per platform) and `poppler` (a system package) do not. Its reading order is
//! the order the page draws its text in, which for the two-column LaTeX templates of the conferences read here
//! (ICML, NeurIPS, ICLR) is column by column, and it leaves a blank line between paragraphs, so reference entries
//! come out whole and apart. A page break still drops a page number and a running header into an entry; the
//! reference reader expects that.
//!
//! # A PDF is read in a process of its own
//!
//! A PDF is a file from somewhere else, and the PDF library trusts it more than it should: a stream that inflates
//! to gigabytes, objects that refer to themselves or nest thousands deep. A panic can be caught, but running out of
//! memory or overflowing the stack aborts the whole process, and that would take the engine down every time it met
//! that paper, in every run that includes it. So the engine reads each PDF in a short-lived child process of its
//! own program (the hidden argument [`CHILD_ARGUMENT`], started through [`crate::api::guard::config::child_process`], so
//! it inherits none of the engine's secrets):
//!
//! ```text
//!     engine ──PDF bytes on stdin──▶ child: heap capped at CHILD_HEAP, reads the pages
//!     engine ◀──pages as JSON on stdout── child (at most LONGEST_ANSWER bytes)
//!     engine kills a child that is still reading after TIME_LIMIT
//! ```
//!
//! The child's heap is capped by its allocator (the program's `#[global_allocator]`, the `cap` crate), the same
//! on every system and without `unsafe` code here: an allocation past the cap aborts the child, which the engine
//! reads as [`Unreadable::TooMuchMemory`]. A stack overflow, any other crash, a hang and an answer too large are
//! each a reason of their own. They are lasting reasons ([`Unreadable::is_lasting`]): the library keeps them by the
//! PDF's hash, so a hostile PDF is tried once, not at every run. Only "the reader could not be started" is not
//! kept, since that says nothing about the PDF.
//!
//! The process costs a few milliseconds per PDF (measured in the tests' `--ignored` benchmark), next to the tens
//! to hundreds a paper's PDF takes to read. Several are read at once, one per computing thread that asks: the
//! library reads a batch of PDFs on the engine's pool ([`crate::threads`]). Since each child may take up to
//! [`CHILD_HEAP`], no more run at once than the memory free holds at that cap ([`readers_at_once`]), and never
//! more than the pool has threads: a thread past that waits for a place.
//!
//! What the cap does not cover: memory the PDF library maps outside the allocator (it maps none), and the
//! operating system's own accounting (the child's address space is not limited, only what it allocates).
//!
//! Until the program says where it is ([`read_in_children`]), a PDF is read in this process, as before: that is
//! for tests and examples, which have no such program. The engine's `main` always says it.
//!
//! # This folder
//!
//! What is read from a paper's PDF: this text, page by page, its passages ([`passages`]), its reference list
//! ([`references`]), where its figures are drawn ([`figures`]), and what its fonts' codes are ([`fonts`]).

pub mod figures;
pub mod fonts;
pub mod passages;
pub mod references;

use std::io::{self, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, ExitStatus, Stdio};
use std::sync::{Condvar, LazyLock, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::threads::{self, Outcome, Progress};

/// Where the PDF draws something its fonts do not say the characters of (most often a formula in a math font
/// with no usable encoding): one of these stands in the page's text for each such stretch, in place of characters
/// that would be wrong. It is not a letter or a digit, so no quote is ever matched on it; the reader shows a note
/// with a link to the page in its place. U+FFFC, the object replacement character, is what Unicode has for "an
/// object that is not text stands here".
pub const UNREADABLE: char = '\u{FFFC}';
/// A PDF larger than this is not one paper (a scanned book, or not a PDF at all).
pub const MAX_PDF_BYTES: usize = 80 * 1024 * 1024;
/// The argument that makes the engine's program read one PDF from standard input and say its pages as JSON on
/// standard output, then stop. Not meant for people, so not in `--help` (there is none).
pub const CHILD_ARGUMENT: &str = "--read-one-pdf";
/// The most a child may allocate: a PDF of a paper (at most [`MAX_PDF_BYTES`]) with its streams inflated fits in
/// it many times over, so a child that needs more is reading something that is not a paper. At most as many
/// children run as the free memory holds at this cap ([`readers_at_once`]).
pub const CHILD_HEAP: usize = 1024 * 1024 * 1024;
/// The exit code of a child whose own input, output or thread failed: a fault of this computer, not of the PDF.
const CHILD_TROUBLE: u8 = 3;
/// How long a child may take to read one PDF. Real papers take well under a second each.
pub const TIME_LIMIT: Duration = Duration::from_secs(60);
/// The most a child may answer: a paper's text as JSON is a few hundred kilobytes, a book's a few megabytes.
pub const LONGEST_ANSWER: usize = 64 * 1024 * 1024;
/// What is kept of a child's error output, to tell why it stopped.
const LONGEST_ERROR: usize = 16 * 1024;
/// The stack the child reads on: deep enough for any real PDF; a deeper one overflows it, which ends the child.
const CHILD_STACK: usize = 16 * 1024 * 1024;

/// Why a PDF gave no text, in words for the person who asked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum Unreadable {
    #[error("The PDF is larger than any paper ({0} bytes)")]
    TooLarge(usize),
    #[error("The PDF could not be read: {0}")]
    Damaged(String),
    #[error("The PDF could not be read: its layout is one the PDF reader does not handle")]
    Unhandled,
    #[error("The PDF has no text in it (it may be scanned images of pages)")]
    NoText,
    #[error("The PDF could not be read: reading it took longer than {0} seconds")]
    TooSlow(u64),
    #[error("The PDF could not be read: it needed more than {0} MB of memory")]
    TooMuchMemory(usize),
    #[error("The PDF could not be read: its text is larger than any paper's")]
    TooMuchText,
    #[error("The PDF could not be read: the PDF reader stopped ({0})")]
    Crashed(String),
    /// The reader's process could not be started: a fault of this computer, not of the PDF.
    #[error("The PDF reader could not be started: {0}")]
    ReaderUnavailable(String),
}

impl Unreadable {
    /// Does this say something about the PDF itself, so that reading it again would come to the same? Only those
    /// are kept: a reader that could not be started is tried again next time.
    pub fn is_lasting(&self) -> bool {
        !matches!(self, Self::ReaderUnavailable(_))
    }
}

/// The engine's own program, once `main` says it: from then on every PDF is read in a child of it.
static PROGRAM: OnceLock<PathBuf> = OnceLock::new();

/// Read every PDF from now on in a child process of `program` (the engine's own, started with
/// [`CHILD_ARGUMENT`]). Said once, at start; false when it was already said.
pub fn read_in_children(program: PathBuf) -> bool {
    PROGRAM.set(program).is_ok()
}

/// The text of the PDF in `bytes`, its pages one after the other.
pub fn pdf_text(bytes: &[u8]) -> Result<String, Unreadable> {
    pdf_pages(bytes).map(|pages| pages.join("\n"))
}

/// The text of many PDFs on every core, one answer per PDF in their order, and `progress` counted as they are
/// read ([`crate::threads::each`]). Each PDF is read from its own bytes alone, so the answers are the same on one
/// thread or many, and a PDF that cannot be read is that PDF's answer: the others are read as usual.
///
/// Every PDF of the batch is held in memory at once: a caller reading a conference hands them over a few dozen
/// at a time, and keeps what was read from each batch (in one write) before the next.
pub fn pdf_texts<B: AsRef<[u8]> + Sync>(pdfs: &[B], progress: &Progress) -> Vec<Outcome<Result<String, Unreadable>>> {
    threads::each(pdfs, progress, |bytes| pdf_text(bytes.as_ref()))
}

/// The text of the PDF in `bytes`, page by page: `pages[0]` is page 1, as the PDF numbers its pages (not as the
/// paper prints its page numbers). A page with no text (a full-page figure) is an empty string, so that the
/// position of every later page stays its page number: a link back to a place in the paper needs the page.
///
/// Read in a child process once the engine's program is known ([`read_in_children`]), else here.
pub fn pdf_pages(bytes: &[u8]) -> Result<Vec<String>, Unreadable> {
    pdf_reading(bytes).map(|reading| reading.pages)
}

/// [`pdf_pages`], with where the PDF's figures and tables are drawn ([`figures`]).
pub fn pdf_reading(bytes: &[u8]) -> Result<Reading, Unreadable> {
    if bytes.len() > MAX_PDF_BYTES {
        return Err(Unreadable::TooLarge(bytes.len()));
    }
    match PROGRAM.get() {
        Some(program) => read_in_child(program, bytes, TIME_LIMIT),
        None => read_in_this_process(bytes),
    }
}

/// The reading itself, in whatever process runs it. A panic of the PDF library is caught and told as
/// [`Unreadable::Unhandled`]; an abort (memory, stack) is not catchable, which is why the engine runs this in a
/// child. Public for the child and for measuring what the process costs; everything else calls [`pdf_pages`].
pub fn read_in_this_process(bytes: &[u8]) -> Result<Reading, Unreadable> {
    if bytes.len() > MAX_PDF_BYTES {
        return Err(Unreadable::TooLarge(bytes.len()));
    }
    let reading = catch_unwind(AssertUnwindSafe(|| read_pages(bytes)))
        .map_err(|_| Unreadable::Unhandled)?
        .map_err(|error| Unreadable::Damaged(error.to_string()))?;
    if reading.pages.iter().all(|page| page.trim().is_empty()) {
        return Err(Unreadable::NoText);
    }
    Ok(reading)
}

/// What is read of a PDF: its pages' text, and where its figures and tables are drawn.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub pages: Vec<String>,
    pub figures: figures::Figures,
}

/// The pages' text as `pdf-extract` reads it once the fonts say what their codes are ([`fonts::mend`]): what
/// `pdf_extract::extract_text_from_mem_by_pages` does, with that step between loading the document and reading it.
/// Pages are read from the first until one cannot be, as there.
fn read_pages(bytes: &[u8]) -> Result<Reading, pdf_extract::OutputError> {
    let mut document = pdf_extract::Document::load_mem(bytes)?;
    if document.is_encrypted() {
        // as pdf-extract does: a PDF encrypted with an empty password is read, any other is not
        document.decrypt("")?;
    }
    fonts::mend(&mut document);
    let mut pages = Vec::new();
    let mut lines = Vec::new();
    for number in 1.. {
        let mut output = PageText::default();
        if pdf_extract::output_doc_page(&document, &mut output, number).is_err() {
            break;
        }
        let (text, page_lines) = output.laid_out();
        pages.push(unreadable_marked(&text));
        lines.push(page_lines);
    }
    // where the figures are is worth having, not worth the text: a panic finding them loses only them
    let figures = catch_unwind(AssertUnwindSafe(|| figures::find(&document, &lines))).unwrap_or_default();
    Ok(Reading { pages, figures })
}

/// One glyph as the page draws it, in the order it is drawn.
#[derive(Clone, Debug)]
struct Glyph {
    /// Where it starts and its baseline, in the page's own coordinates (y up).
    x: f64,
    y: f64,
    /// Its size in text space: the side of the square with the area of the transformed size. NaN for rotated text,
    /// as in `PlainTextOutput`: every comparison with it is false, so it breaks no line.
    size: f64,
    /// How far it advances, in points.
    advance: f64,
    text: String,
    /// The first glyph of a string the page shows: where spaces and line breaks are decided, as in `PlainTextOutput`.
    word_start: bool,
}

impl Glyph {
    /// The box it takes, for finding where figures are (figures.rs).
    fn rect(&self) -> figures::Rect {
        figures::Rect {
            x0: self.x,
            y0: self.y - self.size * 0.25,
            x1: self.x + self.advance.max(0.0),
            y1: self.y + self.size * 0.8,
        }
    }
}

/// A page's text laid out as `pdf_extract::PlainTextOutput` lays it out (a space where the next glyph starts
/// right of where the last ended, a line break where it starts lower or left, a blank line between lines further
/// apart than one and a half times the glyph's size), with what mathematics needs on top:
///
/// - A glyph much smaller than the largest since the last blank line, set over or under the glyph before it (the
///   limits of a displayed sum), is not taken for the start of a paragraph: a blank line there takes
///   [`SCRIPT_GAP`] times that larger size. Its own small size made every such move look like a paragraph's end,
///   and cut one displayed formula into a passage per piece ("R_K =", "K ∑", "k=1").
/// - A superscript or a subscript (smaller than the glyph it follows, raised or lowered by part of that glyph's size)
///   is written `^{…}` or `_{…}` where it stands: "CoT^{gt}_{d}", "R^{m}", where the page's text had "CoTgt d" and
///   "R m". The marks are part of the text every address counts in, and a quote is matched on its letters and digits
///   only, so they move no address and change no quote's match. A number set small after a word (a footnote's or a
///   citation's mark: "models.12") is left as it was, since the citation reader reads it so.
/// - A fraction (glyphs over and under a short horizontal rule the page draws) is written
///   `(numerator)/(denominator)` where it stands: neither read as "1 |T|" nor cut out of its formula.
///
/// A heading or a footnote is not that much larger or smaller than the text around it, so it is still set apart as
/// before.
#[derive(Default)]
struct PageText {
    glyphs: Vec<Glyph>,
    /// Short horizontal rules the page strokes or fills: fraction bars among them.
    rules: Vec<figures::Rect>,
    word_start: bool,
    /// The top of the page's media box, where PlainTextOutput's y starts.
    top: f64,
}

/// How much smaller than its neighbour a glyph is for its moves to be measured by the neighbour's size: script
/// size is 0.7 of the text's in TeX, scriptscript 0.5; a section heading is 0.83, a footnote 0.8.
const SCRIPT_SIZE: f64 = 0.75;
/// How far, in the larger glyph's size, a script-size glyph may be from its neighbour before a blank line is put
/// between them: the lower limit of a displayed sum sits below the sum's whole depth, a little over twice the
/// size of the text (measured on papers set with Computer Modern and with mathabx).
const SCRIPT_GAP: f64 = 2.4;
/// A superscript or subscript is smaller than its base by this much at least (TeX's script size is 0.7)...
const SCRIPT_SMALLER: f64 = 0.85;
/// ...and raised or lowered by more than this share of its base's size, and less than its whole size.
const SCRIPT_SHIFT: f64 = 0.1;

/// Is `glyph` raised (1) or lowered (-1) as a script of a base of `size` on `baseline`, or not a script (0)?
fn script_of(glyph: &Glyph, baseline: f64, size: f64) -> i8 {
    // NaN sizes are never smaller: not a script
    if glyph.size.partial_cmp(&(size * SCRIPT_SMALLER)) != Some(std::cmp::Ordering::Less) {
        return 0;
    }
    let shift = glyph.y - baseline;
    if shift > size * SCRIPT_SHIFT && shift < size {
        1
    } else if shift < -size * SCRIPT_SHIFT && shift > -size {
        -1
    } else {
        0
    }
}

/// The text of glyphs set together (a numerator, a denominator), with their scripts marked.
fn group_text(glyphs: &[&Glyph]) -> String {
    let Some(base) = glyphs.iter().copied().max_by(|a, b| a.size.total_cmp(&b.size)) else { return String::new() };
    let (mut baseline, mut size) = (base.y, base.size);
    let mut text = String::new();
    let mut open = 0i8;
    let mut last_end = f64::INFINITY;
    for glyph in glyphs {
        let script = script_of(glyph, baseline, size);
        if script != open && open != 0 {
            text.push('}');
        }
        if script == 0 && glyph.x > last_end + glyph.size * 0.25 && !text.is_empty() {
            text.push(' ');
        }
        if script != open && script != 0 {
            text.push_str(if script > 0 { "^{" } else { "_{" });
        }
        open = script;
        if script == 0 && glyph.size >= size * SCRIPT_SMALLER {
            (baseline, size) = (glyph.y, glyph.size);
        }
        text.push_str(&glyph.text);
        last_end = glyph.x + glyph.advance;
    }
    if open != 0 {
        text.push('}');
    }
    text
}

impl PageText {
    /// The fractions of the page: for each rule with glyphs right over and right under it, those glyphs (by their
    /// index), and the fraction as one glyph standing where it starts.
    fn fractions(&self) -> (Vec<bool>, Vec<Option<Glyph>>) {
        let mut taken = vec![false; self.glyphs.len()];
        let mut standing: Vec<Option<Glyph>> = vec![None; self.glyphs.len()];
        for rule in &self.rules {
            let over_the_rule = |glyph: &Glyph| {
                let middle = glyph.x + glyph.advance / 2.0;
                glyph.size.is_finite() && middle >= rule.x0 - 1.0 && middle <= rule.x1 + 1.0
            };
            // measured by the largest glyph near the rule: a subscript of the denominator sits lower than its size
            let near = |glyph: &Glyph| over_the_rule(glyph) && (glyph.y - rule.y0).abs() <= glyph.size * 2.5;
            let big = self.glyphs.iter().filter(|glyph| near(glyph)).map(|glyph| glyph.size).fold(0.0, f64::max);
            let above: Vec<usize> = (0..self.glyphs.len())
                .filter(|at| !taken[*at])
                .filter(|at| {
                    let glyph = &self.glyphs[*at];
                    over_the_rule(glyph) && glyph.y > rule.y1 && glyph.y - rule.y1 <= big * 1.2
                })
                .collect();
            let below: Vec<usize> = (0..self.glyphs.len())
                .filter(|at| !taken[*at])
                .filter(|at| {
                    let glyph = &self.glyphs[*at];
                    over_the_rule(glyph) && glyph.y < rule.y0 && rule.y0 - glyph.y <= big * 1.6
                })
                .collect();
            // a fraction's parts are each one run of glyphs; a table's rows over and under its rules are cells far apart
            let together = |group: &[usize]| {
                let mut spans: Vec<(f64, f64, f64)> = group
                    .iter()
                    .map(|at| {
                        let glyph = &self.glyphs[*at];
                        (glyph.x, glyph.x + glyph.advance, glyph.size)
                    })
                    .collect();
                spans.sort_by(|a, b| a.0.total_cmp(&b.0));
                spans.windows(2).all(|pair| pair[1].0 - pair[0].1 <= pair[0].2.max(pair[1].2) * 1.5)
            };
            // a fraction's numerator and denominator stand alone on their lines: a line of text that runs on past the
            // rule's ends (prose over the rule above a page's footnotes, a table's cells) is not one
            let alone = |group: &[usize]| {
                group.iter().all(|at| {
                    let glyph = &self.glyphs[*at];
                    self.glyphs.iter().enumerate().all(|(other_at, other)| {
                        // on the very same baseline, a word's space or less from the rule's end
                        let beside = (other.y - glyph.y).abs() < glyph.size * 0.15
                            && ((other.x + other.advance > rule.x0 - glyph.size * 0.6
                                && other.x + other.advance <= rule.x0 - 0.5)
                                || (other.x >= rule.x1 + 0.5 && other.x < rule.x1 + glyph.size * 0.6));
                        group.contains(&other_at) || !beside
                    })
                })
            };
            let short = above.len() + below.len() <= 80;
            // the bar of a radical starts where its sign ends: what is under it is the radicand, not a denominator
            let radical = self.glyphs.iter().any(|glyph| {
                glyph.text.contains('√') && (glyph.x + glyph.advance - rule.x0).abs() <= 2.0 && glyph.y < rule.y1 + 2.0
            });
            if radical {
                continue;
            }
            if above.is_empty()
                || below.is_empty()
                || !short
                || !together(&above)
                || !together(&below)
                || !alone(&above)
                || !alone(&below)
            {
                continue;
            }
            let first = *above.iter().chain(&below).min().unwrap_or(&above[0]);
            let numerator: Vec<&Glyph> = above.iter().map(|at| &self.glyphs[*at]).collect();
            let denominator: Vec<&Glyph> = below.iter().map(|at| &self.glyphs[*at]).collect();
            let size = numerator.iter().chain(&denominator).map(|glyph| glyph.size).fold(0.0, f64::max);
            for at in above.iter().chain(&below) {
                taken[*at] = true;
            }
            standing[first] = Some(Glyph {
                x: rule.x0,
                // on the baseline of the formula it is in: the bar is on the math axis, a quarter of the size above
                y: rule.y0 - size * 0.25,
                size,
                advance: rule.x1 - rule.x0,
                text: format!("({})/({})", group_text(&numerator), group_text(&denominator)),
                word_start: true,
            });
        }
        (taken, standing)
    }

    /// The page's text, and its lines with the boxes they take.
    fn laid_out(&self) -> (String, Vec<figures::Line>) {
        let (taken, standing) = self.fractions();
        let glyphs: Vec<Glyph> = self
            .glyphs
            .iter()
            .enumerate()
            .filter_map(|(at, glyph)| match &standing[at] {
                Some(fraction) => Some(fraction.clone()),
                None if taken[at] => None,
                None => Some(glyph.clone()),
            })
            .collect();
        let mut text = String::new();
        let mut lines: Vec<figures::Line> = Vec::new();
        // as PlainTextOutput, which measures from the top of the page: the first glyph is far from where it starts
        let (mut last_end, mut last_x, mut last_y, mut last_size) = (100_000.0, 0.0, self.top, 0.0);
        // the largest glyph since the last blank line: what a script-size glyph is measured against
        let mut line_size: f64 = 0.0;
        // the glyph scripts are set on, and the script open now (1 raised, -1 lowered, 0 none)
        let (mut base_x, mut base_y, mut base_size): (f64, f64, f64) = (0.0, 0.0, f64::NAN);
        let mut base_word = String::new();
        let mut open = 0i8;
        let mut flat_run = false;
        for (at, glyph) in glyphs.iter().enumerate() {
            let size = glyph.size;
            let mut script = if base_size.is_nan() { 0 } else { script_of(glyph, base_y, base_size) };
            if script != 0 && !(glyph.x >= base_x - 1.0 && glyph.x <= last_end + base_size * 1.5) {
                // not beside what it would be set on (from its base to just after the last glyph): not its script
                script = 0;
            }
            if script != 0 && open == 0 {
                // a number set small after a word or a stop is a footnote's or a citation's mark: left as it was
                let run: String = glyphs[at..]
                    .iter()
                    .take_while(|next| script_of(next, base_y, base_size) == script)
                    .map(|next| next.text.as_str())
                    .collect();
                let mark = run.chars().all(|c| c.is_ascii_digit() || ",–-*†‡§".contains(c));
                let after_word = base_word.chars().filter(|c| c.is_alphabetic()).count() >= 2
                    || base_word.ends_with(['.', ',', ';', ':', ')']);
                flat_run = mark && after_word;
            }
            // a footnote's mark: laid out as any glyph, and not the word a later script is set on
            let flat = flat_run && script != 0;
            if flat {
                script = 0;
            } else if script == 0 {
                flat_run = false;
            }
            let mut new_line = lines.is_empty();
            let mut spaced = false;
            if script == 0 && open != 0 {
                text.push('}');
                if let Some(line) = lines.last_mut() {
                    line.text.push('}');
                }
                open = 0;
            }
            if glyph.word_start && script == 0 {
                let large = size.max(line_size);
                let stacked = (glyph.x - last_x).abs() <= large * 1.5;
                let script_size = size.min(last_size) < large * SCRIPT_SIZE && stacked;
                let gap = if script_size { large * SCRIPT_GAP } else { size * 1.5 };
                let drop = (glyph.y - last_y).abs();
                if drop > gap {
                    text.push('\n');
                    line_size = 0.0;
                    new_line = true;
                }
                if glyph.x < last_end && drop > size * 0.5 {
                    text.push('\n');
                    new_line = true;
                }
                if glyph.x > last_end + size * 0.1 {
                    text.push(' ');
                    spaced = true;
                    if let Some(line) = lines.last_mut().filter(|_| !new_line) {
                        line.text.push(' ');
                    }
                }
            }
            let mut written = String::new();
            if script != open {
                if open != 0 {
                    written.push('}');
                }
                written.push_str(if script > 0 { "^{" } else { "_{" });
                open = script;
            }
            written.push_str(&glyph.text);
            text.push_str(&written);
            if size.is_finite() && size > 0.0 {
                match lines.last_mut().filter(|_| !new_line) {
                    Some(line) => {
                        line.text.push_str(&written);
                        line.rect = line.rect.union(&glyph.rect());
                        line.size = line.size.max(size);
                    }
                    None => lines.push(figures::Line { text: written.clone(), rect: glyph.rect(), size }),
                }
            }
            if script == 0 && !flat {
                // the word it is part of: a word is often drawn in pieces (kerned), so only a space or a new line
                // ends it
                if new_line || glyph.text.trim().is_empty() || spaced {
                    base_word.clear();
                }
                base_word.push_str(glyph.text.trim());
                let smaller = size.partial_cmp(&(base_size * SCRIPT_SMALLER)) == Some(std::cmp::Ordering::Less);
                if !smaller || new_line || base_size.is_nan() {
                    (base_x, base_y, base_size) = (glyph.x, glyph.y, size);
                }
            }
            last_x = glyph.x;
            last_y = glyph.y;
            last_size = size;
            line_size = line_size.max(size);
            last_end = glyph.x + glyph.advance;
        }
        if open != 0 {
            text.push('}');
            if let Some(line) = lines.last_mut() {
                line.text.push('}');
            }
        }
        (text, lines)
    }
}

/// The box a stroked or filled path takes on the page, when it is a short horizontal rule (a fraction bar).
fn rule_of(ctm: &pdf_extract::Transform, path: &pdf_extract::Path) -> Option<figures::Rect> {
    let apply = |x: f64, y: f64| (ctm.m11 * x + ctm.m21 * y + ctm.m31, ctm.m12 * x + ctm.m22 * y + ctm.m32);
    let mut points = Vec::new();
    for op in &path.ops {
        match *op {
            pdf_extract::PathOp::MoveTo(x, y) | pdf_extract::PathOp::LineTo(x, y) => points.push(apply(x, y)),
            pdf_extract::PathOp::Rect(x, y, width, height) => {
                points.push(apply(x, y));
                points.push(apply(x + width, y + height));
            }
            pdf_extract::PathOp::CurveTo(..) => return None,
            pdf_extract::PathOp::Close => {}
        }
    }
    let (x0, x1) = points.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (y0, y1) = points.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.1), b.max(p.1)));
    let rect = figures::Rect { x0, y0, x1, y1 };
    (x1 - x0 >= 2.0 && x1 - x0 <= 300.0 && y1 - y0 <= 1.5 && rect.x0.is_finite()).then_some(rect)
}

impl pdf_extract::OutputDev for PageText {
    fn begin_page(
        &mut self,
        _page: u32,
        media_box: &pdf_extract::MediaBox,
        _art_box: Option<(f64, f64, f64, f64)>,
    ) -> Result<(), pdf_extract::OutputError> {
        // PlainTextOutput measures y from the page's top as `ury - lly - y`; from 0 there is `ury - lly` here
        self.top = media_box.ury - media_box.lly;
        Ok(())
    }

    fn end_page(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }

    fn output_character(
        &mut self,
        trm: &pdf_extract::Transform,
        width: f64,
        _spacing: f64,
        font_size: f64,
        char: &str,
    ) -> Result<(), pdf_extract::OutputError> {
        // as PlainTextOutput: the glyph's size is the side of the square with the area of the transformed size
        let along = (font_size * trm.m11 + font_size * trm.m21, font_size * trm.m12 + font_size * trm.m22);
        let size = (along.0 * along.1).sqrt();
        self.glyphs.push(Glyph {
            x: trm.m31,
            y: trm.m32,
            size,
            advance: width * size,
            text: char.to_owned(),
            word_start: self.word_start,
        });
        self.word_start = false;
        Ok(())
    }

    fn begin_word(&mut self) -> Result<(), pdf_extract::OutputError> {
        self.word_start = true;
        Ok(())
    }

    fn end_word(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }

    fn end_line(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }

    fn stroke(
        &mut self,
        ctm: &pdf_extract::Transform,
        _colorspace: &pdf_extract::ColorSpace,
        _color: &[f64],
        path: &pdf_extract::Path,
    ) -> Result<(), pdf_extract::OutputError> {
        self.rules.extend(rule_of(ctm, path));
        Ok(())
    }

    fn fill(
        &mut self,
        ctm: &pdf_extract::Transform,
        _colorspace: &pdf_extract::ColorSpace,
        _color: &[f64],
        path: &pdf_extract::Path,
    ) -> Result<(), pdf_extract::OutputError> {
        self.rules.extend(rule_of(ctm, path));
        Ok(())
    }
}

/// A page's text with what its fonts could not say marked: the pieces of drawn glyphs ([`fonts::DROPPED`]) left
/// out, and each run of glyphs no font could read ([`fonts::UNREADABLE`], with the spaces between them) made one
/// [`UNREADABLE`]. A line break is never inside a run, so the lines stay as they were.
fn unreadable_marked(page: &str) -> String {
    let mut marked = String::with_capacity(page.len());
    let mut pending_spaces = String::new();
    let mut in_run = false;
    for c in page.chars() {
        match c {
            fonts::DROPPED => {}
            fonts::UNREADABLE => {
                if !in_run {
                    marked.push_str(&pending_spaces);
                    marked.push(UNREADABLE);
                }
                pending_spaces.clear();
                in_run = true;
            }
            ' ' | '\t' if in_run => pending_spaces.push(c),
            _ => {
                marked.push_str(&pending_spaces);
                pending_spaces.clear();
                in_run = false;
                marked.push(c);
            }
        }
    }
    marked.push_str(&pending_spaces);
    marked
}

/// What a child says on its standard output.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ChildAnswer {
    Read(Reading),
    Unreadable(Unreadable),
}

/// The child's side: read one PDF from standard input, say its pages on standard output, stop. `main` calls this
/// when started with [`CHILD_ARGUMENT`], after capping the heap at [`CHILD_HEAP`].
pub fn serve_one_reading() -> ExitCode {
    let mut bytes = Vec::new();
    // one byte more than allowed, to tell "too large" from "exactly the largest"
    let limit = u64::try_from(MAX_PDF_BYTES).unwrap_or(u64::MAX).saturating_add(1);
    if io::stdin().lock().take(limit).read_to_end(&mut bytes).is_err() {
        return ExitCode::from(CHILD_TROUBLE);
    }
    // on a thread of its own, for a stack of a known size whatever the system gives the main thread
    let reading = std::thread::Builder::new().stack_size(CHILD_STACK).spawn(move || read_in_this_process(&bytes));
    let answer = match reading.map(std::thread::JoinHandle::join) {
        Ok(Ok(Ok(reading))) => ChildAnswer::Read(reading),
        Ok(Ok(Err(why))) => ChildAnswer::Unreadable(why),
        Ok(Err(_)) => ChildAnswer::Unreadable(Unreadable::Unhandled),
        Err(_) => return ExitCode::from(CHILD_TROUBLE),
    };
    let mut out = io::stdout().lock();
    match serde_json::to_writer(&mut out, &answer).map_err(io::Error::from).and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(CHILD_TROUBLE),
    }
}

/// A child that is killed and waited for whatever happens to its reader, so none is ever left behind.
struct Reaped(std::process::Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

/// At most `most` bytes of `from`, and whether there was more (which is read and dropped, so that the writer is
/// never left blocked on a full pipe).
fn read_at_most(from: Option<impl Read>, most: usize) -> (Vec<u8>, bool) {
    let Some(mut from) = from else { return (Vec::new(), false) };
    let mut kept = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut over = false;
    loop {
        match from.read(&mut chunk) {
            Ok(0) | Err(_) => return (kept, over),
            Ok(read) if !over && kept.len() + read <= most => kept.extend_from_slice(&chunk[..read]),
            Ok(_) => over = true,
        }
    }
}

/// The child's exit, or `None` when it was still reading at `deadline` (and has been killed).
fn wait_until(child: &mut Reaped, deadline: Instant) -> Option<ExitStatus> {
    let mut pause = Duration::from_millis(1);
    loop {
        match child.0.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.0.kill();
                let _ = child.0.wait();
                return None;
            }
            Ok(None) => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(Duration::from_millis(20));
            }
            Err(_) => return child.0.wait().ok(),
        }
    }
}

/// How many PDF readers may run at once: one per computing thread (`cores`), and no more than `available` bytes
/// of free memory hold at `cap` bytes each (the most one child may allocate); at least one, so that a PDF is
/// always read, one at a time on a computer short of memory.
pub fn readers_at_once(cores: usize, available: u64, cap: u64) -> usize {
    let by_memory = usize::try_from(available / cap.max(1)).unwrap_or(usize::MAX);
    cores.min(by_memory).max(1)
}

/// The memory this computer has free now, in bytes; None when the system does not say.
fn available_memory() -> Option<u64> {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    Some(system.available_memory()).filter(|bytes| *bytes > 0)
}

/// Places for children: at most [`readers_at_once`] of them read at once, whoever asks.
struct Places {
    most: usize,
    taken: Mutex<usize>,
    freed: Condvar,
}

/// A child's place, given back when dropped (on a panic too).
struct Place<'a>(&'a Places);

impl Places {
    fn take(&self) -> Place<'_> {
        let mut taken = self.taken.lock().unwrap_or_else(PoisonError::into_inner);
        while *taken >= self.most {
            taken = self.freed.wait(taken).unwrap_or_else(PoisonError::into_inner);
        }
        *taken += 1;
        Place(self)
    }
}

impl Drop for Place<'_> {
    fn drop(&mut self) {
        let mut taken = self.0.taken.lock().unwrap_or_else(PoisonError::into_inner);
        *taken = taken.saturating_sub(1);
        drop(taken);
        self.0.freed.notify_one();
    }
}

/// The engine's places, counted once, when the first PDF is read in a child: by the computing threads and the
/// memory free then. A system that does not say its free memory is taken to have one cap's worth per thread.
static PLACES: LazyLock<Places> = LazyLock::new(|| {
    let cores = threads::computing();
    let cap = u64::try_from(CHILD_HEAP).unwrap_or(u64::MAX);
    let available = available_memory().unwrap_or_else(|| cap.saturating_mul(cores as u64));
    Places { most: readers_at_once(cores, available, cap), taken: Mutex::new(0), freed: Condvar::new() }
});

/// Read `bytes` in a child process of `program` within `time`: the pages, or why not. Waits first for a place
/// among the children ([`readers_at_once`]); `time` runs from when the child starts.
pub fn read_in_child(program: &Path, bytes: &[u8], time: Duration) -> Result<Reading, Unreadable> {
    let _place = PLACES.take();
    let mut command = crate::api::guard::config::child_process(program);
    command.arg(CHILD_ARGUMENT).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // a group of its own: Ctrl-C in the engine's terminal stops the engine, which stops its children, rather than
    // reaching them first and being taken for something the PDF did
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000; // no console window flashing up for each PDF
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let deadline = Instant::now() + time;
    let mut child = Reaped(command.spawn().map_err(|error| Unreadable::ReaderUnavailable(error.to_string()))?);
    let (input, output, errors) = (child.0.stdin.take(), child.0.stdout.take(), child.0.stderr.take());
    let (status, (answer, too_long), (said, _)) = std::thread::scope(|scope| {
        scope.spawn(move || {
            // a child that stopped early closes its end: the rest is not needed
            if let Some(mut input) = input {
                let _ = input.write_all(bytes);
            }
        });
        let answer = scope.spawn(move || read_at_most(output, LONGEST_ANSWER));
        let said = scope.spawn(move || read_at_most(errors, LONGEST_ERROR));
        let status = wait_until(&mut child, deadline);
        (status, answer.join().unwrap_or_default(), said.join().unwrap_or_default())
    });
    let Some(status) = status else { return Err(Unreadable::TooSlow(time.as_secs())) };
    if too_long {
        return Err(Unreadable::TooMuchText);
    }
    if !status.success() {
        return Err(why_it_stopped(status, &String::from_utf8_lossy(&said)));
    }
    match serde_json::from_slice(&answer) {
        Ok(ChildAnswer::Read(reading)) => Ok(reading),
        Ok(ChildAnswer::Unreadable(why)) => Err(why),
        Err(_) => Err(Unreadable::Crashed("its answer could not be read".to_owned())),
    }
}

/// What a child that did not finish died of, from what it said last. Rust's runtime writes these words itself.
fn why_it_stopped(status: ExitStatus, said: &str) -> Unreadable {
    if said.contains("memory allocation of") {
        Unreadable::TooMuchMemory(CHILD_HEAP / (1024 * 1024))
    } else if said.contains("overflowed its stack") {
        Unreadable::Crashed("objects nested too deeply".to_owned())
    } else {
        match status.code() {
            Some(code) if code == i32::from(CHILD_TROUBLE) => {
                Unreadable::ReaderUnavailable("the reader's own input or output failed".to_owned())
            }
            Some(code) => Unreadable::Crashed(format!("exit code {code}")),
            // stopped from outside (a signal the engine did not send: its own kill is the time limit, told
            // apart before this): nothing is known of the PDF
            None => Unreadable::ReaderUnavailable("the reader was stopped from outside".to_owned()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A one-page PDF that writes `lines` in Helvetica, one below the other, made here so the test needs no file.
    fn pdf_with(lines: &[&str]) -> Vec<u8> {
        crate::library::papers::passages::fixtures::pdf_with(lines)
    }

    #[test]
    fn no_more_readers_run_at_once_than_the_free_memory_holds_at_their_cap_nor_than_the_threads() {
        const GIB: u64 = 1 << 30;
        assert_eq!(readers_at_once(16, 64 * GIB, GIB), 16, "plenty of memory: one per thread");
        assert_eq!(readers_at_once(16, 6 * GIB + GIB / 2, GIB), 6, "six caps' worth free: six");
        assert_eq!(readers_at_once(4, GIB / 2, GIB), 1, "less than one cap free: still one, one at a time");
        assert_eq!(readers_at_once(0, 64 * GIB, GIB), 1);
        assert_eq!(readers_at_once(8, u64::MAX, 0), 8, "a cap of nothing bounds nothing");
        assert!(available_memory().is_some_and(|bytes| bytes > 0), "this system says what it has free");
    }

    #[test]
    fn a_place_is_given_back_when_its_reading_ends_and_whoever_waits_takes_it() {
        let places = Places { most: 2, taken: Mutex::new(0), freed: Condvar::new() };
        let most = Mutex::new(0usize);
        let now = Mutex::new(0usize);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    let _place = places.take();
                    let at_once = {
                        let mut now = now.lock().unwrap();
                        *now += 1;
                        *now
                    };
                    let mut seen = most.lock().unwrap();
                    *seen = (*seen).max(at_once);
                    drop(seen);
                    std::thread::sleep(Duration::from_millis(10));
                    *now.lock().unwrap() -= 1;
                });
            }
        });
        assert_eq!(*most.lock().unwrap(), 2);
        assert_eq!(*places.taken.lock().unwrap(), 0, "every place given back");
    }

    #[test]
    fn the_text_of_a_pdf_comes_out_line_by_line_in_order() {
        let text =
            pdf_text(&pdf_with(&["References", "Vaswani, A. Attention is all you need. In NeurIPS, 2017."])).unwrap();
        let lines: Vec<&str> = text.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, ["References", "Vaswani, A. Attention is all you need. In NeurIPS, 2017."]);
    }

    #[test]
    fn the_limits_of_a_displayed_sum_stay_with_it_and_headings_and_paragraphs_stay_apart() {
        use fonts::fixtures::{Font, pdf_with_content};
        let helvetica =
            Font { dictionary: "/Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding", streams: &[] };
        // a heading at 12 points, a paragraph at 10 a blank line below, then "R = sum_{k=1}^{K} r_k" set as TeX sets
        // a display: the upper limit at 7 points above the sum, the lower limit at 7 points below it
        let content = "BT /F0 12 Tf 72 740 Td (1. Analysis) Tj ET\n\
                       BT /F0 10 Tf 72 722 Td (We bound the regret by a sum.) Tj ET\n\
                       BT /F0 10 Tf 72 690 Td (R =) Tj ET\n\
                       BT /F0 7 Tf 96 700 Td (K) Tj ET\n\
                       BT /F0 10 Tf 94 690 Td (S) Tj ET\n\
                       BT /F0 7 Tf 92 680 Td (k=1) Tj ET\n\
                       BT /F0 10 Tf 108 690 Td (r) Tj ET\n\
                       BT /F0 10 Tf 72 660 Td (which is what we wanted.) Tj ET";
        let pages = read_in_this_process(&pdf_with_content(&[helvetica], content)).unwrap().pages;
        let blocks: Vec<Vec<&str>> = pages[0]
            .split("\n\n")
            .map(|block| block.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>())
            .filter(|block| !block.is_empty())
            .collect();
        assert_eq!(blocks[0], ["1. Analysis"], "{pages:?}");
        assert_eq!(blocks[1], ["We bound the regret by a sum."]);
        assert_eq!(blocks[2].concat().replace(' ', ""), "R=KSk=1r", "the formula is one block: {blocks:?}");
        assert_eq!(blocks[3], ["which is what we wanted."]);
    }

    /// The text of a one-page PDF drawn in Helvetica by `content`.
    fn drawn(content: &str) -> String {
        use fonts::fixtures::{Font, pdf_with_content};
        let helvetica =
            Font { dictionary: "/Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding", streams: &[] };
        read_in_this_process(&pdf_with_content(&[helvetica], content)).unwrap().pages.remove(0)
    }

    #[test]
    fn scripts_above_and_below_the_baseline_are_marked_and_a_footnotes_number_is_not() {
        let text = drawn(
            "BT /F0 10 Tf 72 700 Td (CoT) Tj ET\n\
             BT /F0 7 Tf 92 704.8 Td (gt) Tj ET\n\
             BT /F0 7 Tf 92 697 Td (d) Tj ET\n\
             BT /F0 10 Tf 104 700 Td (and R) Tj ET\n\
             BT /F0 7 Tf 131 704.5 Td (m) Tj ET\n\
             BT /F0 10 Tf 72 680 Td (models.) Tj ET\n\
             BT /F0 7 Tf 107.5 684 Td (12) Tj ET",
        );
        let lines: Vec<&str> = text.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, ["CoT^{gt}_{d} and R^{m}", "models.12"], "{text:?}");
    }

    #[test]
    fn a_fraction_over_a_rule_is_written_as_one_and_keeps_its_formula_in_one_block() {
        // "v := 1/n S x", the fraction's bar a stroked line; the sum sign set higher than the line, as a large
        // operator is: from the denominator to it is further than a blank line's worth
        let text = drawn(
            "BT /F0 10 Tf 72 700 Td (v :=) Tj ET\n\
             BT /F0 10 Tf 102 708 Td (1) Tj ET\n\
             0.4 w 100 705 m 110 705 l S\n\
             BT /F0 10 Tf 101.5 692 Td (n) Tj ET\n\
             BT /F0 10 Tf 114 710 Td (S) Tj ET\n\
             BT /F0 10 Tf 126 700 Td (x) Tj ET",
        );
        let blocks: Vec<String> = text
            .split("\n\n")
            .map(|block| block.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|block| !block.is_empty())
            .collect();
        assert_eq!(blocks.len(), 1, "one displayed formula, one block: {text:?}");
        assert!(blocks[0].contains("(1)/(n)"), "{blocks:?}");
        assert!(blocks[0].starts_with("v :="), "the left side kept with the right: {blocks:?}");
    }

    #[test]
    fn a_table_s_rows_over_and_under_its_rule_are_not_a_fraction() {
        let text = drawn(
            "BT /F0 10 Tf 72 708 Td (Model) Tj ET\n\
             BT /F0 10 Tf 150 708 Td (Score) Tj ET\n\
             0.4 w 72 705 m 200 705 l S\n\
             BT /F0 10 Tf 72 694 Td (Ours) Tj ET\n\
             BT /F0 10 Tf 150 694 Td (71.2) Tj ET",
        );
        assert!(!text.contains(")/("), "{text:?}");
        // the short rule over a page's footnotes, with a line of prose just above it that runs on past its end
        let text = drawn(
            "BT /F0 10 Tf 72 112 Td (we read the page as it is drawn and nothing else) Tj ET\n\
             0.4 w 72 104 m 140 104 l S\n\
             BT /F0 8 Tf 72 95 Td (1Seoul National University) Tj ET",
        );
        assert!(!text.contains(")/("), "{text:?}");
    }

    #[test]
    fn a_pdf_is_read_page_by_page_with_the_same_text() {
        let bytes = pdf_with(&["Introduction", "We study passages."]);
        let pages = pdf_pages(&bytes).unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(
            pages[0].split_whitespace().collect::<Vec<_>>(),
            pdf_text(&bytes).unwrap().split_whitespace().collect::<Vec<_>>()
        );
        assert!(matches!(pdf_pages(b"not a pdf"), Err(Unreadable::Damaged(_) | Unreadable::Unhandled)));
        assert_eq!(pdf_pages(&pdf_with(&[])), Err(Unreadable::NoText));
    }

    #[test]
    fn many_pdfs_are_read_side_by_side_each_to_its_own_answer_the_same_on_one_thread_and_on_many() {
        let pdfs: Vec<Vec<u8>> = (0..40)
            .map(|paper| match paper % 5 {
                0 => b"not a pdf at all".to_vec(),
                1 => pdf_with(&[]),
                _ => pdf_with(&["References", &format!("Author, A. Paper number {paper}. In ICML, 2025.")]),
            })
            .collect();
        let progress = Progress::new();
        let (one, many) = threads::on_one_and_many(|| pdf_texts(&pdfs, &progress));
        assert_eq!(one, many);
        assert_eq!(progress.done(), 80); // both batches counted
        for (paper, outcome) in many.iter().enumerate() {
            match (paper % 5, outcome) {
                (0, Outcome::Done(Err(Unreadable::Damaged(_) | Unreadable::Unhandled))) => {}
                (1, Outcome::Done(Err(Unreadable::NoText))) => {}
                (_, Outcome::Done(Ok(text))) => assert!(text.contains(&format!("Paper number {paper}."))),
                (_, other) => panic!("paper {paper}: {other:?}"),
            }
        }
    }

    #[test]
    fn what_is_not_a_readable_pdf_is_refused_with_a_reason() {
        assert!(matches!(pdf_text(b"<html>not a paper</html>"), Err(Unreadable::Damaged(_) | Unreadable::Unhandled)));
        assert_eq!(pdf_text(&pdf_with(&[])), Err(Unreadable::NoText));
        let huge = vec![b' '; MAX_PDF_BYTES + 1];
        assert_eq!(pdf_pages(&huge), Err(Unreadable::TooLarge(MAX_PDF_BYTES + 1)));
    }

    #[test]
    fn only_a_reader_that_could_not_be_started_is_not_a_lasting_reason() {
        for lasting in [
            Unreadable::TooSlow(60),
            Unreadable::TooMuchMemory(1024),
            Unreadable::Crashed("exit code 3".into()),
            Unreadable::TooMuchText,
            Unreadable::NoText,
        ] {
            assert!(lasting.is_lasting(), "{lasting:?}");
        }
        assert!(!Unreadable::ReaderUnavailable("no such file".into()).is_lasting());
    }

    #[test]
    fn a_program_that_cannot_be_started_is_told_as_such() {
        let missing = Path::new("this program does not exist anywhere");
        let read = read_in_child(missing, b"%PDF-1.4", Duration::from_secs(5));
        assert!(matches!(read, Err(Unreadable::ReaderUnavailable(_))), "{read:?}");
    }

    #[test]
    fn a_childs_death_is_told_by_what_it_said_last() {
        let status = crate::api::guard::config::child_process(std::env::current_exe().unwrap())
            .arg("--this-flag-is-not-one-the-test-harness-takes")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success());
        assert_eq!(
            why_it_stopped(status, "memory allocation of 1073741824 bytes failed"),
            Unreadable::TooMuchMemory(1024)
        );
        assert_eq!(
            why_it_stopped(status, "thread 'x' has overflowed its stack\nfatal runtime error: stack overflow"),
            Unreadable::Crashed("objects nested too deeply".into())
        );
        assert!(matches!(why_it_stopped(status, ""), Unreadable::Crashed(_)));
    }

    #[test]
    fn the_reasons_survive_the_trip_from_the_child_as_json() {
        for answer in [
            ChildAnswer::Read(Reading {
                pages: vec!["one".into(), String::new()],
                figures: figures::Figures::default(),
            }),
            ChildAnswer::Unreadable(Unreadable::Damaged("bad xref".into())),
        ] {
            let told = serde_json::to_string(&answer).unwrap();
            let back: ChildAnswer = serde_json::from_str(&told).unwrap();
            assert_eq!(format!("{back:?}"), format!("{answer:?}"));
        }
    }

    #[test]
    fn what_a_child_says_is_kept_up_to_a_limit_and_the_rest_is_drained() {
        let (kept, over) = read_at_most(Some(&b"0123456789"[..]), 4);
        assert_eq!((kept.as_slice(), over), (&b""[..], true), "the first chunk alone is past the limit");
        let (kept, over) = read_at_most(Some(&b"0123"[..]), 4);
        assert_eq!((kept.as_slice(), over), (&b"0123"[..], false));
        assert_eq!(read_at_most(None::<&[u8]>, 4), (Vec::new(), false));
    }
}
