//! What is read from the text of a paper or a page: plain functions of the text, which fetch nothing and keep
//! nothing, so that what is read can be tested on every layout that was once read wrongly.
//!
//! | module           | what it reads                                                                  |
//! |------------------|--------------------------------------------------------------------------------|
//! | [`declarations`] | what authors declare in a paper: the NeurIPS checklist, statements, flags      |
//! | [`passages`]     | a paper's text made addressable; where a quote is in it; the sentences citing a reference |
//! | [`pdf`]          | the text of a paper's PDF, in reading order, whole or page by page             |
//! | [`references`]   | the works a paper cites, from its reference list                               |
//! | [`reviews`]      | the scores reviewers gave, from the text of a reviews page                     |
//!
//! Positions and lengths in the readers count characters, not bytes, as the Python readers they replace did:
//! a statement is cut after 1,500 characters whatever script it is written in.

pub mod declarations;
pub mod passages;
pub mod pdf;
pub mod references;
pub mod reviews;

use regex::Regex;

#[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
fn pattern(source: &str) -> Regex {
    Regex::new(source).unwrap()
}

/// The words of `text` with one space between them: text from a PDF or a page wraps and spaces at random.
fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first `count` characters of `text` (all of it when it is shorter).
fn first_chars(text: &str, count: usize) -> &str {
    text.char_indices().nth(count).map_or(text, |(end, _)| &text[..end])
}

/// The length in bytes of the character at `at`, so that a search can move on by one character.
fn char_len_at(text: &str, at: usize) -> usize {
    text[at..].chars().next().map_or(1, char::len_utf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_cut_by_characters_not_bytes() {
        assert_eq!(first_chars("naïve", 3), "naï");
        assert_eq!(first_chars("short", 40), "short");
        assert_eq!(squash("  one\n two\t\tthree "), "one two three");
        assert_eq!(char_len_at("aï", 1), 2);
    }
}
