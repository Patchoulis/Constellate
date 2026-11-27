//! Where one paper of the library cites another: the sentence it does it in, the sentence on either side, the part
//! of the paper it is in, and whether the cited paper is named alone or in a group. Read from the paper's own text
//! with no language model, so it costs nothing and needs no key.
//!
//! The citing sentences come from [`citing_sentences`] (the reference list's entries and the sentences whose
//! markers point at them); the entries are matched to the library's papers by the citation network's [`Linker`].
//! What is added here is what a reader needs to judge a citation for themselves:
//!
//! - **The sentences around it.** scite's "citation statement" is the citing sentence with the one before and
//!   the one after; so is a [`Context`] here. A neighbour is only taken from the same section, and from running
//!   text (or the same caption): a sentence of a footnote is not the next sentence of a paragraph.
//! - **The section, normalised** ([`Section`]): "4.2 Baselines" under "4 Experiments" is in the experiments. The
//!   section's top heading is tried first, then its own heading, then it is [`Section::Other`] with the title.
//! - **Alone or in a group** ([`Grouping`]): "[3, 7-9]" or "(Ilharco et al., 2022; Ruan et al., 2025)" says less
//!   about each work in it than a citation of one work does.
//!
//! [`PaperContexts`] is what is kept by paper (reading a PDF's reference list again takes a second): every entry of
//! the reference list that the text cites, with its contexts, so that it can be linked again when the library
//! grows ([`PaperContexts::relink`]).

pub mod kinds;
pub mod routes;
pub mod store;

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::analysis::citations::{Link, Linker};
use crate::text::passages::{Address, Kind, Marker, Part, Passages, chars_of, citing_sentences};
use crate::text::references::{Citing, Reference, ReferenceList};

/// The version of what is read here. Raise it when a context is read differently: kept contexts are read again.
pub const CONTEXTS: u32 = 1;
/// Sentences kept per reference: a work cited more often than this in one paper is a survey's subject, and the
/// first this many say how.
pub const MOST_CONTEXTS: usize = 20;

/// The part of a paper a citation is in, from the heading of its section.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Section {
    Abstract,
    Introduction,
    RelatedWork,
    /// Background, preliminaries, the problem's setting.
    Background,
    /// The paper's own method, model or approach.
    Method,
    /// Experiments, results, evaluation, ablations.
    Experiments,
    /// Discussion, limitations, conclusions.
    Discussion,
    Appendix,
    /// A heading none of the above words name ("3 Spectral Calibration"), as the paper writes it.
    Other {
        title: String,
    },
}

/// Words of a heading that say which part of a paper it opens, tried in this order.
const SECTION_WORDS: [(&[&str], Section); 7] = [
    (&["abstract"], Section::Abstract),
    (&["introduction", "motivation"], Section::Introduction),
    (&["related work", "prior work", "previous work", "literature", "related research"], Section::RelatedWork),
    (
        &[
            "background",
            "preliminar",
            "problem setup",
            "problem setting",
            "problem statement",
            "problem formulation",
            "notation",
        ],
        Section::Background,
    ),
    (
        &[
            "experiment",
            "result",
            "evaluation",
            "empirical",
            "benchmark",
            "ablation",
            "baseline",
            "implementation detail",
        ],
        Section::Experiments,
    ),
    (&["discussion", "conclusion", "limitation", "future work", "broader impact", "summary"], Section::Discussion),
    (&["method", "approach", "model", "framework", "algorithm", "our ", "proposed", "architecture"], Section::Method),
];

/// A heading's numbering: "3", "4.2.", "B.1".
static NUMBERING: LazyLock<Regex> = LazyLock::new(|| pattern(r"^(?:\d{1,2}|[A-H])(?:\.\d{1,2})*\.?\s+"));
/// A subsection's heading: "4.2 Baselines", "B.1 More results".
static SUBSECTION: LazyLock<Regex> = LazyLock::new(|| pattern(r"^(?:\d{1,2}|[A-H])\.\d"));

#[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
fn pattern(source: &str) -> Regex {
    Regex::new(source).unwrap()
}

/// The part of a paper a heading opens, when its words say so.
fn named_section(heading: &str) -> Option<Section> {
    let words = NUMBERING.replace(heading.trim(), "").to_lowercase();
    if words.is_empty() {
        return None;
    }
    SECTION_WORDS
        .iter()
        .find(|(names, _)| names.iter().any(|name| words.contains(name)))
        .map(|(_, section)| section.clone())
}

/// The part of the paper a sentence is in: from its top heading, else its own (sub)heading, else the heading as
/// written. Anything in the appendix is the appendix.
pub fn section_of(part: Part, top: &str, heading: &str) -> Section {
    if part == Part::Appendix {
        return Section::Appendix;
    }
    named_section(top)
        .or_else(|| named_section(heading))
        .unwrap_or_else(|| Section::Other { title: if heading.is_empty() { top } else { heading }.to_owned() })
}

/// Whether a work is cited on its own or with others in one citation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Grouping {
    Alone,
    /// "[3, 7-9]": cited together with `with` other works.
    Grouped {
        with: usize,
    },
}

/// A sentence next to a citing sentence, at its place in the paper.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Neighbour {
    pub address: Address,
    pub sentence: String,
}

/// One citation of a work in a paper: the sentence, the sentences around it, where it is and how it is written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    /// The citing sentence's place: what [`crate::evidence::Evidence::at`] makes its evidence from.
    pub address: Address,
    pub sentence: String,
    pub before: Option<Neighbour>,
    pub after: Option<Neighbour>,
    pub section: Section,
    pub grouping: Grouping,
    pub marker: Marker,
    /// The citation as the sentence writes it: "[3, 7-9]", "Vaswani et al., 2017".
    pub written: String,
}

impl Context {
    /// The citing sentence and its neighbours, the sentence first: where the words that justify a reading of the
    /// citation must be found ([`crate::features::citation_contexts::kinds`]).
    pub fn window(&self) -> impl Iterator<Item = (&Address, &str)> {
        let own = std::iter::once((&self.address, self.sentence.as_str()));
        let around = [&self.before, &self.after].into_iter().flatten();
        own.chain(around.map(|neighbour| (&neighbour.address, neighbour.sentence.as_str())))
    }
}

/// An entry of the paper's reference list that its text cites, what it was linked to, and its citations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CitedEntry {
    /// Its position in the reference list, from 0.
    pub reference: usize,
    pub entry: Reference,
    pub link: Link,
    pub contexts: Vec<Context>,
}

/// Every citation a paper's text makes of its reference list: what is kept by paper.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaperContexts {
    pub citing: Citing,
    /// Entries of the reference list (cited in the text or not); 0 when no list was found.
    pub listed: usize,
    /// The entries the text cites, in the order listed.
    pub entries: Vec<CitedEntry>,
}

impl PaperContexts {
    /// Link every entry again, against the library as it is now (`own` is the paper itself, never its own
    /// citation). Says whether anything changed.
    pub fn relink(&mut self, linker: &Linker, own: &str) -> bool {
        let mut changed = false;
        for entry in &mut self.entries {
            let link = own_left_out(linker.link(&entry.entry), own);
            if link != entry.link {
                entry.link = link;
                changed = true;
            }
        }
        changed
    }

    /// The entries that are papers of the library, by the paper's id; a paper listed twice has both entries'
    /// citations, each sentence once.
    pub fn in_library(&self) -> Vec<(String, Vec<&Context>)> {
        let mut by_paper: Vec<(String, Vec<&Context>)> = Vec::new();
        for entry in &self.entries {
            let Link::InLibrary { paper } = &entry.link else { continue };
            let at = match by_paper.iter().position(|(known, _)| known == paper) {
                Some(at) => at,
                None => {
                    by_paper.push((paper.clone(), Vec::new()));
                    by_paper.len() - 1
                }
            };
            let contexts = &mut by_paper[at].1;
            for context in &entry.contexts {
                if !contexts.iter().any(|known| known.address == context.address) {
                    contexts.push(context);
                }
            }
        }
        by_paper
    }
}

/// A paper is never its own citation (a preprint listed in its own proceedings' reference list).
fn own_left_out(link: Link, own: &str) -> Link {
    match link {
        Link::InLibrary { paper } if paper == own => Link::Unresolved,
        link => link,
    }
}

/// A sentence of the paper as citing sentences are found in it, with what it needs to find its neighbours.
struct Sentence {
    address: Address,
    text: String,
    passage: usize,
    kind: Kind,
    part: Part,
    /// The heading of the top section it is in ("4 Experiments" for a sentence under "4.2 Baselines").
    top: String,
}

/// Every sentence citations are looked for in ([`citing_sentences`] reads the same ones), in reading order.
fn sentences(passages: &Passages) -> Vec<Sentence> {
    let mut top = String::new();
    let mut all = Vec::new();
    for (index, passage) in passages.passages.iter().enumerate() {
        if passage.kind == Kind::Heading && !SUBSECTION.is_match(&passage.text) {
            top.clone_from(&passage.text);
        }
        if !matches!(passage.kind, Kind::Text | Kind::FigureCaption | Kind::TableCaption | Kind::Footnote) {
            continue;
        }
        for span in &passage.sentences {
            let start = span.start.saturating_sub(passage.address.start);
            let end = span.end.saturating_sub(passage.address.start);
            all.push(Sentence {
                // as `citing_sentences` addresses a sentence, so that the two can be matched
                address: Address {
                    start: span.start,
                    end: span.end,
                    end_page: passage.address.page,
                    ..passage.address.clone()
                },
                text: chars_of(&passage.text, start, end),
                passage: index,
                kind: passage.kind,
                part: passage.part,
                top: top.clone(),
            });
        }
    }
    all
}

/// Is `other` the sentence next to `sentence` for a reader: in the same section, and in the same running text?
fn next_to(sentence: &Sentence, other: &Sentence) -> bool {
    let running = sentence.kind == Kind::Text && other.kind == Kind::Text;
    sentence.address.section == other.address.section && (running || sentence.passage == other.passage)
}

/// The parenthesis around `written` in `sentence`, in bytes: "(Ilharco et al., 2022; Ruan et al., 2025)" around
/// "Ruan et al., 2025". None when the citation is not inside one ("Lovelace and Turing (2024a)").
fn parenthesis_around(sentence: &str, written: &str) -> Option<(usize, usize)> {
    let at = sentence.find(written)?;
    let open = sentence[..at].rfind('(')?;
    if sentence[open..at].contains(')') {
        return None;
    }
    let close = sentence[at..].find(')').map_or(sentence.len(), |close| at + close);
    Some((open, close))
}

/// Whether each reference is cited alone or with others in each sentence that cites it. A numbered marker lists
/// its works ("[3, 7-9]" is one marker for four); an author-year citation is grouped with the others inside the
/// same parenthesis.
fn groupings(cited: &[crate::text::passages::Cited]) -> HashMap<(usize, Address), Grouping> {
    let mut at: HashMap<&Address, Vec<(usize, &str, Marker, &str)>> = HashMap::new();
    for one in cited {
        for sentence in &one.sentences {
            at.entry(&sentence.address).or_default().push((
                one.reference,
                sentence.written.as_str(),
                sentence.marker,
                sentence.sentence.as_str(),
            ));
        }
    }
    let mut groupings = HashMap::new();
    for (address, citations) in &at {
        for &(reference, written, marker, sentence) in citations {
            let together = |other: &&(usize, &str, Marker, &str)| match marker {
                Marker::Number | Marker::Superscript => other.1 == written,
                Marker::AuthorYear => match (parenthesis_around(sentence, written), sentence.find(other.1)) {
                    (Some((open, close)), Some(found)) => found > open && found < close,
                    _ => false,
                },
            };
            let mut others: Vec<usize> =
                citations.iter().filter(|other| other.0 != reference && together(other)).map(|other| other.0).collect();
            others.sort_unstable();
            others.dedup();
            let grouping = if others.is_empty() { Grouping::Alone } else { Grouping::Grouped { with: others.len() } };
            groupings.insert((reference, (*address).clone()), grouping);
        }
    }
    groupings
}

/// Every citation `list`'s entries get in the paper's text, each with its context, its entries linked to the
/// library by `linker` (`own` is the paper's id).
pub fn contexts_of(passages: &Passages, list: &ReferenceList, linker: &Linker, own: &str) -> PaperContexts {
    let cited = citing_sentences(passages, list);
    let groupings = groupings(&cited);
    let all = sentences(passages);
    let index: HashMap<&Address, usize> =
        all.iter().enumerate().map(|(at, sentence)| (&sentence.address, at)).collect();
    let neighbour = |at: Option<usize>, of: &Sentence| {
        let other = &all[at?];
        next_to(of, other).then(|| Neighbour { address: other.address.clone(), sentence: other.text.clone() })
    };
    let entries = cited
        .iter()
        .filter_map(|one| {
            let entry = list.references.get(one.reference)?.clone();
            let contexts = one
                .sentences
                .iter()
                .take(MOST_CONTEXTS)
                .filter_map(|sentence| {
                    let at = *index.get(&sentence.address)?;
                    let own = &all[at];
                    Some(Context {
                        address: sentence.address.clone(),
                        sentence: sentence.sentence.clone(),
                        before: neighbour(at.checked_sub(1), own),
                        after: neighbour(Some(at + 1).filter(|next| *next < all.len()), own),
                        section: section_of(own.part, &own.top, &own.address.section),
                        grouping: groupings
                            .get(&(one.reference, sentence.address.clone()))
                            .cloned()
                            .unwrap_or(Grouping::Alone),
                        marker: sentence.marker,
                        written: sentence.written.clone(),
                    })
                })
                .collect::<Vec<_>>();
            let link = own_left_out(linker.link(&entry), own);
            (!contexts.is_empty()).then_some(CitedEntry { reference: one.reference, entry, link, contexts })
        })
        .collect();
    PaperContexts { citing: list.citing, listed: list.references.len(), entries }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;
    use crate::analysis::citations::LibraryPaper;
    use crate::text::passages::read_passages;
    use crate::text::references::read_reference_list;

    pub(crate) fn library_paper(id: &str, title: &str, year: u16) -> LibraryPaper {
        LibraryPaper {
            id: id.to_owned(),
            title: title.to_owned(),
            authors: vec!["Ada Lovelace".to_owned()],
            year: Some(year),
            venue: None,
            arxiv: None,
            doi: None,
            url: None,
        }
    }

    /// A numbered paper that cites two papers of the library: one alone in its method, one in a group in its
    /// experiments (under a subsection), and one work that is not in the library.
    pub(crate) fn numbered() -> Vec<String> {
        vec![
            "1 Introduction\n\nMerging models is cheap. Task arithmetic adds the updates of each task [1]. We study \
when it fails.\n\n2 Method\n\nWe build on the merging rule of [1] and rescale it. The rest follows.\n\n\
3 Experiments\n\n3.1 Baselines\n\nWe compare against several methods [2, 3]. Ours is better on every task. \
We report the mean of five seeds.\n\nReferences\n\n[1] A. Lovelace. Editing models with task arithmetic. In ICLR, 2023.\n\n\
[2] A. Turing. Ties merging of many fine tuned models. In NeurIPS, 2023.\n\n[3] G. Hopper. Something not in the \
library at all. In ICML, 2022.\n"
                .to_owned(),
        ]
    }

    pub(crate) fn library() -> Vec<LibraryPaper> {
        vec![
            library_paper("arith", "Editing models with task arithmetic", 2023),
            library_paper("ties", "Ties merging of many fine tuned models", 2023),
            library_paper("seed", "Spectral calibration for merging", 2025),
        ]
    }

    fn read(pages: &[String]) -> PaperContexts {
        let papers = library();
        let linker = Linker::new(&papers);
        contexts_of(&read_passages(pages), &read_reference_list(&pages.join("\n")), &linker, "seed")
    }

    #[test]
    fn each_citation_of_a_library_paper_comes_with_its_sentence_its_neighbours_and_its_section() {
        let contexts = read(&numbered());
        assert_eq!((contexts.citing, contexts.listed), (Citing::ByNumber, 3));
        let linked = contexts.in_library();
        assert_eq!(linked.iter().map(|(paper, _)| paper.as_str()).collect::<Vec<_>>(), ["arith", "ties"]);
        let arith = &linked[0].1;
        assert_eq!(arith.len(), 2);
        assert_eq!(arith[0].sentence, "Task arithmetic adds the updates of each task [1].");
        assert_eq!(arith[0].before.as_ref().unwrap().sentence, "Merging models is cheap.");
        assert_eq!(arith[0].after.as_ref().unwrap().sentence, "We study when it fails.");
        assert_eq!(arith[0].section, Section::Introduction);
        assert_eq!(arith[0].grouping, Grouping::Alone);
        assert_eq!(arith[1].section, Section::Method);
        assert!(arith[1].before.is_none(), "the first sentence of its section has no sentence before it");
        let ties = &linked[1].1;
        assert_eq!(ties[0].section, Section::Experiments, "a subsection of the experiments is in the experiments");
        assert_eq!(ties[0].grouping, Grouping::Grouped { with: 1 });
        assert_eq!(ties[0].written, "[2, 3]");
        // the work not in the library is kept, to be linked again when the library has it
        assert_eq!(contexts.entries.len(), 3);
        assert_eq!(contexts.entries[2].link, Link::Unresolved);
    }

    #[test]
    fn every_address_is_the_place_of_its_sentence_in_the_page() {
        let pages = numbered();
        let passages = read_passages(&pages);
        let contexts = read(&pages);
        for entry in &contexts.entries {
            for context in &entry.contexts {
                for (address, sentence) in context.window() {
                    let page = &passages.pages[address.page as usize - 1].text;
                    assert_eq!(chars_of(page, address.start, address.end), sentence);
                }
            }
        }
    }

    #[test]
    fn author_year_citations_in_one_parenthesis_are_a_group_and_a_narrative_one_is_alone() {
        let pages = vec![
            "1. Introduction\n\nMerging is cheap (Ilharco et al., 2023; Ruan et al., 2025). Lovelace and Turing (2024) \
saw this first.\n\nReferences\n\nIlharco, G., Ribeiro, M. T., and Wortsman, M. Editing models with task arithmetic. \
In ICLR, 2023.\n\nLovelace, A. and Turing, A. Counting twice in model merging. In NeurIPS, 2024.\n\nRuan, W., Chen, \
Y., and Li, X. Model merging survey. arXiv preprint, 2025.\n"
                .to_owned(),
        ];
        let papers = vec![
            library_paper("arith", "Editing models with task arithmetic", 2023),
            library_paper("twice", "Counting twice in model merging", 2024),
        ];
        let linker = Linker::new(&papers);
        let contexts = contexts_of(&read_passages(&pages), &read_reference_list(&pages[0]), &linker, "seed");
        let linked = contexts.in_library();
        let of = |id: &str| &linked.iter().find(|(paper, _)| paper == id).unwrap().1[0];
        assert_eq!(of("arith").grouping, Grouping::Grouped { with: 1 });
        assert_eq!(of("twice").grouping, Grouping::Alone);
        assert_eq!(
            of("twice").before.as_ref().unwrap().sentence,
            "Merging is cheap (Ilharco et al., 2023; Ruan et al., 2025)."
        );
    }

    #[test]
    fn sections_are_named_from_their_headings_words() {
        let section = |top: &str, heading: &str| section_of(Part::Body, top, heading);
        assert_eq!(section("1. Introduction", "1. Introduction"), Section::Introduction);
        assert_eq!(section("2 Related Work", "2.3 Model merging"), Section::RelatedWork);
        assert_eq!(section("3 Preliminaries", "3 Preliminaries"), Section::Background);
        assert_eq!(section("5 Experiments", "5.2 Results on GLUE"), Section::Experiments);
        assert_eq!(section("4 Spectral Calibration", "4.1 Baselines"), Section::Experiments, "the top says nothing");
        assert_eq!(section("4 Our Approach", "4.1 Setup"), Section::Method);
        assert_eq!(section("6 Conclusion", "6 Conclusion"), Section::Discussion);
        assert_eq!(section("Abstract", "Abstract"), Section::Abstract);
        assert_eq!(
            section("3 Spectral Calibration", "3.1 Counting Twice"),
            Section::Other { title: "3.1 Counting Twice".to_owned() }
        );
        assert_eq!(section_of(Part::Appendix, "A. Proofs", "A. Proofs"), Section::Appendix);
        let json = serde_json::to_value(Section::Other { title: "3 Spectral".to_owned() }).unwrap();
        assert_eq!(json, serde_json::json!({"kind": "other", "title": "3 Spectral"}));
        assert_eq!(serde_json::to_value(Section::RelatedWork).unwrap(), serde_json::json!({"kind": "related_work"}));
    }

    #[test]
    fn a_paper_is_never_its_own_citation_and_relinking_finds_papers_added_since() {
        let pages = numbered();
        let none: Vec<LibraryPaper> = Vec::new();
        let linker = Linker::new(&none);
        let mut contexts =
            contexts_of(&read_passages(&pages), &read_reference_list(&pages.join("\n")), &linker, "arith");
        assert!(contexts.in_library().is_empty());
        let papers = library();
        assert!(contexts.relink(&Linker::new(&papers), "arith"));
        // "arith" is the paper itself: left out; "ties" is found now
        assert_eq!(contexts.in_library().iter().map(|(paper, _)| paper.as_str()).collect::<Vec<_>>(), ["ties"]);
        assert!(!contexts.relink(&Linker::new(&papers), "arith"), "nothing changed the second time");
    }

    #[test]
    fn a_paper_with_no_reference_list_cites_nothing() {
        let contexts = read(&["1 Introduction\n\nWe cite nothing [1] at all.\n".to_owned()]);
        assert_eq!((contexts.listed, contexts.entries.len()), (0, 0));
    }
}
