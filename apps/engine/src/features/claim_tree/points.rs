//! A review's sections cut into the points a reviewer makes, each with its place in the kept page's text, so that
//! a point mapped onto a claim can always be shown in the reviewer's own words.
//!
//! Only the sections that say what a reviewer thinks of the paper's claims are cut: strengths, weaknesses,
//! questions, limitations, and the single "Review" field of older forms (and ICML's claims, methods and
//! experiments fields). A summary, the scores and the forms' checkboxes are not points.
//!
//! How a section is cut, best first: at its list's bullets or numbers when it has a list; else at its blank
//! lines when it has several paragraphs; else at its sentences. Inside a joint "Strengths And Weaknesses" field
//! the reviewer's own "Strengths:" and "Weaknesses:" (or "Pros:" and "Cons:") say which kind the points after
//! them are. A point is exactly the page's text at its span, trimmed.
//!
//! This is this feature's own splitter; the per-paper weaknesses list (`Feature::Critique`) has another, and
//! the two may be merged later.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::text::discussion::{Discussion, Kind, Note, Section, Span};
use crate::text::passages::{letters_in, sentence_spans};

/// Points kept of one paper's reviews: more is not one paper's discussion, and would not fit one question.
pub const MAX_POINTS: usize = 150;
/// Characters of points kept of one paper's reviews, all points together.
pub const MAX_POINTS_CHARS: usize = 60_000;
/// A point with fewer letters and digits than this ("None.", "Yes.") says nothing to map.
const LEAST_LETTERS: usize = 12;

/// What a reviewer's point is, by the section (or the reviewer's own heading) it is under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PointKind {
    Strength,
    Weakness,
    Question,
    Limitation,
    /// From a field that is not one of those (an older form's single "Review", ICML's "Claims And Evidence").
    General,
}

/// One point of one review.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Point {
    /// "p1", "p2", ... in the page's order: what the model is told to name it by.
    pub id: String,
    /// The review's index among the discussion's notes.
    pub review: usize,
    /// "Reviewer aBcD", as the page signs it.
    pub reviewer: String,
    /// The section it is in, as the venue's form names it.
    pub section: String,
    pub kind: PointKind,
    /// Exactly the kept page's text at `span`.
    pub text: String,
    /// In characters of the kept page's text.
    pub span: Span,
}

/// The points of a paper's reviews, and how many were left out by the bounds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ReviewPoints {
    pub points: Vec<Point>,
    /// Points past [`MAX_POINTS`] or [`MAX_POINTS_CHARS`]: not mapped, and said to be left out.
    pub left_out: usize,
}

/// What a section of a review is, for cutting it into points: `None` when it holds no points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Holds {
    One(PointKind),
    /// "Strengths And Weaknesses": the reviewer's own headings inside say which.
    Both,
}

fn holds(name: &str) -> Option<Holds> {
    let name = name.to_lowercase();
    let has = |word: &str| name.split_whitespace().any(|part| part.starts_with(word));
    if has("strength") && has("weakness") {
        return Some(Holds::Both);
    }
    let kind = if has("strength") {
        PointKind::Strength
    } else if has("weakness") || name == "concerns" {
        PointKind::Weakness
    } else if has("question") {
        PointKind::Question
    } else if has("limitation") {
        PointKind::Limitation
    } else if matches!(
        name.as_str(),
        "review"
            | "main review"
            | "claims and evidence"
            | "theoretical claims"
            | "methods and evaluation criteria"
            | "experimental designs or analyses"
            | "essential references not discussed"
            | "other comments or suggestions"
    ) {
        PointKind::General
    } else {
        return None;
    };
    Some(Holds::One(kind))
}

/// A reviewer's own heading inside a joint field: "Strengths:", "Weaknesses", "- Cons:".
#[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
static INNER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)^[^\S\n]*[-*#]*[^\S\n]*(strengths?|pros|weaknesses?|cons|concerns|questions?|limitations?)[^\S\n]*:?[^\S\n]*")
        .unwrap()
});
/// A line that starts an item of a list: "- ", "* ", "• ", "1. ", "2) ", "(3) ", "(a) ".
#[allow(clippy::unwrap_used)]
static BULLET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[^\S\n]*(?:[-*•–·]|\(?\d{1,2}[.)]|\([a-z]\))[^\S\n]+\S").unwrap());

fn inner_kind(word: &str) -> PointKind {
    let word = word.to_lowercase();
    if word.starts_with("strength") || word == "pros" {
        PointKind::Strength
    } else if word.starts_with("question") {
        PointKind::Question
    } else if word.starts_with("limitation") {
        PointKind::Limitation
    } else {
        PointKind::Weakness
    }
}

/// Byte ranges of `text` that are a reviewer's own heading inside a joint field, with the kind they start.
/// Only a heading at the start of a line that ends the line or is followed by a colon counts: "Strengths of the
/// method are..." is prose.
fn inner_headings(text: &str) -> Vec<(usize, usize, PointKind)> {
    INNER
        .captures_iter(text)
        .filter_map(|found| {
            let whole = found.get(0)?;
            let word = found.get(1)?;
            let after_word = &text[word.end()..whole.end()];
            let rest_of_line = text[whole.end()..].split('\n').next().unwrap_or_default();
            let heading = after_word.contains(':') || rest_of_line.trim().is_empty();
            heading.then(|| (whole.start(), whole.end(), inner_kind(word.as_str())))
        })
        .collect()
}

/// Byte ranges of the points of one stretch of a section, before trimming.
fn cut(text: &str) -> Vec<(usize, usize)> {
    let bullets: Vec<usize> = BULLET.find_iter(text).map(|found| found.start()).collect();
    if !bullets.is_empty() {
        let mut starts = Vec::with_capacity(bullets.len() + 1);
        if !text[..bullets[0]].trim().is_empty() {
            starts.push(0);
        }
        starts.extend(bullets);
        return starts
            .iter()
            .enumerate()
            .map(|(at, start)| (*start, starts.get(at + 1).copied().unwrap_or(text.len())))
            .collect();
    }
    let mut paragraphs = Vec::new();
    let mut start = 0;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        if line.trim().is_empty() {
            if !text[start..at].trim().is_empty() {
                paragraphs.push((start, at));
            }
            start = at + line.len();
        }
        at += line.len();
    }
    if !text[start..].trim().is_empty() {
        paragraphs.push((start, text.len()));
    }
    if paragraphs.len() > 1 {
        return paragraphs;
    }
    // one paragraph: its sentences, which sentence_spans gives in characters
    let byte_of: Vec<usize> = text.char_indices().map(|(byte, _)| byte).chain(std::iter::once(text.len())).collect();
    sentence_spans(text).into_iter().map(|(start, end)| (byte_of[start], byte_of[end.min(byte_of.len() - 1)])).collect()
}

/// `start..end` (bytes of `section.text`) trimmed, as a span of the page in characters; `None` if nothing is left.
fn trimmed(section: &Section, start: usize, end: usize) -> Option<(String, Span)> {
    let piece = &section.text[start..end];
    let lead = piece.len() - piece.trim_start().len();
    let text = piece.trim();
    if text.is_empty() {
        return None;
    }
    let chars = |bytes: usize| section.text[..bytes].chars().count();
    let from = start + lead;
    let span = Span { start: section.span.start + chars(from), end: section.span.start + chars(from + text.len()) };
    Some((text.to_owned(), span))
}

fn section_points(section: &Section, holds: Holds) -> Vec<(PointKind, String, Span)> {
    // stretches of the section, each of one kind
    let stretches: Vec<(usize, usize, PointKind)> = match holds {
        Holds::One(kind) => vec![(0, section.text.len(), kind)],
        Holds::Both => {
            let headings = inner_headings(&section.text);
            let mut stretches = Vec::new();
            let first = headings.first().map_or(section.text.len(), |(start, _, _)| *start);
            stretches.push((0, first, PointKind::General));
            for (at, (_, end, kind)) in headings.iter().enumerate() {
                let next = headings.get(at + 1).map_or(section.text.len(), |(start, _, _)| *start);
                stretches.push((*end, next.max(*end), *kind));
            }
            stretches
        }
    };
    stretches
        .into_iter()
        .flat_map(|(from, to, kind)| {
            cut(&section.text[from..to]).into_iter().filter_map(move |(start, end)| {
                trimmed(section, from + start, from + end).map(|(text, span)| (kind, text, span))
            })
        })
        .filter(|(_, text, _)| letters_in(text) >= LEAST_LETTERS)
        .collect()
}

fn reviewer_of(note: &Note, review_number: usize) -> String {
    match note.reviewer() {
        Some(id) if id.starts_with("AnonReviewer") => id.to_owned(),
        Some(id) => format!("Reviewer {id}"),
        None => format!("Review {review_number}"),
    }
}

/// What reviewers are called in what this feature shows: "Reviewer aBcD", by the review's index among the notes.
pub fn reviewer_names(discussion: &Discussion) -> Vec<(usize, String)> {
    discussion.reviews().enumerate().map(|(number, (index, note))| (index, reviewer_of(note, number + 1))).collect()
}

/// The points of every official review of `discussion`, in the page's order, within the bounds.
pub fn review_points(discussion: &Discussion) -> ReviewPoints {
    let names = reviewer_names(discussion);
    let mut points = Vec::new();
    let mut left_out = 0;
    let mut chars = 0;
    for (index, note) in discussion.notes.iter().enumerate() {
        if !matches!(note.kind, Kind::OfficialReview { .. }) {
            continue;
        }
        let reviewer = names.iter().find(|(at, _)| *at == index).map_or_else(String::new, |(_, name)| name.clone());
        for section in &note.sections {
            let Some(holds) = holds(&section.name) else { continue };
            for (kind, text, span) in section_points(section, holds) {
                let length = text.chars().count();
                if points.len() >= MAX_POINTS || chars + length > MAX_POINTS_CHARS {
                    left_out += 1;
                    continue;
                }
                chars += length;
                points.push(Point {
                    id: format!("p{}", points.len() + 1),
                    review: index,
                    reviewer: reviewer.clone(),
                    section: section.name.clone(),
                    kind,
                    text,
                    span,
                });
            }
        }
    }
    ReviewPoints { points, left_out }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::text::discussion::fixtures::{ICLR_2021, ICML_2025, NEURIPS_2023, NEURIPS_2025};
    use crate::text::discussion::read_discussion;

    fn at(page: &str, span: Span) -> String {
        page.chars().skip(span.start).take(span.end - span.start).collect()
    }

    fn points(page: &str) -> Vec<Point> {
        review_points(&read_discussion(page).unwrap()).points
    }

    #[test]
    fn every_point_is_exactly_the_pages_words_at_its_span() {
        for page in [NEURIPS_2023, NEURIPS_2025, ICLR_2021, ICML_2025] {
            let found = points(page);
            assert!(!found.is_empty());
            for point in &found {
                assert_eq!(at(page, point.span), point.text, "{}", point.id);
            }
        }
    }

    #[test]
    fn a_paragraph_is_cut_at_its_sentences_and_sections_say_what_kind_a_point_is() {
        let found = points(NEURIPS_2023);
        let first: Vec<(&str, PointKind)> =
            found.iter().filter(|point| point.review == 0).map(|point| (point.text.as_str(), point.kind)).collect();
        assert_eq!(
            first,
            [
                ("The routing rule is simple and novel.", PointKind::Strength),
                ("The theoretical analysis is clear and convincing.", PointKind::Strength),
                ("The experiments are limited to two small benchmarks.", PointKind::Weakness),
                ("The baselines are weak, and an ablation study is missing.", PointKind::Weakness),
                ("How does the rule behave with sixty-four experts?", PointKind::Question),
                ("Discussed adequately.", PointKind::Limitation),
            ]
        );
        assert_eq!(found[0].reviewer, "Reviewer aBcD");
        assert_eq!(found.iter().map(|point| point.id.as_str()).take(2).collect::<Vec<_>>(), ["p1", "p2"]);
        // "Yes." says nothing: not a point
        assert!(found.iter().all(|point| point.text != "Yes."));
    }

    #[test]
    fn a_joint_field_is_cut_at_its_bullets_under_the_reviewers_own_headings() {
        let found = points(NEURIPS_2025);
        let first: Vec<(&str, PointKind)> =
            found.iter().filter(|point| point.review == 1).map(|point| (point.text.as_str(), point.kind)).collect();
        assert_eq!(
            first,
            [
                ("- The benchmark is large and carefully documented.", PointKind::Strength),
                ("- The evaluation protocol is clear.", PointKind::Strength),
                ("- The baselines are weak and outdated.", PointKind::Weakness),
                ("- The evaluation ignores contamination.", PointKind::Weakness),
                ("How was contamination checked?", PointKind::Question),
            ]
        );
        // inline headings: "Strengths: the benchmark is large..." on one line
        let second: Vec<PointKind> = found.iter().filter(|point| point.review == 3).map(|point| point.kind).collect();
        assert_eq!(second, [PointKind::Strength, PointKind::Weakness]);
    }

    #[test]
    fn older_and_longer_forms_give_general_points() {
        let older = points(ICLR_2021);
        assert!(older.iter().all(|point| point.kind == PointKind::General && point.section == "Review"));
        assert_eq!(older[0].reviewer, "AnonReviewer2");
        let icml = points(ICML_2025);
        assert!(icml.iter().any(|point| point.section == "Claims And Evidence"));
        assert!(icml.iter().any(|point| point.section == "Questions For Authors" && point.kind == PointKind::Question));
    }

    #[test]
    fn a_numbered_list_with_an_introduction_is_cut_at_its_numbers() {
        let page = "Official Review of Submission1 by Reviewer Ab12\nWeaknesses:\nA few concerns about the evaluation:\n1. The baselines are weak and old.\n   They predate the method by years.\n2) The ablation of the router is missing.\nRating: 4: weak reject\n";
        let found = points(page);
        let texts: Vec<&str> = found.iter().map(|point| point.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "A few concerns about the evaluation:",
                "1. The baselines are weak and old.\n   They predate the method by years.",
                "2) The ablation of the router is missing."
            ]
        );
        for point in &found {
            assert_eq!(at(page, point.span), point.text);
        }
    }

    #[test]
    fn points_past_the_bounds_are_counted_not_kept() {
        let mut page = String::from("Official Review of Submission1 by Reviewer Ab12\nWeaknesses:\n");
        for n in 0..(MAX_POINTS + 7) {
            page.push_str(&format!("- Weakness number {n} of this paper.\n"));
        }
        let read = review_points(&read_discussion(&page).unwrap());
        assert_eq!((read.points.len(), read.left_out), (MAX_POINTS, 7));
    }
}
