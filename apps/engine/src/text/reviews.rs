//! The scores reviewers gave a paper, read from the text of its reviews page.
//!
//! Reviews are on OpenReview, which does not serve them to programs, so nothing here fetches anything: when the
//! user opens a paper's reviews in the reader, the interface hands over the text of the page it shows, and the
//! scores are read from that.
//!
//! The page is a thread of notes, each under a heading: "Official Review of Submission123 by Reviewer aBcD",
//! "Official Comment by Authors", "Meta Review of ...", "Decision by ...". Only the reviews are read, each for one
//! score per field, so a rebuttal quoting a score, a comment revising one, or a meta-review's confidence is not
//! counted as a review. A review lists its scores as "Field: number" or "Field: number: what it means":
//!
//! ```text
//! Rating: 5: Accept              Quality: 2: fair
//! Confidence: 4                  Soundness: 3 good
//! ```
//!
//! Which fields exist and what their scales are differs by conference and year (NeurIPS 2025: quality, clarity,
//! significance and originality from 1 to 4, a rating from 1 to 6; ICLR: soundness, presentation and contribution
//! from 1 to 4, a rating from 1 to 10), so the scores are kept as given, by field, and never mixed into one number.
//! A page with scores but no review headings (another site's layout) is read as a whole, and says so
//! (`exact: false`).

use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::{char_len_at, first_chars, pattern, squash};

/// Characters of page text read; the interface refuses more before it gets here.
pub const MAX_TEXT: usize = 2_000_000;
/// More than this is not one paper's reviews.
pub const MAX_REVIEWS: usize = 40;
/// Characters of a score's meaning kept.
const MEANING_LENGTH: usize = 60;

/// A field reviewers score, in the order shown: the overall judgement first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Field {
    Rating,
    #[serde(rename = "Overall Recommendation")]
    OverallRecommendation,
    Overall,
    Recommendation,
    Quality,
    Clarity,
    Significance,
    Originality,
    Soundness,
    Presentation,
    Contribution,
    Confidence,
}

impl Field {
    pub const ALL: [Self; 12] = [
        Self::Rating,
        Self::OverallRecommendation,
        Self::Overall,
        Self::Recommendation,
        Self::Quality,
        Self::Clarity,
        Self::Significance,
        Self::Originality,
        Self::Soundness,
        Self::Presentation,
        Self::Contribution,
        Self::Confidence,
    ];

    /// As a review page writes it (and as the interface shows it).
    pub fn name(self) -> &'static str {
        match self {
            Self::Rating => "Rating",
            Self::OverallRecommendation => "Overall Recommendation",
            Self::Overall => "Overall",
            Self::Recommendation => "Recommendation",
            Self::Quality => "Quality",
            Self::Clarity => "Clarity",
            Self::Significance => "Significance",
            Self::Originality => "Originality",
            Self::Soundness => "Soundness",
            Self::Presentation => "Presentation",
            Self::Contribution => "Contribution",
            Self::Confidence => "Confidence",
        }
    }

    /// The field a page names, in whatever case.
    fn named(written: &str) -> Option<Self> {
        let written = written.to_lowercase();
        Self::ALL.into_iter().find(|field| field.name().to_lowercase() == written)
    }
}

/// One field's scores: one per review, with what each means as the review puts it ("fair"), or "" when it
/// gives no meaning.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scores {
    pub values: Vec<u8>,
    /// Rounded to two decimals.
    pub mean: f64,
    pub meanings: Vec<String>,
}

/// Reviewers' scores, by field: `Ratings` in the contracts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ratings {
    /// How many reviews gave scores; a guess when not `exact`.
    pub reviews: usize,
    /// Were the reviews told apart by their headings? If not, the scores may include quoted ones.
    #[serde(default)] // readings kept before this was recorded
    pub exact: bool,
    /// In the order of [`Field::ALL`].
    pub ratings: BTreeMap<Field, Scores>,
}

/// What a newly read page does to the reading kept for its paper.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// The new reading is at least as full and as exact as the kept one: it replaces it.
    Replace(Ratings),
    /// The page shown is not this paper's discussion (the reader wandered off), or the paper has none.
    NotItsPage,
    /// The page has no scores: not loaded yet, or the reviews are not public.
    NoScores,
    /// Fewer reviews, or told apart less exactly, than what is kept: the page is still loading, or replies are
    /// collapsed.
    LessFull,
}

/// Any number of spaces of any kind (web text has no-break ones), and up to two line ends.
const GAP: &str = r"[^\S\n]*(?:\n[^\S\n]*){0,2}";

fn fields() -> String {
    Field::ALL.iter().map(|field| field.name()).collect::<Vec<_>>().join("|")
}

/// "Field: 3", "Field: 3: good", "Field: 3 good", each at the start of a line. What follows the number is
/// checked by [`NOT_WHOLE`].
static SCORE: LazyLock<Regex> = LazyLock::new(|| {
    pattern(&format!(
        r"(?mi)^[^\S\n]*({})[^\S\n]*:{GAP}([0-9]{{1,2}})[^\S\n]*(?:([:\-–])[^\S\n]*)?([^\n\d]*)",
        fields()
    ))
});
/// After the number, what makes it not a whole score: "3.5", "3/5", "3 out of 4", "3 of the proofs", "5 -> 7".
static NOT_WHOLE: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)\A(?:[\d.,/]|\s*(?:/|->|out of|of\b))"));
/// A field and its score, starting right here: where fields share a line, each is moved onto its own.
static FIELD_AHEAD: LazyLock<Regex> = LazyLock::new(|| pattern(&format!(r"(?i)\A(?:{})[^\S\n]*:[^\S\n]*\d", fields())));
static SPACES: LazyLock<Regex> = LazyLock::new(|| pattern(r"[^\S\n]+"));
/// The heading of a note in the thread. A sentence that merely starts with one of those words is prose, so a
/// heading is the word alone on its line or goes on to say whose note it is.
static NOTE: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?mi)^[^\S\n]*(Official Review|Official Comment|Public Comment|Meta[- ]?Review|Author Rebuttal|Rebuttal|Paper Decision|Decision|Author[^\n]{0,30}Comment|Ethics Review|Withdrawal)\b(?:[^\n]{0,120}\b(?:by|of)\s+(?:Reviewer|Authors?|Area Chairs?|Senior Area Chairs?|Program Chairs?|Ethics Reviewer|Submission\s?\d+|Paper\s?\d+)\b[^\n]{0,60}|[ \t]*)$",
    )
});
static FORUM: LazyLock<Regex> = LazyLock::new(|| pattern(r"openreview\.net/[^?#]*\?(?:[^#]*&)?id=([\w-]+)"));

/// The text with every field that shares a line with something before it moved onto a line of its own.
fn one_field_a_line(text: &str) -> String {
    let mut lines = String::with_capacity(text.len());
    let mut copied = 0;
    for spaces in SPACES.find_iter(text) {
        let after_words = text[..spaces.start()].chars().next_back().is_some_and(|before| !before.is_whitespace());
        if after_words && FIELD_AHEAD.is_match(&text[spaces.end()..]) {
            lines.push_str(&text[copied..spaces.start()]);
            lines.push('\n');
            copied = spaces.end();
        }
    }
    lines.push_str(&text[copied..]);
    lines
}

/// The scores in `text` as (field, score, meaning); with `once`, only the first of each field (one review gives
/// one).
fn scores(text: &str, once: bool) -> Vec<(Field, u8, String)> {
    let lines = one_field_a_line(text);
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let mut from = 0;
    while let Some(score) = SCORE.captures_at(&lines, from) {
        let (Some(whole), Some(field), Some(value)) = (score.get(0), score.get(1), score.get(2)) else {
            break;
        };
        if NOT_WHOLE.is_match(&lines[value.end()..]) {
            // no score on this line: the next one may start inside what was matched
            from = whole.start() + char_len_at(&lines, whole.start());
            continue;
        }
        from = whole.end();
        let (Some(field), Ok(value)) = (Field::named(field.as_str()), value.as_str().parse::<u8>()) else {
            continue;
        };
        let meaning = squash(score.get(4).map_or("", |meaning| meaning.as_str()));
        let meaning = meaning.trim_matches([' ', '.', ':']);
        let prose = score.get(3).is_none() && meaning.split_whitespace().count() > 1; // not a score's meaning
        if (once && seen.contains(&field)) || prose {
            continue;
        }
        seen.insert(field);
        found.push((field, value, first_chars(meaning, MEANING_LENGTH).to_owned()));
    }
    found
}

/// The mean, rounded to two decimals half to even, as Python's `round` does, so that readings kept by the
/// Python backend compare equal.
fn mean(values: &[u8]) -> f64 {
    let count = values.len().max(1) as u64;
    let hundredths = values.iter().map(|&value| u64::from(value)).sum::<u64>() * 100;
    let (whole, left) = (hundredths / count, hundredths % count);
    let rounded = match (2 * left).cmp(&count) {
        std::cmp::Ordering::Less => whole,
        std::cmp::Ordering::Greater => whole + 1,
        std::cmp::Ordering::Equal => whole + whole % 2,
    };
    rounded as f64 / 100.0
}

/// The scores on a reviews page, or `None` if it has none.
pub fn read_ratings(text: &str) -> Option<Ratings> {
    let text = first_chars(text, MAX_TEXT);
    let notes: Vec<_> = NOTE.captures_iter(text).filter_map(|note| Some((note.get(0)?, note.get(1)?))).collect();
    let mut per_review: Vec<Vec<(Field, u8, String)>> = notes
        .iter()
        .enumerate()
        .filter(|(_, (_, kind))| kind.as_str().to_lowercase() == "official review")
        .map(|(index, (note, _))| {
            let end = notes.get(index + 1).map_or(text.len(), |(next, _)| next.start());
            scores(&text[note.end()..end], true)
        })
        .filter(|scores| !scores.is_empty())
        .collect();
    let exact = !per_review.is_empty();
    if !exact {
        per_review = vec![scores(text, false)]; // no headings to go by: every score on the page
    }
    let mut gathered: BTreeMap<Field, (Vec<u8>, Vec<String>)> = BTreeMap::new();
    for (field, value, meaning) in per_review.iter().take(MAX_REVIEWS).flatten() {
        let (values, meanings) = gathered.entry(*field).or_default();
        if values.len() < MAX_REVIEWS {
            values.push(*value);
            meanings.push(meaning.clone());
        }
    }
    if gathered.is_empty() {
        return None;
    }
    let mut counts: Vec<usize> = gathered.values().map(|(values, _)| values.len()).collect();
    counts.sort_unstable();
    let reviews = if exact {
        per_review.len().min(MAX_REVIEWS)
    } else {
        counts[counts.len() / 2] // without headings, the usual number of scores per field is the best guess
    };
    let ratings = gathered
        .into_iter()
        .map(|(field, (values, meanings))| (field, Scores { mean: mean(&values), values, meanings }))
        .collect();
    Some(Ratings { reviews, exact, ratings })
}

/// The id of the OpenReview discussion an address points at.
pub fn forum(address: &str) -> Option<&str> {
    FORUM.captures(address).and_then(|found| found.get(1)).map(|id| id.as_str())
}

/// What the text of a reviews page does to the reading kept for a paper. `paper_address` is the paper's own
/// address, `shown` the address of the page the text came from. A new reading replaces the kept one only if it
/// is at least as exact and, as exactly, has at least as many reviews: a page still loading, or one with replies
/// collapsed, does not undo a fuller reading.
pub fn verdict(paper_address: Option<&str>, shown: Option<&str>, text: &str, kept: Option<&Ratings>) -> Verdict {
    let discussion = paper_address.and_then(forum);
    if discussion.is_none() || shown.and_then(forum) != discussion {
        return Verdict::NotItsPage;
    }
    let Some(ratings) = read_ratings(text) else {
        return Verdict::NoScores;
    };
    if kept.is_some_and(|kept| (kept.exact, kept.reviews) > (ratings.exact, ratings.reviews)) {
        return Verdict::LessFull;
    }
    Verdict::Replace(ratings)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    const FORUM_ADDRESS: &str = "https://openreview.net/forum?id=abc123XYZ";

    const NEURIPS: &str = "
Paper Title
Official Review of Submission1234 by Reviewer aBcD
Summary:
The paper proposes a method. Quality: it is fine overall.
Strengths And Weaknesses:
Quality: 2 of the 3 proofs are sketched only. Clarity: 3/5 I would say.
Quality: 2: fair
Clarity: 2: fair
Significance: 3: good
Originality: 4: excellent
Rating: 4: Borderline accept
Confidence: 3: You are fairly confident in your assessment.

Official Comment by Authors
We thank the reviewer. To recall, the review said:
Quality: 2: fair
Rating: 4: Borderline accept

Official Comment by Reviewer aBcD
Thanks, I raise my score. Rating: 4 -> 5

Official Review of Submission1234 by Reviewer eFgH
Quality:
3: good
Clarity: 4: excellent
Significance: 3: good Originality: 3: good
Rating: 5: Accept
Confidence: 4

Meta Review of Submission1234 by Area Chair XyZw
Recommendation: Accept
Confidence: 5: Certain
";

    const ICLR: &str = "
Official Review of Submission77 by Reviewer One
Soundness: 3: good
Presentation: 2: fair
Contribution: 3: good
Rating: 6: marginally above the acceptance threshold
Confidence: 4: You are confident in your assessment
Official Review of Submission77 by Reviewer Two
soundness: 4 excellent
Presentation: 3 good
Contribution: 2 fair
Rating: 8: accept, good paper
";

    const NO_HEADINGS: &str =
        "Soundness: 3: good\nRating: 6: weak accept\nSoundness: 4: excellent\nRating: 8: accept\n";

    fn scores_of(text: &str, field: Field) -> Scores {
        read_ratings(text).unwrap().ratings[&field].clone()
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    /// What the library keeps for one paper after a page is read, as the Python `keep` returned it.
    fn keep(kept: &mut Option<Ratings>, paper_address: &str, text: &str, shown: &str) -> Option<Ratings> {
        if let Verdict::Replace(ratings) = verdict(Some(paper_address), Some(shown), text, kept.as_ref()) {
            *kept = Some(ratings);
        }
        kept.clone()
    }

    #[test]
    fn only_reviews_are_read_one_score_per_field_each() {
        let read = read_ratings(NEURIPS).unwrap();
        assert_eq!((read.reviews, read.exact), (2, true));
        assert_eq!(
            read.ratings.keys().copied().collect::<Vec<_>>(),
            [Field::Rating, Field::Quality, Field::Clarity, Field::Significance, Field::Originality, Field::Confidence]
        );
        assert_eq!(read.ratings[&Field::Quality].values, [2, 3]); // not the prose, not the authors' quote
        assert_eq!(
            read.ratings[&Field::Clarity],
            Scores { values: vec![2, 4], meanings: strings(&["fair", "excellent"]), mean: 3.0 }
        );
        assert_eq!(read.ratings[&Field::Rating].values, [4, 5]); // not the comment's "4 -> 5"
        assert_eq!(read.ratings[&Field::Rating].meanings[0], "Borderline accept");
        assert_eq!(read.ratings[&Field::Confidence].values, [3, 4]); // not the meta-review's
        assert_eq!(read.ratings[&Field::Significance].meanings, ["good", "good"]); // two fields on one line
        assert_eq!(
            read.ratings[&Field::Originality],
            Scores { values: vec![4, 3], meanings: strings(&["excellent", "good"]), mean: 3.5 }
        );
    }

    #[test]
    fn another_conferences_fields_and_ways_of_writing_them_are_read() {
        assert_eq!(
            scores_of(ICLR, Field::Soundness),
            Scores { values: vec![3, 4], meanings: strings(&["good", "excellent"]), mean: 3.5 }
        );
        assert_eq!(scores_of(ICLR, Field::Rating).mean, 7.0);
        assert_eq!(read_ratings(ICLR).unwrap().reviews, 2);
    }

    #[test]
    fn scores_that_are_not_whole_scores_are_left_alone() {
        for line in
            ["Rating: 6.5", "Clarity: 3/5", "Originality: 2 out of 4", "Rating: 5 -> 7", "Quality: 2 of the proofs"]
        {
            assert_eq!(
                read_ratings(&format!("Official Review of Submission1 by Reviewer A\n{line}\n")),
                None,
                "{line}"
            );
        }
    }

    #[test]
    fn a_page_without_review_headings_is_read_whole_and_says_it_is_a_guess() {
        let read = read_ratings(NO_HEADINGS).unwrap();
        assert_eq!((read.exact, read.reviews), (false, 2));
        assert_eq!(read.ratings[&Field::Rating].values, [6, 8]);
    }

    #[test]
    fn a_page_without_scores_says_nothing_and_changes_nothing() {
        assert_eq!(read_ratings("Just a moment... checking your browser."), None);
        assert_eq!(read_ratings(""), None);
        let mut kept = None;
        let first = keep(&mut kept, FORUM_ADDRESS, ICLR, FORUM_ADDRESS).unwrap();
        assert_eq!(first.reviews, 2);
        assert_eq!(verdict(Some(FORUM_ADDRESS), Some(FORUM_ADDRESS), "Loading", kept.as_ref()), Verdict::NoScores);
        assert_eq!(keep(&mut kept, FORUM_ADDRESS, "Loading", FORUM_ADDRESS), Some(first));
    }

    #[test]
    fn a_fuller_reading_replaces_a_partial_one_but_not_the_other_way_round() {
        let mut kept = None;
        let half = &ICLR[..ICLR.find("Official Review of Submission77 by Reviewer Two").unwrap()];
        assert_eq!(keep(&mut kept, FORUM_ADDRESS, half, FORUM_ADDRESS).unwrap().reviews, 1); // still loading
        assert_eq!(keep(&mut kept, FORUM_ADDRESS, ICLR, FORUM_ADDRESS).unwrap().reviews, 2);
        assert_eq!(verdict(Some(FORUM_ADDRESS), Some(FORUM_ADDRESS), half, kept.as_ref()), Verdict::LessFull);
        assert_eq!(keep(&mut kept, FORUM_ADDRESS, half, FORUM_ADDRESS).unwrap().reviews, 2); // collapsed replies later
        assert!(keep(&mut kept, FORUM_ADDRESS, NO_HEADINGS, FORUM_ADDRESS).unwrap().exact); // nor a vaguer reading
    }

    #[test]
    fn another_papers_page_is_not_read_as_this_papers() {
        for elsewhere in [
            "https://openreview.net/forum?id=SOMEONEELSE",
            "https://openreview.net/group?id=NeurIPS.cc",
            "",
            "https://example.org/?id=abc123XYZ",
        ] {
            assert_eq!(keep(&mut None, FORUM_ADDRESS, ICLR, elsewhere), None, "{elsewhere}");
            assert_eq!(verdict(Some(FORUM_ADDRESS), Some(elsewhere), ICLR, None), Verdict::NotItsPage);
        }
        assert_eq!(verdict(Some(FORUM_ADDRESS), None, ICLR, None), Verdict::NotItsPage);
        assert_eq!(keep(&mut None, "https://arxiv.org/abs/2601.00001", ICLR, FORUM_ADDRESS), None);
        assert_eq!(verdict(None, Some(FORUM_ADDRESS), ICLR, None), Verdict::NotItsPage);
        let shown_with_a_note = format!("{FORUM_ADDRESS}&noteId=xyz");
        assert_eq!(keep(&mut None, FORUM_ADDRESS, ICLR, &shown_with_a_note).unwrap().reviews, 2);
        let reordered = "https://openreview.net/forum?noteId=xyz&id=abc123XYZ#discussion";
        assert_eq!(keep(&mut None, FORUM_ADDRESS, ICLR, reordered).unwrap().reviews, 2);
        assert_eq!(forum(reordered), Some("abc123XYZ"));
    }

    #[test]
    fn a_sentence_that_starts_like_a_heading_does_not_end_a_review() {
        let page = "Official Review of Submission1 by Reviewer A\nRating: 5: accept\n\
                    Rebuttal acknowledged, I keep my concerns.\nDecision boundaries are poorly explained.\nConfidence: 4\n\
                    Official Review of Submission1 by Reviewer B\nRating: 3: reject\nConfidence: 2\n\
                    Official Comment by Authors\nRating: 9\n\
                    Meta Review of Submission1 by Area Chair X\nConfidence: 5\n\
                    Decision by Program Chairs\nRating: 8\n";
        let read = read_ratings(page).unwrap();
        assert_eq!((read.reviews, read.exact), (2, true));
        assert_eq!(read.ratings[&Field::Rating].values, [5, 3]);
        assert_eq!(read.ratings[&Field::Confidence].values, [4, 2]); // the first review's, after the sentences
    }

    #[test]
    fn what_is_read_has_the_shape_the_interface_reads() {
        let read = read_ratings("Official Review of Submission1 by Reviewer A\nRating: 4: weak accept\n").unwrap();
        assert_eq!(
            serde_json::to_value(&read).unwrap(),
            json!({"reviews": 1, "exact": true,
                   "ratings": {"Rating": {"values": [4], "mean": 4.0, "meanings": ["weak accept"]}}})
        );
        for field in Field::ALL {
            assert_eq!(serde_json::to_value(field).unwrap(), field.name());
        }
        let older: Ratings = serde_json::from_value(json!({"reviews": 1, "ratings": {}})).unwrap();
        assert!(!older.exact); // kept before exactness was recorded
    }

    #[test]
    fn a_mean_is_rounded_as_it_always_was() {
        assert_eq!(mean(&[1, 1, 1, 1, 1, 1, 1, 2]), 1.12); // 1.125: half to even
        assert_eq!(mean(&[1, 1, 2]), 1.33);
        assert_eq!(mean(&[2, 3, 3]), 2.67);
    }
}
