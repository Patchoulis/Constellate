//! How well passages are read from real PDFs, on the PDFs given as arguments:
//!
//! ```text
//! cargo run --release --example passages_eval -- path/to/a.pdf path/to/b.pdf ...
//! ```
//!
//! For each paper: what share of its text lands in passages (in letters and digits, against everything the PDF
//! reader gave), how much is the reference list and how much was dropped as running headers and page numbers;
//! then how often quote location finds sentences sampled from the PDF's own raw text (with its line breaks and
//! hyphenation), the same sentences with two letters changed, and sentences of another paper (which must not be
//! found). Nothing of the papers is printed but counts, unless `SHOW_MISSED=1` is set: then the sampled sentences
//! that were not found are printed too, to look into why on one's own machine.

use std::time::Instant;

use constellate::text::passages::{Kind, Match, Passages, citing_sentences, letters_in, read_passages, sentence_spans};
use constellate::text::pdf::pdf_pages;
use constellate::text::references::read_reference_list;

/// Sentences of the raw text before its reference list: what a model reading the PDF's text might quote. Each
/// page is taken by itself and its first and last sentence left out, since those run into the page's running
/// header and number, which no one would quote.
fn raw_sentences(pages: &[String]) -> Vec<String> {
    let mut sentences = Vec::new();
    for page in pages {
        if page.lines().any(|line| line.trim() == "References") {
            break;
        }
        let squashed = page.split_whitespace().collect::<Vec<_>>().join(" ");
        let chars: Vec<char> = squashed.chars().collect();
        let spans = sentence_spans(&squashed);
        let inner = spans.len().saturating_sub(1);
        sentences.extend(
            spans
                .into_iter()
                .take(inner)
                .skip(1)
                .map(|(from, to)| chars[from..to].iter().collect::<String>())
                .filter(|sentence| sentence.split_whitespace().count() >= 8 && letters_in(sentence) >= 40),
        );
    }
    sentences
}

/// Every `step`-th of them, so the sample spreads over the paper.
fn sample(sentences: &[String], step: usize) -> Vec<&String> {
    sentences.iter().step_by(step.max(1)).collect()
}

/// The sentence with two letters changed, at places fixed by its length (no randomness: the same every run).
fn changed(sentence: &str) -> String {
    let mut chars: Vec<char> = sentence.chars().collect();
    let letters: Vec<usize> = (0..chars.len()).filter(|&at| chars[at].is_ascii_lowercase()).collect();
    for pick in [letters.len() / 3, 2 * letters.len() / 3] {
        if let Some(&at) = letters.get(pick) {
            chars[at] = if chars[at] == 'x' { 'q' } else { 'x' };
        }
    }
    chars.into_iter().collect()
}

#[derive(Default)]
struct Found {
    exact: usize,
    close: usize,
    missed: usize,
}

fn tally(passages: &Passages, quotes: &[&str]) -> Found {
    let mut found = Found::default();
    for located in passages.locate_all(quotes) {
        match located.map(|located| located.matched) {
            Some(Match::Exact) => found.exact += 1,
            Some(Match::Close { .. }) => found.close += 1,
            None => found.missed += 1,
        }
    }
    found
}

/// Sentences of the next paper, which this one should not contain.
fn foreign_sample(papers: &[(String, Vec<String>)], index: usize) -> Vec<&'static str> {
    if papers.len() < 2 {
        return Vec::new();
    }
    let foreign = raw_sentences(&papers[(index + 1) % papers.len()].1);
    sample(&foreign, foreign.len() / 60)
        .into_iter()
        .map(|sentence| &*Box::leak(sentence.clone().into_boxed_str()))
        .collect()
}

fn percent(part: usize, whole: usize) -> String {
    if whole == 0 { "-".to_owned() } else { format!("{:.1}%", 100.0 * part as f64 / whole as f64) }
}

fn main() {
    let papers: Vec<(String, Vec<String>)> = std::env::args()
        .skip(1)
        .filter_map(|path| {
            let bytes = std::fs::read(&path).ok()?;
            let name = std::path::Path::new(&path).file_stem()?.to_string_lossy().into_owned();
            Some((name, pdf_pages(&bytes).ok()?))
        })
        .collect();
    let read: Vec<Passages> = papers.iter().map(|(_, pages)| read_passages(pages)).collect();
    for (index, ((name, pages), passages)) in papers.iter().zip(&read).enumerate() {
        let started = Instant::now();
        let raw: usize = pages.iter().map(|page| letters_in(page)).sum();
        let (in_passages, in_pages) = passages.letters();
        let by_kind = |kind: Kind| passages.passages.iter().filter(|passage| passage.kind == kind).count();
        println!("{name}: {} pages, {} passages", pages.len(), passages.passages.len());
        println!(
            "  text, kind by kind: {} text, {} headings, {} figure and {} table captions, {} equations, {} footnotes, {} fragments",
            by_kind(Kind::Text),
            by_kind(Kind::Heading),
            by_kind(Kind::FigureCaption),
            by_kind(Kind::TableCaption),
            by_kind(Kind::Equation),
            by_kind(Kind::Footnote),
            by_kind(Kind::Fragment),
        );
        println!(
            "  letters and digits: {} in passages of {} the PDF reader gave ({}); reference list {} ({}); dropped as headers, footers, page numbers {} ({})",
            in_passages,
            raw,
            percent(in_passages, raw),
            in_pages - in_passages,
            percent(in_pages - in_passages, raw),
            raw - in_pages,
            percent(raw - in_pages, raw),
        );
        let sentences = raw_sentences(pages);
        let picked = sample(&sentences, sentences.len() / 60);
        let quotes: Vec<&str> = picked.iter().map(|sentence| sentence.as_str()).collect();
        let found = tally(passages, &quotes);
        if std::env::var_os("SHOW_MISSED").is_some() {
            for (quote, located) in quotes.iter().zip(passages.locate_all(&quotes)) {
                if located.is_none() {
                    println!("    missed: {quote}");
                }
            }
            for (quote, located) in
                foreign_sample(&papers, index).iter().zip(passages.locate_all(&foreign_sample(&papers, index)))
            {
                if let Some(located) = located {
                    println!("    found in the wrong paper: {quote} -> {}", located.wording);
                }
            }
        }
        println!(
            "  {} sentences of the raw text: {} found exactly, {} with differences, {} not found ({} found)",
            quotes.len(),
            found.exact,
            found.close,
            found.missed,
            percent(found.exact + found.close, quotes.len())
        );
        let typos: Vec<String> = picked.iter().map(|sentence| changed(sentence)).collect();
        let typos: Vec<&str> = typos.iter().map(String::as_str).collect();
        let found = tally(passages, &typos);
        println!(
            "  the same with two letters changed: {} exactly, {} with differences, {} not found ({} found)",
            found.exact,
            found.close,
            found.missed,
            percent(found.exact + found.close, typos.len())
        );
        if let Some((other, other_pages)) = papers.get((index + 1) % papers.len()).filter(|_| papers.len() > 1) {
            let foreign = raw_sentences(other_pages);
            let foreign: Vec<&str> = sample(&foreign, foreign.len() / 60).into_iter().map(String::as_str).collect();
            let found = tally(passages, &foreign);
            println!("  {} sentences of {other}: {} found (should be none)", foreign.len(), found.exact + found.close);
        }
        let list = read_reference_list(&pages.join("\n"));
        let cited = citing_sentences(passages, &list);
        println!(
            "  reference list: {} entries ({:?}); {} cited in the text by {} sentences",
            list.references.len(),
            list.citing,
            cited.len(),
            cited.iter().map(|cited| cited.sentences.len()).sum::<usize>()
        );
        println!("  checked in {:.2} s", started.elapsed().as_secs_f64());
    }
}
