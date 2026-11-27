//! Words as the lexical matchers compare them: the passages index of cited answers ([`super::retrieval`]), the
//! related passages of an explanation (`explanation::related`) and the stances' pre-filter
//! (`analysis::stances`). One way to cut text into words, one way to fold a plural, and one list of words that
//! say nothing about what a text is about, so that "studies" and "Studies" and "studíes" meet in all three.
//!
//! What stays each matcher's own, and why: the shortest word kept (the index keeps two-letter words and single
//! digits, "3D", "k 2", since a question may name them; the other two need three letters, as they score shared
//! words and a short one is noise), the words each adds to [`STOP_WORDS`] (the pre-filter also drops "improve",
//! "better", "effect", which every question of whether something helps uses), and the pre-filter's cutting of
//! "-ing" and "-ed", which suits a yes/no question's verbs but would merge distinct terms in a passages index.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Words that match nearly every passage and say nothing about what it is about: in every matcher's list.
pub const STOP_WORDS: [&str; 44] = [
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "do", "does", "for", "from", "has", "have", "how", "in",
    "into", "is", "it", "its", "of", "on", "or", "our", "paper", "that", "the", "their", "them", "these", "they",
    "this", "to", "use", "using", "was", "we", "what", "when", "which", "while", "with", "why",
];

/// The words of `text`, in order (repeats kept): cut at every character that is not a letter or a digit, in
/// lower case, accents dropped.
pub fn split(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    for c in text.nfkd().filter(|c| !is_combining_mark(*c)) {
        if c.is_alphanumeric() {
            word.extend(c.to_lowercase());
        } else if !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// A plural folded to its singular, roughly: "methods" and "method", "studies" and "study"; "class", "corpus"
/// and "analysis" are not plurals.
pub fn singular(word: &str) -> String {
    let length = word.chars().count();
    if length > 4 && word.ends_with("ies") {
        return format!("{}y", &word[..word.len() - 3]);
    }
    if length > 3 && word.ends_with('s') && !word.ends_with("ss") && !word.ends_with("us") && !word.ends_with("is") {
        return word[..word.len() - 1].to_owned();
    }
    word.to_owned()
}

/// Is `word` (as [`split`] gives it) one of [`STOP_WORDS`]?
pub fn is_stop_word(word: &str) -> bool {
    STOP_WORDS.contains(&word)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_cut_at_what_is_not_a_letter_or_digit_in_lower_case_without_accents() {
        assert_eq!(split("Étude: GPT-4's 3D studíes."), ["etude", "gpt", "4", "s", "3d", "studies"]);
        assert!(split("  — ").is_empty());
    }

    #[test]
    fn a_plural_meets_its_singular_and_a_word_ending_in_s_that_is_not_one_stays() {
        for (word, folded) in [
            ("methods", "method"),
            ("studies", "study"),
            ("ties", "tie"),
            ("class", "class"),
            ("corpus", "corpus"),
            ("analysis", "analysis"),
            ("gas", "gas"),
        ] {
            assert_eq!(singular(word), folded, "{word}");
        }
    }

    #[test]
    fn the_shared_stop_words_are_in_lower_case_and_found() {
        assert!(STOP_WORDS.iter().all(|word| *word == word.to_lowercase()));
        assert!(is_stop_word("paper") && !is_stop_word("papers") && !is_stop_word("model"));
    }
}
