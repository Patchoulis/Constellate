//! Where a paper says what its symbols and terms mean: for a passage, each symbol ("α", "∆Wi") and each
//! capitalised term ("LoRA", "TIES") it uses, and the sentence of the paper that introduces it ("where α denotes
//! the learning rate", "Low-Rank Adaptation (LoRA)", "we call this TIES").
//!
//! It reads the paper's passages only: no model, nothing fetched. What it finds is useful by itself (the reader
//! shows it as "defined here:" with links to those sentences) and is handed to the model with the passage, so
//! that an explanation of an equation uses the paper's own meanings of its symbols rather than guessed ones.
//!
//! Where it looks, in this order, the first sentence found winning:
//!
//! 1. the passages before the one explained, from the start of the paper, since a term is introduced before it
//!    is used;
//! 2. the passage itself;
//! 3. the passage just after it when that one starts with "where": what papers write under a displayed equation
//!    ("where ∆Wi is the update of task i").
//!
//! A sentence introduces a term when the term stands in it as a word of its own and is next to one of the cues
//! papers use for that: "T denotes", "T is the", "T stands for", "T :=" after it; "where T", "let T", "we
//! define T", "called T", "denoted by T" before it; or "(T)" right after the words it abbreviates. It is a
//! program's reading: the interface says so.

use serde::Serialize;

use crate::text::pdf::passages::{Address, Kind, Passage, Passages, chars_of};

/// Terms of one passage looked up: an equation has a handful of symbols, a paragraph a few acronyms.
pub const MOST_TERMS: usize = 24;
/// Characters around a term looked at for a cue.
const WINDOW: usize = 48;
/// The longest symbol or term taken from a passage, in characters.
const LONGEST_TERM: usize = 12;

/// Letters of the alphabets equations are written in, and the operators that are part of a symbol's name.
const GREEK: &str = "αβγδεζηθικλμνξοπρστυφχψωΓΔΘΛΞΠΣΥΦΨΩ∆∇∂ϵϑϕℓ";

/// What sort of term it is: a symbol of mathematics, or a name the paper gives something.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TermKind {
    /// "α", "∆Wi", or a short name of a variable in an equation ("W").
    Symbol,
    /// A capitalised term: an acronym or a method's name ("LoRA", "TIES", "BM25").
    Name,
}

/// A symbol or a term a passage uses.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Term {
    pub kind: TermKind,
    pub text: String,
}

/// Where the paper introduces a term: the sentence, at its place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Definition {
    pub term: Term,
    /// The sentence's place in the paper: what the evidence link opens.
    pub address: Address,
    /// The sentence, as the paper's text has it.
    pub sentence: String,
}

fn is_greek(c: char) -> bool {
    GREEK.contains(c)
}

/// A token stripped of the punctuation around it ("(α," is "α").
fn trimmed(token: &str) -> &str {
    token.trim_matches(|c: char| !(c.is_alphanumeric() || is_greek(c)))
}

/// Is `token` a capitalised term: two capitals or more, letters, digits and hyphens only, not a number?
fn is_name(token: &str) -> bool {
    let capitals = token.chars().filter(|c| c.is_uppercase()).count();
    capitals >= 2
        && token.chars().all(|c| c.is_alphanumeric() || c == '-')
        && token.chars().any(char::is_alphabetic)
        && !token.chars().any(is_greek)
}

/// Is `token` a symbol: it has a Greek letter (or ∆, ∇, ∂) in it, or, in an equation, it is a short name of
/// letters and digits ("W", "Wi", "x1").
fn is_symbol(token: &str, in_equation: bool) -> bool {
    let greek = token.chars().any(is_greek);
    let short = token.chars().count() <= 3
        && token.chars().all(char::is_alphanumeric)
        && token.chars().next().is_some_and(char::is_alphabetic);
    greek || (in_equation && short)
}

/// The symbols and capitalised terms `passage` uses, in the order they first come, each once, at most
/// [`MOST_TERMS`].
pub fn terms_of(passage: &Passage) -> Vec<Term> {
    let in_equation = passage.kind == Kind::Equation;
    let mut terms: Vec<Term> = Vec::new();
    for token in passage.text.split_whitespace().map(trimmed) {
        if token.is_empty() || token.chars().count() > LONGEST_TERM {
            continue;
        }
        let kind = if is_symbol(token, in_equation) {
            TermKind::Symbol
        } else if is_name(token) {
            TermKind::Name
        } else {
            continue;
        };
        if !terms.iter().any(|known| known.text == token) {
            terms.push(Term { kind, text: token.to_owned() });
        }
        if terms.len() == MOST_TERMS {
            break;
        }
    }
    terms
}

/// What comes right after a term that introduces it.
const AFTER: [&str; 21] = [
    "denotes",
    "denote ",
    "is defined",
    "are defined",
    "is called",
    "stands for",
    "stand for",
    "refers to",
    "represents",
    "represent ",
    "is the ",
    "is a ",
    "is an ",
    "are the ",
    "be the ",
    "be a ",
    "be an ",
    ":=",
    "≜",
    "≡",
    "means ",
];
/// What comes right before a term that introduces it.
const BEFORE: [&str; 16] = [
    "where",
    "let",
    "we define",
    "define",
    "defined as",
    "called",
    "named",
    "termed",
    "dubbed",
    "we call",
    "known as",
    "referred to as",
    "denoted",
    "denoted by",
    "denoted as",
    "denote by",
];

/// Does the occurrence of a term at `at..at + length` (characters of `sentence`) introduce it?
fn introduces(sentence: &[char], at: usize, length: usize) -> bool {
    let end = at + length;
    let before: String = sentence[at.saturating_sub(WINDOW)..at].iter().collect::<String>().to_lowercase();
    let after: String = sentence[end..(end + WINDOW).min(sentence.len())].iter().collect::<String>().to_lowercase();
    let (before, after) = (before.trim_end(), after.trim_start());
    // "Low-Rank Adaptation (LoRA)": the term in brackets right after what it abbreviates
    let bracketed = before.ends_with('(') && after.starts_with(')') && before.len() > 1;
    let after_cue = AFTER.iter().any(|cue| after.starts_with(cue));
    let before_cue = BEFORE.iter().any(|cue| {
        before.strip_suffix(cue).is_some_and(|rest| rest.is_empty() || rest.ends_with(|c: char| !c.is_alphanumeric()))
    });
    // "we denote the update by ∆W"
    let denoted_by = before.ends_with(" by") && before.contains("denot");
    bracketed || after_cue || before_cue || denoted_by
}

/// Is a character one that a term can run into ("α" in "αt" is not the term "α")?
fn joins(c: char) -> bool {
    c.is_alphanumeric() || is_greek(c) || c == '_'
}

/// Where `term` stands as a word of its own in `sentence` and is introduced there.
fn introduced_in(sentence: &[char], term: &[char]) -> bool {
    if term.is_empty() || term.len() > sentence.len() {
        return false;
    }
    (0..=sentence.len() - term.len()).any(|at| {
        sentence[at..at + term.len()] == *term
            && (at == 0 || !joins(sentence[at - 1]))
            && sentence.get(at + term.len()).is_none_or(|c| !joins(*c))
            && introduces(sentence, at, term.len())
    })
}

/// The sentences of `passage`, each with its place.
fn sentences(passage: &Passage) -> impl Iterator<Item = (Address, String)> + '_ {
    passage.sentences.iter().map(move |span| {
        let from = span.start.saturating_sub(passage.address.start);
        let to = span.end.saturating_sub(passage.address.start);
        let address =
            Address { start: span.start, end: span.end, end_page: passage.address.page, ..passage.address.clone() };
        (address, chars_of(&passage.text, from, to))
    })
}

/// May a definition be read from a passage of this kind? Headings and the text inside figures are not sentences.
fn searched(kind: Kind) -> bool {
    !matches!(kind, Kind::Title | Kind::Heading | Kind::Fragment)
}

/// The passages looked in for the definitions of passage `index`'s terms, in the order they are looked in.
fn looked_in(passages: &Passages, index: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..index).collect();
    order.push(index);
    if let Some(next) = passages.passages.get(index + 1) {
        if next.text.trim_start().to_lowercase().starts_with("where") {
            order.push(index + 1);
        }
    }
    order
}

/// For each term passage `index` uses, the first sentence of the paper that introduces it, in the order the
/// terms come in the passage. Terms the paper does not introduce anywhere it looks are left out.
pub fn definitions(passages: &Passages, index: usize) -> Vec<Definition> {
    let Some(passage) = passages.passages.get(index) else { return Vec::new() };
    let terms = terms_of(passage);
    let mut found: Vec<Option<(Address, String)>> = vec![None; terms.len()];
    let spelled: Vec<Vec<char>> = terms.iter().map(|term| term.text.chars().collect()).collect();
    for at in looked_in(passages, index) {
        let place = &passages.passages[at];
        if !searched(place.kind) {
            continue;
        }
        for (address, sentence) in sentences(place) {
            let letters: Vec<char> = sentence.chars().collect();
            for (slot, term) in found.iter_mut().zip(&spelled) {
                if slot.is_none() && introduced_in(&letters, term) {
                    *slot = Some((address.clone(), sentence.clone()));
                }
            }
        }
        if found.iter().all(Option::is_some) {
            break;
        }
    }
    terms
        .into_iter()
        .zip(found)
        .filter_map(|(term, found)| found.map(|(address, sentence)| Definition { term, address, sentence }))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::text::pdf::passages::read_passages;

    /// Two pages of a paper: an abstract that introduces an acronym, a method section that introduces symbols
    /// in several ways, a displayed equation and the "where" paragraph under it.
    fn paper() -> Passages {
        read_passages(&[
            "Spectral Merging\n\nAbstract\n\nWe study Low-Rank Adaptation (LoRA) for merging models. Our method, \
which we call SCM, rescales singular values.\n\n1. Introduction\n\nMerging is cheap. Let α be the merging \
coefficient used for every task. LoRA is popular.\n"
                .to_owned(),
            "2. Method\n\nWe denote the update of the merged model by ∆W and keep the rest fixed. The rank r is \
small.\n\n∆W = α ∑ i ∆Wi + λ (1)\n\nwhere ∆Wi is the update of task i and λ stands for a small correction \
term.\n\nWe apply SCM with LoRA and α to every layer.\n"
                .to_owned(),
        ])
    }

    fn index_of(passages: &Passages, start: &str) -> usize {
        passages.passages.iter().position(|passage| passage.text.starts_with(start)).unwrap()
    }

    fn defined(passages: &Passages, index: usize) -> Vec<(String, String)> {
        definitions(passages, index).into_iter().map(|found| (found.term.text, found.sentence)).collect()
    }

    #[test]
    fn an_equations_symbols_are_found_where_the_paper_introduces_them_before_and_just_under_it() {
        let passages = paper();
        let equation = index_of(&passages, "∆W =");
        assert_eq!(passages.passages[equation].kind, Kind::Equation);
        let found = defined(&passages, equation);
        let of = |term: &str| found.iter().find(|(text, _)| text == term).map(|(_, sentence)| sentence.as_str());
        assert_eq!(of("∆W"), Some("We denote the update of the merged model by ∆W and keep the rest fixed."));
        assert_eq!(of("α"), Some("Let α be the merging coefficient used for every task."));
        assert_eq!(of("∆Wi"), Some("where ∆Wi is the update of task i and λ stands for a small correction term."));
        assert_eq!(of("λ"), of("∆Wi"), "both in the sentence under the equation");
        // the symbols come in the order the equation has them; "i" is never introduced
        let terms: Vec<&str> = found.iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(terms, ["∆W", "α", "∆Wi", "λ"]);
    }

    #[test]
    fn a_capitalised_term_is_found_where_it_is_abbreviated_or_named_and_the_first_place_wins() {
        let passages = paper();
        let later = index_of(&passages, "We apply SCM");
        let found = defined(&passages, later);
        assert_eq!(
            found,
            [
                ("SCM".to_owned(), "Our method, which we call SCM, rescales singular values.".to_owned()),
                ("LoRA".to_owned(), "We study Low-Rank Adaptation (LoRA) for merging models.".to_owned()),
                ("α".to_owned(), "Let α be the merging coefficient used for every task.".to_owned()),
            ]
        );
    }

    #[test]
    fn a_term_that_is_only_used_is_not_said_to_be_defined() {
        let passages = paper();
        // "LoRA is popular." comes after the abstract introduced it: the abstract's sentence is the definition
        let introduction = index_of(&passages, "Merging is cheap");
        let found = defined(&passages, introduction);
        assert_eq!(found.iter().map(|(term, _)| term.as_str()).collect::<Vec<_>>(), ["α", "LoRA"]);
        // a symbol inside a longer token is not that symbol: "αt" does not define "α"
        assert!(!introduced_in(&"where αt is the rate".chars().collect::<Vec<_>>(), &['α']));
        assert!(introduced_in(&"where α is the rate".chars().collect::<Vec<_>>(), &['α']));
        // a word that merely ends in a cue is not the cue: "parallel α" is not "let α"
        assert!(!introduced_in(&"a parallel α update".chars().collect::<Vec<_>>(), &['α']));
    }

    #[test]
    fn the_terms_of_a_passage_are_its_symbols_and_names_each_once_and_bounded() {
        let passages = paper();
        let equation = &passages.passages[index_of(&passages, "∆W =")];
        let terms = terms_of(equation);
        assert!(terms.iter().all(|term| term.kind == TermKind::Symbol));
        assert_eq!(terms.iter().map(|term| term.text.as_str()).collect::<Vec<_>>(), ["∆W", "α", "i", "∆Wi", "λ"]);
        let many: String = (0..100).map(|n| format!("ABC{n} ")).collect();
        let long = Passage { text: many, ..equation.clone() };
        assert_eq!(terms_of(&long).len(), MOST_TERMS);
        // in prose, short words are not symbols; capitalised words of one capital are not names
        let prose =
            Passage { kind: Kind::Text, text: "We use it on BERT and The data.".to_owned(), ..equation.clone() };
        assert_eq!(terms_of(&prose), [Term { kind: TermKind::Name, text: "BERT".to_owned() }]);
    }

    #[test]
    fn every_definition_is_a_sentence_at_its_place_in_the_papers_text() {
        let passages = paper();
        for index in 0..passages.passages.len() {
            for found in definitions(&passages, index) {
                let page = &passages.pages[found.address.page as usize - 1];
                assert_eq!(chars_of(&page.text, found.address.start, found.address.end), found.sentence);
            }
        }
        assert!(definitions(&passages, 999).is_empty());
    }
}
