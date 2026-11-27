//! The words of every paper, counted once, to find the terms that set any group of papers apart: a community's
//! "keywords".
//!
//! The counting is the Python's (scikit-learn's `CountVectorizer` as `rp/describe.py` set it up), so that a run
//! made again lists the same terms: the text is a paper's title, a space and its abstract, in lower case; a token
//! is a letter followed by two or more letters or hyphens (`\b[a-zA-Z][a-zA-Z\-]{2,}\b`); scikit-learn's English
//! stop words are dropped; the terms are the tokens and the pairs of tokens next to each other once stop words
//! are gone; a term must be in at least two papers, and at most [`MOST_TERMS`] are kept (the most frequent).
//!
//! A group's terms are those whose share of the group's words is high and whose share of the words around it
//! (all papers, or its parent community's) is lower: each term's contribution to the divergence of the two word
//! distributions, `inside * ln(inside / around)`.
//!
//! Where two terms weigh exactly the same, the one first in alphabetical order comes first. NumPy's `argsort`,
//! which the Python used, leaves such ties in no particular order for long lists; the result is otherwise the
//! same.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::LazyLock;

use rayon::prelude::*;
use regex::Regex;

/// The most terms kept, the most frequent first (scikit-learn's `max_features` as the Python set it).
pub const MOST_TERMS: usize = 200_000;
/// The fewest papers a term must be in.
const LEAST_PAPERS: u32 = 2;
/// Added to both shares so that a term absent from either side has a finite weight (as in the Python).
const TINY: f64 = 1e-12;

#[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[a-zA-Z][a-zA-Z\-]{2,}\b").unwrap());

/// scikit-learn's English stop words (`ENGLISH_STOP_WORDS`), in alphabetical order.
const STOP_WORDS: [&str; 318] = [
    "a",
    "about",
    "above",
    "across",
    "after",
    "afterwards",
    "again",
    "against",
    "all",
    "almost",
    "alone",
    "along",
    "already",
    "also",
    "although",
    "always",
    "am",
    "among",
    "amongst",
    "amoungst",
    "amount",
    "an",
    "and",
    "another",
    "any",
    "anyhow",
    "anyone",
    "anything",
    "anyway",
    "anywhere",
    "are",
    "around",
    "as",
    "at",
    "back",
    "be",
    "became",
    "because",
    "become",
    "becomes",
    "becoming",
    "been",
    "before",
    "beforehand",
    "behind",
    "being",
    "below",
    "beside",
    "besides",
    "between",
    "beyond",
    "bill",
    "both",
    "bottom",
    "but",
    "by",
    "call",
    "can",
    "cannot",
    "cant",
    "co",
    "con",
    "could",
    "couldnt",
    "cry",
    "de",
    "describe",
    "detail",
    "do",
    "done",
    "down",
    "due",
    "during",
    "each",
    "eg",
    "eight",
    "either",
    "eleven",
    "else",
    "elsewhere",
    "empty",
    "enough",
    "etc",
    "even",
    "ever",
    "every",
    "everyone",
    "everything",
    "everywhere",
    "except",
    "few",
    "fifteen",
    "fifty",
    "fill",
    "find",
    "fire",
    "first",
    "five",
    "for",
    "former",
    "formerly",
    "forty",
    "found",
    "four",
    "from",
    "front",
    "full",
    "further",
    "get",
    "give",
    "go",
    "had",
    "has",
    "hasnt",
    "have",
    "he",
    "hence",
    "her",
    "here",
    "hereafter",
    "hereby",
    "herein",
    "hereupon",
    "hers",
    "herself",
    "him",
    "himself",
    "his",
    "how",
    "however",
    "hundred",
    "i",
    "ie",
    "if",
    "in",
    "inc",
    "indeed",
    "interest",
    "into",
    "is",
    "it",
    "its",
    "itself",
    "keep",
    "last",
    "latter",
    "latterly",
    "least",
    "less",
    "ltd",
    "made",
    "many",
    "may",
    "me",
    "meanwhile",
    "might",
    "mill",
    "mine",
    "more",
    "moreover",
    "most",
    "mostly",
    "move",
    "much",
    "must",
    "my",
    "myself",
    "name",
    "namely",
    "neither",
    "never",
    "nevertheless",
    "next",
    "nine",
    "no",
    "nobody",
    "none",
    "noone",
    "nor",
    "not",
    "nothing",
    "now",
    "nowhere",
    "of",
    "off",
    "often",
    "on",
    "once",
    "one",
    "only",
    "onto",
    "or",
    "other",
    "others",
    "otherwise",
    "our",
    "ours",
    "ourselves",
    "out",
    "over",
    "own",
    "part",
    "per",
    "perhaps",
    "please",
    "put",
    "rather",
    "re",
    "same",
    "see",
    "seem",
    "seemed",
    "seeming",
    "seems",
    "serious",
    "several",
    "she",
    "should",
    "show",
    "side",
    "since",
    "sincere",
    "six",
    "sixty",
    "so",
    "some",
    "somehow",
    "someone",
    "something",
    "sometime",
    "sometimes",
    "somewhere",
    "still",
    "such",
    "system",
    "take",
    "ten",
    "than",
    "that",
    "the",
    "their",
    "them",
    "themselves",
    "then",
    "thence",
    "there",
    "thereafter",
    "thereby",
    "therefore",
    "therein",
    "thereupon",
    "these",
    "they",
    "thick",
    "thin",
    "third",
    "this",
    "those",
    "though",
    "three",
    "through",
    "throughout",
    "thru",
    "thus",
    "to",
    "together",
    "too",
    "top",
    "toward",
    "towards",
    "twelve",
    "twenty",
    "two",
    "un",
    "under",
    "until",
    "up",
    "upon",
    "us",
    "very",
    "via",
    "was",
    "we",
    "well",
    "were",
    "what",
    "whatever",
    "when",
    "whence",
    "whenever",
    "where",
    "whereafter",
    "whereas",
    "whereby",
    "wherein",
    "whereupon",
    "wherever",
    "whether",
    "which",
    "while",
    "whither",
    "who",
    "whoever",
    "whole",
    "whom",
    "whose",
    "why",
    "will",
    "with",
    "within",
    "without",
    "would",
    "yet",
    "you",
    "your",
    "yours",
    "yourself",
    "yourselves",
];

fn is_stop_word(token: &str) -> bool {
    STOP_WORDS.binary_search(&token).is_ok()
}

/// The terms of one text: its tokens without stop words, then each pair of neighbours among them.
fn terms_of(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let tokens: Vec<&str> =
        TOKEN.find_iter(&lower).map(|found| found.as_str()).filter(|token| !is_stop_word(token)).collect();
    let mut terms: Vec<String> = tokens.iter().map(|token| (*token).to_owned()).collect();
    terms.extend(tokens.windows(2).map(|pair| format!("{} {}", pair[0], pair[1])));
    terms
}

/// Every paper's term counts, and the share each term has of all the words.
#[derive(Clone, Debug, PartialEq)]
pub struct Vocabulary {
    /// The terms, in alphabetical order; a term's index is its number.
    terms: Vec<String>,
    /// Per paper: (term number, count), by term number.
    per_paper: Vec<Vec<(u32, u32)>>,
    /// Each term's share of every paper's words.
    overall: Vec<f64>,
}

impl Vocabulary {
    /// The vocabulary of these texts (a paper's title, a space, its abstract), one per paper. Each text is read on
    /// whichever thread rayon gives it; what is counted from them does not depend on which.
    pub fn new(texts: &[String]) -> Self {
        let read: Vec<Vec<String>> = texts.par_iter().map(|text| terms_of(text)).collect();
        // term -> (papers it is in, times it is used)
        let mut seen: HashMap<&str, (u32, u64)> = HashMap::new();
        for terms in &read {
            let mut counted: Vec<&str> = terms.iter().map(String::as_str).collect();
            counted.sort_unstable();
            for (index, term) in counted.iter().enumerate() {
                let entry = seen.entry(term).or_insert((0, 0));
                entry.1 += 1;
                if index == 0 || counted[index - 1] != *term {
                    entry.0 += 1;
                }
            }
        }
        let mut kept: Vec<(&str, u64)> = seen
            .into_iter()
            .filter(|(_, (papers, _))| *papers >= LEAST_PAPERS)
            .map(|(term, (_, used))| (term, used))
            .collect();
        if kept.len() > MOST_TERMS {
            kept.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            kept.truncate(MOST_TERMS);
        }
        kept.sort_unstable_by(|a, b| a.0.cmp(b.0));
        let terms: Vec<String> = kept.iter().map(|(term, _)| (*term).to_owned()).collect();
        let number: HashMap<&str, u32> =
            terms.iter().enumerate().map(|(index, term)| (term.as_str(), index as u32)).collect();
        let per_paper: Vec<Vec<(u32, u32)>> = read
            .par_iter()
            .map(|terms| {
                let mut numbers: Vec<u32> =
                    terms.iter().filter_map(|term| number.get(term.as_str()).copied()).collect();
                numbers.sort_unstable();
                let mut counts: Vec<(u32, u32)> = Vec::new();
                for term in numbers {
                    match counts.last_mut() {
                        Some((last, count)) if *last == term => *count += 1,
                        _ => counts.push((term, 1)),
                    }
                }
                counts
            })
            .collect();
        let everyone: Vec<usize> = (0..per_paper.len()).collect();
        let mut vocabulary = Self { terms, per_paper, overall: Vec::new() };
        vocabulary.overall = vocabulary.share(&everyone);
        vocabulary
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Each term's share of the words of `papers`; all zeros for papers with no words.
    fn share(&self, papers: &[usize]) -> Vec<f64> {
        let mut counts = vec![0.0_f64; self.terms.len()];
        for &paper in papers {
            for &(term, count) in &self.per_paper[paper] {
                counts[term as usize] += f64::from(count);
            }
        }
        let total: f64 = counts.iter().sum::<f64>().max(1.0);
        counts.iter_mut().for_each(|count| *count /= total);
        counts
    }

    /// The `top` terms that set `group` apart from the papers of `within` (all papers when none), most telling
    /// first.
    pub fn keywords(&self, group: &[usize], within: Option<&[usize]>, top: usize) -> Vec<String> {
        let inside = self.share(group);
        let around_within;
        let around = match within {
            Some(within) => {
                around_within = self.share(within);
                &around_within
            }
            None => &self.overall,
        };
        let weights: Vec<f64> = inside
            .iter()
            .zip(around)
            .map(|(inside, around)| inside * ((inside + TINY) / (around + TINY)).ln())
            .collect();
        // heaviest first; equal weights (-0 and 0 among them) in the terms' order
        let heavier =
            |a: &usize, b: &usize| weights[*b].partial_cmp(&weights[*a]).unwrap_or(Ordering::Equal).then(a.cmp(b));
        let mut order: Vec<usize> = (0..weights.len()).collect();
        let top = top.min(order.len());
        if top == 0 {
            return Vec::new();
        }
        if top < order.len() {
            order.select_nth_unstable_by(top - 1, heavier);
            order.truncate(top);
        }
        order.sort_unstable_by(heavier);
        order.into_iter().map(|term| self.terms[term].clone()).collect()
    }

    /// The weight `term` has for `group` within `within` (what [`Vocabulary::keywords`] orders by), if it is a term.
    pub fn weight(&self, group: &[usize], within: Option<&[usize]>, term: &str) -> Option<f64> {
        let number = self.number(term)?;
        let inside = self.share(group)[number];
        let around = match within {
            Some(within) => self.share(within)[number],
            None => self.overall[number],
        };
        Some(inside * ((inside + TINY) / (around + TINY)).ln())
    }

    /// Each paper's term counts, for a tool that scores how coherent a community's terms are (examples/compare).
    pub fn counts(&self, paper: usize) -> &[(u32, u32)] {
        &self.per_paper[paper]
    }

    /// The number of a term, if it is one.
    pub fn number(&self, term: &str) -> Option<usize> {
        self.terms.binary_search_by(|known| known.as_str().cmp(term)).ok()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_stop_words_are_in_order_so_that_they_can_be_looked_up() {
        assert!(STOP_WORDS.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(is_stop_word("the") && is_stop_word("yourselves") && !is_stop_word("graph"));
    }

    #[test]
    fn terms_are_long_enough_tokens_without_stop_words_and_their_neighbouring_pairs() {
        assert_eq!(
            terms_of("The Graph-based method of an x-ray, 3d and GNNs"),
            ["graph-based", "method", "x-ray", "gnns", "graph-based method", "method x-ray", "x-ray gnns"]
        );
        // a pair is made across a stop word, once the stop word is gone (as scikit-learn does)
        assert_eq!(terms_of("neural and network"), ["neural", "network", "neural network"]);
    }

    fn texts(pairs: &[(&str, usize)]) -> Vec<String> {
        pairs.iter().flat_map(|(text, times)| std::iter::repeat_n((*text).to_owned(), *times)).collect()
    }

    #[test]
    fn characteristic_terms_are_the_ones_a_group_has_and_others_lack() {
        // the Python's test: six graph papers, six diffusion papers
        let vocabulary = Vocabulary::new(&texts(&[
            ("graph neural network message passing on graphs", 6),
            ("diffusion model denoising score sampling", 6),
        ]));
        let graph_terms = vocabulary.keywords(&[0, 1, 2, 3, 4, 5], None, 4);
        assert!(graph_terms.iter().any(|term| term == "graph" || term == "graphs"), "{graph_terms:?}");
        assert!(!graph_terms.iter().any(|term| term == "diffusion" || term == "denoising"));
        // against the graph papers alone, a graph paper's words are no different: every weight is zero, and the
        // terms come in alphabetical order
        let within: Vec<usize> = (0..6).collect();
        let inside = vocabulary.share(&[0, 1, 2]);
        let around = vocabulary.share(&within);
        assert!(inside.iter().zip(&around).all(|(a, b)| (a - b).abs() < 1e-15));
    }

    #[test]
    fn a_term_in_one_paper_only_is_not_a_term() {
        let vocabulary = Vocabulary::new(&texts(&[("shared words here", 2), ("lonely", 1)]));
        assert_eq!(vocabulary.number("lonely"), None);
        assert!(vocabulary.number("shared words").is_some());
    }

    #[test]
    fn no_words_at_all_give_no_terms() {
        let vocabulary = Vocabulary::new(&texts(&[("a b", 3)]));
        assert!(vocabulary.is_empty());
        assert!(vocabulary.keywords(&[0, 1], None, 12).is_empty());
    }

    #[test]
    fn the_vocabulary_is_the_same_on_one_thread_and_on_many() {
        let many: Vec<String> = (0..300)
            .map(|i| format!("topic{} word{} common words appear {}", i % 7, i % 13, "again ".repeat(i % 3)))
            .collect();
        let (one, other) = crate::threads::on_one_and_many(|| {
            let vocabulary = Vocabulary::new(&many);
            let terms = vocabulary.keywords(&(0..40).collect::<Vec<_>>(), None, 12);
            (vocabulary, terms)
        });
        assert_eq!(one, other);
    }
}
