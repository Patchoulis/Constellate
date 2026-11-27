//! What each reviewer raised against a paper, point by point: the Weaknesses, Questions and Limitations sections
//! of every review on a reviews page ([`super::discussion`]) cut into the points they make, each with its exact
//! place in the page's text, and the authors' replies attached to each review with how they were attached.
//!
//! Nothing here asks a language model or fetches anything: it is a reading of the page the user opened. It is the
//! unit both the list of a paper's weaknesses (`analysis::critique`) and a mapping of reviews onto a paper's claims
//! start from, so it is kept apart from either.
//!
//! # Which sections
//!
//! A section is read for points when its name says it holds criticism ("Weaknesses", "Concerns", "Reasons To
//! Reject"), questions ("Questions", "Questions For Authors") or limitations ("Limitations"). A section of both
//! praise and criticism ("Strengths And Weaknesses", "Other Strengths And Weaknesses") is cut at the reviewer's own
//! headings inside it ("Weaknesses:", "Cons:"), and only what stands under a heading of criticism is read; one
//! with no such headings cannot be told apart and is counted as not split ([`ReviewPoints::not_split`]), never
//! shown as a list of weaknesses it may not be. Ratings, summaries and strengths are not points.
//!
//! # Points
//!
//! The page's text is what the browser shows (`innerText`), which keeps the reviewer's own markers ("-", "1.",
//! "W2:") in plain-text fields but loses them in rendered lists, where each item is a line of its own; and prose
//! is never wrapped by it, though a reviewer may have wrapped their own. So a new point starts at:
//!
//! - a line that starts with a marker: a bullet (`-`, `*`, `•`, `–`), a number or letter (`1.`, `(2)`, `a)`,
//!   `[3]`) or a label with a number (`W1:`, `Q2.`, `Weakness 3:`); a marker indented deeper than the point it
//!   follows continues that point (a nested list belongs to its item);
//! - a line after a blank line, unless it is indented deeper than the marked point it follows;
//! - a line after one that ends a sentence (`.`, `?`, `!`, `;`, maybe closed by a quote or a bracket), when it
//!   starts as a sentence does (a capital, a digit, a quote mark or a bracket), unless it is indented deeper than
//!   the marked point it follows. A line that ends with a colon does not end a point: what follows says it.
//!
//! A point that only leads into others ("My main concerns are:") or says there is nothing ("None.", "N/A",
//! "Yes.", "Discussed adequately.") is not a point; under Limitations, where the form asks whether the authors
//! addressed them, "No." and "Not discussed." are criticism and stay points. A point's text is exactly the page's characters at its
//! [`Span`], from after its marker to the end of its last line, so whatever shows it can point back at the
//! reviewer's own words.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::discussion::{Basis, Discussion, Kind, Reply, Section, Span};
use crate::text::{CharIndex, pattern};

/// A review's sections, at most, read for points: a form has a dozen.
const MOST_SECTIONS: usize = 40;
/// Points of one section, at most: past this a section is a list of something else.
pub const MOST_POINTS: usize = 60;

/// What a point raises, as the name of the section it is in says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Concern {
    Weakness,
    Question,
    Limitation,
}

impl Concern {
    /// The concern a section's name (or a reviewer's own heading inside one) says its points are; none for a
    /// section of something else, or of praise and criticism together.
    pub fn of(name: &str) -> Option<Self> {
        let name = name.to_lowercase();
        let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|word| !word.is_empty()).collect();
        let has = |start: &str| words.iter().any(|word| word.starts_with(start));
        if has("formatting") || has("ethic") || both(&name) {
            return None;
        }
        if has("weakness") || has("concern") || words.contains(&"cons") || name.contains("reasons to reject") {
            Some(Self::Weakness)
        } else if has("question") {
            Some(Self::Question)
        } else if has("limitation") {
            Some(Self::Limitation)
        } else {
            None
        }
    }
}

/// Is this a section of praise and criticism together?
fn both(name: &str) -> bool {
    let name = name.to_lowercase();
    (name.contains("strength") && name.contains("weakness")) || (name.contains("pros") && name.contains("cons"))
}

/// One point a reviewer made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewPoint {
    /// The review's index among the discussion's notes.
    pub note: usize,
    /// The section's index among the review's sections.
    pub section: usize,
    pub concern: Concern,
    /// Exactly the page's text at `span`.
    pub text: String,
    /// Where it is in the page's text, in characters.
    pub span: Span,
}

/// How a reply is attached to the review it answers, as [`super::discussion::Reply`] says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attachment {
    /// The reply's own title names the review's reviewer.
    Named,
    /// A guess, and what it rests on.
    Guessed { basis: Basis },
}

/// A note by the paper's authors attached to a review.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorReply {
    /// The reply's index among the discussion's notes.
    pub note: usize,
    pub attachment: Attachment,
}

/// One review's points, and what could not be read for points.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewPoints {
    /// The review's index among the discussion's notes.
    pub note: usize,
    /// In the order the review has them.
    pub points: Vec<ReviewPoint>,
    /// Sections of praise and criticism together with no heading of the reviewer's to tell them apart: by
    /// index among the review's sections.
    pub not_split: Vec<usize>,
    /// The authors' replies attached to this review, in the page's order.
    pub replies: Vec<AuthorReply>,
}

/// The points of every official review of `discussion`, in the page's order (reviews with none included).
pub fn review_points(discussion: &Discussion) -> Vec<ReviewPoints> {
    discussion
        .reviews()
        .map(|(note, review)| {
            let mut points = Vec::new();
            let mut not_split = Vec::new();
            for (index, section) in review.sections.iter().enumerate().take(MOST_SECTIONS) {
                if both(&section.name) {
                    match criticism_in(section) {
                        Some(stretches) => {
                            for (from, to) in stretches {
                                points.extend(points_in(note, index, Concern::Weakness, section, from, to));
                            }
                        }
                        None => not_split.push(index),
                    }
                } else if let Some(concern) = Concern::of(&section.name) {
                    points.extend(points_in(note, index, concern, section, 0, section.text.len()));
                }
            }
            ReviewPoints { note, points, not_split, replies: author_replies(discussion, note) }
        })
        .collect()
}

/// The authors' notes attached to the review at `note`, with how each is attached.
pub fn author_replies(discussion: &Discussion, note: usize) -> Vec<AuthorReply> {
    discussion
        .notes
        .iter()
        .enumerate()
        .filter(|(_, reply)| reply.kind == Kind::AuthorResponse)
        .filter_map(|(index, reply)| match reply.reply {
            Reply::Named { note: answered } if answered == note => {
                Some(AuthorReply { note: index, attachment: Attachment::Named })
            }
            Reply::Guessed { note: answered, basis } if answered == note => {
                Some(AuthorReply { note: index, attachment: Attachment::Guessed { basis } })
            }
            _ => None,
        })
        .collect()
}

/// A reviewer's own heading inside a section of praise and criticism: on a line of its own, or inline.
static HEADING: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?im)(?:^[^\S\n]*[-*#]*[^\S\n]*\**(strengths?|pros|weaknesses?|cons|concerns)\**[^\S\n]*:?\**[^\S\n]*$|\b(strengths?|pros|weaknesses?|cons|concerns)[^\S\n]*:)",
    )
});

/// The stretches (byte ranges of the section's text) under the reviewer's headings of criticism; none when the
/// section has no heading of criticism: with only "Strengths:", what follows it may be praise or criticism and
/// cannot be told apart, so the section is counted as not split rather than read as raising nothing.
fn criticism_in(section: &Section) -> Option<Vec<(usize, usize)>> {
    let text = section.text.as_str();
    let marks: Vec<(usize, usize, bool)> = HEADING
        .captures_iter(text)
        .filter_map(|found| {
            let whole = found.get(0)?;
            let word = found.get(1).or(found.get(2))?;
            Some((whole.start(), whole.end(), Concern::of(word.as_str()) == Some(Concern::Weakness)))
        })
        .collect();
    if !marks.iter().any(|(_, _, criticism)| *criticism) {
        return None;
    }
    Some(
        marks
            .iter()
            .enumerate()
            .filter(|(_, (_, _, criticism))| *criticism)
            .map(|(at, (_, end, _))| (*end, marks.get(at + 1).map_or(text.len(), |next| next.0)))
            .collect(),
    )
}

/// The points of `section.text[from..to]`, placed in the page.
fn points_in(
    note: usize,
    index: usize,
    concern: Concern,
    section: &Section,
    from: usize,
    to: usize,
) -> Vec<ReviewPoint> {
    let chars = CharIndex::new(&section.text);
    split_points(&section.text[from..to], concern)
        .into_iter()
        .take(MOST_POINTS)
        .map(|(start, end)| {
            let (start, end) = (from + start, from + end);
            ReviewPoint {
                note,
                section: index,
                concern,
                text: section.text[start..end].to_owned(),
                span: Span { start: section.span.start + chars.at(start), end: section.span.start + chars.at(end) },
            }
        })
        .collect()
}

/// A line that starts with a marker: its indentation, then the marker, then the point's first character.
static MARKER: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"^([^\S\n]*)(?:\*\*)?(?:[-*•·▪◦‣–—+]|\(?\d{1,2}[.):]|\(?[a-hA-H]\)|[a-h]\.|\[\d{1,2}\]|(?:[WQL]|Weakness|Question|Limitation|Concern|Issue|Point)[^\S\n]?#?\d{1,2}[.:)]?)(?:\*\*)?[^\S\n]+\S",
    )
});

/// Words that, alone, say there is nothing to raise.
const NOTHING: &[&str] = &[
    "none",
    "yes",
    "n",
    "a",
    "na",
    "nil",
    "nothing",
    "discussed",
    "adequately",
    "addressed",
    "applicable",
    "see",
    "above",
    "weaknesses",
    "the",
    "ok",
    "fine",
    "major",
    "minor",
    "and",
    "sufficiently",
];

/// Is `text` not a point: a lead-in to others, or a word or three saying there is nothing? Under Limitations,
/// where the form asks whether the authors addressed them, "No." and "Not discussed." are criticism and stay
/// points; "Not applicable." is nothing anywhere.
fn not_a_point(text: &str, concern: Concern) -> bool {
    let words: Vec<String> =
        text.split(|c: char| !c.is_alphanumeric()).filter(|word| !word.is_empty()).map(str::to_lowercase).collect();
    let negative = |word: &str| (word == "no" || word == "not") && concern != Concern::Limitation;
    let nothing = |word: &String| NOTHING.contains(&word.as_str()) || negative(word);
    words.is_empty()
        || text.trim_end().ends_with(':')
        || words == ["not", "applicable"]
        || (words.len() <= 3 && words.iter().all(nothing))
}

/// Does `line` end a sentence, so that the next line may start a point of its own?
fn ends_sentence(line: &str) -> bool {
    line.trim_end().trim_end_matches(['"', '\'', '”', '’', ')', ']', '*']).ends_with(['.', '?', '!', ';'])
}

/// Does `line` start as a sentence does?
fn starts_sentence(line: &str) -> bool {
    line.trim_start().starts_with(|c: char| c.is_uppercase() || c.is_ascii_digit() || "\"'“‘([".contains(c))
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The points of a stretch of a reviewer's text, as byte ranges of `text`: from the first character after a
/// point's marker to the end of its last line, as the module says.
pub fn split_points(text: &str, concern: Concern) -> Vec<(usize, usize)> {
    /// A point being read: where it starts and ends, and the indentation of its marker if it has one.
    struct Open {
        start: usize,
        end: usize,
        marked: Option<usize>,
    }
    let mut points: Vec<(usize, usize)> = Vec::new();
    let mut open: Option<Open> = None;
    let mut blank_before = false;
    let mut previous_line = "";
    let mut at = 0;
    for raw in text.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\n', '\r']);
        let line_start = at;
        at += raw.len();
        if line.trim().is_empty() {
            blank_before = true;
            continue;
        }
        let content_start = line_start + indent(line);
        let content_end = line_start + line.trim_end().len();
        let marker = MARKER.captures(line);
        let deeper = |open: &Option<Open>| {
            open.as_ref().and_then(|open| open.marked).is_some_and(|marked| indent(line) > marked)
        };
        let starts = match (&marker, &open) {
            (_, None) => true,
            (Some(_), _) => !deeper(&open),
            (None, Some(_)) if deeper(&open) => false,
            (None, Some(_)) => blank_before || (ends_sentence(previous_line) && starts_sentence(line)),
        };
        if starts {
            if let Some(done) = open.take() {
                points.push((done.start, done.end));
            }
            let (start, marked) = match marker.as_ref().and_then(|found| found.get(0)) {
                // the point's words start at the last character the marker's pattern took
                Some(whole) => (line_start + whole.end() - last_char_len(whole.as_str()), Some(indent(line))),
                None => (content_start, None),
            };
            open = Some(Open { start, end: content_end, marked });
        } else if let Some(open) = open.as_mut() {
            open.end = content_end;
        }
        blank_before = false;
        previous_line = line;
    }
    if let Some(done) = open {
        points.push((done.start, done.end));
    }
    points.into_iter().filter(|(start, end)| start < end && !not_a_point(&text[*start..*end], concern)).collect()
}

fn last_char_len(text: &str) -> usize {
    text.chars().last().map_or(0, char::len_utf8)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::super::discussion::fixtures::{ICLR_2021, ICLR_2024, ICML_2024, ICML_2025, NEURIPS_2023, NEURIPS_2025};
    use super::super::discussion::read_discussion;
    use super::*;

    fn split(text: &str) -> Vec<&str> {
        split_points(text, Concern::Weakness).into_iter().map(|(start, end)| &text[start..end]).collect()
    }

    /// The page's text at a span, cut by characters.
    fn at(page: &str, span: Span) -> String {
        page.chars().skip(span.start).take(span.end - span.start).collect()
    }

    #[test]
    fn bullets_numbers_and_labels_each_start_a_point_without_their_marker() {
        let text = "My main concerns are:\n- The baselines are weak.\n* No ablation.\n1. Only two datasets.\n(2) The \
                    notation is heavy.\nW3: Compute is not reported.\nWeakness 4. Seeds are missing.\n[5] Code is not released.\n\
                    b) The proof of Lemma 2 skips a step.\n• A last bullet.";
        assert_eq!(
            split(text),
            [
                "The baselines are weak.",
                "No ablation.",
                "Only two datasets.",
                "The notation is heavy.",
                "Compute is not reported.",
                "Seeds are missing.",
                "Code is not released.",
                "The proof of Lemma 2 skips a step.",
                "A last bullet."
            ]
        );
    }

    #[test]
    fn a_wrapped_or_nested_item_stays_one_point() {
        let text = "1. The experiments are limited\n   to two small benchmarks, and\n   - the second is synthetic.\n2. The \
                    baselines\nare weak: see Table 2.\n";
        assert_eq!(
            split(text),
            [
                "The experiments are limited\n   to two small benchmarks, and\n   - the second is synthetic.",
                "The baselines\nare weak: see Table 2."
            ]
        );
    }

    #[test]
    fn paragraphs_and_lines_that_end_a_sentence_are_points_and_a_colon_carries_on() {
        let paragraphs = "The method is only tested on\nimageNet subsets.\n\nThe writing could be clearer in places.";
        assert_eq!(
            split(paragraphs),
            ["The method is only tested on\nimageNet subsets.", "The writing could be clearer in places."]
        );
        // a rendered list loses its markers: each item is a line
        let rendered = "The baselines are weak.\nThe evaluation ignores contamination.\nNovelty:\nThe idea is known from prior work.";
        assert_eq!(
            split(rendered),
            [
                "The baselines are weak.",
                "The evaluation ignores contamination.",
                "Novelty:\nThe idea is known from prior work."
            ]
        );
    }

    #[test]
    fn nothing_to_raise_is_no_point() {
        for nothing in
            ["None.", "N/A", "Yes.", "No.", "Discussed adequately.", "  \n ", "Not applicable.", "See above.", "-"]
        {
            assert!(split(nothing).is_empty(), "{nothing:?}");
        }
        assert_eq!(split("Does it scale?"), ["Does it scale?"]);
        // asked whether the limitations are addressed, "No." is a criticism
        let limitations = |text: &str| split_points(text, Concern::Limitation).len();
        assert_eq!(
            (limitations("No."), limitations("Not discussed."), limitations("Not adequately addressed.")),
            (1, 1, 1)
        );
        assert_eq!(
            (limitations("Yes."), limitations("Discussed adequately."), limitations("Not applicable.")),
            (0, 0, 0)
        );
    }

    #[test]
    fn a_number_or_initial_inside_prose_is_no_marker() {
        let text = "3.5 times slower than the baseline, e.g. on long inputs.\nIt is 2023 work.";
        assert_eq!(split(text), ["3.5 times slower than the baseline, e.g. on long inputs.", "It is 2023 work."]);
    }

    #[test]
    fn a_neurips_2023_review_gives_its_weaknesses_questions_and_limitations_at_their_place_in_the_page() {
        let discussion = read_discussion(NEURIPS_2023).unwrap();
        let points = review_points(&discussion);
        assert_eq!(points.len(), 2);
        let first = &points[0];
        assert_eq!(first.note, 0);
        let said: Vec<(Concern, &str)> =
            first.points.iter().map(|point| (point.concern, point.text.as_str())).collect();
        assert_eq!(
            said,
            [
                (
                    Concern::Weakness,
                    "The experiments are limited to two small benchmarks. The baselines are weak, and an ablation study is missing."
                ),
                (Concern::Question, "How does the rule behave with sixty-four experts?"),
            ],
            "\"Discussed adequately.\" is no limitation raised"
        );
        for review in &points {
            for point in &review.points {
                assert_eq!(at(NEURIPS_2023, point.span), point.text);
                assert!(!discussion.notes[point.note].sections[point.section].name.is_empty());
            }
        }
        // the rebuttal names reviewer aBcD in its first lines; the titled reply names eFgH
        assert_eq!(
            first.replies,
            [AuthorReply { note: 2, attachment: Attachment::Guessed { basis: Basis::MentionsReviewer } }]
        );
        assert_eq!(points[1].replies, [AuthorReply { note: 4, attachment: Attachment::Named }]);
    }

    #[test]
    fn a_joint_section_is_read_under_the_reviewers_headings_of_criticism_only() {
        let discussion = read_discussion(NEURIPS_2025).unwrap();
        let points = review_points(&discussion);
        let first: Vec<(Concern, &str)> =
            points[0].points.iter().map(|point| (point.concern, point.text.as_str())).collect();
        assert_eq!(
            first,
            [
                (Concern::Weakness, "The baselines are weak and outdated."),
                (Concern::Weakness, "The evaluation ignores contamination."),
                (Concern::Question, "How was contamination checked?"),
                (Concern::Limitation, "No."),
            ]
        );
        assert_eq!(
            points[0].replies,
            [AuthorReply { note: 2, attachment: Attachment::Guessed { basis: Basis::MentionsReviewer } }]
        );
        // inline headings: "Strengths: ... Weaknesses: the baselines are weak."
        assert_eq!(points[1].points[0].text, "the baselines are weak.");
        let icml = review_points(&read_discussion(ICML_2024).unwrap());
        assert_eq!(icml[0].points[0].text, "the theory is thin.");
        for page in [NEURIPS_2025, ICML_2024] {
            for review in review_points(&read_discussion(page).unwrap()) {
                for point in review.points {
                    assert_eq!(at(page, point.span), point.text);
                }
            }
        }
    }

    #[test]
    fn a_joint_section_with_no_headings_is_counted_not_guessed_and_one_field_reviews_have_no_points() {
        let page = "Official Review of Submission1 by Reviewer Ab12\nSummary:\nA paper.\nStrengths And Weaknesses:\nThe \
                    idea is new. The experiments are thin.\nRating: 5: ok\n";
        let points = review_points(&read_discussion(page).unwrap());
        assert_eq!((points[0].points.len(), points[0].not_split.clone()), (0, vec![1]));
        let older = review_points(&read_discussion(ICLR_2021).unwrap());
        assert_eq!(older.len(), 2);
        assert!(older.iter().all(|review| review.points.is_empty() && review.not_split.is_empty()));
        assert_eq!(older[0].replies, [AuthorReply { note: 2, attachment: Attachment::Named }]);
    }

    #[test]
    fn a_joint_section_with_only_a_heading_of_praise_is_counted_not_split_rather_than_raising_nothing() {
        let page = "Official Review of Submission1 by Reviewer Ab12\nSummary:\nA paper.\nStrengths And Weaknesses:\n\
                    Strengths:\n- Clear writing.\n\nThe method lacks baselines.\nRating: 5: ok\n";
        let points = review_points(&read_discussion(page).unwrap());
        assert_eq!((points[0].points.len(), points[0].not_split.clone()), (0, vec![1]));
    }

    #[test]
    fn the_long_icml_form_reads_its_questions_for_authors_and_its_other_strengths_and_weaknesses() {
        let points = review_points(&read_discussion(ICML_2025).unwrap());
        let said: Vec<(Concern, &str)> =
            points[0].points.iter().map(|point| (point.concern, point.text.as_str())).collect();
        // "Other Strengths And Weaknesses" has no headings of the reviewer's: counted, not guessed at
        assert_eq!(said, [(Concern::Question, "Does the theory predict the delay?")]);
        assert_eq!(points[0].not_split, [8]);
        let iclr = review_points(&read_discussion(ICLR_2024).unwrap());
        assert_eq!(iclr[0].points[0].text, "The experiments are limited to small models.");
        assert_eq!(
            iclr[0].replies,
            [AuthorReply { note: 3, attachment: Attachment::Guessed { basis: Basis::FollowsReview } }]
        );
    }

    #[test]
    fn spans_count_characters_where_the_page_has_letters_of_several_bytes() {
        let page = "Résumé ünïcödé\nOfficial Review of Submission1 by Reviewer Zz99\nSummary:\nÉtude.\nWeaknesses:\n- Très \
                    peu d’expériences.\n- Pas d’ablation de la règle.\nRating: 4: weak reject\n";
        let points = review_points(&read_discussion(page).unwrap());
        assert_eq!(points[0].points.len(), 2);
        for point in &points[0].points {
            assert_eq!(at(page, point.span), point.text);
        }
        assert_eq!(points[0].points[1].text, "Pas d’ablation de la règle.");
    }

    #[test]
    fn section_names_say_what_their_points_raise() {
        for (name, concern) in [
            ("Weaknesses", Some(Concern::Weakness)),
            ("Reasons To Reject", Some(Concern::Weakness)),
            ("Concerns", Some(Concern::Weakness)),
            ("Questions For Authors", Some(Concern::Question)),
            ("Limitations", Some(Concern::Limitation)),
            ("Strengths And Weaknesses", None),
            ("Paper Formatting Concerns", None),
            ("Flag For Ethics Review", None),
            ("Summary", None),
            ("Strengths", None),
        ] {
            assert_eq!(Concern::of(name), concern, "{name}");
        }
    }
}
