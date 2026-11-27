//! Who cites whom, among the papers Constellate has read: the citation network around one paper, and what
//! communities cite and are cited.
//!
//! Everything here is worked out from the reference lists of the papers whose PDFs Constellate has read
//! ([`crate::text::references`]), and from nothing else. It is not the whole literature: a paper's *references*
//! are complete once its own PDF is read, but the papers that *cite* it are only those among the read ones. The
//! interface says so wherever it shows these figures.
//!
//! Three steps, each a plain function of what it is given:
//!
//! 1. **Linking** ([`Linker`]): each reference is matched to a paper of the user's library, by arXiv id or DOI
//!    when the entry has one, else by its title. Titles are compared by their letters and digits only
//!    ([`title_key`]), so line breaks and hyphens from the PDF make no difference; when that finds nothing, a
//!    close title (nine in ten of the words shared, [`SIMILAR_TITLES`]) from the same year or the next counts,
//!    which survives a word lost at a page break. Below that nothing is linked: two papers are never taken to be
//!    one on a resemblance. A reference that is not in the library but names an arXiv id is "on arXiv"; any other
//!    is unresolved, shown with what was read of it. ([`crate::sources::citations`] can look the unresolved ones
//!    up on arXiv.)
//! 2. **The network around a seed paper** ([`network`]): its references (prior work), the read papers whose
//!    references include it (derivative work), the citations between those neighbours, so that the picture is a
//!    network and not a star, and two similarities between neighbours that tools like Connected Papers draw:
//!    *bibliographic coupling* (how many references two papers share) and *co-citation* (how many papers cite
//!    both).
//! 3. **What communities cite and are cited** ([`community_citations`]), over the nested community tree.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::NaiveDate;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::text::references::{Confidence, Reference, title_key, title_words};

/// The share of their words two titles must have in common to be taken for the same title.
pub const SIMILAR_TITLES: f64 = 0.9;
/// A close title is only trusted with this many words: two short titles share most of them by chance.
const FUZZY_WORDS: usize = 4;
/// An exact title shorter than this (letters and digits) is common enough to need the year to agree as well.
const DISTINCT_TITLE: usize = 20;
/// Citing papers listed around a seed: the most cited first.
pub const MAX_CITATIONS: usize = 200;
/// Pairs of neighbours listed as similar: the most similar first.
pub const MAX_SIMILAR: usize = 400;
/// Authors listed for a neighbour; the rest are counted.
const SHOWN_AUTHORS: usize = 3;

/// A paper of the user's library, as the linker needs it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LibraryPaper {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<u16>,
    pub venue: Option<String>,
    /// Without its version.
    pub arxiv: Option<String>,
    pub doi: Option<String>,
    /// Its page, to read it outside the app.
    pub url: Option<String>,
}

/// What a reference was found to be.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Link {
    /// A paper of the user's library: it can be opened in the app.
    InLibrary { paper: String },
    /// Not in the library, but on arXiv under this id.
    OnArxiv { arxiv: String },
    /// Neither: only what was read of the entry is known.
    Unresolved,
}

/// A reference and what it was found to be.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkedReference {
    pub reference: Reference,
    pub link: Link,
}

/// A paper whose reference list has been read, with each reference linked: what the caller keeps by paper id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadPaper {
    pub paper: String,
    pub references: Vec<LinkedReference>,
}

/// Are the two years at most one apart? A preprint is often a year older than the proceedings.
fn near(year: Option<u16>, other: Option<u16>) -> bool {
    matches!((year, other), (Some(year), Some(other)) if year.abs_diff(other) <= 1)
}

/// Words shared, over the words of the longer title.
fn word_overlap(words: &BTreeSet<String>, other: &BTreeSet<String>) -> f64 {
    let longer = words.len().max(other.len());
    if longer == 0 {
        return 0.0;
    }
    words.intersection(other).count() as f64 / longer as f64
}

fn word_set(title: &str) -> BTreeSet<String> {
    title_words(title).into_iter().collect()
}

/// Are these one paper's title, as written in two places? The same letters and digits (and, for a short title,
/// years at most one apart), or nearly the same words from years at most one apart.
pub fn same_title(title: &str, year: Option<u16>, other: &str, other_year: Option<u16>) -> bool {
    let key = title_key(title);
    if !key.is_empty() && key == title_key(other) {
        return key.len() >= DISTINCT_TITLE || near(year, other_year);
    }
    let (words, other_words) = (word_set(title), word_set(other));
    words.len().min(other_words.len()) >= FUZZY_WORDS
        && near(year, other_year)
        && word_overlap(&words, &other_words) >= SIMILAR_TITLES
}

/// Matches references to the papers of a library.
pub struct Linker<'a> {
    papers: &'a [LibraryPaper],
    by_id: HashMap<&'a str, usize>,
    by_arxiv: HashMap<String, usize>,
    by_doi: HashMap<String, usize>,
    by_key: HashMap<String, Vec<usize>>,
    words: Vec<BTreeSet<String>>,
    /// word -> the papers whose title has it
    postings: HashMap<String, Vec<usize>>,
}

impl<'a> Linker<'a> {
    pub fn new(papers: &'a [LibraryPaper]) -> Self {
        let mut linker = Self {
            papers,
            by_id: HashMap::new(),
            by_arxiv: HashMap::new(),
            by_doi: HashMap::new(),
            by_key: HashMap::new(),
            words: Vec::with_capacity(papers.len()),
            postings: HashMap::new(),
        };
        for (index, paper) in papers.iter().enumerate() {
            linker.by_id.insert(&paper.id, index);
            if let Some(arxiv) = &paper.arxiv {
                linker.by_arxiv.entry(arxiv.to_lowercase()).or_insert(index);
            }
            if let Some(doi) = &paper.doi {
                linker.by_doi.entry(doi.to_lowercase()).or_insert(index);
            }
            linker.by_key.entry(title_key(&paper.title)).or_default().push(index);
            let words = word_set(&paper.title);
            for word in &words {
                linker.postings.entry(word.clone()).or_default().push(index);
            }
            linker.words.push(words);
        }
        linker
    }

    /// The library's paper with this id.
    pub fn paper(&self, id: &str) -> Option<&'a LibraryPaper> {
        self.by_id.get(id).map(|&index| &self.papers[index])
    }

    /// What this reference is: a paper of the library, a paper on arXiv, or unresolved.
    pub fn link(&self, reference: &Reference) -> Link {
        let by_id = reference
            .arxiv
            .as_ref()
            .and_then(|arxiv| self.by_arxiv.get(&arxiv.to_lowercase()))
            .or_else(|| reference.doi.as_ref().and_then(|doi| self.by_doi.get(&doi.to_lowercase())));
        let found = by_id.copied().or_else(|| {
            let title = reference.title.as_deref()?;
            self.by_exact_title(title, reference.year).or_else(|| self.by_close_title(title, reference.year))
        });
        match (found, &reference.arxiv) {
            (Some(index), _) => Link::InLibrary { paper: self.papers[index].id.clone() },
            (None, Some(arxiv)) => Link::OnArxiv { arxiv: arxiv.clone() },
            (None, None) => Link::Unresolved,
        }
    }

    fn by_exact_title(&self, title: &str, year: Option<u16>) -> Option<usize> {
        let key = title_key(title);
        // a title in another script has no key: every such title would be "the same"
        if key.is_empty() {
            return None;
        }
        let candidates = self.by_key.get(&key)?;
        // the same title twice in the library is one paper twice (its preprint and its proceedings): the year
        // closest to the reference's is the one it cites
        let closest = candidates.iter().copied().min_by_key(|&index| match (year, self.papers[index].year) {
            (Some(year), Some(other)) => year.abs_diff(other),
            _ => u16::MAX,
        })?;
        (key.len() >= DISTINCT_TITLE || near(year, self.papers[closest].year)).then_some(closest)
    }

    fn by_close_title(&self, title: &str, year: Option<u16>) -> Option<usize> {
        let words = word_set(title);
        if words.len() < FUZZY_WORDS || year.is_none() {
            return None;
        }
        // a title sharing nine words in ten has at least one of the rarest tenth-plus-one: only papers that have
        // one of those need comparing
        let mut rarest: Vec<&String> = words.iter().collect();
        rarest.sort_by_key(|word| self.postings.get(*word).map_or(0, Vec::len));
        // (the tolerance keeps 0.1 x 10 from coming out as 0.99999 and losing the second word)
        let needed = ((1.0 - SIMILAR_TITLES) * words.len() as f64 + 1e-9).floor() as usize + 1;
        let candidates: HashSet<usize> =
            rarest.iter().take(needed).filter_map(|word| self.postings.get(*word)).flatten().copied().collect();
        let mut best: Option<(f64, usize)> = None;
        let mut tied = false;
        for index in candidates {
            let overlap = word_overlap(&words, &self.words[index]);
            if overlap < SIMILAR_TITLES || self.words[index].len() < FUZZY_WORDS || !near(year, self.papers[index].year)
            {
                continue;
            }
            match best {
                Some((top, _)) if overlap < top => {}
                Some((top, _)) if overlap == top => tied = true,
                _ => {
                    best = Some((overlap, index));
                    tied = false;
                }
            }
        }
        // two papers equally close: neither is linked, rather than a guess
        best.filter(|_| !tied).map(|(_, index)| index)
    }

    /// A paper's reference list, each reference linked.
    pub fn read(&self, paper: &str, references: Vec<Reference>) -> ReadPaper {
        let references = references
            .into_iter()
            .map(|reference| LinkedReference { link: self.link(&reference), reference })
            .collect();
        ReadPaper { paper: paper.to_owned(), references }
    }

    /// Many papers' reference lists, each linked: [`Linker::read`] on every core, in the order given.
    ///
    /// The linker is only read (its indexes are built once, in [`Linker::new`]), and what a reference is linked to
    /// depends on that reference alone: a close title that two papers match equally is linked to neither, so not
    /// even the order the candidates are looked at shows. The answer is the same however many threads made it.
    pub fn read_all(&self, papers: Vec<(String, Vec<Reference>)>) -> Vec<ReadPaper> {
        papers.into_par_iter().map(|(paper, references)| self.read(&paper, references)).collect()
    }
}

/// What a cited work is known as within a network: its library paper, its arXiv id, its DOI, else its title's
/// letters (or, when no title was read, the entry's).
pub fn work_key(reference: &LinkedReference) -> String {
    match &reference.link {
        Link::InLibrary { paper } => format!("paper:{paper}"),
        Link::OnArxiv { arxiv } => format!("arxiv:{arxiv}"),
        Link::Unresolved => match (&reference.reference.doi, &reference.reference.title) {
            (Some(doi), _) => format!("doi:{}", doi.to_lowercase()),
            (None, Some(title)) => format!("title:{}", letters(title)),
            (None, None) => format!("entry:{}", letters(&reference.reference.raw)),
        },
    }
}

/// A text by its letters and digits: [`title_key`], or for a text in another script (which that leaves empty)
/// its own letters, so that two different titles in Cyrillic are never one work.
fn letters(text: &str) -> String {
    let key = title_key(text);
    if !key.is_empty() {
        return key;
    }
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn paper_key(id: &str) -> String {
    format!("paper:{id}")
}

/// A work next to the seed in its network.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Neighbour {
    /// Its name within this network ([`work_key`]): what [`Edge`] and [`Similar`] refer to.
    pub key: String,
    /// The title, or the entry as written when no title could be read from it.
    pub title: String,
    /// The first few, as written.
    pub authors: Vec<String>,
    pub authors_total: usize,
    pub year: Option<u16>,
    pub venue: Option<String>,
    pub link: Link,
    /// Where to read it outside the app, when known.
    pub url: Option<String>,
    /// How far the entry it comes from was understood; a paper of the library is always identified.
    pub confidence: Confidence,
    /// How many papers Constellate has read cite it.
    pub cited_by: usize,
    /// The reference as written in the seed's list; none for a citing paper.
    pub entry: Option<String>,
}

/// One neighbour's reference list includes the other.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
}

/// Two neighbours that share references or are cited together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Similar {
    pub a: String,
    pub b: String,
    /// References both have (bibliographic coupling); the seed itself is not counted.
    pub shared_references: usize,
    /// Read papers that cite both (co-citation); the seed itself is not counted.
    pub co_cited: usize,
}

/// Has the seed's own reference list been read?
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedState {
    Read,
    NotRead,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Totals {
    /// Entries in the seed's reference list (each work once).
    pub references: usize,
    pub references_in_library: usize,
    pub references_on_arxiv: usize,
    pub references_unresolved: usize,
    /// Read papers that cite the seed.
    pub citations: usize,
    /// Of those, how many are listed ([`MAX_CITATIONS`] at most).
    pub citations_shown: usize,
    /// Of those, how many are papers of the run asked about, when one was.
    pub citations_in_run: Option<usize>,
}

/// The network around one paper: `CitationNetwork` in the contracts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CitationNetwork {
    pub seed: String,
    pub seed_state: SeedState,
    /// Prior work: the seed's references, in the order listed.
    pub references: Vec<Neighbour>,
    /// Derivative work: read papers that cite the seed, the most cited first.
    pub citations: Vec<Neighbour>,
    /// Citations between neighbours; those from and to the seed are the lists above.
    pub edges: Vec<Edge>,
    pub similar: Vec<Similar>,
    pub totals: Totals,
    /// Whose reference lists this comes from.
    pub papers_read: usize,
    pub papers_in_library: usize,
    pub built: NaiveDate,
}

/// Who cites each work, among the read papers: work key -> the citing papers' ids.
///
/// One thread builds it. Shared out among threads (each its own map, merged after) it was no faster for 20,000
/// read papers: merging maps of a few hundred thousand works costs what building them does.
fn citers(read: &[ReadPaper]) -> HashMap<String, BTreeSet<&str>> {
    let mut citers: HashMap<String, BTreeSet<&str>> = HashMap::new();
    for paper in read {
        let own = paper_key(&paper.paper);
        for reference in &paper.references {
            let key = work_key(reference);
            if key != own {
                citers.entry(key).or_default().insert(&paper.paper);
            }
        }
    }
    citers
}

fn neighbour_from_library(paper: &LibraryPaper, cited_by: usize) -> Neighbour {
    Neighbour {
        key: paper_key(&paper.id),
        title: paper.title.clone(),
        authors: paper.authors.iter().take(SHOWN_AUTHORS).cloned().collect(),
        authors_total: paper.authors.len(),
        year: paper.year,
        venue: paper.venue.clone(),
        link: Link::InLibrary { paper: paper.id.clone() },
        url: paper.url.clone(),
        confidence: Confidence::Identified,
        cited_by,
        entry: None,
    }
}

fn neighbour_from_reference(linked: &LinkedReference, library: &Linker, cited_by: usize) -> Neighbour {
    let reference = &linked.reference;
    if let Link::InLibrary { paper } = &linked.link {
        if let Some(paper) = library.paper(paper) {
            return Neighbour { entry: Some(reference.raw.clone()), ..neighbour_from_library(paper, cited_by) };
        }
    }
    let url = match (&linked.link, &reference.doi) {
        (Link::OnArxiv { arxiv }, _) => Some(format!("https://arxiv.org/abs/{arxiv}")),
        (_, Some(doi)) => Some(format!("https://doi.org/{doi}")),
        _ => reference.url.clone(),
    };
    Neighbour {
        key: work_key(linked),
        title: reference.title.clone().unwrap_or_else(|| reference.raw.clone()),
        authors: reference.authors.iter().take(SHOWN_AUTHORS).cloned().collect(),
        authors_total: reference.authors.len(),
        year: reference.year,
        venue: reference.venue.clone(),
        link: linked.link.clone(),
        url,
        confidence: reference.confidence,
        cited_by,
        entry: Some(reference.raw.clone()),
    }
}

/// The network around `seed`, from the reference lists in `read` (papers of `library`). With `run` (the ids of
/// a run's papers), the totals also say how many of the citing papers are in that run.
pub fn network(
    seed: &str,
    library: &Linker,
    read: &[ReadPaper],
    run: Option<&HashSet<String>>,
    built: NaiveDate,
) -> CitationNetwork {
    let citers = citers(read);
    let cited = |key: &str| citers.get(key).map_or(0, BTreeSet::len);
    let seed_key = paper_key(seed);
    let seed_paper = read.iter().find(|paper| paper.paper == seed);

    let mut seen = HashSet::new();
    let references: Vec<Neighbour> = seed_paper
        .map(|paper| paper.references.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|linked| {
            let key = work_key(linked);
            key != seed_key && seen.insert(key)
        })
        .map(|linked| neighbour_from_reference(linked, library, cited(&work_key(linked))))
        .collect();

    let citing: Vec<&str> = citers.get(&seed_key).map(|papers| papers.iter().copied().collect()).unwrap_or_default();
    let mut citations: Vec<Neighbour> = citing
        .iter()
        .filter_map(|id| library.paper(id))
        .map(|paper| neighbour_from_library(paper, cited(&paper_key(&paper.id))))
        .collect();
    citations.sort_by(|a, b| b.cited_by.cmp(&a.cited_by).then(b.year.cmp(&a.year)).then(a.title.cmp(&b.title)));
    let citations_total = citations.len();
    citations.truncate(MAX_CITATIONS);

    let count = |wanted: fn(&Link) -> bool| references.iter().filter(|neighbour| wanted(&neighbour.link)).count();
    let totals = Totals {
        references: references.len(),
        references_in_library: count(|link| matches!(link, Link::InLibrary { .. })),
        references_on_arxiv: count(|link| matches!(link, Link::OnArxiv { .. })),
        references_unresolved: count(|link| matches!(link, Link::Unresolved)),
        citations: citations_total,
        citations_shown: citations.len(),
        citations_in_run: run.map(|run| citing.iter().filter(|id| run.contains(**id)).count()),
    };

    let keys: HashSet<&str> = references.iter().chain(&citations).map(|neighbour| neighbour.key.as_str()).collect();
    let by_paper: HashMap<String, &ReadPaper> = read.iter().map(|paper| (paper_key(&paper.paper), paper)).collect();
    // each read neighbour's references among the neighbours, the seed left out: the edges and the coupling
    let cites: HashMap<&str, BTreeSet<String>> = keys
        .iter()
        .filter_map(|&key| by_paper.get(key).map(|paper| (key, paper)))
        .map(|(key, paper)| {
            (key, paper.references.iter().map(work_key).filter(|cited| *cited != seed_key && cited != key).collect())
        })
        .collect();
    let mut edges: Vec<Edge> = cites
        .iter()
        .flat_map(|(&from, cited)| {
            cited
                .iter()
                .filter(|to| keys.contains(to.as_str()))
                .map(move |to| Edge { from: from.to_owned(), to: to.clone() })
        })
        .collect();
    edges.sort();

    let mut ordered: Vec<&str> = keys.iter().copied().collect();
    ordered.sort_unstable();
    let co_citers = |key: &str| -> BTreeSet<&str> {
        citers
            .get(key)
            .map(|papers| papers.iter().copied().filter(|paper| *paper != seed).collect())
            .unwrap_or_default()
    };
    let citing_each: HashMap<&str, BTreeSet<&str>> = ordered.iter().map(|&key| (key, co_citers(key))).collect();
    let mut similar = Vec::new();
    for (index, &a) in ordered.iter().enumerate() {
        for &b in &ordered[index + 1..] {
            let shared_references = match (cites.get(a), cites.get(b)) {
                (Some(left), Some(right)) => left.intersection(right).count(),
                _ => 0,
            };
            let co_cited = citing_each[a].intersection(&citing_each[b]).count();
            if shared_references + co_cited > 0 {
                similar.push(Similar { a: a.to_owned(), b: b.to_owned(), shared_references, co_cited });
            }
        }
    }
    similar.sort_by(|x, y| {
        (y.shared_references + y.co_cited)
            .cmp(&(x.shared_references + x.co_cited))
            .then(x.a.cmp(&y.a))
            .then(x.b.cmp(&y.b))
    });
    similar.truncate(MAX_SIMILAR);

    CitationNetwork {
        seed: seed.to_owned(),
        seed_state: if seed_paper.is_some() { SeedState::Read } else { SeedState::NotRead },
        references,
        citations,
        edges,
        similar,
        totals,
        papers_read: read.len(),
        papers_in_library: library.papers.len(),
        built,
    }
}

/// A community of the run's tree, by the ids of its papers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Members {
    pub tree_path: String,
    pub papers: Vec<String>,
    pub children: Vec<Members>,
}

/// What a community cites and how often it is cited, among the run's read papers: `citations` of `Community`
/// in the contracts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunityCitations {
    pub tree_path: String,
    pub papers: usize,
    /// Of its papers, how many have had their reference list read: what `cites` is counted over.
    pub papers_read: usize,
    /// Citations its papers receive from the run's read papers, inside the community and outside it: each pair
    /// of citing and cited paper once, a paper never citing itself.
    pub cited_by: usize,
    /// Of those, from its own papers.
    pub cited_by_inside: usize,
    /// Distinct works its read papers cite, resolved or not ([`work_key`]).
    pub cites: usize,
    /// Of those, papers of the user's library.
    pub cites_in_library: usize,
    pub children: Vec<CommunityCitations>,
}

/// The citation figures of a run's communities.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCitations {
    pub papers: usize,
    /// Of the run's papers, how many have had their reference list read: whose citations `cited_by` counts.
    pub papers_read: usize,
    pub communities: Vec<CommunityCitations>,
}

/// How much each community of `tree` cites and is cited, from the reference lists in `read`, counting only
/// citations from papers of the run (`run`).
pub fn community_citations(read: &[ReadPaper], run: &[String], tree: &[Members]) -> RunCitations {
    let in_run: HashSet<&str> = run.iter().map(String::as_str).collect();
    let read_in_run: HashMap<&str, &ReadPaper> = read
        .iter()
        .filter(|paper| in_run.contains(paper.paper.as_str()))
        .map(|paper| (paper.paper.as_str(), paper))
        .collect();
    // cited paper -> the run's papers that cite it
    let mut cited_by: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for paper in read_in_run.values() {
        for reference in &paper.references {
            if let Link::InLibrary { paper: cited } = &reference.link {
                if *cited != paper.paper {
                    cited_by.entry(cited.as_str()).or_default().insert(&paper.paper);
                }
            }
        }
    }
    fn of(
        community: &Members,
        read: &HashMap<&str, &ReadPaper>,
        cited_by: &HashMap<&str, BTreeSet<&str>>,
    ) -> CommunityCitations {
        let members: HashSet<&str> = community.papers.iter().map(String::as_str).collect();
        let mut works = HashSet::new();
        let mut papers_read = 0;
        for paper in community.papers.iter().filter_map(|id| read.get(id.as_str())) {
            papers_read += 1;
            works.extend(paper.references.iter().map(work_key));
        }
        let citing = || members.iter().filter_map(|paper| cited_by.get(paper)).flatten();
        CommunityCitations {
            tree_path: community.tree_path.clone(),
            papers: community.papers.len(),
            papers_read,
            cited_by: citing().count(),
            cited_by_inside: citing().filter(|citer| members.contains(*citer)).count(),
            cites_in_library: works.iter().filter(|work| work.starts_with("paper:")).count(),
            cites: works.len(),
            children: community.children.par_iter().map(|child| of(child, read, cited_by)).collect(),
        }
    }
    // Every community, and every sub-community, is counted on a thread of its own: its figures are counts over
    // its own papers in maps nobody writes any more, collected in the tree's order.
    RunCitations {
        papers: run.len(),
        papers_read: read_in_run.len(),
        communities: tree.par_iter().map(|community| of(community, &read_in_run, &cited_by)).collect(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn paper(id: &str, title: &str, year: u16) -> LibraryPaper {
        LibraryPaper {
            id: id.to_owned(),
            title: title.to_owned(),
            authors: vec![
                "Ada Lovelace".to_owned(),
                "Alan Turing".to_owned(),
                "Grace Hopper".to_owned(),
                "Edsger Dijkstra".to_owned(),
            ],
            year: Some(year),
            venue: Some("NeurIPS 2025".to_owned()),
            arxiv: None,
            doi: None,
            url: Some(format!("https://example.org/{id}")),
        }
    }

    fn cited(title: &str, year: u16) -> Reference {
        Reference {
            raw: format!("Someone, A. {title}. In Somewhere, {year}."),
            authors: vec!["Someone, A.".to_owned()],
            title: Some(title.to_owned()),
            year: Some(year),
            venue: Some("Somewhere".to_owned()),
            arxiv: None,
            doi: None,
            url: None,
            confidence: Confidence::TitleParsed,
        }
    }

    fn library() -> Vec<LibraryPaper> {
        vec![
            LibraryPaper {
                arxiv: Some("2505.11792".to_owned()),
                ..paper("2505.11792", "Solver-Informed RL: Grounding Large Language Models for Optimization", 2025)
            },
            paper("neurips-2025-1", "Attention Is All You Need", 2017),
            paper("neurips-2025-2", "Gated Attention for Large Language Models: Non-linearity and Sparsity", 2025),
            paper("neurips-2025-3", "Deep Learning", 2015),
            paper("neurips-2025-4", "A Study of Sparse Attention in Large Language Model Training at Scale", 2025),
        ]
    }

    #[test]
    fn a_reference_is_linked_by_its_id_first_then_by_its_title() {
        let papers = library();
        let linker = Linker::new(&papers);
        let by_id = Reference { arxiv: Some("2505.11792".to_owned()), title: None, ..cited("whatever", 2025) };
        assert_eq!(linker.link(&by_id), Link::InLibrary { paper: "2505.11792".to_owned() });
        // the PDF broke the title over lines and hyphenated it: the letters are the same
        assert_eq!(
            linker.link(&cited("Attention is all you\u{2010}need", 2017)),
            Link::InLibrary { paper: "neurips-2025-1".to_owned() }
        );
        let preprint = Reference { arxiv: Some("1706.03762".to_owned()), ..cited("Attention is all you need", 2017) };
        assert_eq!(linker.link(&preprint), Link::InLibrary { paper: "neurips-2025-1".to_owned() });
        let elsewhere =
            Reference { arxiv: Some("2401.00001".to_owned()), ..cited("Something not in the library at all", 2024) };
        assert_eq!(linker.link(&elsewhere), Link::OnArxiv { arxiv: "2401.00001".to_owned() });
        assert_eq!(linker.link(&cited("Something not in the library at all", 2024)), Link::Unresolved);
    }

    #[test]
    fn a_short_title_needs_its_year_and_a_close_title_needs_nearly_all_its_words() {
        let papers = library();
        let linker = Linker::new(&papers);
        assert_eq!(linker.link(&cited("Deep learning", 2015)), Link::InLibrary { paper: "neurips-2025-3".to_owned() });
        assert_eq!(linker.link(&cited("Deep learning", 2009)), Link::Unresolved, "a common title from another year");
        // a word lost at a page break: 9 of the 10 words are there
        let lost = "Gated Attention for Large Language Models Non-linearity and";
        assert_eq!(linker.link(&cited(lost, 2025)), Link::InLibrary { paper: "neurips-2025-2".to_owned() });
        let two_lost = "Gated Attention for Large Language Models and Sparsity";
        assert_eq!(linker.link(&cited(two_lost, 2025)), Link::Unresolved, "8 of 10 words is not enough");
        let unhyphenated = "Gated Attention for Large Language Models: Nonlinearity and Sparsity";
        assert_eq!(linker.link(&cited(unhyphenated, 2025)), Link::InLibrary { paper: "neurips-2025-2".to_owned() });
        let one_typo = "A Study of Sparse Attention in Large Language Model Training at Scales";
        assert_eq!(linker.link(&cited(one_typo, 2025)), Link::InLibrary { paper: "neurips-2025-4".to_owned() });
        assert_eq!(
            linker.link(&cited(one_typo, 2021)),
            Link::Unresolved,
            "a close title from another time is another paper"
        );
        assert!(same_title("Attention is all you need", Some(2017), "Attention Is All You Need.", None));
        assert!(!same_title("Sparse attention", Some(2020), "Sparse attention", Some(2023)));
    }

    #[test]
    fn two_equally_close_titles_link_to_neither() {
        let papers = vec![
            paper("a", "Learning to Rank Research Papers with Graph Neural Networks for Search Today", 2024),
            paper("b", "Learning to Rank Research Papers with Graph Neural Networks for Search Tomorrow", 2024),
        ];
        let linker = Linker::new(&papers);
        let between = "Learning to Rank Research Papers with Graph Neural Networks for Search Yesterday";
        assert_eq!(linker.link(&cited(between, 2024)), Link::Unresolved, "11 of 12 words for both");
        let one = "Learning to Rank Research Papers with Graph Neural Networks for Search Today!";
        assert_eq!(linker.link(&cited(one, 2024)), Link::InLibrary { paper: "a".to_owned() });
    }

    #[test]
    fn a_close_title_is_found_even_when_its_missing_word_is_its_rarest() {
        let papers = vec![paper("p", "alpha beta gamma delta epsilon zeta eta theta iota kappa", 2024)];
        let linker = Linker::new(&papers);
        // 9 of 10 words, and "lambda" (in no title of the library) is the word looked at first
        let reference = cited("alpha beta gamma delta epsilon zeta eta theta iota lambda", 2024);
        assert_eq!(linker.link(&reference), Link::InLibrary { paper: "p".to_owned() });
    }

    #[test]
    fn titles_in_another_script_are_neither_linked_nor_merged_by_their_missing_latin_letters() {
        let papers = vec![paper("p", "Нейронные сети", 2023)];
        let linker = Linker::new(&papers);
        let first =
            linker.read("seed", vec![cited("Глубокое обучение", 2023), cited("Обучение с подкреплением", 2023)]);
        assert!(first.references.iter().all(|linked| linked.link == Link::Unresolved));
        assert_ne!(work_key(&first.references[0]), work_key(&first.references[1]));
    }

    /// seed cites 1 and an outside work; 2 cites the seed and 1; 4 cites the seed, 1 and 2.
    fn read_library(linker: &Linker) -> Vec<ReadPaper> {
        let outside = Reference { arxiv: Some("2401.00001".to_owned()), ..cited("An outside work on arXiv", 2024) };
        let seed = "2505.11792";
        let title_of = |id: &str| linker.paper(id).unwrap().title.clone();
        let of = |id: &str| cited(&title_of(id), linker.paper(id).unwrap().year.unwrap());
        vec![
            linker.read(
                seed,
                vec![
                    of("neurips-2025-1"),
                    outside.clone(),
                    of("neurips-2025-1"),
                    cited("A raw-ish entry nobody has", 2001),
                ],
            ),
            linker.read("neurips-2025-2", vec![of(seed), of("neurips-2025-1"), outside]),
            linker.read("neurips-2025-4", vec![of(seed), of("neurips-2025-1"), of("neurips-2025-2")]),
        ]
    }

    #[test]
    fn the_network_of_a_seed_has_its_references_its_citations_and_the_links_between_them() {
        let papers = library();
        let linker = Linker::new(&papers);
        let read = read_library(&linker);
        let run: HashSet<String> = ["neurips-2025-4".to_owned()].into();
        let built = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
        let network = network("2505.11792", &linker, &read, Some(&run), built);
        assert_eq!(network.seed_state, SeedState::Read);
        let keys = |neighbours: &[Neighbour]| neighbours.iter().map(|n| n.key.clone()).collect::<Vec<_>>();
        assert_eq!(
            keys(&network.references),
            ["paper:neurips-2025-1", "arxiv:2401.00001", "title:arawishentrynobodyhas"]
        );
        assert_eq!(keys(&network.citations), ["paper:neurips-2025-2", "paper:neurips-2025-4"], "the most cited first");
        assert_eq!(network.references[0].cited_by, 3);
        assert_eq!((network.references[0].authors.len(), network.references[0].authors_total), (3, 4));
        assert_eq!(network.references[1].url.as_deref(), Some("https://arxiv.org/abs/2401.00001"));
        assert_eq!(network.references[2].link, Link::Unresolved);
        assert_eq!(
            network.totals,
            Totals {
                references: 3,
                references_in_library: 1,
                references_on_arxiv: 1,
                references_unresolved: 1,
                citations: 2,
                citations_shown: 2,
                citations_in_run: Some(1),
            }
        );
        let edge = |from: &str, to: &str| Edge { from: format!("paper:{from}"), to: to.to_owned() };
        assert_eq!(
            network.edges,
            [
                edge("neurips-2025-2", "arxiv:2401.00001"),
                edge("neurips-2025-2", "paper:neurips-2025-1"),
                edge("neurips-2025-4", "paper:neurips-2025-1"),
                edge("neurips-2025-4", "paper:neurips-2025-2"),
            ]
        );
        // 2 and 4 both cite 1 (coupling); 1 and the arXiv work are both cited by 2 (co-citation)
        let pair = |a: &str, b: &str| network.similar.iter().find(|s| s.a == a && s.b == b).cloned();
        assert_eq!(
            pair("paper:neurips-2025-2", "paper:neurips-2025-4").map(|s| (s.shared_references, s.co_cited)),
            Some((1, 0))
        );
        assert_eq!(
            pair("arxiv:2401.00001", "paper:neurips-2025-1").map(|s| (s.shared_references, s.co_cited)),
            Some((0, 1))
        );
        assert_eq!((network.papers_read, network.papers_in_library), (3, 5));
        let json = serde_json::to_value(&network).unwrap();
        assert_eq!(json["references"][0]["link"], serde_json::json!({"kind": "in_library", "paper": "neurips-2025-1"}));
        assert_eq!(json["built"], "2026-11-26");
    }

    #[test]
    fn a_seed_not_read_yet_still_shows_who_cites_it() {
        let papers = library();
        let linker = Linker::new(&papers);
        let read = read_library(&linker);
        let built = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
        let network = network("neurips-2025-1", &linker, &read, None, built);
        assert_eq!(network.seed_state, SeedState::NotRead);
        assert!(network.references.is_empty());
        assert_eq!(network.totals.citations, 3);
        assert_eq!(network.totals.citations_in_run, None);
    }

    #[test]
    fn communities_count_citations_from_the_run_and_the_works_they_cite() {
        let papers = library();
        let linker = Linker::new(&papers);
        let read = read_library(&linker);
        let run: Vec<String> = papers.iter().map(|paper| paper.id.clone()).collect();
        let members = |path: &str, ids: &[&str], children: Vec<Members>| Members {
            tree_path: path.to_owned(),
            papers: ids.iter().map(|id| (*id).to_owned()).collect(),
            children,
        };
        let tree = [
            members(
                "c0",
                &["2505.11792", "neurips-2025-1", "neurips-2025-3"],
                vec![
                    members("c0.0", &["2505.11792"], vec![]),
                    members("c0.1", &["neurips-2025-1", "neurips-2025-3"], vec![]),
                ],
            ),
            members("c1", &["neurips-2025-2", "neurips-2025-4"], vec![]),
        ];
        let figures = community_citations(&read, &run, &tree);
        assert_eq!((figures.papers, figures.papers_read), (5, 3));
        let c0 = &figures.communities[0];
        // the seed is cited by 2 and 4; paper 1 by the seed, 2 and 4: five citations, one from inside (the seed)
        assert_eq!((c0.papers, c0.papers_read, c0.cited_by, c0.cited_by_inside), (3, 1, 5, 1));
        // the seed cites paper 1, the arXiv work and an unresolved one (paper 1 listed twice counts once)
        assert_eq!((c0.cites, c0.cites_in_library), (3, 1));
        assert_eq!(c0.children[1].papers_read, 0, "nothing of it read: the interface says so rather than 0");
        assert_eq!((c0.children[1].cited_by, c0.children[1].cited_by_inside), (3, 0));
        let c1 = &figures.communities[1];
        assert_eq!((c1.cited_by, c1.cited_by_inside, c1.cites, c1.cites_in_library), (1, 1, 4, 3));
    }

    #[test]
    fn citations_from_papers_outside_the_run_are_not_counted() {
        let papers = library();
        let linker = Linker::new(&papers);
        let read = read_library(&linker);
        let run = vec!["2505.11792".to_owned(), "neurips-2025-1".to_owned()];
        let figures = community_citations(
            &read,
            &run,
            &[Members { tree_path: "c0".to_owned(), papers: run.clone(), children: vec![] }],
        );
        assert_eq!((figures.papers_read, figures.communities[0].cited_by), (1, 1));
    }

    #[test]
    fn linking_the_network_and_community_figures_are_the_same_on_one_thread_and_on_many() {
        // 300 papers whose titles share words, each citing 12 others by title, by id, or as a near miss
        let words = ["sparse", "attention", "graph", "kernel", "learning", "robust", "neural", "models", "scale"];
        let papers: Vec<LibraryPaper> = (0..300)
            .map(|index| {
                let title: Vec<&str> = (0..6).map(|place| words[(index * 7 + place * index / 3 + place) % 9]).collect();
                LibraryPaper {
                    arxiv: (index % 4 == 0).then(|| format!("2501.{index:05}")),
                    ..paper(&format!("p{index}"), &format!("{} {index}", title.join(" ")), 2020 + (index % 5) as u16)
                }
            })
            .collect();
        let lists: Vec<(String, Vec<Reference>)> = (0..300)
            .map(|index| {
                let references = (1..=12)
                    .map(|step| {
                        let other = &papers[(index * 31 + step * 17) % 300];
                        match step % 3 {
                            0 => Reference { arxiv: other.arxiv.clone(), ..cited(&other.title, 2024) },
                            1 => cited(&other.title, other.year.unwrap_or(2020)),
                            _ => cited(&format!("{} revisited", other.title), 2023),
                        }
                    })
                    .collect();
                (format!("p{index}"), references)
            })
            .collect();
        let run: Vec<String> = (0..300).map(|index| format!("p{index}")).collect();
        let tree: Vec<Members> = (0..6)
            .map(|community| {
                let members: Vec<String> = run.iter().skip(community).step_by(6).cloned().collect();
                let half = members.len() / 2;
                let child = |part: usize, papers: &[String]| Members {
                    tree_path: format!("c{community}.{part}"),
                    papers: papers.to_vec(),
                    children: vec![],
                };
                Members {
                    tree_path: format!("c{community}"),
                    children: vec![child(0, &members[..half]), child(1, &members[half..])],
                    papers: members,
                }
            })
            .collect();
        let built = NaiveDate::from_ymd_opt(2026, 11, 27).unwrap();
        let work = || {
            let linker = Linker::new(&papers);
            let read = linker.read_all(lists.clone());
            let around: Vec<CitationNetwork> =
                ["p0", "p17", "p250"].iter().map(|seed| network(seed, &linker, &read, None, built)).collect();
            (community_citations(&read, &run, &tree), around, read)
        };
        let (one, many) = crate::threads::on_one_and_many(work);
        assert_eq!(one, many);
        let (_, around, read) = one;
        assert_eq!(read.iter().map(|paper| paper.paper.as_str()).collect::<Vec<_>>(), run, "in the order given");
        assert!(around.iter().all(|network| network.totals.citations > 0), "the papers do cite each other");
    }
}
