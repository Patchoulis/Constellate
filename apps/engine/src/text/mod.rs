//! What is read from the text of a paper or a page: plain functions of the text, which fetch nothing and keep
//! nothing, so that what is read can be tested on every layout that was once read wrongly.
//!
//! | module           | what it reads                                                                  |
//! |------------------|--------------------------------------------------------------------------------|
//! | [`declarations`] | what authors declare in a paper: the NeurIPS checklist, statements, flags      |
//! | [`passages`]     | a paper's text made addressable; where a quote is in it; the sentences citing a reference |
//! | [`pdf`]          | the text of a paper's PDF, in reading order, whole or page by page             |
//! | [`references`]   | the works a paper cites, from its reference list                               |
//! | [`retrieval`]    | where a run's or a community's papers talk about something: a lexical index    |
//! | [`review_points`] | what each reviewer raised, point by point, with the authors' replies          |
//! | [`reviews`]      | the scores reviewers gave, from the text of a reviews page                     |
//! | [`discussion`]   | the whole thread of a reviews page: reviews by section, replies, the decision  |
//!
//! Positions and lengths in the readers count characters, not bytes, as the Python readers they replace did:
//! a statement is cut after 1,500 characters whatever script it is written in.

pub mod declarations;
pub mod discussion;
pub mod passages;
pub mod pdf;
pub mod references;
pub mod retrieval;
pub mod review_points;
pub mod reviews;

use regex::Regex;

#[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
fn pattern(source: &str) -> Regex {
    Regex::new(source).unwrap()
}

/// The words of `text` with one space between them: text from a PDF or a page wraps and spaces at random.
pub(crate) fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first `count` characters of `text` (all of it when it is shorter).
pub(crate) fn first_chars(text: &str, count: usize) -> &str {
    text.char_indices().nth(count).map_or(text, |(end, _)| &text[..end])
}

/// `at`, or the character boundary just before it.
pub(crate) fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The length in bytes of the character at `at`, so that a search can move on by one character.
fn char_len_at(text: &str, at: usize) -> usize {
    text[at..].chars().next().map_or(1, char::len_utf8)
}

/// Byte offsets of one text turned into character offsets, for many positions without counting from the start
/// each time: the count of characters is kept at every [`CharIndex::STEP`] bytes, and only the bytes since the
/// last of those are counted. A character starts at every byte that is not a UTF-8 continuation byte, so the
/// count is right wherever a step falls.
pub(crate) struct CharIndex<'a> {
    bytes: &'a [u8],
    /// Characters starting before byte `i * STEP`.
    counted: Vec<usize>,
}

impl<'a> CharIndex<'a> {
    const STEP: usize = 4096;

    pub(crate) fn new(text: &'a str) -> Self {
        let bytes = text.as_bytes();
        let mut counted = Vec::with_capacity(bytes.len() / Self::STEP + 1);
        let mut so_far = 0;
        counted.push(0);
        for block in bytes.chunks(Self::STEP) {
            so_far += starts(block);
            counted.push(so_far);
        }
        Self { bytes, counted }
    }

    /// The character offset of byte offset `byte` (a character boundary, or the end).
    pub(crate) fn at(&self, byte: usize) -> usize {
        let byte = byte.min(self.bytes.len());
        let block = byte / Self::STEP;
        self.counted[block] + starts(&self.bytes[block * Self::STEP..byte])
    }
}

fn starts(bytes: &[u8]) -> usize {
    bytes.iter().filter(|byte| (**byte & 0xC0) != 0x80).count()
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

    #[test]
    fn a_byte_offset_is_turned_into_the_character_offset_wherever_it_lies() {
        let text = "ïé€😀a".repeat(3000); // two, two, three and four bytes: steps fall inside characters
        let index = CharIndex::new(&text);
        for (characters, (byte, _)) in text.char_indices().enumerate().step_by(97) {
            assert_eq!(index.at(byte), characters);
        }
        assert_eq!(index.at(text.len()), text.chars().count());
        assert_eq!(CharIndex::new("").at(0), 0);
    }
}
