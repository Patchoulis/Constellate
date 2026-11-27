//! Passages of other papers that are about the same thing as a passage: found by the words they share, with no
//! model and nothing fetched, so it costs nothing and can be asked as often as the reader likes.
//!
//! Which papers are searched is the caller's choice (the route takes the paper's neighbours in the run's graph,
//! then the rest of its community, at most [`super::NEARBY_PAPERS`]): this module scores what it is given. Each
//! paper is searched by its passages when its PDF was read, by its abstract when not.
//!
//! The score is a plain weighted overlap: each word of the passage asked about that a candidate passage also has
//! counts by how rare it is among the candidates (inverse document frequency) and how often the candidate uses
//! it, over the square root of the candidate's length, so that a long paragraph does not win by being long. It is
//! used only to order the results; the interface shows the shared words, never the number.
//!
//! A small matcher of its own on purpose: a passage index for answering questions over many papers (BM25) is
//! being built elsewhere, and the two could later share one index.
//!
//! The papers are scored on every core (rayon); the result does not depend on how many there are: every count is
//! an integer summed in any order, every score is computed from one paper's words in one fixed order, and ties
//! are broken by paper and place.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use rayon::prelude::*;

use crate::text::passages::{Address, Kind, Passages, chars_of, sentence_spans};
use crate::text::words::{is_stop_word, singular, split};

/// The most results told, over all papers.
pub const MOST_RESULTS: usize = 10;
/// The most results from one paper: the others' passages are as much worth seeing.
pub const PER_PAPER: usize = 2;
/// Fewest distinct words a passage must share with the one asked about to be told.
pub const FEWEST_SHARED: usize = 2;
/// The most shared words shown with a result.
pub const WORDS_SHOWN: usize = 6;

/// Words too common to say what a passage is about, besides [`crate::text::words::STOP_WORDS`].
const STOPWORDS: &str = "the and for are with that this from which these those have has had was were been being \
    can could would should may might must will shall not but its their them they than then there here where when \
    what who how all any each both such into onto over under between about also only more most other some one \
    two three our ours you your his her use used using show shows shown see per via while whose does did done \
    very much many same well however thus therefore since because paper work";

/// What a candidate paper is searched by.
#[derive(Clone, Copy, Debug)]
pub enum Searched<'a> {
    /// Its passages, read from its PDF.
    Passages(&'a Passages),
    /// Its abstract, when its PDF was not read.
    Abstract(&'a str),
}

/// A paper to search.
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    pub paper: &'a str,
    pub text: Searched<'a>,
}

/// Where a result is: a sentence of a read paper, or one of its abstract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// The sentence of the passage that shares most with the one asked about, at its address.
    Sentence(Address),
    /// The sentence of the abstract that does, as written there.
    Abstract(String),
}

/// A passage of another paper about the same thing.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub paper: String,
    pub place: Place,
    /// The words it shares with the passage asked about, the rarest first.
    pub words: Vec<String>,
    /// What it was ordered by; not told to the interface.
    pub score: f64,
}

fn common(word: &str) -> bool {
    is_stop_word(word) || STOPWORDS.split_whitespace().any(|common| common == word)
}

/// The words of `text` that say what it is about, as compared ([`crate::text::words`]: lower case, accents and a
/// plural's "s" dropped), in order (repeats kept). A word is common as written ("this", "does") or once its
/// plural's "s" is dropped ("shows").
pub fn words_of(text: &str) -> Vec<String> {
    split(text)
        .into_iter()
        .filter(|word| word.chars().count() >= 3 && !word.chars().all(|c| c.is_ascii_digit()))
        .filter(|word| !common(word))
        .map(|word| singular(&word))
        .filter(|word| !common(word))
        .collect()
}

/// A stretch of a candidate scored as one: a passage, or an abstract.
struct Unit {
    /// Which of the candidate's passages it is (0 for an abstract).
    index: usize,
    /// How often it uses each word.
    counts: BTreeMap<String, u32>,
    length: usize,
}

fn unit(index: usize, text: &str) -> Unit {
    let words = words_of(text);
    let mut counts = BTreeMap::new();
    for word in &words {
        *counts.entry(word.clone()).or_insert(0) += 1;
    }
    Unit { index, counts, length: words.len() }
}

/// Is a passage of this kind searched? Running text and captions say what a paper is about; headings, the text
/// inside figures, footnotes and equations (whose extracted text is rarely words) do not.
fn searched(kind: Kind) -> bool {
    matches!(kind, Kind::Text | Kind::FigureCaption | Kind::TableCaption)
}

fn units_of(candidate: &Candidate<'_>) -> Vec<Unit> {
    match candidate.text {
        Searched::Passages(passages) => passages
            .passages
            .iter()
            .enumerate()
            .filter(|(_, passage)| searched(passage.kind))
            .map(|(index, passage)| unit(index, &passage.text))
            .collect(),
        Searched::Abstract(text) => vec![unit(0, text)],
    }
}

/// How rare a word is among `units` of all candidates, as a weight: rarer counts more.
fn rarity(units: usize, with: usize) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let weight = (1.0 + units as f64 / (1.0 + with as f64)).ln();
    weight
}

/// The sentence of `text` that has most of `weights`' words, by weight: what is quoted for a result.
fn best_sentence(text: &str, weights: &BTreeMap<String, f64>) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let score = |(from, to): &(usize, usize)| -> f64 {
        let sentence: String = chars[*from..*to].iter().collect();
        let words: BTreeSet<String> = words_of(&sentence).into_iter().collect();
        words.iter().filter_map(|word| weights.get(word)).sum()
    };
    sentence_spans(text)
        .into_iter()
        .map(|span| (score(&span), span))
        // the first of the best, whatever the floating point does with equal sums
        .fold(None, |best: Option<(f64, (usize, usize))>, (value, span)| match best {
            Some((top, _)) if value <= top => best,
            _ => Some((value, span)),
        })
        .map_or((0, chars.len()), |(_, span)| span)
}

/// The passages of `candidates` that share most words with `query` (a passage's text), the best first: at most
/// [`PER_PAPER`] of one paper and [`MOST_RESULTS`] in all, each sharing at least [`FEWEST_SHARED`] words.
pub fn related(query: &str, candidates: &[Candidate<'_>]) -> Vec<Found> {
    let mut asked: BTreeMap<String, u32> = BTreeMap::new();
    for word in words_of(query) {
        *asked.entry(word).or_insert(0) += 1;
    }
    if asked.is_empty() {
        return Vec::new();
    }
    let units: Vec<Vec<Unit>> = candidates.par_iter().map(units_of).collect();
    // in how many units each word asked about is: integers, so the sum is the same in any order
    let words: Vec<&String> = asked.keys().collect();
    let with: Vec<usize> = units
        .par_iter()
        .map(|units| {
            words
                .iter()
                .map(|word| units.iter().filter(|unit| unit.counts.contains_key(*word)).count())
                .collect::<Vec<usize>>()
        })
        .reduce(|| vec![0; words.len()], |a, b| a.iter().zip(&b).map(|(x, y)| x + y).collect());
    let all: usize = units.iter().map(Vec::len).sum();
    let weights: BTreeMap<String, f64> =
        words.iter().zip(&with).map(|(word, with)| ((*word).clone(), rarity(all, *with))).collect();

    let mut found: Vec<(usize, Found)> = candidates
        .par_iter()
        .zip(units.par_iter())
        .enumerate()
        .flat_map_iter(|(order, (candidate, units))| {
            let mut scored: Vec<(f64, &Unit, Vec<&String>)> = units
                .iter()
                .filter_map(|unit| {
                    let shared: Vec<&String> = asked.keys().filter(|word| unit.counts.contains_key(*word)).collect();
                    if shared.len() < FEWEST_SHARED {
                        return None;
                    }
                    let sum: f64 = shared
                        .iter()
                        .map(|word| {
                            let used = f64::from(unit.counts[*word]);
                            let asked_times = f64::from(asked[*word]);
                            weights[*word] * weights[*word] * (1.0 + used.ln()) * (1.0 + asked_times.ln())
                        })
                        .sum();
                    #[allow(clippy::cast_precision_loss)]
                    let length = (unit.length.max(1) as f64).sqrt();
                    Some((sum / length, unit, shared))
                })
                .collect();
            scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.index.cmp(&b.1.index)));
            scored.truncate(PER_PAPER);
            scored
                .into_iter()
                .map(|(score, unit, mut shared)| {
                    shared.sort_by(|a, b| weights[*b].total_cmp(&weights[*a]).then(a.cmp(b)));
                    let words: Vec<String> = shared.into_iter().take(WORDS_SHOWN).cloned().collect();
                    let place = place_of(candidate, unit.index, &weights);
                    (order, Found { paper: candidate.paper.to_owned(), place, words, score })
                })
                .collect::<Vec<_>>()
        })
        .collect();
    found.sort_by(|(a_order, a), (b_order, b)| {
        b.score.total_cmp(&a.score).then(a.paper.cmp(&b.paper)).then(a_order.cmp(b_order)).then(
            match (&a.place, &b.place) {
                (Place::Sentence(x), Place::Sentence(y)) => (x.page, x.start).cmp(&(y.page, y.start)),
                _ => Ordering::Equal,
            },
        )
    });
    found.truncate(MOST_RESULTS);
    found.into_iter().map(|(_, found)| found).collect()
}

/// The place quoted for a result: its best sentence.
fn place_of(candidate: &Candidate<'_>, index: usize, weights: &BTreeMap<String, f64>) -> Place {
    match candidate.text {
        Searched::Passages(passages) => {
            let passage = &passages.passages[index];
            let (from, to) = best_sentence(&passage.text, weights);
            let to_u32 = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
            Place::Sentence(Address {
                start: passage.address.start.saturating_add(to_u32(from)),
                end: passage.address.start.saturating_add(to_u32(to)),
                ..passage.address.clone()
            })
        }
        Searched::Abstract(text) => {
            let (from, to) = best_sentence(text, weights);
            let chars = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
            let to_u32 = |value: usize| u32::try_from(value).unwrap_or(chars);
            Place::Abstract(chars_of(text, to_u32(from), to_u32(to)))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::text::passages::read_passages;

    fn read(text: &str) -> Passages {
        read_passages(&[text.to_owned()])
    }

    const ASKED: &str = "We merge task vectors by rescaling the singular values of the shared directions, which \
                         merging counts twice.";

    fn papers() -> Vec<(String, Passages)> {
        vec![
            (
                "b".to_owned(),
                read(
                    "1. Method\n\nOur distillation trains a small student from a large teacher.\n\nThe shared \
                      directions of task vectors are inflated: their singular values grow with each merged task. \
                      We rescale them.\n",
                ),
            ),
            (
                "a".to_owned(),
                read(
                    "2. Analysis\n\nSingular values of merged task vectors count shared directions twice. \
                      Rescaling the singular values fixes it.\n\nWe evaluate on eight vision tasks.\n",
                ),
            ),
            ("c".to_owned(), read("1. Introduction\n\nProtein folding is predicted from sequences alone.\n")),
        ]
    }

    fn candidates<'a>(papers: &'a [(String, Passages)], abstracts: &'a [(String, String)]) -> Vec<Candidate<'a>> {
        papers
            .iter()
            .map(|(paper, passages)| Candidate { paper, text: Searched::Passages(passages) })
            .chain(abstracts.iter().map(|(paper, text)| Candidate { paper, text: Searched::Abstract(text) }))
            .collect()
    }

    #[test]
    fn passages_about_the_same_thing_are_found_with_the_words_they_share_and_unrelated_ones_are_not() {
        let papers = papers();
        let abstracts = vec![(
            "d".to_owned(),
            "We study cats. Merging task vectors inflates the singular values of shared directions.".to_owned(),
        )];
        let found = related(ASKED, &candidates(&papers, &abstracts));
        let named: Vec<&str> = found.iter().map(|found| found.paper.as_str()).collect();
        assert!(!named.contains(&"c"), "protein folding shares nothing: {named:?}");
        assert_eq!(found.len(), 3, "{found:?}");
        for result in &found {
            assert!(result.words.len() >= FEWEST_SHARED && result.words.len() <= WORDS_SHOWN);
            assert!(result.words.iter().all(|word| words_of(ASKED).contains(word)), "{:?}", result.words);
        }
        let a = found.iter().find(|found| found.paper == "a").unwrap();
        let Place::Sentence(address) = &a.place else { panic!("a was read") };
        let page = &papers[1].1.pages[0].text;
        assert_eq!(
            chars_of(page, address.start, address.end),
            "Singular values of merged task vectors count shared directions twice."
        );
        let d = found.iter().find(|found| found.paper == "d").unwrap();
        assert_eq!(
            d.place,
            Place::Abstract("Merging task vectors inflates the singular values of shared directions.".to_owned())
        );
    }

    #[test]
    fn the_results_are_bounded_per_paper_and_in_all() {
        let paragraph = "The singular values of shared directions in merged task vectors are rescaled.";
        let many: String = (0..20).map(|n| format!("{paragraph} Case {n}.\n\n")).collect();
        let papers: Vec<(String, Passages)> =
            (0..12).map(|n| (format!("p{n:02}"), read(&format!("1. Method\n\n{many}")))).collect();
        let found = related(ASKED, &candidates(&papers, &[]));
        assert_eq!(found.len(), MOST_RESULTS);
        for paper in &papers {
            assert!(found.iter().filter(|found| found.paper == paper.0).count() <= PER_PAPER);
        }
        assert!(related("the and of", &candidates(&papers, &[])).is_empty(), "nothing to look for");
    }

    #[test]
    fn the_results_are_the_same_whatever_the_number_of_threads() {
        let paragraph = |n: usize| {
            format!("Merged task vectors {n} inflate singular values of shared directions in layer {}.", n % 7)
        };
        let papers: Vec<(String, Passages)> = (0..40)
            .map(|n| {
                let text: String = (0..(n % 5 + 1)).map(|k| format!("{}\n\n", paragraph(n * 3 + k))).collect();
                (format!("paper-{}", (n * 17) % 40), read(&format!("1. Method\n\n{text}")))
            })
            .collect();
        let abstracts = vec![("z".to_owned(), "Shared directions of merged task vectors.".to_owned())];
        let run = |threads: usize| {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            pool.install(|| related(ASKED, &candidates(&papers, &abstracts)))
        };
        let one = run(1);
        assert!(!one.is_empty());
        for threads in [2, 4, 8] {
            assert_eq!(run(threads), one, "{threads} threads");
        }
    }

    #[test]
    fn words_are_compared_without_case_plurals_or_common_words() {
        assert_eq!(words_of("The Models use 2024 vectors, and Studies of α-βs!"), ["model", "vector", "study"]);
        // a common word that ends in "s" is common before its "s" is dropped: "this" is not the word "thi"
        assert_eq!(words_of("This does show merging; thus it shows these."), ["merging"]);
    }

    #[test]
    fn two_passages_sharing_only_common_words_are_not_related() {
        let papers = vec![("b".to_owned(), read("1. Method\n\nThis is what this does, as it does this.\n"))];
        assert!(related("This rule does this for routing.", &candidates(&papers, &[])).is_empty());
    }
}
