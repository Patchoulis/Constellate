//! What reviewers said, taken together: per paper, the figures of each scored field; per community or run, how
//! many papers have reviews read at all, how their ratings are spread per conference and year, which papers
//! reviewers disagreed on, and what they most often praise and criticise.
//!
//! # Scales
//!
//! Every figure is on the scale its venue and year scores that field on, and carries that scale: ICLR's rating
//! runs from 1 to 10, NeurIPS 2025's from 1 to 6, and a 6 means different things on each. Figures on different
//! scales are never averaged or counted together. Where papers of different venues are compared (the most
//! disagreed-upon papers of a community that has both), the comparison is made on a figure named as normalised:
//! the position within the scale, 0 at its lowest and 1 at its highest. A scale read in the venue's own reviewer
//! guidelines is [`ScaleSource::Published`]; one believed but not found there, and the usual scale of a field for
//! a venue this module does not know, are said to be assumed ([`ScaleSource::Assumed`]): see [`scale`].
//!
//! # Themes, without a language model
//!
//! What reviewers praise and criticise is counted from the words of the sections whose names say which they
//! are (Strengths, Weaknesses; inside a joint "Strengths And Weaknesses", the reviewer's own "Strengths:" and
//! "Weaknesses:"). Each such section is compared with every section of every review of the set: a term (a word
//! or two words in a row, common words left out) is a theme when it is in more of the praising (criticising)
//! sections than its share of all sections would make it, in sections of at least two papers. Each theme keeps
//! where it was said, as (paper, review, section, character span of the sentence), so the interface can show
//! the reviewer's own words.
//!
//! For one paper ([`in_common`]) the same terms are counted by reviewer instead of by paper: a term is a point
//! the reviewers have in common when at least two of them use it in a praising (criticising) section. These are
//! ranked by how many reviewers use it, then by how much more it is used in that tone than in the paper's review
//! sections overall, then alphabetically. Every term that at least two reviewers share is kept, however common it
//! is elsewhere: with four reviews, being shared is what makes it worth showing.
//!
//! # Threads and the same answer every time
//!
//! The per-paper work (figures, the terms of each review) runs on every core with rayon. Nothing that is
//! computed depends on how the work was split: papers are put in order of their id first; a paper's sums run
//! over its scores in sorted order; papers' figures are combined one after the other in that order; counts are
//! whole numbers, whose sums do not depend on order; and where only some of a theme's sources are kept, they are
//! the first by (paper, review, position), whichever thread found them. The same reviews give byte-identical
//! results on one thread or many.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use rayon::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::text::reviews::Field;
use crate::text::reviews::discussion::{Discussion, Kind, Note, Section, Signer, Span, Venue};
use crate::text::{CharIndex, first_chars, floor_boundary, squash};

/// Papers listed among the most and least agreed-upon, and the highest rated.
pub const RANKED: usize = 5;
/// Themes of praise, and of criticism.
pub const THEMES: usize = 12;
/// Where a theme was said: at most this many places are listed with it.
pub const SOURCES: usize = 40;
/// A theme is said in sections of at least this many papers (of a set) or reviewers (of one paper).
const LEAST_VOTERS: u32 = 2;
/// A section with fewer words than this (a score, "None.") says nothing to count.
const LEAST_WORDS: usize = 4;
/// How far (in bytes) from a theme's word the ends of its sentence are looked for.
const SENTENCE_REACH: usize = 400;
/// Characters of a sentence shown with a theme's source.
const EXCERPT: usize = 240;
/// The fields that are a review's overall judgement, best first: whichever of them a review has.
const OVERALL: [Field; 4] = [Field::Rating, Field::OverallRecommendation, Field::Overall, Field::Recommendation];

/// Where a scale comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleSource {
    /// The venue's own form for that year.
    Published,
    /// Not known for this venue or year: the usual scale of the field, an estimate.
    Assumed,
}

/// The lowest and highest score a field can be given at a venue in a year.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scale {
    pub minimum: u8,
    pub maximum: u8,
    pub source: ScaleSource,
}

impl Scale {
    /// `value`'s position within the scale: 0 at its lowest, 1 at its highest (a value outside it is held to it).
    pub fn normalised(&self, value: f64) -> f64 {
        let width = f64::from(self.maximum.saturating_sub(self.minimum)).max(1.0);
        ((value - f64::from(self.minimum)) / width).clamp(0.0, 1.0)
    }

    fn width(&self) -> f64 {
        f64::from(self.maximum.saturating_sub(self.minimum)).max(1.0)
    }
}

/// The scale `field` is scored on at `venue`: as the venue's own reviewer guidelines publish it where they were
/// read, else the scale the venue is believed to use, else the field's usual scale, both said to be assumed.
///
/// Published (read on the venue's own site): NeurIPS 2022 to 2025 (neurips.cc/Conferences/<year>/
/// ReviewerGuidelines: a rating of 1 to 10 until 2024 and of 1 to 6 from 2025, confidence 1 to 5, soundness,
/// presentation and contribution 1 to 4 in 2022 to 2024, quality, clarity, significance and originality 1 to 4
/// from 2025); ICML 2024 and 2025 (icml.cc/Conferences/<year>/ReviewerInstructions: 2024 a rating of 1 to 10,
/// confidence 1 to 5, soundness, presentation and contribution 1 to 4; 2025 an overall recommendation of 1 to 5).
/// Assumed: every ICLR scale (iclr.cc's reviewer guides do not print the form's scales), ICML before 2024 (its
/// instructions are no longer on icml.cc), ICML 2025's confidence (not on its main track's form), and NeurIPS
/// before 2022 and after 2025 (not read).
pub fn scale(venue: Option<&Venue>, field: Field) -> Scale {
    use Field::*;
    use ScaleSource::{Assumed, Published};
    let known = venue.and_then(|venue| {
        let year = venue.year;
        let neurips = if (2022..=2025).contains(&year) { Published } else { Assumed };
        let icml = if (2024..=2025).contains(&year) { Published } else { Assumed };
        match (venue.conference.as_str(), field) {
            ("NeurIPS", Confidence) => Some((1, 5, neurips)),
            ("NeurIPS", Rating) if year >= 2025 => Some((1, 6, neurips)),
            ("NeurIPS", Rating) => Some((1, 10, neurips)),
            ("NeurIPS", Quality | Clarity | Significance | Originality) if year >= 2025 => Some((1, 4, neurips)),
            ("NeurIPS", Soundness | Presentation | Contribution) if (2022..=2024).contains(&year) => {
                Some((1, 4, neurips))
            }
            ("ICML", Confidence) if year <= 2024 => Some((1, 5, icml)),
            ("ICML", Confidence) => Some((1, 5, Assumed)),
            ("ICML", OverallRecommendation) if year >= 2025 => Some((1, 5, icml)),
            ("ICML", Rating) if year <= 2024 => Some((1, 10, icml)),
            ("ICML", Soundness | Presentation | Contribution) if (2023..=2024).contains(&year) => Some((1, 4, icml)),
            ("ICLR", Confidence) => Some((1, 5, Assumed)),
            ("ICLR", Rating) if year >= 2026 => Some((0, 10, Assumed)),
            ("ICLR", Rating) => Some((1, 10, Assumed)),
            ("ICLR", Soundness | Presentation | Contribution) if year >= 2023 => Some((1, 4, Assumed)),
            _ => None,
        }
    });
    match known {
        Some((minimum, maximum, source)) => Scale { minimum, maximum, source },
        None => {
            let (minimum, maximum) = match field {
                Rating | Overall | Recommendation => (1, 10),
                OverallRecommendation | Confidence => (1, 5),
                _ => (1, 4),
            };
            Scale { minimum, maximum, source: ScaleSource::Assumed }
        }
    }
}

/// A mean in which each review counts as much as the confidence its reviewer gave: a separate figure from the
/// plain mean, over only the reviews that gave a confidence.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfidenceWeighted {
    /// On the field's scale.
    pub mean: f64,
    /// Of the field's reviews, how many gave a confidence and so are in it.
    pub reviews: u32,
}

/// One scored field of one paper: every figure on `scale`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldFigures {
    pub field: Field,
    pub scale: Scale,
    /// How many reviews scored it.
    pub reviews: u32,
    pub mean: f64,
    pub median: f64,
    pub minimum: u8,
    pub maximum: u8,
    /// Standard deviation of the reviews' scores (of these reviews, not an estimate of a wider population).
    pub spread: f64,
    /// The mean's position within the scale, 0 to 1: normalised, for comparing across venues only.
    pub normalised_mean: f64,
    pub confidence_weighted: Option<ConfidenceWeighted>,
    /// Scores outside the scale (a venue that changed its form): counted in the figures as given.
    pub outside_scale: u32,
}

/// How far a paper's reviewers were apart on their overall judgement, when at least two gave one.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Disagreement {
    pub field: Field,
    pub scale: Scale,
    /// The standard deviation of the overall scores, on the scale.
    pub spread: f64,
    /// Highest score less lowest, on the scale.
    pub range: u8,
    /// The spread as a share of the scale's width: normalised, for comparing across venues.
    pub normalised_spread: f64,
}

/// What a paper's reviews come to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaperFigures {
    pub paper_id: String,
    pub venue: Option<Venue>,
    /// Its official reviews read.
    pub reviews: u32,
    /// In [`Field::ALL`] order, confidence last.
    pub fields: Vec<FieldFigures>,
    pub disagreement: Option<Disagreement>,
}

impl PaperFigures {
    /// The figures of its overall judgement (rating, overall recommendation...), if any review gave one.
    pub fn overall(&self) -> Option<&FieldFigures> {
        OVERALL.iter().find_map(|field| self.fields.iter().find(|figures| figures.field == *field))
    }
}

/// A paper whose reviews were read, with the venue its scales are taken from: the venue its id names, else the
/// one its page names.
#[derive(Clone, Copy, Debug)]
pub struct Reviewed<'a> {
    pub paper_id: &'a str,
    pub venue: Option<&'a Venue>,
    pub discussion: &'a Discussion,
}

impl<'a> Reviewed<'a> {
    pub fn new(paper_id: &'a str, venue: Option<&'a Venue>, discussion: &'a Discussion) -> Self {
        Self { paper_id, venue, discussion }
    }
}

/// The venue to take a paper's scales from: the one its id names, else the one its reviews page names.
pub fn venue_of(paper_id: &str, discussion: &Discussion) -> Option<Venue> {
    Venue::of_paper(paper_id).or_else(|| discussion.venue.clone())
}

fn figures_of(field: Field, scale: Scale, mut scores: Vec<(u8, Option<u8>)>) -> FieldFigures {
    scores.sort_unstable(); // sums in one order, whatever order the reviews came in
    let count = scores.len();
    let values: Vec<u8> = scores.iter().map(|(value, _)| *value).collect();
    let total: u32 = values.iter().map(|value| u32::from(*value)).sum();
    let mean = f64::from(total) / count.max(1) as f64;
    let median = match count {
        0 => 0.0,
        _ if count % 2 == 1 => f64::from(values[count / 2]),
        _ => (f64::from(values[count / 2 - 1]) + f64::from(values[count / 2])) / 2.0,
    };
    let squares: f64 = values.iter().map(|value| (f64::from(*value) - mean).powi(2)).sum();
    let spread = (squares / count.max(1) as f64).sqrt();
    let weighed: Vec<(u32, u32)> =
        scores.iter().filter_map(|(value, confidence)| Some((u32::from(*value), u32::from((*confidence)?)))).collect();
    let weight: u32 = weighed.iter().map(|(_, confidence)| confidence).sum();
    let confidence_weighted = (weight > 0).then(|| ConfidenceWeighted {
        mean: f64::from(weighed.iter().map(|(value, confidence)| value * confidence).sum::<u32>()) / f64::from(weight),
        reviews: u32::try_from(weighed.len()).unwrap_or(u32::MAX),
    });
    let outside = values.iter().filter(|value| **value < scale.minimum || **value > scale.maximum).count();
    FieldFigures {
        field,
        scale,
        reviews: u32::try_from(count).unwrap_or(u32::MAX),
        mean,
        median,
        minimum: values.first().copied().unwrap_or_default(),
        maximum: values.last().copied().unwrap_or_default(),
        spread,
        normalised_mean: scale.normalised(mean),
        confidence_weighted,
        outside_scale: u32::try_from(outside).unwrap_or(u32::MAX),
    }
}

/// The figures of one paper's reviews.
pub fn paper_figures(paper: &Reviewed<'_>) -> PaperFigures {
    let mut by_field: BTreeMap<Field, Vec<(u8, Option<u8>)>> = BTreeMap::new();
    for (_, note) in paper.discussion.reviews() {
        if let Kind::OfficialReview { scores, confidence } = &note.kind {
            let confidence = confidence.as_ref().map(|score| score.value);
            for score in scores {
                by_field.entry(score.field).or_default().push((score.value, confidence));
            }
            if let Some(confidence) = confidence {
                by_field.entry(Field::Confidence).or_default().push((confidence, None));
            }
        }
    }
    let fields: Vec<FieldFigures> =
        by_field.into_iter().map(|(field, scores)| figures_of(field, scale(paper.venue, field), scores)).collect();
    let figures = PaperFigures {
        paper_id: paper.paper_id.to_owned(),
        venue: paper.venue.cloned(),
        reviews: u32::try_from(paper.discussion.review_count()).unwrap_or(u32::MAX),
        fields,
        disagreement: None,
    };
    let disagreement = figures.overall().filter(|overall| overall.reviews >= 2).map(|overall| Disagreement {
        field: overall.field,
        scale: overall.scale,
        spread: overall.spread,
        range: overall.maximum - overall.minimum,
        normalised_spread: overall.spread / overall.scale.width(),
    });
    PaperFigures { disagreement, ..figures }
}

/// A paper among the most or least agreed-upon, or the highest rated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RankedPaper {
    pub paper_id: String,
    pub venue: Option<Venue>,
    /// Its overall judgement, on its own scale.
    pub overall: FieldFigures,
    /// What it was ranked by, normalised to the scale so that venues compare: the spread's share of the scale's
    /// width for agreement, the mean's position within it for rating.
    pub normalised: f64,
}

/// One unit of a scale, and the papers whose mean rating falls in it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bin {
    /// From this score (included) to `to` (excluded, except for the scale's top, which the last bin includes).
    pub from: u8,
    pub to: u8,
    pub papers: u32,
}

/// How the papers of one venue and year are rated: the spread of their per-paper mean of one field, on that
/// venue's scale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueRatings {
    pub venue: Venue,
    pub field: Field,
    pub scale: Scale,
    pub papers: u32,
    /// Of the per-paper means.
    pub mean: f64,
    pub median: f64,
    pub lowest: f64,
    pub highest: f64,
    /// One bin per unit of the scale.
    pub bins: Vec<Bin>,
}

/// Where a theme was said: the sentence of a review's section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeSource {
    pub paper_id: String,
    /// The review's index among the paper's notes.
    pub note: usize,
    pub signer: Signer,
    pub section: String,
    /// The sentence, in characters of the paper's kept page text.
    pub span: Span,
    /// The sentence, spaces squashed, cut at [`EXCERPT`] characters.
    pub excerpt: String,
}

/// A term reviewers often use when they praise (or criticise).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub term: String,
    /// Praising (criticising) sections that use it.
    pub sections: u32,
    /// Papers whose reviews do.
    pub papers: u32,
    /// Its share of the praising (criticising) sections, 0 to 1.
    pub share: f64,
    /// How many times more often it is in praising (criticising) sections than in the reviews' sections overall.
    pub distinctiveness: f64,
    /// The first [`SOURCES`] places it was said, by paper id, review and position.
    pub sources: Vec<ThemeSource>,
    /// Places left out of `sources`.
    pub more_sources: u32,
}

/// What the reviews of a community or a run come to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewSummary {
    /// Its papers, read or not.
    pub papers: usize,
    /// Of them, how many had their reviews page read: reviews are only known for pages the user opened.
    pub pages_read: usize,
    /// Of those, how many have at least one review.
    pub papers_with_reviews: usize,
    pub reviews: usize,
    /// One per venue and year and overall field, in that order.
    pub ratings: Vec<VenueRatings>,
    /// Papers with reviews whose venue is not known: left out of `ratings`, whose scales would be guesses.
    pub papers_without_venue: usize,
    pub most_disagreed: Vec<RankedPaper>,
    pub most_agreed: Vec<RankedPaper>,
    pub highest_rated: Vec<RankedPaper>,
    pub praise: Vec<Theme>,
    pub criticism: Vec<Theme>,
}

/// A term at least two of one paper's reviewers use when they praise (or criticise) it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaperTheme {
    pub term: String,
    /// Of the paper's reviewers, how many use it in praising (criticising) sections.
    pub reviewers: u32,
    /// The paper's reviewers in all.
    pub all_reviewers: u32,
    /// How many times more often it is in the paper's praising (criticising) sections than in its review
    /// sections overall.
    pub distinctiveness: f64,
    /// The first sentence of each review that uses it, in the order of the notes, at most [`SOURCES`].
    pub sources: Vec<ThemeSource>,
    /// Places left out of `sources`.
    pub more_sources: u32,
}

/// What one paper's reviewers have in common: none when it has fewer than two reviewers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InCommon {
    pub praise: Vec<PaperTheme>,
    pub criticism: Vec<PaperTheme>,
}

/// Who counts as one voice for a term: across a set, a paper (however many of its reviewers use it); within a
/// paper, a reviewer (however many times they use it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Voter {
    Paper,
    Reviewer,
}

/// One voice, as [`Voter`] says what one is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Voice<'a> {
    Paper,
    /// A reviewer by their id: two reviews signed by the same reviewer are one voice.
    Reviewer(&'a str),
    /// A review whose reviewer is not shown, by its index among the notes.
    Review(usize),
}

impl Voter {
    fn voice(self, note_index: usize, note: &Note) -> Voice<'_> {
        match self {
            Self::Paper => Voice::Paper,
            Self::Reviewer => note.reviewer().map_or(Voice::Review(note_index), Voice::Reviewer),
        }
    }
}

/// Praise or criticism.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Tone {
    Praise,
    Criticism,
}

impl Tone {
    /// What a section's name (or a reviewer's own heading) says it holds; none for one of both or of neither,
    /// and none for concerns about formatting or ethics, which are not about the work.
    fn of(name: &str) -> Option<Self> {
        let name = name.to_lowercase();
        let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|word| !word.is_empty()).collect();
        let has = |starts: &[&str]| words.iter().any(|word| starts.iter().any(|start| word.starts_with(start)));
        if has(&["formatting", "ethic"]) {
            return None;
        }
        let praise = has(&["strength", "positive"]) || words.contains(&"pros") || name.contains("reasons to accept");
        let criticism =
            has(&["weakness", "concern", "negative"]) || words.contains(&"cons") || name.contains("reasons to reject");
        match (praise, criticism) {
            (true, false) => Some(Self::Praise),
            (false, true) => Some(Self::Criticism),
            _ => None,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Praise => 0,
            Self::Criticism => 1,
        }
    }
}

#[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
static PART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)(?:^[^\S\n]*[-*#]*[^\S\n]*(strengths?|pros|weaknesses?|cons|concerns)[^\S\n]*:?[^\S\n]*$|\b(strengths?|pros|weaknesses?|cons|concerns)[^\S\n]*:)").unwrap()
});
#[allow(clippy::unwrap_used)]
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{L}\p{N}][\p{L}\p{N}'’-]*").unwrap());

/// Words left out of themes: the commonest of English, and of reviews.
static COMMON: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    "a about above after again against all also although am an and any are as at be because been before being \
     below between both but by can could did do does doing done down during each either else etc even ever every \
     few for from further had has have having he her here hers him his how however i if in into is it its itself \
     just least less like made make makes many may me might more most much must my need needs no nor not now of \
     off often on once one only or other others otherwise our ours out over own per perhaps quite rather really \
     same see seem seems several she should since so some such than that the their theirs them then there \
     therefore these they this those though through thus to too two under until up upon us very via was we well \
     were what when where whether which while who whom whose why will with within without would yet you your \
     paper papers authors author work works method methods approach approaches proposed propose proposes \
     results result section sections figure figures table tables appendix e.g i.e et al use used uses using \
     show shows shown showing provide provides provided present presents presented think believe find found \
     overall clear clearly also however main"
        .split_whitespace()
        .collect()
});

/// A term as counted: where in the section's text it first is.
type Terms = HashMap<String, usize>;

/// The words and two-word terms of `text`, each with the byte offset of its first use.
fn terms(text: &str) -> (Terms, usize) {
    let mut found = Terms::new();
    let mut words = 0;
    let mut previous: Option<(String, usize, usize)> = None; // content word, where it starts, where it ends
    for word in WORD.find_iter(text) {
        words += 1;
        let lower = word.as_str().trim_end_matches(['-', '\'', '’']).to_lowercase();
        let content =
            lower.chars().count() >= 3 && !COMMON.contains(lower.as_str()) && !lower.chars().all(|c| c.is_numeric());
        if !content {
            previous = None;
            continue;
        }
        if let Some((before, start, end)) = previous.take()
            && text[end..word.start()].chars().all(char::is_whitespace)
        {
            found.entry(format!("{before} {lower}")).or_insert(start);
        }
        found.entry(lower.clone()).or_insert(word.start());
        previous = Some((lower, word.start(), word.end()));
    }
    (found, words)
}

/// A stretch of one review's section, with what it holds.
struct Part<'a> {
    tone: Option<Tone>,
    /// The section's index among the note's sections.
    section: usize,
    /// Byte offset of the stretch in the section's text.
    from: usize,
    text: &'a str,
}

/// A section's stretches: the whole section, or, for one of both praise and criticism, the stretches under
/// the reviewer's own headings.
fn parts<'a>(index: usize, section: &'a Section) -> Vec<Part<'a>> {
    let text = section.text.as_str();
    let whole = |tone| vec![Part { tone, section: index, from: 0, text }];
    let name = section.name.to_lowercase();
    let both =
        (name.contains("strength") && name.contains("weakness")) || (name.contains("pros") && name.contains("cons"));
    if !both {
        return whole(Tone::of(&section.name));
    }
    let marks: Vec<(usize, usize, Option<Tone>)> = PART
        .captures_iter(text)
        .filter_map(|found| {
            let whole = found.get(0)?;
            let word = found.get(1).or(found.get(2))?;
            Some((whole.start(), whole.end(), Tone::of(word.as_str())))
        })
        .collect();
    if marks.is_empty() {
        return whole(None);
    }
    let mut stretches = vec![Part { tone: None, section: index, from: 0, text: &text[..marks[0].0] }];
    for (at, (_, end, tone)) in marks.iter().enumerate() {
        let until = marks.get(at + 1).map_or(text.len(), |next| next.0);
        stretches.push(Part { tone: *tone, section: index, from: *end, text: &text[*end..until] });
    }
    stretches
}

/// Where in all the reviews one use of a term is: (paper's place in the sorted list, note, section, sentence
/// span in the page's characters). Small, since a term may be used in tens of thousands of reviews.
type Place = (u32, u32, u32, Span);

/// One term's counts over a set of papers, or over one paper's reviews.
#[derive(Default)]
struct Counted {
    /// Sections of any tone, and of praise and of criticism, that use it.
    sections: u32,
    toned: [u32; 2],
    /// Voices ([`Voter`]: papers, or reviewers) that use it in praise and in criticism.
    voters: [u32; 2],
    /// Places of praise and of criticism, at most [`SOURCES`] each, the first by place; and how many in all.
    places: [Vec<Place>; 2],
    said: [u32; 2],
}

impl Counted {
    fn merge(&mut self, other: Counted) {
        self.sections += other.sections;
        for tone in 0..2 {
            self.toned[tone] += other.toned[tone];
            self.voters[tone] += other.voters[tone];
            self.said[tone] += other.said[tone];
            let mut places = std::mem::take(&mut self.places[tone]);
            places.extend(other.places[tone].iter().copied());
            places.sort_unstable();
            places.truncate(SOURCES);
            self.places[tone] = places;
        }
    }
}

/// Every section of every review counted, and the counts by term.
#[derive(Default)]
struct Tally {
    sections: u32,
    toned: [u32; 2],
    terms: HashMap<String, Counted>,
}

impl Tally {
    fn merge(mut self, other: Tally) -> Tally {
        self.sections += other.sections;
        for tone in 0..2 {
            self.toned[tone] += other.toned[tone];
        }
        for (term, counted) in other.terms {
            self.terms.entry(term).or_default().merge(counted);
        }
        self
    }
}

/// The sentence around byte `at` of a section's `text`, as a byte range.
fn sentence(text: &str, at: usize) -> (usize, usize) {
    let ends = |c: char| matches!(c, '.' | '!' | '?' | '\n');
    // looked for within a few hundred bytes either side, so that a section without full stops costs no more
    let back = floor_boundary(text, at.saturating_sub(SENTENCE_REACH));
    let ahead = floor_boundary(text, at + SENTENCE_REACH);
    let start = text[back..at].rfind(ends).map_or(back, |end| back + end + 1);
    let end = text[at..ahead].find(ends).map_or(ahead, |end| at + end + 1);
    let start = start + (text[start..end].len() - text[start..end].trim_start().len());
    (start, start.max(end - (text[start..end].len() - text[start..end].trim_end().len())))
}

/// The terms of one paper's reviews, each voice ([`Voter`]) counted once per term and tone.
fn tally(place: u32, paper: &Reviewed<'_>, voter: Voter) -> Tally {
    let mut tally = Tally::default();
    let mut voted: [HashSet<(Voice<'_>, String)>; 2] = [HashSet::new(), HashSet::new()];
    for (note_index, note) in paper.discussion.reviews() {
        let voice = voter.voice(note_index, note);
        let mut said_in_review: [HashSet<String>; 2] = [HashSet::new(), HashSet::new()];
        for (index, section) in note.sections.iter().enumerate() {
            let in_section = CharIndex::new(&section.text);
            for part in parts(index, section) {
                let (found, words) = terms(part.text);
                if words < LEAST_WORDS {
                    continue;
                }
                tally.sections += 1;
                if let Some(tone) = part.tone {
                    tally.toned[tone.index()] += 1;
                }
                for (term, at) in found {
                    let counted = tally.terms.entry(term.clone()).or_default();
                    counted.sections += 1;
                    let Some(tone) = part.tone.map(Tone::index) else {
                        continue;
                    };
                    counted.toned[tone] += 1;
                    if said_in_review[tone].insert(term.clone()) {
                        let (start, end) = sentence(&section.text, part.from + at);
                        let chars = |byte: usize| section.span.start + in_section.at(byte);
                        let span = Span { start: chars(start), end: chars(end) };
                        let place = (
                            place,
                            u32::try_from(note_index).unwrap_or(u32::MAX),
                            u32::try_from(part.section).unwrap_or(u32::MAX),
                            span,
                        );
                        counted.places[tone].push(place);
                        counted.said[tone] += 1;
                    }
                    if voted[tone].insert((voice, term)) {
                        counted.voters[tone] += 1;
                    }
                }
            }
        }
    }
    for counted in tally.terms.values_mut() {
        for places in &mut counted.places {
            places.sort_unstable();
            places.truncate(SOURCES);
        }
    }
    tally
}

/// A term chosen as a theme of one tone, with its counts.
struct Chosen<'t> {
    term: &'t str,
    counted: &'t Counted,
    distinctiveness: f64,
}

/// The themes of one tone, most telling first: at most [`THEMES`], each said by at least [`LEAST_VOTERS`] voices.
/// Across papers, a term is kept when it is more often in that tone than overall, and ranked by how many
/// sections of that tone use it, weighed by how distinctive it is; within one paper, every term enough reviewers
/// share is kept, ranked by how many do, then by how distinctive it is. Ties go alphabetically.
fn choose(tally: &Tally, tone: Tone, voter: Voter) -> Vec<Chosen<'_>> {
    let index = tone.index();
    let (toned, all) = (f64::from(tally.toned[index]), f64::from(tally.sections));
    // how much a term weighs against the words it is part of or holds: in sections, or in reviewers
    let weight = |counted: &Counted| match voter {
        Voter::Paper => counted.toned[index],
        Voter::Reviewer => counted.voters[index],
    };
    // (first key, second key, term, counts, distinctiveness), the keys highest first
    let mut scored: Vec<(f64, f64, &str, &Counted, f64)> = tally
        .terms
        .iter()
        .filter(|(_, counted)| counted.voters[index] >= LEAST_VOTERS)
        .filter_map(|(term, counted)| {
            let here = (f64::from(counted.toned[index]) + 0.5) / (toned + 1.0);
            let anywhere = (f64::from(counted.sections) + 0.5) / (all + 1.0);
            let distinctiveness = here / anywhere;
            match voter {
                Voter::Paper => (distinctiveness > 1.0).then(|| {
                    let score = f64::from(counted.toned[index]) * distinctiveness.ln();
                    (score, 0.0, term.as_str(), counted, distinctiveness)
                }),
                Voter::Reviewer => {
                    Some((f64::from(counted.voters[index]), distinctiveness, term.as_str(), counted, distinctiveness))
                }
            }
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.1.total_cmp(&a.1)).then_with(|| a.2.cmp(b.2)));
    // a word said mostly as part of one two-word term ("small" in "small models") is that term, not a theme
    let mut in_phrases: HashMap<&str, u32> = HashMap::new();
    for (_, _, term, counted, _) in &scored {
        if let Some((first, second)) = term.split_once(' ') {
            for word in [first, second] {
                let most = in_phrases.entry(word).or_default();
                *most = (*most).max(weight(counted));
            }
        }
    }
    let mut chosen: Vec<Chosen<'_>> = Vec::new();
    for (_, _, term, counted, distinctiveness) in scored {
        if chosen.len() == THEMES {
            break;
        }
        if !term.contains(' ') && in_phrases.get(term).is_some_and(|most| 4 * most >= 3 * weight(counted)) {
            continue;
        }
        // a word already said by a chosen two-word term (or the other way round) is not a theme of its own
        let words: Vec<&str> = term.split(' ').collect();
        if chosen.iter().any(|theme| {
            let other: Vec<&str> = theme.term.split(' ').collect();
            words.iter().all(|word| other.contains(word)) || other.iter().all(|word| words.contains(word))
        }) {
            continue;
        }
        chosen.push(Chosen { term, counted, distinctiveness });
    }
    chosen
}

/// Where a chosen term was said in `tone`, as the reviewers' sentences, and how many places are left out.
fn sources_of(chosen: &Chosen<'_>, tone: Tone, papers: &[Reviewed<'_>]) -> (Vec<ThemeSource>, u32) {
    let index = tone.index();
    let sources: Vec<ThemeSource> = chosen.counted.places[index]
        .iter()
        .filter_map(|(paper, note, section, span)| {
            let paper = papers.get(usize::try_from(*paper).ok()?)?;
            let note_index = usize::try_from(*note).ok()?;
            let note = paper.discussion.notes.get(note_index)?;
            let section = note.sections.get(usize::try_from(*section).ok()?)?;
            let from = span.start.checked_sub(section.span.start)?;
            let said: String = section.text.chars().skip(from).take(span.end - span.start).collect();
            Some(ThemeSource {
                paper_id: paper.paper_id.to_owned(),
                note: note_index,
                signer: note.signer.clone(),
                section: section.name.clone(),
                span: *span,
                excerpt: first_chars(&squash(&said), EXCERPT).to_owned(),
            })
        })
        .collect();
    let listed = u32::try_from(sources.len()).unwrap_or(u32::MAX);
    let more = chosen.counted.said[index].saturating_sub(listed);
    (sources, more)
}

/// The themes of one tone over a set of papers, most telling first.
fn themes(tally: &Tally, tone: Tone, papers: &[Reviewed<'_>]) -> Vec<Theme> {
    let index = tone.index();
    let toned = f64::from(tally.toned[index]);
    choose(tally, tone, Voter::Paper)
        .iter()
        .map(|chosen| {
            let (sources, more_sources) = sources_of(chosen, tone, papers);
            Theme {
                term: chosen.term.to_owned(),
                sections: chosen.counted.toned[index],
                papers: chosen.counted.voters[index],
                share: f64::from(chosen.counted.toned[index]) / toned.max(1.0),
                distinctiveness: chosen.distinctiveness,
                sources,
                more_sources,
            }
        })
        .collect()
}

/// What one paper's reviewers have in common: the terms at least two of them use when they praise it, and when
/// they criticise it, most widely shared first. A paper with fewer than two reviewers has none.
pub fn in_common(paper: &Reviewed<'_>) -> InCommon {
    let voices: HashSet<Voice<'_>> =
        paper.discussion.reviews().map(|(note_index, note)| Voter::Reviewer.voice(note_index, note)).collect();
    let all_reviewers = u32::try_from(voices.len()).unwrap_or(u32::MAX);
    if all_reviewers < LEAST_VOTERS {
        return InCommon::default();
    }
    let tally = tally(0, paper, Voter::Reviewer);
    let papers = std::slice::from_ref(paper);
    let points = |tone: Tone| -> Vec<PaperTheme> {
        choose(&tally, tone, Voter::Reviewer)
            .iter()
            .map(|chosen| {
                let (sources, more_sources) = sources_of(chosen, tone, papers);
                PaperTheme {
                    term: chosen.term.to_owned(),
                    reviewers: chosen.counted.voters[tone.index()],
                    all_reviewers,
                    distinctiveness: chosen.distinctiveness,
                    sources,
                    more_sources,
                }
            })
            .collect()
    };
    InCommon { praise: points(Tone::Praise), criticism: points(Tone::Criticism) }
}

fn ranked(
    figures: &[PaperFigures],
    key: impl Fn(&PaperFigures) -> Option<f64>,
    highest_first: bool,
) -> Vec<RankedPaper> {
    let mut ranked: Vec<(f64, &PaperFigures, &FieldFigures)> =
        figures.iter().filter_map(|paper| Some((key(paper)?, paper, paper.overall()?))).collect();
    ranked.sort_by(|a, b| {
        let order = if highest_first { b.0.total_cmp(&a.0) } else { a.0.total_cmp(&b.0) };
        order.then_with(|| b.2.reviews.cmp(&a.2.reviews)).then_with(|| a.1.paper_id.cmp(&b.1.paper_id))
    });
    ranked
        .into_iter()
        .take(RANKED)
        .map(|(normalised, paper, overall)| RankedPaper {
            paper_id: paper.paper_id.clone(),
            venue: paper.venue.clone(),
            overall: overall.clone(),
            normalised,
        })
        .collect()
}

fn venue_ratings(figures: &[PaperFigures]) -> Vec<VenueRatings> {
    let mut by_venue: BTreeMap<(Venue, Field), (Scale, Vec<f64>)> = BTreeMap::new();
    for paper in figures {
        if let (Some(venue), Some(overall)) = (&paper.venue, paper.overall()) {
            by_venue.entry((venue.clone(), overall.field)).or_insert((overall.scale, Vec::new())).1.push(overall.mean);
        }
    }
    by_venue
        .into_iter()
        .map(|((venue, field), (scale, means))| {
            let count = means.len();
            let mean = means.iter().sum::<f64>() / count.max(1) as f64; // in paper order: the same sum every time
            let mut sorted = means.clone();
            sorted.sort_by(f64::total_cmp);
            let median = match count {
                0 => 0.0,
                _ if count % 2 == 1 => sorted[count / 2],
                _ => (sorted[count / 2 - 1] + sorted[count / 2]) / 2.0,
            };
            let mut bins: Vec<Bin> =
                (scale.minimum..scale.maximum).map(|from| Bin { from, to: from + 1, papers: 0 }).collect();
            for value in &means {
                let at = (value.floor() - f64::from(scale.minimum)).clamp(0.0, (bins.len().max(1) - 1) as f64);
                if let Some(bin) = bins.get_mut(at as usize) {
                    bin.papers += 1;
                }
            }
            VenueRatings {
                venue,
                field,
                scale,
                papers: u32::try_from(count).unwrap_or(u32::MAX),
                mean,
                median,
                lowest: sorted.first().copied().unwrap_or_default(),
                highest: sorted.last().copied().unwrap_or_default(),
                bins,
            }
        })
        .collect()
}

/// What the reviews of a set of `papers` papers come to, from those of them whose reviews were read. The papers
/// handed in may come in any order; the same set gives the same summary, on one thread or many.
pub fn summarise(papers: usize, reviewed: &[Reviewed<'_>]) -> ReviewSummary {
    let mut reviewed: Vec<Reviewed<'_>> = reviewed.to_vec();
    reviewed.sort_by(|a, b| a.paper_id.cmp(b.paper_id));
    reviewed.dedup_by(|a, b| a.paper_id == b.paper_id);
    let figures: Vec<PaperFigures> = reviewed.par_iter().map(paper_figures).collect();
    let tally = reviewed
        .par_iter()
        .enumerate()
        .map(|(place, paper)| tally(u32::try_from(place).unwrap_or(u32::MAX), paper, Voter::Paper))
        .reduce(Tally::default, Tally::merge);
    let with_reviews: Vec<PaperFigures> = figures.into_iter().filter(|paper| paper.reviews > 0).collect();
    let agreement = |paper: &PaperFigures| paper.disagreement.map(|disagreement| disagreement.normalised_spread);
    ReviewSummary {
        papers,
        pages_read: reviewed.len(),
        papers_with_reviews: with_reviews.len(),
        reviews: with_reviews.iter().map(|paper| paper.reviews as usize).sum(),
        ratings: venue_ratings(&with_reviews),
        papers_without_venue: with_reviews.iter().filter(|paper| paper.venue.is_none()).count(),
        most_disagreed: ranked(&with_reviews, agreement, true),
        most_agreed: ranked(&with_reviews, agreement, false),
        highest_rated: ranked(&with_reviews, |paper| paper.overall().map(|overall| overall.normalised_mean), true),
        praise: themes(&tally, Tone::Praise, &reviewed),
        criticism: themes(&tally, Tone::Criticism, &reviewed),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::text::reviews::discussion::fixtures::{
        ICLR_2024, ICML_2025, NEURIPS_2023, NEURIPS_2025, synthetic_page,
    };
    use crate::text::reviews::discussion::read_discussion;

    fn figures(page: &str) -> PaperFigures {
        let discussion = read_discussion(page).unwrap();
        let venue = discussion.venue.clone();
        paper_figures(&Reviewed::new("p", venue.as_ref(), &discussion))
    }

    fn field(figures: &PaperFigures, field: Field) -> FieldFigures {
        figures.fields.iter().find(|figures| figures.field == field).unwrap().clone()
    }

    /// Papers made up for a test: each a page of reviews with the given (rating, confidence).
    fn made_up(reviews: &[Vec<(u8, u8)>]) -> Vec<(String, Discussion)> {
        reviews
            .iter()
            .enumerate()
            .map(|(paper, scores)| {
                (format!("paper-{paper:05}"), read_discussion(&synthetic_page(paper, scores)).unwrap())
            })
            .collect()
    }

    fn summary_of(papers: &[(String, Discussion)]) -> ReviewSummary {
        let reviewed: Vec<Reviewed<'_>> =
            papers.iter().map(|(id, discussion)| Reviewed::new(id, discussion.venue.as_ref(), discussion)).collect();
        summarise(papers.len() * 2, &reviewed)
    }

    #[test]
    fn a_papers_figures_are_on_its_venues_own_scale() {
        let neurips = figures(NEURIPS_2023);
        let rating = field(&neurips, Field::Rating);
        assert_eq!(rating.scale, Scale { minimum: 1, maximum: 10, source: ScaleSource::Published });
        assert_eq!((rating.reviews, rating.mean, rating.median, rating.minimum, rating.maximum), (2, 5.0, 5.0, 4, 6));
        assert_eq!(rating.spread, 1.0);
        assert_eq!(rating.normalised_mean, 4.0 / 9.0);
        // 6 weighed by 4, 4 weighed by 3
        assert_eq!(rating.confidence_weighted, Some(ConfidenceWeighted { mean: 36.0 / 7.0, reviews: 2 }));
        assert_eq!(field(&neurips, Field::Confidence).mean, 3.5);
        let disagreement = neurips.disagreement.unwrap();
        assert_eq!(
            (disagreement.field, disagreement.range, disagreement.normalised_spread),
            (Field::Rating, 2, 1.0 / 9.0)
        );

        let later = figures(NEURIPS_2025);
        assert_eq!(field(&later, Field::Rating).scale.maximum, 6); // the same conference, another year's scale
        assert_eq!(
            field(&later, Field::Quality).scale,
            Scale { minimum: 1, maximum: 4, source: ScaleSource::Published }
        );
        let icml = figures(ICML_2025);
        assert_eq!(icml.overall().unwrap().field, Field::OverallRecommendation);
        assert_eq!(icml.overall().unwrap().scale.maximum, 5);
        assert_eq!(icml.disagreement, None); // one review: nobody to disagree with
        assert_eq!(icml.overall().unwrap().confidence_weighted, None);
    }

    #[test]
    fn a_venue_not_known_here_has_its_scale_said_to_be_assumed() {
        let elsewhere = Venue::new("colm", 2025);
        assert_eq!(scale(Some(&elsewhere), Field::Rating).source, ScaleSource::Assumed);
        assert_eq!(scale(None, Field::Soundness), Scale { minimum: 1, maximum: 4, source: ScaleSource::Assumed });
        assert_eq!(scale(Some(&Venue::new("NeurIPS", 2024)), Field::Rating).source, ScaleSource::Published);
    }

    #[test]
    fn only_the_scales_read_on_the_venues_own_site_are_said_to_be_published() {
        let at = |conference: &str, year: u16, field: Field| scale(Some(&Venue::new(conference, year)), field);
        let published = [
            ("NeurIPS", 2022, Field::Soundness, 1, 4),
            ("NeurIPS", 2024, Field::Rating, 1, 10),
            ("NeurIPS", 2025, Field::Rating, 1, 6),
            ("NeurIPS", 2025, Field::Originality, 1, 4),
            ("NeurIPS", 2023, Field::Confidence, 1, 5),
            ("ICML", 2024, Field::Rating, 1, 10),
            ("ICML", 2024, Field::Contribution, 1, 4),
            ("ICML", 2025, Field::OverallRecommendation, 1, 5),
        ];
        for (conference, year, field, minimum, maximum) in published {
            let scale = at(conference, year, field);
            assert_eq!(
                scale,
                Scale { minimum, maximum, source: ScaleSource::Published },
                "{conference} {year} {field:?}"
            );
        }
        // the same ranges where the venue's own pages do not print them: believed, and said to be assumed
        let assumed = [
            ("ICLR", 2024, Field::Rating, 1, 10),
            ("ICLR", 2026, Field::Rating, 0, 10),
            ("ICLR", 2024, Field::Soundness, 1, 4),
            ("ICLR", 2025, Field::Confidence, 1, 5),
            ("ICML", 2023, Field::Rating, 1, 10),
            ("ICML", 2023, Field::Presentation, 1, 4),
            ("ICML", 2025, Field::Confidence, 1, 5),
            ("NeurIPS", 2021, Field::Rating, 1, 10),
            ("NeurIPS", 2026, Field::Rating, 1, 6),
        ];
        for (conference, year, field, minimum, maximum) in assumed {
            let scale = at(conference, year, field);
            assert_eq!(
                scale,
                Scale { minimum, maximum, source: ScaleSource::Assumed },
                "{conference} {year} {field:?}"
            );
        }
    }

    #[test]
    fn ratings_of_different_scales_are_never_put_together() {
        let pages = [("iclr-2024-1", ICLR_2024), ("neurips-2025-1", NEURIPS_2025), ("neurips-2023-1", NEURIPS_2023)];
        let read: Vec<(&str, Discussion)> =
            pages.iter().map(|(id, page)| (*id, read_discussion(page).unwrap())).collect();
        let venues: Vec<Option<Venue>> = read.iter().map(|(id, discussion)| venue_of(id, discussion)).collect();
        let reviewed: Vec<Reviewed<'_>> = read
            .iter()
            .zip(&venues)
            .map(|((id, discussion), venue)| Reviewed::new(id, venue.as_ref(), discussion))
            .collect();
        let summary = summarise(10, &reviewed);
        assert_eq!((summary.papers, summary.pages_read, summary.papers_with_reviews, summary.reviews), (10, 3, 3, 6));
        let groups: Vec<(String, Field, u8)> =
            summary.ratings.iter().map(|group| (group.venue.to_string(), group.field, group.scale.maximum)).collect();
        assert_eq!(
            groups,
            [
                ("ICLR 2024".into(), Field::Rating, 10),
                ("NeurIPS 2023".into(), Field::Rating, 10),
                ("NeurIPS 2025".into(), Field::Rating, 6)
            ]
        );
        let neurips_2025 = &summary.ratings[2];
        assert_eq!((neurips_2025.mean, neurips_2025.bins.len()), (4.0, 5));
        assert_eq!(neurips_2025.bins[3], Bin { from: 4, to: 5, papers: 1 });
        // compared across venues only on the normalised spread: NeurIPS 2025's 3 and 5 of 6 are the furthest apart
        assert_eq!(summary.most_disagreed[0].paper_id, "neurips-2025-1");
        assert_eq!(summary.most_disagreed[0].normalised, 1.0 / 5.0);
        assert_eq!(summary.most_agreed[0].paper_id, "iclr-2024-1"); // as agreed as NeurIPS 2023's: by id
        assert_eq!(summary.highest_rated[0].paper_id, "iclr-2024-1");
    }

    #[test]
    fn what_reviewers_praise_and_criticise_is_counted_and_points_back_at_their_words() {
        let papers = made_up(&(0..30).map(|paper| vec![(6, 3); 2 + paper % 3]).collect::<Vec<_>>());
        let summary = summary_of(&papers);
        let praised: Vec<&str> = summary.praise.iter().map(|theme| theme.term.as_str()).collect();
        let criticised: Vec<&str> = summary.criticism.iter().map(|theme| theme.term.as_str()).collect();
        assert!(
            praised.iter().any(|term| ["novel", "clear", "theoretical analysis", "code"].contains(term)),
            "{praised:?}"
        );
        assert!(
            criticised.iter().any(|term| ["small models", "baselines", "ablation study"].contains(term)),
            "{criticised:?}"
        );
        assert!(criticised.contains(&"small models") && !criticised.contains(&"small"), "{criticised:?}");
        assert!(praised.iter().all(|term| !criticised.contains(term)));
        for theme in summary.praise.iter().chain(&summary.criticism) {
            assert!(theme.papers >= LEAST_VOTERS && theme.distinctiveness > 1.0);
            assert!(!theme.sources.is_empty() && theme.sources.len() <= SOURCES);
            for source in &theme.sources {
                let (_, discussion) = papers.iter().find(|(id, _)| *id == source.paper_id).unwrap();
                let page = synthetic_page(
                    source.paper_id[6..].parse().unwrap(),
                    &vec![(6, 3); 2 + source.paper_id[6..].parse::<usize>().unwrap() % 3],
                );
                let said: String =
                    page.chars().skip(source.span.start).take(source.span.end - source.span.start).collect();
                assert_eq!(said, source.excerpt);
                assert!(
                    said.to_lowercase().contains(theme.term.split(' ').next().unwrap()),
                    "{said} for {}",
                    theme.term
                );
                assert!(matches!(discussion.notes[source.note].kind, Kind::OfficialReview { .. }));
                assert!(source.section == "Strengths" || source.section == "Weaknesses");
            }
        }
    }

    /// A made-up ICLR-style page of one paper with one review per (reviewer, strengths, weaknesses).
    fn page_of(reviews: &[(&str, &str, &str)]) -> String {
        let mut page = "Go to ICLR 2024 Conference homepage\nPaper 1\n".to_owned();
        for (reviewer, strengths, weaknesses) in reviews {
            page.push_str(&format!(
                "Official Review of Submission1 by Reviewer {reviewer}\nSummary:\nThe paper studies routing in \
                 sparse experts.\nSoundness: 3 good\nStrengths:\n{strengths}\nWeaknesses:\n{weaknesses}\n\
                 Questions:\nNone.\nRating: 6: a rating\nConfidence: 3: a confidence\n"
            ));
        }
        page
    }

    fn in_common_of(page: &str) -> InCommon {
        let discussion = read_discussion(page).unwrap();
        in_common(&Reviewed::new("iclr-2024-1", discussion.venue.as_ref(), &discussion))
    }

    const FOUR_REVIEWS: [(&str, &str, &str); 4] = [
        (
            "aB1c",
            "The routing rule is elegant. The writing is easy to follow.",
            "The baselines are weak and dated. Compute cost is never reported.",
        ),
        (
            "dE2f",
            "An elegant routing rule with a short proof.",
            "Only two benchmarks are tried. The compute cost is not reported.",
        ),
        (
            "gH3i",
            "Strong benchmarks. The routing rule is elegant and cheap.",
            "The notation in the second part is confusing, and the notation is never defined.",
        ),
        ("jK4l", "Thorough experiments on many tasks.", "Weak baselines throughout the experiments."),
    ];

    #[test]
    fn what_most_of_a_papers_reviewers_praise_and_criticise_is_counted_by_reviewer() {
        let common = in_common_of(&page_of(&FOUR_REVIEWS));
        let praised: Vec<(&str, u32, u32)> =
            common.praise.iter().map(|theme| (theme.term.as_str(), theme.reviewers, theme.all_reviewers)).collect();
        // three of four praise the elegant routing rule: the two-word term, not its words again on their own; both
        // are only ever said in praise, so they tie and go alphabetically
        assert_eq!(praised[..2], [("elegant", 3, 4), ("routing rule", 3, 4)], "{praised:?}");
        assert!(praised.iter().all(|(term, _, _)| !["routing", "rule"].contains(term)), "{praised:?}");
        let criticised: Vec<(&str, u32)> =
            common.criticism.iter().map(|theme| (theme.term.as_str(), theme.reviewers)).collect();
        assert!(criticised.contains(&("compute cost", 2)), "{criticised:?}");
        assert!(criticised.contains(&("baselines", 2)), "{criticised:?}");
        // one reviewer saying "notation" twice is not a point in common
        assert!(criticised.iter().all(|(term, _)| !term.contains("notation")), "{criticised:?}");
        for theme in common.praise.iter().chain(&common.criticism) {
            assert!(theme.reviewers >= LEAST_VOTERS);
            assert_eq!(theme.sources.len() as u32, theme.reviewers); // one sentence of each reviewer who says it
        }
    }

    #[test]
    fn a_papers_points_in_common_are_ranked_by_reviewers_then_distinctiveness_then_term() {
        let common = in_common_of(&page_of(&FOUR_REVIEWS));
        for themes in [&common.praise, &common.criticism] {
            for pair in themes.windows(2) {
                let (a, b) = (&pair[0], &pair[1]);
                let in_order = a.reviewers > b.reviewers
                    || (a.reviewers == b.reviewers && a.distinctiveness > b.distinctiveness)
                    || (a.reviewers == b.reviewers && a.distinctiveness == b.distinctiveness && a.term < b.term);
                assert!(in_order, "{} before {}", a.term, b.term);
            }
        }
        // the same answer every time, and whichever order the reviews are read in counts the same reviewers
        let again = in_common_of(&page_of(&FOUR_REVIEWS));
        assert_eq!(serde_json::to_string(&common).unwrap(), serde_json::to_string(&again).unwrap());
        let mut reversed = FOUR_REVIEWS;
        reversed.reverse();
        let backwards = in_common_of(&page_of(&reversed));
        let terms = |common: &InCommon| -> Vec<(String, u32)> {
            common.praise.iter().chain(&common.criticism).map(|theme| (theme.term.clone(), theme.reviewers)).collect()
        };
        assert_eq!(terms(&common), terms(&backwards));
    }

    #[test]
    fn a_points_sources_are_the_reviewers_own_sentences_in_the_kept_page() {
        let page = page_of(&FOUR_REVIEWS);
        let discussion = read_discussion(&page).unwrap();
        let common = in_common(&Reviewed::new("iclr-2024-1", None, &discussion));
        let rule = common.praise.iter().find(|theme| theme.term == "routing rule").unwrap();
        let signers: Vec<Signer> = rule.sources.iter().map(|source| source.signer.clone()).collect();
        let reviewer = |id: &str| Signer::Reviewer { id: id.to_owned() };
        assert_eq!(signers, [reviewer("aB1c"), reviewer("dE2f"), reviewer("gH3i")]);
        assert_eq!(rule.sources[0].excerpt, "The routing rule is elegant.");
        assert_eq!(rule.sources[1].excerpt, "An elegant routing rule with a short proof.");
        for source in common.praise.iter().chain(&common.criticism).flat_map(|theme| &theme.sources) {
            let said: String = page.chars().skip(source.span.start).take(source.span.end - source.span.start).collect();
            assert_eq!(said, source.excerpt);
            let note = &discussion.notes[source.note];
            assert_eq!(note.signer, source.signer);
            assert!(note.sections.iter().any(|section| section.name == source.section && section.text.contains(&said)));
            assert!(source.section == "Strengths" || source.section == "Weaknesses");
        }
    }

    #[test]
    fn a_paper_with_one_review_has_no_points_in_common() {
        let one = in_common_of(&page_of(&FOUR_REVIEWS[..1]));
        assert_eq!(one, InCommon::default());
        // two reviews by the same reviewer are one voice
        let twice = in_common_of(&page_of(&[FOUR_REVIEWS[0], ("aB1c", FOUR_REVIEWS[0].1, FOUR_REVIEWS[0].2)]));
        assert_eq!(twice, InCommon::default());
    }

    #[test]
    fn a_joint_strengths_and_weaknesses_section_is_split_by_the_reviewers_own_headings() {
        let discussion = read_discussion(NEURIPS_2025).unwrap();
        let section = &discussion.notes[1].sections[1];
        let tones: Vec<Option<Tone>> = parts(1, section).iter().map(|part| part.tone).collect();
        assert_eq!(tones, [None, Some(Tone::Praise), Some(Tone::Criticism)]);
        let inline = Section {
            name: "Strengths And Weaknesses".into(),
            text: "Pros: thorough. Cons: thin theory.".into(),
            span: Span { start: 0, end: 34 },
        };
        let split: Vec<(Option<Tone>, &str)> =
            parts(0, &inline).iter().map(|part| (part.tone, part.text.trim())).collect();
        assert_eq!(split, [(None, ""), (Some(Tone::Praise), "thorough."), (Some(Tone::Criticism), "thin theory.")]);
    }

    #[test]
    fn the_summary_is_the_same_on_one_thread_and_on_many_and_in_any_order() {
        let papers = made_up(
            &(0..400)
                .map(|paper| vec![((paper % 10) as u8 + 1, (paper % 5) as u8 + 1); 1 + paper % 4])
                .collect::<Vec<_>>(),
        );
        let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap().install(|| summary_of(&papers));
        let many = rayon::ThreadPoolBuilder::new().num_threads(8).build().unwrap().install(|| summary_of(&papers));
        let mut reversed = papers.clone();
        reversed.reverse();
        let backwards = summary_of(&reversed);
        let text = serde_json::to_string(&one).unwrap();
        assert_eq!(text, serde_json::to_string(&many).unwrap());
        assert_eq!(text, serde_json::to_string(&backwards).unwrap());
    }

    #[test]
    fn a_panic_in_one_worker_reaches_the_caller_and_the_pool_goes_on() {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pool.install(|| {
                (0..64)
                    .into_par_iter()
                    .map(|paper| if paper == 17 { panic!("one paper failed") } else { paper })
                    .sum::<i32>()
            })
        }));
        assert!(failed.is_err());
        let papers = made_up(&[vec![(6, 3), (8, 4)], vec![(3, 2)]]);
        assert_eq!(pool.install(|| summary_of(&papers)).papers_with_reviews, 2);
    }

    /// The timing reported for the reviews of a large run: `cargo test --release -- --ignored --nocapture
    /// reviews_of_twenty_thousand`.
    #[test]
    #[ignore]
    fn reviews_of_twenty_thousand_papers_are_summarised_on_every_core() {
        let papers = made_up(
            &(0..20_000)
                .map(|paper| (0..4).map(|review| (((paper + review) % 10) as u8 + 1, (review % 5) as u8 + 1)).collect())
                .collect::<Vec<_>>(),
        );
        let time = |threads: usize| {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            pool.install(|| summary_of(&papers)); // warmed up
            let started = std::time::Instant::now();
            let summary = pool.install(|| summary_of(&papers));
            (started.elapsed(), serde_json::to_string(&summary).unwrap())
        };
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        let (alone, one) = time(1);
        let (together, all) = time(cores);
        assert_eq!(one, all);
        println!("20,000 papers, 4 reviews each: 1 thread {alone:?}, {cores} threads {together:?}");
    }
}
