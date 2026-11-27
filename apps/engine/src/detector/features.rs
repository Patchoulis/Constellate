//! Words and word pairs, weighted as scikit-learn's `TfidfVectorizer` weighs them with the settings the Python
//! detector used (`lowercase=True, ngram_range=(1, 2), min_df=3, sublinear_tf=True, max_features=60_000,
//! token_pattern=r"(?u)\b[a-zA-Z][a-zA-Z'\-]+\b"`, the rest left at scikit-learn's defaults: `smooth_idf=True`,
//! `norm="l2"`, `max_df=1.0`).
//!
//! * **Words.** The text is lower-cased, then a word is an ASCII letter followed by one or more ASCII letters,
//!   apostrophes or hyphens, with a word boundary on both sides. A boundary is where a "word character" (a letter
//!   or digit of any script, or `_`) meets anything else, as Python's `re` decides it; the pattern is matched left
//!   to right, the longest match that ends on a boundary first, as Python's backtracking does. So "café" gives no
//!   word (it does not end on a boundary after an ASCII letter), "don't-" gives "don't", "x2" gives none.
//! * **Features.** Every word, then every pair of neighbouring words joined by a space.
//! * **Vocabulary.** Features found in at least [`MIN_DF`] documents; when there are more than [`MAX_FEATURES`],
//!   those counted most often across all documents (ties: in alphabetical order, where numpy's sort leaves the
//!   order unspecified), then sorted alphabetically (by code point, as Python sorts strings).
//! * **Weights.** A feature counted `n` times in a document weighs `(1 + ln n) * idf`, with
//!   `idf = ln((1 + documents) / (1 + documents with it)) + 1`; each document's weights are then divided by their
//!   Euclidean length.
//!
//! One known difference from Python's `re`: Rust's `char::is_alphanumeric` counts a few combining marks as
//! letters (Unicode's Other_Alphabetic: some Indic vowel signs, U+0345) that Python does not. It can only matter
//! for a word that touches one of them, which English abstracts do not have.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The version of the features: raised whenever anything here changes what a text is turned into, which makes a
/// kept classifier stale ([`super::Availability::Stale`]).
pub const FEATURES: u32 = 1;
/// A feature must be in at least this many documents to be in the vocabulary.
pub const MIN_DF: usize = 3;
/// The vocabulary's largest size.
pub const MAX_FEATURES: usize = 60_000;

/// A document as weights: (column, weight), by column.
pub type Row = Vec<(u32, f64)>;

/// Python's `\w` in a `str` pattern: a letter or a digit of any script, or `_`.
fn is_word(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

fn inside(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '\'' || c == '-'
}

/// The words of `text`, lower-cased, as `(?u)\b[a-zA-Z][a-zA-Z'\-]+\b` finds them.
pub fn words(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.to_lowercase().chars().collect();
    let boundary = |at: usize| {
        let before = at > 0 && is_word(chars[at - 1]);
        let after = at < chars.len() && is_word(chars[at]);
        before != after
    };
    let mut found = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        if chars[start].is_ascii_alphabetic() && boundary(start) {
            let mut end = start + 1;
            while end < chars.len() && inside(chars[end]) {
                end += 1;
            }
            // the longest match first, as the greedy `+` backtracks; it takes at least two characters
            if let Some(stop) = (start + 2..=end).rev().find(|&stop| boundary(stop)) {
                found.push(chars[start..stop].iter().collect());
                start = stop;
                continue;
            }
        }
        start += 1;
    }
    found
}

/// The features of `text`: its words, then its pairs of neighbouring words.
pub fn features(text: &str) -> Vec<String> {
    let words = words(text);
    let pairs = words.windows(2).map(|pair| format!("{} {}", pair[0], pair[1]));
    let mut features = words.clone();
    features.extend(pairs);
    features
}

/// How many times each feature is in `text`.
fn counted(text: &str) -> HashMap<String, u32> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    for feature in features(text) {
        *counts.entry(feature).or_default() += 1;
    }
    counts
}

/// The vocabulary and the weights learnt from the training documents.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vectorizer {
    /// Sorted: a feature's column is its place here.
    pub terms: Vec<String>,
    #[serde(with = "super::classifier::exact")]
    pub idf: Vec<f64>,
    #[serde(skip)]
    columns: HashMap<String, u32>,
}

impl Vectorizer {
    /// Learn the vocabulary of `documents`, and give each of them its row.
    pub fn fit(documents: &[&str]) -> (Self, Vec<Row>) {
        use rayon::prelude::*;
        // each document counted on its own thread; collected in order, so the result is the same on any count
        let counts: Vec<HashMap<String, u32>> = documents.par_iter().map(|text| counted(text)).collect();
        let mut documents_with: HashMap<&str, (usize, u64)> = HashMap::new();
        for document in &counts {
            for (feature, &times) in document {
                let entry = documents_with.entry(feature.as_str()).or_default();
                entry.0 += 1;
                entry.1 += u64::from(times);
            }
        }
        let mut kept: Vec<(&str, usize, u64)> = documents_with
            .into_iter()
            .filter(|(_, (with, _))| *with >= MIN_DF)
            .map(|(term, (with, total))| (term, with, total))
            .collect();
        if kept.len() > MAX_FEATURES {
            kept.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(b.0)));
            kept.truncate(MAX_FEATURES);
        }
        kept.sort_by(|a, b| a.0.cmp(b.0));
        let n = documents.len() as f64;
        let idf = kept.iter().map(|(_, with, _)| ((1.0 + n) / (1.0 + *with as f64)).ln() + 1.0).collect();
        let terms: Vec<String> = kept.iter().map(|(term, ..)| (*term).to_owned()).collect();
        let vectorizer = Self::with(terms, idf);
        let rows = counts.iter().map(|document| vectorizer.weigh(document)).collect();
        (vectorizer, rows)
    }

    /// A vectorizer kept before, made ready to use.
    pub fn with(terms: Vec<String>, idf: Vec<f64>) -> Self {
        let columns = terms.iter().enumerate().map(|(column, term)| (term.clone(), column as u32)).collect();
        Self { terms, idf, columns }
    }

    /// The columns again after being read back (they are not kept).
    pub fn ready(self) -> Self {
        Self::with(self.terms, self.idf)
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    pub fn column(&self, term: &str) -> Option<u32> {
        self.columns.get(term).copied()
    }

    /// The row of a text not seen in training: features outside the vocabulary are left out.
    pub fn transform(&self, text: &str) -> Row {
        self.weigh(&counted(text))
    }

    fn weigh(&self, counts: &HashMap<String, u32>) -> Row {
        let mut row: Row = counts
            .iter()
            .filter_map(|(feature, &times)| {
                let column = self.column(feature)?;
                Some((column, (1.0 + f64::from(times).ln()) * self.idf[column as usize]))
            })
            .collect();
        row.sort_by_key(|(column, _)| *column);
        let length = row.iter().map(|(_, weight)| weight * weight).sum::<f64>().sqrt();
        if length > 0.0 {
            for (_, weight) in &mut row {
                *weight /= length;
            }
        }
        row
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn words_are_found_as_pythons_pattern_finds_them() {
        let found = words("We've a Well-known CNN, naïve café x2 don't- e-commerce 'quoted' i.e. A_b ab3 -dash it's.");
        assert_eq!(
            found,
            ["we've", "well-known", "cnn", "don't", "e-commerce", "quoted", "dash", "it's"],
            "one-letter words (\"i.e.\" is two), words that run into a digit, an underscore or a non-ASCII letter \
             are not words"
        );
        assert_eq!(words("state-of-the-art--x"), ["state-of-the-art--x"]);
        assert_eq!(words("ÉCOLE Schrödinger"), Vec::<String>::new());
    }

    #[test]
    fn features_are_the_words_then_their_neighbouring_pairs() {
        assert_eq!(features("A cat sat, the cat"), ["cat", "sat", "the", "cat", "cat sat", "sat the", "the cat"]);
    }

    #[test]
    fn the_vocabulary_keeps_features_of_three_documents_and_rows_have_unit_length() {
        let documents = ["red fox jumps", "red fox sleeps", "red fox jumps high", "blue whale"];
        let (vectorizer, rows) = Vectorizer::fit(&documents);
        assert_eq!(vectorizer.terms, ["fox", "red", "red fox"]);
        // every term in 3 of 4 documents: ln(5/4) + 1
        assert!((vectorizer.idf[0] - ((5.0f64 / 4.0).ln() + 1.0)).abs() < 1e-15);
        assert!(rows[3].is_empty(), "a document with no word of the vocabulary has no weights");
        let length: f64 = rows[0].iter().map(|(_, weight)| weight * weight).sum();
        assert!((length - 1.0).abs() < 1e-12);
        assert_eq!(vectorizer.transform("Red fox, red fox!"), vectorizer.transform("red fox red fox"));
    }
}
