//! Finding where the papers of a run or a community talk about something: a lexical index over their passages,
//! asked with a few words, answering the passages that match best with their addresses. It calls no model and
//! costs nothing; "find where papers talk about X" is useful by itself, and it is also what picks the passages a
//! question to the model is answered from (`model::answers`).
//!
//! # What is indexed
//!
//! One unit per paragraph-like passage of every paper whose PDF has been read ([`crate::text::pdf::passages`]): running
//! text, figure and table captions, the title. Headings, equations, footnotes and fragments are left out (they
//! match words without saying anything), and so is the NeurIPS checklist. A paper whose PDF is not kept, or could
//! not be read, is one unit: its title and abstract. [`Coverage`] says how many of each there are, so that the
//! interface can say "searched the full text of 212 of 739 papers".
//!
//! # How it ranks
//!
//! BM25 ([`K1`], [`B`]) over words folded to lower case without accents, plural `s` and a few stop words, at most
//! [`PER_PAPER`] passages of one paper among the [`MOST_HITS`] returned, so that one long paper does not fill the
//! list. When the question's embedding is at hand ([`Ranking::Embedded`]: the similarity of each paper's
//! embedding to it), the [`CANDIDATE_PAPERS`] most similar papers are the candidates, BM25 ranks the passages
//! within them, and the two ranks are combined by reciprocal rank fusion ([`FUSION`]). Ties are broken by the
//! order the units were added in (papers in the order given, passages in reading order), so the same index and
//! query always give the same list.
//!
//! # What it costs, and its bounds
//!
//! An index keeps no text: per unit its paper and passage number (16 bytes), and per distinct word of a unit one
//! posting (8 bytes), plus the vocabulary. Measured on made-up text with a vocabulary of 30,000 words
//! (`twelve_thousand_papers_are_indexed_within_the_budget`, run in release): a title and abstract takes about
//! 1.6 KB, a paper read from its PDF (120 paragraphs) 40 to 70 KB. So 739 papers of which 212 were read take
//! about 12 MB; 12,000 papers about 19 MB of abstracts plus 40 to 70 KB per paper read. A paper's full text is
//! taken only while the index stays under [`INDEX_BUDGET`] (128 MiB, about 2,000 to 3,000 read papers); the
//! papers after (in the order given, so deterministically) keep only their first [`CAPPED_PASSAGES`] passages,
//! which hold their title and abstract, and are counted as [`Coverage::capped`]. Beyond the budget an index
//! therefore grows by at most about 2 KB a paper. The indexes of a session are held together under
//! [`CACHE_BUDGET`] (256 MiB): the least recently used is dropped when another is built ([`Indexes`]), and built
//! again, identically, when asked for again.
//!
//! Building tokenizes the papers on every core (rayon), a chunk at a time, and merges them in the order given, so
//! the index is the same whatever the number of threads. 12,000 abstracts and 3,000 full texts took about 7
//! seconds on the cores of one desktop computer, a search about 2 ms. What takes longer the first time is reading
//! PDFs that were kept but never read (up to a second each), which the caller does
//! ([`crate::library::Library::paper_texts`]) before handing the papers over.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::text::pdf::passages::{Kind, Part, Passages};
use crate::text::words::{is_stop_word, singular, split};

/// BM25's term-frequency saturation.
pub const K1: f64 = 1.2;
/// BM25's length normalisation.
pub const B: f64 = 0.75;
/// The most passages a search answers.
pub const MOST_HITS: usize = 40;
/// The most passages of one paper among them.
pub const PER_PAPER: usize = 3;
/// With embeddings, how many of the most similar papers are searched.
pub const CANDIDATE_PAPERS: usize = 60;
/// Reciprocal rank fusion's constant: `1 / (FUSION + rank)` from each ranking.
pub const FUSION: f64 = 60.0;
/// The longest query looked for, in characters.
pub const LONGEST_QUERY: usize = 1_000;
/// The most one index may take, in bytes as [`Index::bytes`] counts them.
pub const INDEX_BUDGET: usize = 128 * 1024 * 1024;
/// The most all the indexes kept at once may take.
pub const CACHE_BUDGET: usize = 256 * 1024 * 1024;
/// Passages kept of a read paper once its index is full: its title and abstract, as a rule.
pub const CAPPED_PASSAGES: usize = 4;

/// The words of `text` as the index compares them ([`crate::text::words`]): lower case, accents and a plural `s`
/// dropped, stop words and single letters left out (single digits stay: "3D", "k 2").
pub fn words(text: &str) -> Vec<String> {
    split(text)
        .into_iter()
        .map(|word| singular(&word))
        .filter(|word| !(word.chars().count() < 2 && !word.chars().all(|c| c.is_ascii_digit())) && !is_stop_word(word))
        .collect()
}

/// Why a paper is in an index by its title and abstract only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unread {
    /// No PDF of it is kept.
    NoPdf,
    /// Its PDF could not be read.
    Unreadable,
}

/// One paper as it is handed to an index.
#[derive(Clone, Copy, Debug)]
pub enum Document<'a> {
    /// Read from its PDF: its passages are indexed.
    Read { paper: &'a str, passages: &'a Passages },
    /// Only its record: its title and abstract are one unit.
    Record { paper: &'a str, title: &'a str, abstract_text: &'a str, why: Unread },
    /// Not a paper of the library: counted, nothing indexed.
    Missing,
}

/// Which part of a paper a unit is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Unit {
    /// The passage at this place of the paper's [`Passages::passages`].
    Passage { index: u32 },
    /// Its title and abstract, from its record.
    Record,
}

/// A passage found: which paper, which unit of it, and how well it matched (BM25, or the fused rank's score).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub paper: String,
    pub unit: Unit,
    pub score: f64,
}

/// How much of the papers an index searches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Coverage {
    /// Papers handed over.
    pub papers: usize,
    /// Searched in their full text, read from their PDF.
    pub full_text: usize,
    /// Read, but the index was full: only their first passages are searched.
    pub capped: usize,
    /// Searched in their title and abstract: no PDF kept.
    pub no_pdf: usize,
    /// Searched in their title and abstract: their PDF could not be read.
    pub unreadable: usize,
    /// Not in the library: not searched at all.
    pub missing: usize,
}

/// One unit of the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    paper: u32,
    unit: Unit,
    /// Its length in words.
    length: u32,
}

/// The index over one set of papers. Built once ([`Builder`]) and only read after, so it is shared between
/// requests as `Arc<Index>` with no lock.
#[derive(Debug, PartialEq)]
pub struct Index {
    papers: Vec<String>,
    entries: Vec<Entry>,
    terms: HashMap<String, u32>,
    /// By term: (entry, how often the term is in it), entries in the order added.
    postings: Vec<Vec<(u32, u32)>>,
    words: u64,
    coverage: Coverage,
    bytes: usize,
    /// The papers indexed by their record because no PDF of them was kept (by their place in `papers`): those
    /// whose PDF, kept since, makes the index out of date ([`Index::without_pdf`]).
    without_pdf: Vec<u32>,
}

/// How a search ranks.
#[derive(Clone, Copy, Debug)]
pub enum Ranking<'a> {
    /// BM25 alone.
    Lexical,
    /// The cosine similarity of each paper's embedding to the question's, in the index's paper order
    /// ([`Index::papers`]); a list of any other length is ignored and the search is lexical.
    Embedded { similarity: &'a [f32] },
}

impl Index {
    pub fn coverage(&self) -> Coverage {
        self.coverage
    }

    /// The papers indexed, in the order they were handed over.
    pub fn papers(&self) -> &[String] {
        &self.papers
    }

    /// What it takes in memory, roughly (what [`INDEX_BUDGET`] and [`CACHE_BUDGET`] are counted in).
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// The papers indexed by their title and abstract because no PDF of them was kept ([`Unread::NoPdf`]), in the
    /// order handed over. Once one of them has a PDF, the index no longer says what the papers say: whoever keeps
    /// it checks these, and builds it again.
    pub fn without_pdf(&self) -> impl Iterator<Item = &str> {
        self.without_pdf.iter().filter_map(|paper| self.papers.get(*paper as usize)).map(String::as_str)
    }

    /// The units that match `query` best, best first, as the module says. No words, no hits.
    ///
    /// The order is total and the same whatever order the papers were handed over in (and whatever the number of
    /// threads): terms are summed in the order of their words, not of their ids (which follow the order the papers
    /// came in), and ties are broken by paper id and then by the unit's place in it. Callers key on it: a cited
    /// answer is kept under the hits it was given, in order (`model::answers::key_of`), so another order of equal
    /// hits would be another key, and the question would be paid for again.
    pub fn search(&self, query: &str, ranking: Ranking<'_>) -> Vec<Hit> {
        let mut wanted: Vec<(String, u32)> =
            words(query).into_iter().filter_map(|word| self.terms.get(&word).map(|id| (word, *id))).collect();
        wanted.sort_unstable();
        wanted.dedup();
        if wanted.is_empty() || self.entries.is_empty() {
            return Vec::new();
        }
        // where a unit is, for breaking ties: its paper's id and its place in the paper, then the entry itself (a
        // paper handed over twice)
        let address = |entry: u32| {
            let unit = &self.entries[entry as usize];
            (self.papers[unit.paper as usize].as_str(), unit.unit, entry)
        };
        let candidates: Option<Vec<usize>> = match ranking {
            Ranking::Embedded { similarity } if similarity.len() == self.papers.len() => {
                let mut order: Vec<usize> = (0..self.papers.len()).collect();
                order.sort_by(|a, b| {
                    similarity[*b]
                        .total_cmp(&similarity[*a])
                        .then_with(|| (&self.papers[*a], a).cmp(&(&self.papers[*b], b)))
                });
                order.truncate(CANDIDATE_PAPERS);
                // a paper's rank among the candidates, by its place in the index
                let mut rank = vec![usize::MAX; self.papers.len()];
                for (place, paper) in order.iter().enumerate() {
                    rank[*paper] = place;
                }
                Some(rank)
            }
            Ranking::Embedded { .. } | Ranking::Lexical => None,
        };
        let units = self.entries.len() as f64;
        let average = (self.words as f64 / units).max(1.0);
        // summed term by term in a fixed order, so the same query gives the same scores to the last bit
        let mut scores: HashMap<u32, f64> = HashMap::new();
        for (_, term) in wanted {
            let postings = &self.postings[term as usize];
            let found = postings.len() as f64;
            let idf = (1.0 + (units - found + 0.5) / (found + 0.5)).ln();
            for &(entry, times) in postings {
                let unit = &self.entries[entry as usize];
                if let Some(rank) = &candidates
                    && rank[unit.paper as usize] == usize::MAX
                {
                    continue;
                }
                let times = f64::from(times);
                let length = f64::from(unit.length);
                let score = idf * times * (K1 + 1.0) / (times + K1 * (1.0 - B + B * length / average));
                *scores.entry(entry).or_insert(0.0) += score;
            }
        }
        let mut ranked: Vec<(u32, f64)> = scores.into_iter().collect();
        let best_first =
            |a: &(u32, f64), b: &(u32, f64)| b.1.total_cmp(&a.1).then_with(|| address(a.0).cmp(&address(b.0)));
        ranked.sort_by(best_first);
        if let Some(rank) = &candidates {
            let fused = |(place, (entry, _)): (usize, &(u32, f64))| {
                let paper = rank[self.entries[*entry as usize].paper as usize];
                (*entry, 1.0 / (FUSION + place as f64 + 1.0) + 1.0 / (FUSION + paper as f64 + 1.0))
            };
            ranked = ranked.iter().enumerate().map(fused).collect();
            ranked.sort_by(best_first);
        }
        let mut per_paper: HashMap<u32, usize> = HashMap::new();
        let mut hits = Vec::new();
        for (entry, score) in ranked {
            let unit = self.entries[entry as usize];
            let taken = per_paper.entry(unit.paper).or_insert(0);
            if *taken == PER_PAPER {
                continue;
            }
            *taken += 1;
            hits.push(Hit { paper: self.papers[unit.paper as usize].clone(), unit: unit.unit, score });
            if hits.len() == MOST_HITS {
                break;
            }
        }
        hits
    }
}

/// Is this passage one the index searches?
fn indexed(kind: Kind, part: Part) -> bool {
    matches!(kind, Kind::Text | Kind::Title | Kind::FigureCaption | Kind::TableCaption) && part != Part::Checklist
}

/// A unit's distinct words, each with how often it is there, in the order first seen.
type Counts = Vec<(String, u32)>;

/// A unit's words counted, and its length in words.
fn counted(text: &str) -> (Counts, u32) {
    let words = words(text);
    let mut counts: Counts = Vec::new();
    let mut at: HashMap<&str, usize> = HashMap::new();
    for word in &words {
        match at.get(word.as_str()) {
            Some(&place) => counts[place].1 += 1,
            None => {
                at.insert(word, counts.len());
                counts.push((word.clone(), 1));
            }
        }
    }
    (counts, u32::try_from(words.len()).unwrap_or(u32::MAX))
}

/// One document's units, tokenized (on any thread) before they are merged (in order).
struct Tokenized {
    paper: Option<String>,
    units: Vec<(Unit, Counts, u32)>,
    kind: Counted,
}

#[derive(Clone, Copy)]
enum Counted {
    FullText,
    Unread(Unread),
    Missing,
}

fn tokenized(document: &Document<'_>) -> Tokenized {
    match *document {
        Document::Read { paper, passages } => {
            let units = passages
                .passages
                .iter()
                .enumerate()
                .filter(|(_, passage)| indexed(passage.kind, passage.part))
                .filter_map(|(index, passage)| {
                    let (counts, length) = counted(&passage.text);
                    let index = u32::try_from(index).ok()?;
                    (!counts.is_empty()).then_some((Unit::Passage { index }, counts, length))
                })
                .collect();
            Tokenized { paper: Some(paper.to_owned()), units, kind: Counted::FullText }
        }
        Document::Record { paper, title, abstract_text, why } => {
            let (counts, length) = counted(&format!("{title}\n{abstract_text}"));
            let units = if counts.is_empty() { Vec::new() } else { vec![(Unit::Record, counts, length)] };
            Tokenized { paper: Some(paper.to_owned()), units, kind: Counted::Unread(why) }
        }
        Document::Missing => Tokenized { paper: None, units: Vec::new(), kind: Counted::Missing },
    }
}

/// Bytes a posting list and a term take, roughly, in the count [`Index::bytes`] keeps.
const POSTING: usize = 8;
const ENTRY: usize = 16;
const LIST: usize = 24;
const TERM: usize = 40;

/// Makes an [`Index`], a chunk of papers at a time: hand it a few hundred papers, read the next few hundred,
/// hand those over, and so on, so that no more than one chunk's passages are in memory at once.
#[derive(Debug)]
pub struct Builder {
    index: Index,
    budget: usize,
}

impl Default for Builder {
    fn default() -> Self {
        Self::with_budget(INDEX_BUDGET)
    }
}

impl Builder {
    /// A builder whose index takes no more full texts once it reaches `budget` bytes.
    pub fn with_budget(budget: usize) -> Self {
        let index = Index {
            papers: Vec::new(),
            entries: Vec::new(),
            terms: HashMap::new(),
            postings: Vec::new(),
            words: 0,
            coverage: Coverage::default(),
            bytes: 0,
            without_pdf: Vec::new(),
        };
        Self { index, budget }
    }

    /// Add `documents`, tokenized on every core and merged in their order.
    pub fn add(&mut self, documents: &[Document<'_>]) {
        let tokenized: Vec<Tokenized> = documents.par_iter().map(tokenized).collect();
        for document in tokenized {
            self.merge(document);
        }
    }

    fn merge(&mut self, document: Tokenized) {
        let index = &mut self.index;
        index.coverage.papers += 1;
        let Some(paper) = document.paper else {
            index.coverage.missing += 1;
            return;
        };
        let mut units = document.units;
        let full: usize = units.iter().map(|(_, counts, _)| ENTRY + counts.len() * POSTING).sum();
        match document.kind {
            Counted::FullText if index.bytes + full > self.budget => {
                units.truncate(CAPPED_PASSAGES);
                index.coverage.capped += 1;
            }
            Counted::FullText => index.coverage.full_text += 1,
            Counted::Unread(Unread::NoPdf) => index.coverage.no_pdf += 1,
            Counted::Unread(Unread::Unreadable) => index.coverage.unreadable += 1,
            Counted::Missing => {}
        }
        let Ok(number) = u32::try_from(index.papers.len()) else { return };
        index.bytes += paper.len() + LIST;
        index.papers.push(paper);
        if matches!(document.kind, Counted::Unread(Unread::NoPdf)) {
            index.without_pdf.push(number);
            index.bytes += 4;
        }
        for (unit, counts, length) in units {
            let Ok(entry) = u32::try_from(index.entries.len()) else { return };
            index.entries.push(Entry { paper: number, unit, length });
            index.words += u64::from(length);
            index.bytes += ENTRY + counts.len() * POSTING;
            for (word, times) in counts {
                let next = u32::try_from(index.postings.len()).unwrap_or(u32::MAX);
                let term = match index.terms.get(&word) {
                    Some(&term) => term,
                    None => {
                        index.bytes += word.len() + TERM + LIST;
                        index.terms.insert(word, next);
                        index.postings.push(Vec::new());
                        next
                    }
                };
                index.postings[term as usize].push((entry, times));
            }
        }
    }

    pub fn finish(self) -> Index {
        self.index
    }
}

/// How far the building of an index has got: papers handed over of all.
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicUsize,
    of: AtomicUsize,
}

impl Progress {
    pub fn set(&self, done: usize, of: usize) {
        self.of.store(of, Ordering::SeqCst);
        self.done.store(done, Ordering::SeqCst);
    }

    fn read(&self) -> (usize, usize) {
        (self.done.load(Ordering::SeqCst), self.of.load(Ordering::SeqCst))
    }
}

/// An index being built, which whoever else asks for it waits for.
#[derive(Debug, Default)]
struct Building {
    outcome: Mutex<Option<Result<Arc<Index>, String>>>,
    ready: Condvar,
    progress: Progress,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Building {
    /// What came of it, waiting at most until `deadline` (forever when none).
    fn wait(&self, deadline: Option<Instant>) -> Option<Result<Arc<Index>, String>> {
        let mut outcome = locked(&self.outcome);
        loop {
            if let Some(done) = outcome.as_ref() {
                return Some(done.clone());
            }
            match deadline {
                None => outcome = self.ready.wait(outcome).unwrap_or_else(PoisonError::into_inner),
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return None;
                    }
                    outcome = self.ready.wait_timeout(outcome, left).unwrap_or_else(PoisonError::into_inner).0;
                }
            }
        }
    }

    fn publish(&self, outcome: Result<Arc<Index>, String>) {
        *locked(&self.outcome) = Some(outcome);
        self.ready.notify_all();
    }
}

/// One scope's index on the shelf: being built, or built (with when it was last used, in the shelf's ticks).
#[derive(Debug)]
enum Shelved {
    Building(Arc<Building>),
    Ready { index: Arc<Index>, used: u64 },
}

#[derive(Debug, Default)]
struct Shelf {
    scopes: HashMap<String, Shelved>,
    tick: u64,
}

impl Shelf {
    fn ready_bytes(&self) -> usize {
        self.scopes
            .values()
            .map(|shelved| match shelved {
                Shelved::Ready { index, .. } => index.bytes(),
                Shelved::Building(_) => 0,
            })
            .sum()
    }

    /// Drop the least recently used built indexes, other than `keep`, until the rest fit in `budget`. Whoever
    /// still holds one keeps reading it: it is freed when they are done.
    fn evict(&mut self, budget: usize, keep: &str) {
        while self.ready_bytes() > budget {
            let oldest = self
                .scopes
                .iter()
                .filter_map(|(scope, shelved)| match shelved {
                    Shelved::Ready { used, .. } if scope != keep => Some((*used, scope.clone())),
                    Shelved::Ready { .. } | Shelved::Building(_) => None,
                })
                .min();
            let Some((_, scope)) = oldest else { return };
            self.scopes.remove(&scope);
        }
    }
}

/// Put what came of building `scope` on the shelf (the index, evicting others to make room; or, on failure, the
/// claim taken off), if the claim is still `building`'s; then tell whoever waits.
fn finish(shelf: &Mutex<Shelf>, scope: &str, building: &Building, budget: usize, outcome: Result<Arc<Index>, String>) {
    {
        let mut shelf = locked(shelf);
        let ours = matches!(
            shelf.scopes.get(scope),
            Some(Shelved::Building(claim)) if std::ptr::eq(Arc::as_ptr(claim), building)
        );
        if ours {
            match &outcome {
                Ok(index) => {
                    shelf.tick += 1;
                    let used = shelf.tick;
                    shelf.scopes.insert(scope.to_owned(), Shelved::Ready { index: Arc::clone(index), used });
                    shelf.evict(budget, scope);
                }
                Err(_) => {
                    shelf.scopes.remove(scope);
                }
            }
        }
    }
    building.publish(outcome);
}

/// What [`Indexes::get`] found.
#[derive(Clone, Debug)]
pub enum Got {
    Ready(Arc<Index>),
    /// Still being built after the wait asked for: papers handed over so far, of all.
    Building {
        done: usize,
        of: usize,
    },
}

/// How long [`Indexes::get`] waits for an index being built.
#[derive(Clone, Copy, Debug)]
pub enum Wait {
    Forever,
    AtMost(Duration),
}

/// The indexes of a session, by scope (whatever names the papers they are over: a run, a community, with the
/// library they are read from). Each is built once however many requests ask for it at the same moment: the
/// first starts building it on a thread of its own, the others wait for that one. Built, it is shared read-only
/// (`Arc<Index>`). All together they are kept under a budget in bytes, the least recently used dropped first.
#[derive(Debug)]
pub struct Indexes {
    budget: usize,
    /// Shared with the threads that build, which put what they built here after the request that started them
    /// may have returned.
    shelf: Arc<Mutex<Shelf>>,
}

impl Default for Indexes {
    fn default() -> Self {
        Self::with_budget(CACHE_BUDGET)
    }
}

impl Indexes {
    pub fn with_budget(budget: usize) -> Self {
        Self { budget, shelf: Arc::new(Mutex::new(Shelf::default())) }
    }

    /// The index of `scope`, built with `build` if no one has built it or is building it. `build` runs on a thread
    /// of its own (never a rayon worker, so it may read papers through the library), reporting how far it has got
    /// through the [`Progress`] it is given; a failure (or a panic) is told to everyone waiting and nothing is
    /// kept, so the next request tries again.
    pub fn get<F>(&self, scope: &str, wait: Wait, build: F) -> Result<Got, String>
    where
        F: FnOnce(&Progress) -> Result<Index, String> + Send + 'static,
    {
        self.get_on(scope, wait, build, |work| thread::Builder::new().name("index".to_owned()).spawn(work).map(drop))
    }

    /// [`Indexes::get`], a thread started by `spawn` if one is needed.
    fn get_on<F, S>(&self, scope: &str, wait: Wait, build: F, spawn: S) -> Result<Got, String>
    where
        F: FnOnce(&Progress) -> Result<Index, String> + Send + 'static,
        S: FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
    {
        let building = {
            let mut shelf = locked(&self.shelf);
            shelf.tick += 1;
            let now = shelf.tick;
            match shelf.scopes.get_mut(scope) {
                Some(Shelved::Ready { index, used }) => {
                    *used = now;
                    return Ok(Got::Ready(Arc::clone(index)));
                }
                Some(Shelved::Building(building)) => Arc::clone(building),
                None => {
                    let building = Arc::new(Building::default());
                    shelf.scopes.insert(scope.to_owned(), Shelved::Building(Arc::clone(&building)));
                    drop(shelf);
                    self.start_on(scope.to_owned(), Arc::clone(&building), build, spawn);
                    building
                }
            }
        };
        let deadline = match wait {
            Wait::Forever => None,
            Wait::AtMost(patience) => Some(Instant::now() + patience),
        };
        match building.wait(deadline) {
            Some(Ok(index)) => Ok(Got::Ready(index)),
            Some(Err(why)) => Err(why),
            None => {
                let (done, of) = building.progress.read();
                Ok(Got::Building { done, of })
            }
        }
    }

    /// Build on a thread of its own, started by `spawn`; put the index on the shelf (evicting others to make room),
    /// or take the claim off it on failure; then tell whoever waits ([`finish`]). A thread that cannot be started
    /// (the system has none left) is a failed build, told and taken off the shelf like any other: never a claim
    /// that stays "being built" for ever.
    fn start_on<F, S>(&self, scope: String, building: Arc<Building>, build: F, spawn: S)
    where
        F: FnOnce(&Progress) -> Result<Index, String> + Send + 'static,
        S: FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
    {
        let shelf = Arc::clone(&self.shelf);
        let budget = self.budget;
        let (claim, told) = (Arc::clone(&building), scope.clone());
        let work = Box::new(move || {
            let outcome = match catch_unwind(AssertUnwindSafe(|| build(&building.progress))) {
                Ok(Ok(index)) => Ok(Arc::new(index)),
                Ok(Err(why)) => Err(why),
                Err(_) => Err("the index could not be built".to_owned()),
            };
            finish(&shelf, &scope, &building, budget, outcome);
        });
        if spawn(work).is_err() {
            let why = "the index could not be built: no thread could be started for it".to_owned();
            finish(&self.shelf, &told, &claim, self.budget, Err(why));
        }
    }

    /// Drop the built index of `scope`, if any (its papers' texts changed since: a PDF was read since it was
    /// built), so that the next request builds it again. One being built is left to finish.
    pub fn forget(&self, scope: &str) {
        let mut shelf = locked(&self.shelf);
        if matches!(shelf.scopes.get(scope), Some(Shelved::Ready { .. })) {
            shelf.scopes.remove(scope);
        }
    }

    /// The scopes whose index is built and kept now, for the tests and the log.
    pub fn kept(&self) -> Vec<String> {
        let shelf = locked(&self.shelf);
        let mut kept: Vec<String> = shelf
            .scopes
            .iter()
            .filter(|(_, shelved)| matches!(shelved, Shelved::Ready { .. }))
            .map(|(scope, _)| scope.clone())
            .collect();
        kept.sort_unstable();
        kept
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Barrier;

    use super::*;
    use crate::text::pdf::passages::read_passages;

    type Paper = (String, Option<Passages>, (String, String));

    fn read(pages: &[&str]) -> Passages {
        read_passages(&pages.iter().map(|page| (*page).to_owned()).collect::<Vec<_>>())
    }

    /// Three read papers and two by their record.
    pub(crate) fn library() -> Vec<Paper> {
        let paper = |id: &str, pages: Option<&[&str]>, title: &str, abstract_text: &str| {
            (id.to_owned(), pages.map(read), (title.to_owned(), abstract_text.to_owned()))
        };
        vec![
            paper(
                "2601.00001",
                Some(&[
                    "1. Introduction\n\nWe merge task vectors and find that merging counts shared directions twice.\n\n2. Method\n\nThe cure is to rescale the inflated singular values of the merged update.\n",
                ]),
                "",
                "",
            ),
            paper(
                "2601.00002",
                Some(&[
                    "1. Method\n\nOur method distils a large teacher into a small student with no labelled data.\n\n2. Results\n\nDistillation keeps accuracy within a point on every benchmark we tried.\n",
                ]),
                "",
                "",
            ),
            paper(
                "2601.00003",
                Some(&["1. Introduction\n\nSparse attention lets transformers read long documents at linear cost.\n"]),
                "",
                "",
            ),
            paper(
                "icml-2026-4",
                None,
                "Calibrated Merging of Task Vectors",
                "Merging fine-tuned models inflates the spectrum of shared directions; we calibrate it.",
            ),
            paper(
                "icml-2026-5",
                None,
                "Studies of Students",
                "Knowledge distillation studies: which students learn from which teachers.",
            ),
        ]
    }

    fn documents(papers: &[Paper]) -> Vec<Document<'_>> {
        papers
            .iter()
            .map(|(paper, passages, (title, abstract_text))| match passages {
                Some(passages) => Document::Read { paper, passages },
                None => Document::Record { paper, title, abstract_text, why: Unread::NoPdf },
            })
            .collect()
    }

    fn index(papers: &[Paper]) -> Index {
        let mut builder = Builder::default();
        builder.add(&documents(papers));
        builder.finish()
    }

    /// What an index of a large run takes, in memory and time, on made-up text with a realistic vocabulary. Run
    /// in release: `cargo test --release -- --ignored twelve_thousand_papers --nocapture`
    #[test]
    #[ignore = "a timing, to be run in release"]
    fn twelve_thousand_papers_are_indexed_within_the_budget() {
        // words drawn from a vocabulary of 30,000, as a field's papers use one
        let mut seed = 7_u64;
        let mut word = || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            format!("w{}", (seed >> 33) % 30_000)
        };
        let mut text = |words: usize| (0..words).map(|_| word()).collect::<Vec<_>>().join(" ");
        let abstracts: Vec<(String, String)> = (0..12_000).map(|n| (format!("a{n}"), text(180))).collect();
        let read: Vec<(String, Passages)> = (0..3_000)
            .map(|n| {
                let page: String = (0..120).map(|_| format!("{}.\n\n", text(70))).collect();
                (format!("r{n}"), read_passages(&[format!("1. Introduction\n\n{page}")]))
            })
            .collect();
        let started = Instant::now();
        let mut builder = Builder::default();
        let records: Vec<Document<'_>> = abstracts
            .iter()
            .map(|(paper, text)| Document::Record { paper, title: "", abstract_text: text, why: Unread::NoPdf })
            .collect();
        builder.add(&records);
        let after_abstracts = builder.index.bytes;
        let full: Vec<Document<'_>> = read.iter().map(|(paper, passages)| Document::Read { paper, passages }).collect();
        builder.add(&full);
        let index = builder.finish();
        let took = started.elapsed();
        let searched = Instant::now();
        let hits = index.search(&text(8), Ranking::Lexical);
        println!(
            "12,000 abstracts: {} MB; with 3,000 full texts: {} MB, {:?}; {:?} ({} full, {} capped); a search {:?}, {} hits",
            after_abstracts / 1_000_000,
            index.bytes() / 1_000_000,
            took,
            index.coverage(),
            index.coverage().full_text,
            index.coverage().capped,
            searched.elapsed(),
            hits.len()
        );
        // full texts within the budget; beyond it, about 2 KB a paper for the abstracts and first passages
        assert!(index.bytes() <= INDEX_BUDGET + 2_000 * index.coverage().papers, "{}", index.bytes());
    }

    #[test]
    fn words_are_folded_so_that_forms_of_one_word_match() {
        assert_eq!(words("The Methods of Studies"), ["method", "study"]);
        assert_eq!(words("naïve café, 3D k-NN"), ["naive", "cafe", "3d", "nn"]);
        assert_eq!(words("losses, class, corpus, analysis"), ["losse", "class", "corpus", "analysis"]);
        assert!(words("a of the we").is_empty());
    }

    #[test]
    fn a_search_finds_the_passages_that_talk_about_it_with_their_place() {
        let papers = library();
        let index = index(&papers);
        let hits = index.search("merging shared directions", Ranking::Lexical);
        let found: Vec<(&str, Unit)> = hits.iter().map(|hit| (hit.paper.as_str(), hit.unit)).collect();
        assert_eq!(found[0].0, "2601.00001");
        assert!(found.contains(&("icml-2026-4", Unit::Record)), "{found:?}");
        assert!(!found.iter().any(|(paper, _)| *paper == "2601.00003"), "a paper about something else is not found");
        let Unit::Passage { index: at } = found[0].1 else { panic!() };
        let passage = &papers[0].1.as_ref().unwrap().passages[at as usize];
        assert!(passage.text.contains("counts shared directions twice"));
        assert!(hits.windows(2).all(|pair| pair[0].score >= pair[1].score));
        assert!(index.search("the of and", Ranking::Lexical).is_empty(), "no words, no hits");
        assert!(index.search("quantum chromodynamics", Ranking::Lexical).is_empty());
    }

    #[test]
    fn coverage_says_what_was_searched_in_full_and_what_by_its_abstract() {
        let papers = library();
        let mut builder = Builder::default();
        let mut documents = documents(&papers);
        documents.push(Document::Missing);
        let (title, abstract_text) = ("Unreadable", "A scanned paper about distillation.");
        documents.push(Document::Record { paper: "scan", title, abstract_text, why: Unread::Unreadable });
        builder.add(&documents);
        let index = builder.finish();
        let coverage = index.coverage();
        assert_eq!(coverage, Coverage { papers: 7, full_text: 3, capped: 0, no_pdf: 2, unreadable: 1, missing: 1 });
        let counted = [coverage.full_text, coverage.capped, coverage.no_pdf, coverage.unreadable, coverage.missing];
        assert_eq!(counted.iter().sum::<usize>(), coverage.papers, "every paper is counted once");
        assert_eq!(index.without_pdf().collect::<Vec<_>>(), ["icml-2026-4", "icml-2026-5"], "not the unreadable one");
    }

    #[test]
    fn a_full_index_takes_only_the_first_passages_of_later_papers() {
        let papers = library();
        let first = index(&papers[..1]).bytes();
        // room for the first paper's full text and no more
        let mut builder = Builder::with_budget(first + 8);
        builder.add(&documents(&papers));
        let index = builder.finish();
        assert_eq!((index.coverage().full_text, index.coverage().capped), (1, 2));
        assert!(index.bytes() > 0);
    }

    #[test]
    fn no_more_than_a_few_passages_of_one_paper_are_handed_back() {
        let text: String =
            (0..12).map(|n| format!("Paragraph {n} is about graph neural networks and graphs.\n\n")).collect();
        let pages = [format!("1. Introduction\n\n{text}")];
        let passages = read_passages(&pages);
        let mut builder = Builder::default();
        builder.add(&[Document::Read { paper: "p", passages: &passages }]);
        assert_eq!(builder.finish().search("graph neural network", Ranking::Lexical).len(), PER_PAPER);
    }

    #[test]
    fn with_embeddings_the_most_similar_papers_are_searched_and_their_rank_counts() {
        let papers = library();
        let index = index(&papers);
        // the question is closest to icml-2026-5 and 2601.00002 (distillation); the merging papers are far
        let similarity = [0.1_f32, 0.8, 0.2, 0.1, 0.9];
        let hits = index.search("distillation students merging", Ranking::Embedded { similarity: &similarity });
        let order: Vec<&str> = hits.iter().map(|hit| hit.paper.as_str()).collect();
        assert_eq!(order[0], "icml-2026-5", "{order:?}");
        let place = |paper: &str| order.iter().position(|found| *found == paper);
        assert!(place("2601.00002") < place("2601.00001"), "{order:?}");
        // a list of the wrong length is not used
        let lexical = index.search("distillation students merging", Ranking::Lexical);
        assert_eq!(index.search("distillation students merging", Ranking::Embedded { similarity: &[1.0] }), lexical);
    }

    #[test]
    fn the_index_and_its_answers_are_the_same_whatever_the_number_of_threads() {
        let mut papers = library();
        for n in 0..60 {
            let pages = [format!(
                "1. Introduction\n\nPaper {n} studies merging, distillation and sparse attention in turn {n}.\n\n2. More\n\nIt also discusses task vectors and student models, variant {}.\n",
                n % 7
            )];
            papers.push((format!("2602.{n:05}"), Some(read_passages(&pages)), (String::new(), String::new())));
        }
        let built = |threads: usize| {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            pool.install(|| {
                let mut builder = Builder::default();
                // in two chunks, as the engine hands them over
                let documents = documents(&papers);
                builder.add(&documents[..20]);
                builder.add(&documents[20..]);
                builder.finish()
            })
        };
        let one = built(1);
        for threads in [2, 3, 8] {
            let other = built(threads);
            assert_eq!(other, one, "{threads} threads");
            for query in ["merging task vectors", "student distillation", "sparse attention variant"] {
                assert_eq!(other.search(query, Ranking::Lexical), one.search(query, Ranking::Lexical));
            }
        }
    }

    #[test]
    fn equal_hits_come_in_the_same_order_whatever_order_the_papers_were_handed_over_in() {
        let mut papers = library();
        // papers that say the same thing, so that their passages score the same: the order between them is a tie
        for id in ["2603.00009", "2603.00001", "2603.00005"] {
            let pages = ["1. Introduction\n\nMerging task vectors counts shared directions twice.\n".to_owned()];
            papers.push((id.to_owned(), Some(read_passages(&pages)), (String::new(), String::new())));
        }
        let forwards = index(&papers);
        let mut reversed = papers.clone();
        reversed.reverse();
        let backwards = index(&reversed);
        let similarity = |index: &Index| -> Vec<f32> {
            index.papers().iter().map(|paper| if paper.starts_with("2603") { 0.5 } else { 0.25 }).collect()
        };
        for query in ["merging task vectors twice", "shared directions", "distillation students"] {
            let one = forwards.search(query, Ranking::Lexical);
            assert_eq!(backwards.search(query, Ranking::Lexical), one, "{query}");
            let ties: Vec<&str> =
                one.iter().filter(|hit| hit.paper.starts_with("2603")).map(|hit| hit.paper.as_str()).collect();
            let mut sorted = ties.clone();
            sorted.sort_unstable();
            assert_eq!(ties, sorted, "ties by paper id: {query}");
            let embedded = |index: &Index| index.search(query, Ranking::Embedded { similarity: &similarity(index) });
            assert_eq!(embedded(&backwards), embedded(&forwards), "{query}, with embeddings");
        }
    }

    #[test]
    fn a_build_whose_thread_cannot_be_started_is_told_as_failed_and_tried_again_next_time() {
        let indexes = Indexes::default();
        let no_thread = |_work: Box<dyn FnOnce() + Send>| Err(std::io::Error::other("no threads left"));
        let failed = indexes.get_on("r", Wait::Forever, |_| Ok(index(&library())), no_thread);
        assert!(failed.unwrap_err().contains("no thread could be started"), "told, not waited for for ever");
        assert!(indexes.kept().is_empty());
        let again = indexes.get_on("r", Wait::AtMost(Duration::from_secs(5)), |_| Ok(index(&library())), no_thread);
        assert!(again.is_err(), "the claim was taken off the shelf: asked again, it is tried again");
        assert!(matches!(indexes.get("r", Wait::Forever, |_| Ok(index(&library()))).unwrap(), Got::Ready(_)));
    }

    #[test]
    fn an_index_is_built_once_however_many_ask_for_it_at_once() {
        let indexes = Indexes::default();
        let builds = Arc::new(AtomicUsize::new(0));
        let start = Barrier::new(16);
        thread::scope(|scope| {
            for _ in 0..16 {
                let (indexes, builds, start) = (&indexes, Arc::clone(&builds), &start);
                scope.spawn(move || {
                    start.wait();
                    let got = indexes
                        .get("run r1", Wait::Forever, move |progress| {
                            builds.fetch_add(1, Ordering::SeqCst);
                            progress.set(0, 5);
                            thread::sleep(Duration::from_millis(30));
                            Ok(index(&library()))
                        })
                        .unwrap();
                    let Got::Ready(index) = got else { panic!("waited for it") };
                    assert_eq!(index.search("counts twice", Ranking::Lexical)[0].paper, "2601.00001");
                });
            }
        });
        assert_eq!(builds.load(Ordering::SeqCst), 1);
        assert_eq!(indexes.kept(), ["run r1"]);
    }

    #[test]
    fn a_request_that_cannot_wait_is_told_how_far_the_building_has_got() {
        let indexes = Indexes::default();
        let release = Arc::new(Barrier::new(2));
        let gate = Arc::clone(&release);
        let got = indexes
            // long enough for the builder to have begun on a busy machine
            .get("run r1", Wait::AtMost(Duration::from_millis(500)), move |progress| {
                progress.set(2, 5);
                gate.wait();
                Ok(index(&library()))
            })
            .unwrap();
        assert!(matches!(got, Got::Building { done: 2, of: 5 }), "{got:?}");
        release.wait();
        let again = indexes.get("run r1", Wait::Forever, |_| Err("not built twice".to_owned())).unwrap();
        assert!(matches!(again, Got::Ready(_)));
    }

    #[test]
    fn a_failed_or_panicking_build_is_told_to_everyone_and_tried_again_next_time() {
        let indexes = Indexes::default();
        let failed = indexes.get("r", Wait::Forever, |_| Err("the library could not be read".to_owned()));
        assert_eq!(failed.unwrap_err(), "the library could not be read");
        let panicked = indexes.get("r", Wait::Forever, |_| panic!("boom"));
        assert_eq!(panicked.unwrap_err(), "the index could not be built");
        assert!(matches!(indexes.get("r", Wait::Forever, |_| Ok(index(&library()))).unwrap(), Got::Ready(_)));
    }

    #[test]
    fn the_least_recently_used_index_is_dropped_first_and_the_one_just_built_is_kept_even_alone_over_the_budget() {
        let papers = Arc::new(library());
        let one = index(&papers).bytes();
        let built = |indexes: &Indexes, scope: &str| {
            let papers = Arc::clone(&papers);
            assert!(matches!(indexes.get(scope, Wait::Forever, move |_| Ok(index(&papers))).unwrap(), Got::Ready(_)));
        };
        let not_again = |indexes: &Indexes, scope: &str| {
            let got = indexes.get(scope, Wait::Forever, |_| Err("built again".to_owned()));
            assert!(matches!(got, Ok(Got::Ready(_))), "{scope}: {got:?}");
        };
        // room for two
        let indexes = Indexes::with_budget(one * 2 + one / 2);
        built(&indexes, "a");
        built(&indexes, "b");
        not_again(&indexes, "a"); // a is now the more recently used
        built(&indexes, "c");
        assert_eq!(indexes.kept(), ["a", "c"], "b, used least recently, was dropped");
        // room for none: the one just built stays, for whoever asks next
        let small = Indexes::with_budget(1);
        built(&small, "a");
        built(&small, "b");
        assert_eq!(small.kept(), ["b"]);
        not_again(&small, "b");
        // forgotten, it is built again
        small.forget("b");
        assert!(small.kept().is_empty());
        built(&small, "b");
    }

    #[test]
    fn many_threads_searching_while_indexes_are_built_and_evicted_all_get_the_same_answers() {
        let papers = Arc::new(library());
        let expected = index(&papers).search("merging distillation", Ranking::Lexical);
        let one = index(&papers).bytes();
        // room for two indexes of the four scopes: building a third evicts the least recently used
        let indexes = Indexes::with_budget(one * 2 + one / 2);
        let builds = Arc::new(AtomicUsize::new(0));
        let start = Barrier::new(24);
        thread::scope(|scope| {
            for asker in 0..24 {
                let (indexes, papers, builds, start, expected) =
                    (&indexes, Arc::clone(&papers), Arc::clone(&builds), &start, &expected);
                scope.spawn(move || {
                    start.wait();
                    for round in 0..20 {
                        let scope = format!("scope {}", (asker + round) % 4);
                        let (papers, builds) = (Arc::clone(&papers), Arc::clone(&builds));
                        let got = indexes
                            .get(&scope, Wait::Forever, move |_| {
                                builds.fetch_add(1, Ordering::SeqCst);
                                thread::sleep(Duration::from_millis(2));
                                Ok(index(&papers))
                            })
                            .unwrap();
                        let Got::Ready(index) = got else { panic!() };
                        assert_eq!(&index.search("merging distillation", Ranking::Lexical), expected);
                    }
                });
            }
        });
        assert!(indexes.kept().len() <= 2, "{:?}", indexes.kept());
        assert!(builds.load(Ordering::SeqCst) > 4, "evicted indexes were built again");
    }
}
