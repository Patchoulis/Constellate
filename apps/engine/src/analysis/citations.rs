//! Who cites whom, among the papers Constellate has read: the citation network around one paper, and what
//! communities cite and are cited.
//!
//! Everything here is worked out from the reference lists of the papers whose PDFs Constellate has read
//! ([`crate::text::pdf::references`]), and from nothing else. It is not the whole literature: a paper's *references*
//! are complete once its own PDF is read, but the papers that *cite* it are only those among the read ones. The
//! interface says so wherever it shows these figures.
//!
//! The papers that cite are every paper of the library whose reference list has been read, whether or not it is
//! in the run being looked at: a run's figures say how many of the citations come from inside a community, from
//! the rest of the run and from the library outside the run. What cannot be counted is a paper that is not in the
//! library at all: nothing of it has been read, and no outside citation service is asked.
//!
//! Four steps, each a plain function of what it is given:
//!
//! 1. **Linking** ([`Linker`]): each reference is matched to a paper of the user's library, by arXiv id or DOI
//!    when the entry has one, else by its title. Titles are compared by their letters and digits only
//!    ([`title_key`]), so line breaks and hyphens from the PDF make no difference; when that finds nothing, a
//!    close title (nine in ten of the words shared, [`SIMILAR_TITLES`]) from the same year or the next counts,
//!    which survives a word lost at a page break. Below that nothing is linked: two papers are never taken to be
//!    one on a resemblance. A reference that is not in the library but names an arXiv id is "on arXiv"; any other
//!    is unresolved, shown with what was read of it. ([`crate::sources::arxiv::citations`] can look the unresolved ones
//!    up on arXiv.)
//! 2. **The library linked** ([`LinkedLibrary`]): every read list linked once and kept, with who cites each work.
//!    It is brought up to date from what changed alone ([`LinkedLibrary::update`]): a list read or changed since is
//!    linked; a paper added since links again only the references it could be ([`Linker::might_link`]), which
//!    gives what linking everything again would; only a paper removed, or one whose title, year or ids changed,
//!    links every list again.
//! 3. **The network around a seed paper** ([`network`]): its references (prior work), the read papers whose
//!    references include it (derivative work), the citations between those neighbours, so that the picture is a
//!    network and not a star, and two similarities between neighbours that tools like Connected Papers draw:
//!    *bibliographic coupling* (how many references two papers share) and *co-citation* (how many papers cite
//!    both).
//! 4. **What a run and its communities cite and are cited** ([`community_citations`]), over the nested community
//!    tree.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::NaiveDate;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::text::pdf::references::{Confidence, Reference, title_key, title_words};

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

    /// Could this reference be linked to one of these papers? True when one of them has its arXiv id or DOI, the
    /// same title's letters, or nine in ten of its title's words (whatever the years): what [`Linker::link`]
    /// looks at, and more. A library that gains these papers links again only the references this says yes to:
    /// any other is linked as before, since no paper it could be was added.
    pub fn might_link(&self, reference: &Reference) -> bool {
        let by_id = reference.arxiv.as_ref().is_some_and(|arxiv| self.by_arxiv.contains_key(&arxiv.to_lowercase()))
            || reference.doi.as_ref().is_some_and(|doi| self.by_doi.contains_key(&doi.to_lowercase()));
        let Some(title) = reference.title.as_deref() else { return by_id };
        let key = title_key(title);
        if by_id || (!key.is_empty() && self.by_key.contains_key(&key)) {
            return true;
        }
        let words = word_set(title);
        // a paper sharing nine words in ten shares one of any tenth-plus-one of them (see by_close_title)
        let needed = ((1.0 - SIMILAR_TITLES) * words.len() as f64 + 1e-9).floor() as usize + 1;
        words.len() >= FUZZY_WORDS
            && words
                .iter()
                .take(needed)
                .filter_map(|word| self.postings.get(word))
                .flatten()
                .any(|&index| word_overlap(&words, &self.words[index]) >= SIMILAR_TITLES)
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
    /// Is it one of the papers of the run asked about? None when no run was asked about, or when the work is not
    /// a paper of the library.
    pub in_run: Option<bool>,
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
    /// Of those in the library, how many are papers of the run asked about, and how many are not, when one was.
    pub references_in_run: Option<usize>,
    pub references_outside_run: Option<usize>,
    pub references_on_arxiv: usize,
    pub references_unresolved: usize,
    /// Read papers of the library that cite the seed, in the run asked about or not.
    pub citations: usize,
    /// Of those, how many are listed ([`MAX_CITATIONS`] at most).
    pub citations_shown: usize,
    /// Of those, how many are papers of the run asked about, and how many are papers of the library outside it,
    /// when one was.
    pub citations_in_run: Option<usize>,
    pub citations_outside_run: Option<usize>,
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
    /// When reading the seed's references looked works up on arXiv and stopped before every one was asked
    /// about: why. None when nothing was looked up, or every work worth asking about was.
    pub lookups_stopped: Option<LookupsStopped>,
}

/// Why looking a paper's references up on arXiv stopped before every one was asked about: `lookups_stopped` of
/// `CitationNetwork` in the contracts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LookupsStopped {
    /// The most one request asks was asked; `left` works are asked about when the references are read again.
    AtMost { left: usize },
    /// arXiv refused or could not be reached; `left` works are asked about when the references are read again.
    Failed { why: String, left: usize },
}

/// Which version of a paper's kept reference list this is, as its store numbers them: a list whose stamp is the
/// one kept here is not linked again.
pub type Stamp = i64;

/// A read list as kept here: the version linked, and the list linked.
#[derive(Clone, Debug, PartialEq)]
struct KeptList {
    stamp: Stamp,
    read: ReadPaper,
}

/// Every read reference list of the library, linked once and kept, and who cites each work: what the network
/// around a paper and a run's citation figures are counted from.
///
/// It is brought up to date by [`LinkedLibrary::update`] from what changed alone, and the answer is what linking
/// the whole library afresh gives (a test holds it to that), on one thread or many.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkedLibrary {
    /// The library's papers, in the order the store lists them (by id): the order the linker reads them in.
    papers: Vec<LibraryPaper>,
    by_id: HashMap<String, usize>,
    /// By the id of the paper whose list it is.
    lists: BTreeMap<String, KeptList>,
    /// Work key ([`work_key`]) -> the read papers whose lists cite it; a paper is never its own citer.
    citers: HashMap<String, BTreeSet<String>>,
    /// Raised whenever anything linked changed: what is counted from it is counted again only then.
    revision: u64,
}

/// What bringing the linked library up to date did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Relinked {
    /// Lists linked whole because they were new or changed (read again, or looked up on arXiv) since.
    pub lists_linked: usize,
    /// References of lists kept from before linked again because a paper added since might be what they cite.
    pub references_relinked: usize,
    /// Every list linked again: a paper was removed, or what links a reference to it (title, year, ids) changed.
    pub all_relinked: bool,
    /// Lists no longer kept, let go.
    pub lists_dropped: usize,
}

/// What linking reads of a paper: a change to anything else (its authors, its address) links nothing again.
fn linked_by(paper: &LibraryPaper) -> (&str, &str, Option<u16>, Option<&str>, Option<&str>) {
    (&paper.title, &paper.id, paper.year, paper.arxiv.as_deref(), paper.doi.as_deref())
}

/// Add `read`'s citations to `citers`, or (`add` false) take them out.
fn index(citers: &mut HashMap<String, BTreeSet<String>>, read: &ReadPaper, add: bool) {
    let own = paper_key(&read.paper);
    for reference in &read.references {
        let key = work_key(reference);
        if key == own {
            continue;
        }
        if add {
            citers.entry(key).or_default().insert(read.paper.clone());
        } else if let Some(citing) = citers.get_mut(&key) {
            citing.remove(&read.paper);
            if citing.is_empty() {
                citers.remove(&key);
            }
        }
    }
}

impl LinkedLibrary {
    /// A library linked afresh: `papers`, and the reference list of each paper in `lists`.
    pub fn of(papers: Vec<LibraryPaper>, lists: Vec<(String, Vec<Reference>)>) -> Self {
        let stamps = lists.iter().map(|(paper, _)| (paper.clone(), 0)).collect();
        let mut linked = Self::default();
        let Ok(_) = linked.update::<std::convert::Infallible>(Some(papers), Some(stamps), |_| Ok(lists));
        linked
    }

    /// Bring the linked library up to date: `papers` is the library's papers as they are now, or None when they
    /// are known not to have changed; `stamps` every kept list's paper and [`Stamp`], or None when no list is known
    /// to have changed. `load` is asked for the lists that are new or changed, by paper id, and only for those.
    ///
    /// A paper added links again only the references of kept lists that it might be ([`Linker::might_link`]):
    /// whatever else a reference could be linked to was there before, so it is linked as before. A paper removed,
    /// or changed in what links to it, links every list again.
    pub fn update<E>(
        &mut self,
        papers: Option<Vec<LibraryPaper>>,
        stamps: Option<Vec<(String, Stamp)>>,
        load: impl FnOnce(&[String]) -> Result<Vec<(String, Vec<Reference>)>, E>,
    ) -> Result<Relinked, E> {
        let mut done = Relinked::default();
        let mut added: Vec<LibraryPaper> = Vec::new();
        // what changed is worked out, and the lists asked for, before anything kept is touched: a list that cannot
        // be had leaves the library linked as it was
        let papers = papers.filter(|papers| {
            let before: HashMap<&str, &LibraryPaper> =
                self.papers.iter().map(|paper| (paper.id.as_str(), paper)).collect();
            let mut still_there = 0;
            let mut changed = false;
            for paper in papers {
                match before.get(paper.id.as_str()) {
                    None => added.push(paper.clone()),
                    Some(old) => {
                        still_there += 1;
                        done.all_relinked |= linked_by(old) != linked_by(paper);
                        changed |= *old != paper;
                    }
                }
            }
            done.all_relinked |= still_there < before.len();
            changed || done.all_relinked || !added.is_empty()
        });
        // the lists gone, and those new or changed since with the stamp each is now at
        let (gone, wanted): (Vec<String>, BTreeMap<String, Stamp>) = match stamps {
            None => (Vec::new(), BTreeMap::new()),
            Some(stamps) => {
                let now: HashSet<&str> = stamps.iter().map(|(paper, _)| paper.as_str()).collect();
                let gone = self.lists.keys().filter(|paper| !now.contains(paper.as_str())).cloned().collect();
                let wanted = stamps
                    .into_iter()
                    .filter(|(paper, stamp)| self.lists.get(paper).is_none_or(|kept| kept.stamp != *stamp))
                    .collect();
                (gone, wanted)
            }
        };
        let loaded = if wanted.is_empty() { Vec::new() } else { load(&wanted.keys().cloned().collect::<Vec<_>>())? };
        if papers.is_none() && wanted.is_empty() && gone.is_empty() {
            return Ok(done);
        }
        if let Some(papers) = papers {
            self.by_id = papers.iter().enumerate().map(|(index, paper)| (paper.id.clone(), index)).collect();
            self.papers = papers;
        }
        for paper in &gone {
            if let Some(kept) = self.lists.remove(paper) {
                index(&mut self.citers, &kept.read, false);
            }
        }
        done.lists_dropped = gone.len();

        let Self { papers, lists, citers, revision, .. } = self;
        // a change of a paper's address or authors alone links nothing
        if done.all_relinked || !added.is_empty() || !loaded.is_empty() {
            let linker = Linker::new(papers);
            // the lists kept from before (and not read anew) that a change of the library links again, whole or in part
            let unchanged = |paper: &&String| !wanted.contains_key(*paper);
            let relinked: Vec<(String, ReadPaper, usize)> = if done.all_relinked {
                lists
                    .par_iter()
                    .filter(|(paper, _)| unchanged(paper))
                    .map(|(paper, kept)| {
                        let references = kept.read.references.iter().map(|linked| linked.reference.clone()).collect();
                        (paper.clone(), linker.read(paper, references), kept.read.references.len())
                    })
                    .collect()
            } else if !added.is_empty() {
                let probe = Linker::new(&added);
                lists
                    .par_iter()
                    .filter(|(paper, _)| unchanged(paper))
                    .filter_map(|(paper, kept)| {
                        let mut read = kept.read.clone();
                        let mut again = 0;
                        for linked in &mut read.references {
                            if probe.might_link(&linked.reference) {
                                linked.link = linker.link(&linked.reference);
                                again += 1;
                            }
                        }
                        (again > 0).then(|| (paper.clone(), read, again))
                    })
                    .collect()
            } else {
                Vec::new()
            };
            for (paper, read, again) in relinked {
                done.references_relinked += again;
                if let Some(kept) = lists.get_mut(&paper) {
                    if kept.read != read {
                        index(citers, &kept.read, false);
                        index(citers, &read, true);
                        kept.read = read;
                    }
                }
            }

            done.lists_linked = loaded.len();
            for read in linker.read_all(loaded) {
                let Some(&stamp) = wanted.get(&read.paper) else { continue };
                if let Some(old) = lists.remove(&read.paper) {
                    index(citers, &old.read, false);
                }
                index(citers, &read, true);
                lists.insert(read.paper.clone(), KeptList { stamp, read });
            }
        }
        // a list asked for that did not come was removed meanwhile
        for paper in wanted.keys() {
            if lists.get(paper).is_some_and(|kept| kept.stamp != wanted[paper]) {
                if let Some(kept) = lists.remove(paper) {
                    index(citers, &kept.read, false);
                    done.lists_dropped += 1;
                }
            }
        }
        *revision += 1;
        Ok(done)
    }

    /// Raised whenever anything linked changed.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The library's paper with this id.
    pub fn paper(&self, id: &str) -> Option<&LibraryPaper> {
        self.by_id.get(id).map(|&index| &self.papers[index])
    }

    /// The paper's reference list, linked, when it has been read.
    pub fn read(&self, id: &str) -> Option<&ReadPaper> {
        self.lists.get(id).map(|kept| &kept.read)
    }

    /// The read papers that cite the work of this key ([`work_key`]).
    pub fn citers(&self, key: &str) -> Option<&BTreeSet<String>> {
        self.citers.get(key)
    }

    /// Papers whose reference list has been read.
    pub fn papers_read(&self) -> usize {
        self.lists.len()
    }

    pub fn papers_in_library(&self) -> usize {
        self.papers.len()
    }
}

fn neighbour_from_library(paper: &LibraryPaper, cited_by: usize, in_run: Option<bool>) -> Neighbour {
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
        in_run,
        entry: None,
    }
}

fn neighbour_from_reference(
    linked: &LinkedReference,
    library: &LinkedLibrary,
    cited_by: usize,
    run: Option<&HashSet<String>>,
) -> Neighbour {
    let reference = &linked.reference;
    if let Link::InLibrary { paper } = &linked.link {
        if let Some(paper) = library.paper(paper) {
            let in_run = run.map(|run| run.contains(&paper.id));
            return Neighbour { entry: Some(reference.raw.clone()), ..neighbour_from_library(paper, cited_by, in_run) };
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
        in_run: None,
        entry: Some(reference.raw.clone()),
    }
}

/// The network around `seed`, from every read reference list of the library (`library`). With `run` (the ids of
/// a run's papers), each paper of the library in it says whether it is of that run, and the totals how many are.
pub fn network(
    seed: &str,
    library: &LinkedLibrary,
    run: Option<&HashSet<String>>,
    built: NaiveDate,
) -> CitationNetwork {
    let cited = |key: &str| library.citers(key).map_or(0, BTreeSet::len);
    let seed_key = paper_key(seed);
    let seed_paper = library.read(seed);

    let mut seen = HashSet::new();
    let references: Vec<Neighbour> = seed_paper
        .map(|paper| paper.references.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|linked| {
            let key = work_key(linked);
            key != seed_key && seen.insert(key)
        })
        .map(|linked| neighbour_from_reference(linked, library, cited(&work_key(linked)), run))
        .collect();

    let citing: Vec<&str> =
        library.citers(&seed_key).map(|papers| papers.iter().map(String::as_str).collect()).unwrap_or_default();
    let mut citations: Vec<Neighbour> = citing
        .iter()
        .filter_map(|id| library.paper(id))
        .map(|paper| {
            neighbour_from_library(paper, cited(&paper_key(&paper.id)), run.map(|run| run.contains(&paper.id)))
        })
        .collect();
    citations.sort_by(|a, b| b.cited_by.cmp(&a.cited_by).then(b.year.cmp(&a.year)).then(a.title.cmp(&b.title)));
    let citations_total = citations.len();
    let citations_in_run =
        run.map(|run| citations.iter().filter(|neighbour| run.contains(paper_id_of(&neighbour.key))).count());
    citations.truncate(MAX_CITATIONS);

    let count = |wanted: &dyn Fn(&Neighbour) -> bool| references.iter().filter(|neighbour| wanted(neighbour)).count();
    let in_library = |neighbour: &Neighbour| matches!(neighbour.link, Link::InLibrary { .. });
    let totals = Totals {
        references: references.len(),
        references_in_library: count(&in_library),
        references_in_run: run.map(|_| count(&|neighbour| neighbour.in_run == Some(true))),
        references_outside_run: run
            .map(|_| count(&|neighbour| in_library(neighbour) && neighbour.in_run != Some(true))),
        references_on_arxiv: count(&|neighbour| matches!(neighbour.link, Link::OnArxiv { .. })),
        references_unresolved: count(&|neighbour| matches!(neighbour.link, Link::Unresolved)),
        citations: citations_total,
        citations_shown: citations.len(),
        citations_in_run,
        citations_outside_run: citations_in_run.map(|inside| citations_total - inside),
    };

    let keys: HashSet<&str> = references.iter().chain(&citations).map(|neighbour| neighbour.key.as_str()).collect();
    let read_of = |key: &str| key.strip_prefix("paper:").and_then(|id| library.read(id));
    // each read neighbour's references among the neighbours, the seed left out: the edges and the coupling
    let cites: HashMap<&str, BTreeSet<String>> = keys
        .iter()
        .filter_map(|&key| read_of(key).map(|paper| (key, paper)))
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
        library
            .citers(key)
            .map(|papers| papers.iter().map(String::as_str).filter(|paper| *paper != seed).collect())
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
        papers_read: library.papers_read(),
        papers_in_library: library.papers_in_library(),
        built,
        lookups_stopped: None,
    }
}

/// The id of the library paper a neighbour's key names ([`paper_key`]).
fn paper_id_of(key: &str) -> &str {
    key.strip_prefix("paper:").unwrap_or(key)
}

/// A community of the run's tree, by the ids of its papers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Members {
    pub tree_path: String,
    pub papers: Vec<String>,
    pub children: Vec<Members>,
}

/// What a group of a run's papers (a community, or the run itself) cites and how often it is cited, from every
/// read reference list of the library. Each split adds up to its total.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub papers: usize,
    /// Of its papers, how many have had their reference list read: what `cites` is counted over.
    pub papers_read: usize,
    /// Citations its papers receive from the read papers of the library, in the run or not: each pair of citing
    /// and cited paper once, a paper never citing itself.
    pub cited_by: usize,
    /// Of those, from its own papers.
    pub cited_by_inside: usize,
    /// From the run's other papers (none, for the run itself).
    pub cited_by_rest_of_run: usize,
    /// From papers of the library that are not in the run.
    pub cited_by_outside_run: usize,
    /// Distinct works its read papers cite, resolved or not ([`work_key`]).
    pub cites: usize,
    /// Of those, papers of the user's library: `cites_in_run` and `cites_outside_run` together.
    pub cites_in_library: usize,
    /// Papers of the run.
    pub cites_in_run: usize,
    /// Papers of the library that are not in the run.
    pub cites_outside_run: usize,
    /// Not in the library, but on arXiv.
    pub cites_on_arxiv: usize,
    /// Not in the library, and not known to arXiv.
    pub cites_unresolved: usize,
}

/// A community's figures: `CountedCommunity` in the contracts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunityCitations {
    pub tree_path: String,
    #[serde(flatten)]
    pub counts: Counts,
    pub children: Vec<CommunityCitations>,
}

/// The citation figures of a run (`RunCitations` in the contracts): the run's as one group (`cited_by_inside`
/// counts the citations from its own papers), and each community's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCitations {
    #[serde(flatten)]
    pub counts: Counts,
    /// The papers of the library, and of those, how many have had their reference list read: whose citations
    /// are counted.
    pub library_papers: usize,
    pub library_papers_read: usize,
    pub communities: Vec<CommunityCitations>,
}

/// The figures of the papers `papers`, the run's papers being `run`.
fn counts_of(papers: &[String], library: &LinkedLibrary, run: &HashSet<&str>) -> Counts {
    let members: HashSet<&str> = papers.iter().map(String::as_str).collect();
    let mut counts = Counts { papers: papers.len(), ..Counts::default() };
    let mut works = HashSet::new();
    for paper in papers.iter().filter_map(|id| library.read(id)) {
        counts.papers_read += 1;
        works.extend(paper.references.iter().map(work_key));
    }
    for citer in members.iter().filter_map(|paper| library.citers(&paper_key(paper))).flatten() {
        counts.cited_by += 1;
        match (members.contains(citer.as_str()), run.contains(citer.as_str())) {
            (true, _) => counts.cited_by_inside += 1,
            (false, true) => counts.cited_by_rest_of_run += 1,
            (false, false) => counts.cited_by_outside_run += 1,
        }
    }
    counts.cites = works.len();
    for work in &works {
        if let Some(paper) = work.strip_prefix("paper:") {
            counts.cites_in_library += 1;
            if run.contains(paper) {
                counts.cites_in_run += 1;
            } else {
                counts.cites_outside_run += 1;
            }
        } else if work.starts_with("arxiv:") {
            counts.cites_on_arxiv += 1;
        } else {
            counts.cites_unresolved += 1;
        }
    }
    counts
}

/// How much the run (`run`, its papers' ids) and each community of `tree` cite and are cited, from every read
/// reference list of `library`: citations from inside a community, from the rest of the run and from the library
/// outside the run are counted apart.
pub fn community_citations(library: &LinkedLibrary, run: &[String], tree: &[Members]) -> RunCitations {
    fn of(community: &Members, library: &LinkedLibrary, run: &HashSet<&str>) -> CommunityCitations {
        CommunityCitations {
            tree_path: community.tree_path.clone(),
            counts: counts_of(&community.papers, library, run),
            children: community.children.par_iter().map(|child| of(child, library, run)).collect(),
        }
    }
    let in_run: HashSet<&str> = run.iter().map(String::as_str).collect();
    // Every community, and every sub-community, is counted on a thread of its own: its figures are counts over
    // its own papers in maps nobody writes any more, collected in the tree's order.
    let (counts, communities) = rayon::join(
        || counts_of(run, library, &in_run),
        || tree.par_iter().map(|community| of(community, library, &in_run)).collect(),
    );
    RunCitations {
        counts,
        library_papers: library.papers_in_library(),
        library_papers_read: library.papers_read(),
        communities,
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
    fn lists_of(papers: &[LibraryPaper]) -> Vec<(String, Vec<Reference>)> {
        let outside = Reference { arxiv: Some("2401.00001".to_owned()), ..cited("An outside work on arXiv", 2024) };
        let seed = "2505.11792";
        let of = |id: &str| {
            let paper = papers.iter().find(|paper| paper.id == id).unwrap();
            cited(&paper.title, paper.year.unwrap())
        };
        vec![
            (
                seed.to_owned(),
                vec![
                    of("neurips-2025-1"),
                    outside.clone(),
                    of("neurips-2025-1"),
                    cited("A raw-ish entry nobody has", 2001),
                ],
            ),
            ("neurips-2025-2".to_owned(), vec![of(seed), of("neurips-2025-1"), outside]),
            ("neurips-2025-4".to_owned(), vec![of(seed), of("neurips-2025-1"), of("neurips-2025-2")]),
        ]
    }

    fn linked_library() -> LinkedLibrary {
        let papers = library();
        let lists = lists_of(&papers);
        LinkedLibrary::of(papers, lists)
    }

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    fn members(path: &str, papers: &[&str], children: Vec<Members>) -> Members {
        Members { tree_path: path.to_owned(), papers: ids(papers), children }
    }

    /// Every split of `counts` adds up to its total.
    fn adds_up(counts: &Counts) {
        assert_eq!(
            counts.cited_by_inside + counts.cited_by_rest_of_run + counts.cited_by_outside_run,
            counts.cited_by,
            "{counts:?}"
        );
        assert_eq!(
            counts.cites_in_run + counts.cites_outside_run + counts.cites_on_arxiv + counts.cites_unresolved,
            counts.cites,
            "{counts:?}"
        );
        assert_eq!(counts.cites_in_run + counts.cites_outside_run, counts.cites_in_library, "{counts:?}");
    }

    fn all_add_up(figures: &RunCitations) {
        fn each(community: &CommunityCitations) {
            adds_up(&community.counts);
            community.children.iter().for_each(each);
        }
        adds_up(&figures.counts);
        figures.communities.iter().for_each(each);
    }

    #[test]
    fn the_network_of_a_seed_has_its_references_its_citations_and_the_links_between_them() {
        let library = linked_library();
        let run: HashSet<String> = ["neurips-2025-4".to_owned()].into();
        let built = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
        let network = network("2505.11792", &library, Some(&run), built);
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
                references_in_run: Some(0),
                references_outside_run: Some(1),
                references_on_arxiv: 1,
                references_unresolved: 1,
                citations: 2,
                citations_shown: 2,
                citations_in_run: Some(1),
                citations_outside_run: Some(1),
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
    fn a_paper_outside_the_run_that_cites_the_seed_is_in_its_network_marked_outside() {
        let library = linked_library();
        // the run has the seed and paper 4; paper 2, of the library but not of the run, cites the seed too
        let run: HashSet<String> = ["2505.11792".to_owned(), "neurips-2025-4".to_owned()].into();
        let built = NaiveDate::from_ymd_opt(2026, 11, 28).unwrap();
        let network = network("2505.11792", &library, Some(&run), built);
        let marked: Vec<(&str, Option<bool>)> =
            network.citations.iter().map(|work| (work.key.as_str(), work.in_run)).collect();
        assert_eq!(marked, [("paper:neurips-2025-2", Some(false)), ("paper:neurips-2025-4", Some(true))]);
        assert_eq!((network.totals.citations_in_run, network.totals.citations_outside_run), (Some(1), Some(1)));
        // what the seed cites: paper 1 is of the library and not of the run; the others are not papers of it
        let cites: Vec<Option<bool>> = network.references.iter().map(|work| work.in_run).collect();
        assert_eq!(cites, [Some(false), None, None]);
        // without a run, nothing is said of one
        let any = super::network("2505.11792", &library, None, built);
        assert!(any.citations.iter().chain(&any.references).all(|work| work.in_run.is_none()));
        assert_eq!((any.totals.citations, any.totals.citations_outside_run), (2, None));
    }

    #[test]
    fn a_seed_not_read_yet_still_shows_who_cites_it() {
        let library = linked_library();
        let built = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
        let network = network("neurips-2025-1", &library, None, built);
        assert_eq!(network.seed_state, SeedState::NotRead);
        assert!(network.references.is_empty());
        assert_eq!(network.totals.citations, 3);
        assert_eq!(network.totals.citations_in_run, None);
    }

    #[test]
    fn communities_count_citations_from_the_run_and_the_works_they_cite() {
        let library = linked_library();
        let run: Vec<String> = library.papers.iter().map(|paper| paper.id.clone()).collect();
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
        let figures = community_citations(&library, &run, &tree);
        all_add_up(&figures);
        assert_eq!((figures.counts.papers, figures.counts.papers_read), (5, 3));
        assert_eq!((figures.library_papers, figures.library_papers_read), (5, 3));
        let c0 = &figures.communities[0].counts;
        // the seed is cited by 2 and 4; paper 1 by the seed, 2 and 4: five citations, one from inside (the seed)
        assert_eq!((c0.papers, c0.papers_read, c0.cited_by, c0.cited_by_inside), (3, 1, 5, 1));
        assert_eq!((c0.cited_by_rest_of_run, c0.cited_by_outside_run), (4, 0), "the whole library is the run");
        // the seed cites paper 1, the arXiv work and an unresolved one (paper 1 listed twice counts once)
        assert_eq!(
            (c0.cites, c0.cites_in_library, c0.cites_in_run, c0.cites_on_arxiv, c0.cites_unresolved),
            (3, 1, 1, 1, 1)
        );
        let unread = &figures.communities[0].children[1].counts;
        assert_eq!(unread.papers_read, 0, "nothing of it read: the interface says so rather than 0");
        assert_eq!((unread.cited_by, unread.cited_by_inside), (3, 0));
        let c1 = &figures.communities[1].counts;
        assert_eq!((c1.cited_by, c1.cited_by_inside, c1.cites, c1.cites_in_library), (1, 1, 4, 3));
        // the run as a whole: every citation is from inside it
        assert_eq!((figures.counts.cited_by, figures.counts.cited_by_inside), (6, 6));
    }

    #[test]
    fn papers_of_the_library_outside_the_run_are_counted_apart_and_the_three_add_up() {
        let library = linked_library();
        // the run has the seed and papers 1 and 3; papers 2 and 4 are of the library only, and both cite it
        let run = ids(&["2505.11792", "neurips-2025-1", "neurips-2025-3"]);
        let tree = [members(
            "c0",
            &["2505.11792", "neurips-2025-1", "neurips-2025-3"],
            vec![
                members("c0.0", &["2505.11792"], vec![]),
                members("c0.1", &["neurips-2025-1", "neurips-2025-3"], vec![]),
            ],
        )];
        let figures = community_citations(&library, &run, &tree);
        all_add_up(&figures);
        let split = |counts: &Counts| {
            (counts.cited_by, counts.cited_by_inside, counts.cited_by_rest_of_run, counts.cited_by_outside_run)
        };
        // the seed cites 1 (inside the run); 2 and 4 cite the seed and 1 (outside it)
        assert_eq!(split(&figures.counts), (5, 1, 0, 4));
        assert_eq!((figures.counts.papers_read, figures.library_papers_read), (1, 3));
        let seed = &figures.communities[0].children[0].counts;
        assert_eq!(split(seed), (2, 0, 0, 2));
        let rest = &figures.communities[0].children[1].counts;
        assert_eq!(split(rest), (3, 0, 1, 2), "1 is cited by the seed, of the rest of the run, and by 2 and 4");

        // a run of paper 4 alone: what it cites is in the library, outside the run
        let alone =
            community_citations(&library, &ids(&["neurips-2025-4"]), &[members("c0", &["neurips-2025-4"], vec![])]);
        all_add_up(&alone);
        let four = &alone.communities[0].counts;
        assert_eq!((four.cites, four.cites_in_run, four.cites_outside_run, four.cites_on_arxiv), (3, 0, 3, 0));
        assert_eq!(four.cited_by, 0);
    }

    #[test]
    fn figures_saved_without_the_splits_still_read() {
        // a community's figures as runs kept them before the splits were counted
        let before = serde_json::json!({
            "papers_read": 3, "cited_by": 5, "cited_by_inside": 1, "cites": 3, "cites_in_library": 1
        });
        #[derive(Deserialize)]
        struct Kept {
            papers_read: usize,
            cited_by: usize,
            #[serde(default)]
            cited_by_outside_run: Option<usize>,
        }
        let kept: Kept = serde_json::from_value(before).unwrap();
        assert_eq!((kept.papers_read, kept.cited_by, kept.cited_by_outside_run), (3, 5, None));
        // and what is counted now has every field the contracts name
        let figures =
            community_citations(&linked_library(), &ids(&["2505.11792"]), &[members("c0", &["2505.11792"], vec![])]);
        let json = serde_json::to_value(&figures).unwrap();
        for field in
            ["cited_by_rest_of_run", "cited_by_outside_run", "cites_in_run", "cites_outside_run", "library_papers_read"]
        {
            assert!(json.get(field).is_some(), "{field}");
            assert!(json["communities"][0].get(field).is_some() || field.starts_with("library"), "{field}");
        }
    }

    /// The lists of `library`, linked afresh: what bringing it up to date must give.
    fn afresh(papers: &[LibraryPaper], lists: &[(String, Vec<Reference>)]) -> LinkedLibrary {
        LinkedLibrary::of(papers.to_vec(), lists.to_vec())
    }

    /// The same links and citers, whatever revision either is at.
    fn same_links(one: &LinkedLibrary, other: &LinkedLibrary) {
        let links = |linked: &LinkedLibrary| -> Vec<ReadPaper> {
            linked.lists.values().map(|kept| kept.read.clone()).collect()
        };
        assert_eq!(links(one), links(other));
        assert_eq!(one.citers, other.citers);
        assert_eq!(one.papers, other.papers);
    }

    #[test]
    fn the_linked_library_is_kept_and_only_what_changed_is_linked_again() {
        let papers = library();
        let lists = lists_of(&papers);
        let stamps = |lists: &[(String, Vec<Reference>)], stamp: Stamp| -> Vec<(String, Stamp)> {
            lists.iter().map(|(paper, _)| (paper.clone(), stamp)).collect()
        };
        let mut linked = LinkedLibrary::default();
        let mut asked: Vec<Vec<String>> = Vec::new();
        let load = |wanted: &[String], asked: &mut Vec<Vec<String>>| {
            asked.push(wanted.to_vec());
            Ok::<_, ()>(lists.iter().filter(|(paper, _)| wanted.contains(paper)).cloned().collect())
        };
        // the first two lists read
        let first =
            linked.update(Some(papers.clone()), Some(stamps(&lists[..2], 1)), |wanted| load(wanted, &mut asked));
        assert_eq!(first.unwrap().lists_linked, 2);
        let revision = linked.revision();

        // nothing changed: nothing is read or linked, and the revision stays
        let again =
            linked.update(Some(papers.clone()), Some(stamps(&lists[..2], 1)), |wanted| load(wanted, &mut asked));
        assert_eq!(again.unwrap(), Relinked::default());
        assert_eq!((linked.revision(), asked.len()), (revision, 1), "not asked for a list");
        let unchanged = linked.update(None, None, |_| -> Result<_, ()> { panic!("nothing is asked for") });
        assert_eq!(unchanged.unwrap(), Relinked::default());

        // a newly read list: only it is asked for and linked, and the seed's network finds its citer
        let mut three = stamps(&lists[..2], 1);
        three.push((lists[2].0.clone(), 1));
        let read = linked.update(None, Some(three.clone()), |wanted| load(wanted, &mut asked)).unwrap();
        assert_eq!((read.lists_linked, read.references_relinked, read.all_relinked), (1, 0, false));
        assert_eq!(asked.last().unwrap(), &["neurips-2025-4".to_owned()]);
        assert!(linked.revision() > revision);
        assert!(linked.citers("paper:2505.11792").unwrap().contains("neurips-2025-4"));
        same_links(&linked, &afresh(&papers, &lists));

        // a list looked up on arXiv since (a new stamp) is read again, alone
        three[0].1 = 2;
        let looked = linked.update(None, Some(three.clone()), |wanted| load(wanted, &mut asked)).unwrap();
        assert_eq!((looked.lists_linked, asked.last().unwrap().as_slice()), (1, ["2505.11792".to_owned()].as_slice()));

        // a list gone is let go, and its citations with it
        let dropped = linked.update(None, Some(three[1..].to_vec()), |wanted| load(wanted, &mut asked)).unwrap();
        assert_eq!(dropped.lists_dropped, 1);
        assert!(!linked.citers("paper:neurips-2025-1").unwrap().contains("2505.11792"));
        same_links(&linked, &afresh(&papers, &lists[1..]));
    }

    #[test]
    fn a_paper_added_links_again_only_the_references_it_might_be_and_gives_what_linking_afresh_does() {
        let mut papers = library();
        let lists = {
            let mut lists = lists_of(&papers);
            // the seed also cites a work not in the library yet, by title
            lists[0].1.push(cited("Learning to Rank Research Papers with Graph Neural Networks", 2024));
            lists
        };
        let mut linked = afresh(&papers, &lists);
        assert_eq!(linked.read("2505.11792").unwrap().references[4].link, Link::Unresolved);
        let total: usize = lists.iter().map(|(_, references)| references.len()).sum();

        papers.push(paper("neurips-2025-5", "Learning to Rank Research Papers with Graph Neural Networks", 2024));
        papers.sort_by(|a, b| a.id.cmp(&b.id));
        let added = linked.update::<()>(Some(papers.clone()), None, |_| panic!("no list changed")).unwrap();
        assert!(!added.all_relinked);
        assert!(added.references_relinked > 0 && added.references_relinked < total, "{added:?}");
        assert_eq!(
            linked.read("2505.11792").unwrap().references[4].link,
            Link::InLibrary { paper: "neurips-2025-5".to_owned() }
        );
        same_links(&linked, &afresh(&papers, &lists));

        // a title changed, or a paper removed: every list is linked again
        papers[0].title = "Another title altogether for the first paper".to_owned();
        let changed = linked.update::<()>(Some(papers.clone()), None, |_| panic!("no list changed")).unwrap();
        assert!(changed.all_relinked);
        same_links(&linked, &afresh(&papers, &lists));
        papers.pop();
        let removed = linked.update::<()>(Some(papers.clone()), None, |_| panic!("no list changed")).unwrap();
        assert!(removed.all_relinked);
        same_links(&linked, &afresh(&papers, &lists));
        // an address changed: nothing is linked again, but the network shows the new one
        papers[1].url = Some("https://example.org/moved".to_owned());
        let moved = linked.update::<()>(Some(papers.clone()), None, |_| panic!("no list changed")).unwrap();
        assert_eq!(moved, Relinked::default());
        assert_eq!(linked.paper(&papers[1].id).unwrap().url.as_deref(), Some("https://example.org/moved"));
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
                    ..paper(&format!("p{index:03}"), &format!("{} {index}", title.join(" ")), 2020 + (index % 5) as u16)
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
                (format!("p{index:03}"), references)
            })
            .collect();
        // the run is two thirds of the library: the rest cites it from outside
        let run: Vec<String> = (0..200).map(|index| format!("p{index:03}")).collect();
        let in_run: HashSet<String> = run.iter().cloned().collect();
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
            // linked afresh, and linked a part at a time (a quarter of the papers added after)
            let library = afresh(&papers, &lists);
            let mut grown = afresh(&papers[..225], &lists);
            let Ok(_) = grown.update::<std::convert::Infallible>(Some(papers.clone()), None, |_| Ok(Vec::new()));
            let around: Vec<CitationNetwork> =
                ["p000", "p017", "p250"].iter().map(|seed| network(seed, &library, Some(&in_run), built)).collect();
            let grown_around: Vec<CitationNetwork> =
                ["p000", "p017", "p250"].iter().map(|seed| network(seed, &grown, Some(&in_run), built)).collect();
            assert_eq!(around, grown_around, "grown or afresh, the same");
            (community_citations(&library, &run, &tree), around, read)
        };
        let (one, many) = crate::threads::on_one_and_many(work);
        assert_eq!(one, many);
        let (figures, around, read) = one;
        all_add_up(&figures);
        assert!(figures.counts.cited_by_outside_run > 0, "the papers outside the run cite it");
        assert_eq!(
            read.iter().map(|paper| paper.paper.as_str()).collect::<Vec<_>>(),
            lists.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
            "in the order given"
        );
        assert!(around.iter().all(|network| network.totals.citations > 0), "the papers do cite each other");
    }
}
