//! The runs the person has: listed, opened, renamed and deleted, with the run index ([`crate::runs::index`]) and
//! the cache of loaded runs ([`crate::runs::cache`]) wired up as `runs/mod.rs` describes.
//!
//! A run is a folder of the runs folder ([`super::files`]), whoever made it: the engine, or the Python before it.
//! The list is answered from small summaries kept in the library, so it reads no run it has described before;
//! a run whose results were written since (made again, or by another program) is read once more when it is shown.
//! The folders are listed again (names and times, no file opened) whenever the list is asked for, so a run added
//! or removed on disk shows at once.
//!
//! What is served of a run is its files as they are on disk, so a run made by the Python opens exactly as it
//! did. Opening a run (its communities) counts as an open, for the order of the list and for what the cache
//! keeps; the graph, asked for with it, does not count again.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::UNIX_EPOCH;

use serde::Serialize;
use serde::de::IgnoredAny;

use super::embedding::Onnx;
use super::files::{self, RunFileError};
use super::model_files::Network;
use super::steps::Board;
use crate::library::{Library, LibraryError};
use crate::runs::cache::{self, RunCache, Weigh};
use crate::runs::index::{Facts, MAX_LIMIT, MOST_ROUNDS, Page, Query, Row, RunIndex, RunStamp, Sort, SummaryRecord};
use crate::runs::usage::{Clock, Moment, SystemClock};

/// A run's files as served: the text of its communities and of its graph.
#[derive(Debug)]
pub struct Loaded {
    pub communities: String,
    pub graph: String,
}

impl Weigh for Loaded {
    fn bytes(&self) -> u64 {
        (self.communities.len() + self.graph.len()) as u64
    }
}

/// Which file of a run is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Communities,
    Graph,
}

/// One run of the list as the interface's sidebar reads it today (`RunSummary` in the contracts).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Listed {
    pub name: String,
    pub title: String,
    pub papers: u64,
    pub communities: u64,
    pub modularity: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("no such run")]
    NotFound,
    #[error("{0} is being made right now; try again when it is done")]
    BeingMade(String),
    #[error(transparent)]
    Library(#[from] LibraryError),
    #[error("{0}")]
    Files(String),
}

impl From<RunFileError> for ServeError {
    fn from(error: RunFileError) -> Self {
        match error {
            RunFileError::NotFound => ServeError::NotFound,
            other => ServeError::Files(other.to_string()),
        }
    }
}

/// The index, once it has been built from the runs folder and the library.
enum Indexed {
    NotBuilt,
    Built {
        index: RunIndex,
        /// Every run's folder as last listed, with the time its results were written.
        seen: HashMap<String, Moment>,
    },
}

/// Everything about the person's runs that outlives a request.
pub struct Runs {
    /// The step of the run being made, for the status route.
    pub board: Board,
    /// The name of the run being made, so that it is not deleted under the run making it.
    making: Mutex<Option<String>>,
    indexed: Mutex<Indexed>,
    cache: Arc<RunCache<Loaded, RunFileError>>,
    /// The run open on screen, kept in the cache while it is shown.
    shown: Mutex<Option<String>>,
    /// The embedding model, loaded once and kept for the next run.
    pub embedding: Arc<Onnx>,
}

impl Default for Runs {
    fn default() -> Self {
        Self {
            board: Board::default(),
            making: Mutex::new(None),
            indexed: Mutex::new(Indexed::NotBuilt),
            cache: Arc::new(RunCache::default()),
            shown: Mutex::new(None),
            embedding: Arc::new(Onnx::new(Arc::new(Network::default()))),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // every change under these locks is complete before it is made visible: a panic leaves nothing half-done
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// When a file was last written, in whole seconds.
fn written(path: &Path) -> Option<Moment> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let seconds = modified.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(Moment(i64::try_from(seconds).unwrap_or(i64::MAX)))
}

/// The runs in the runs folder, with the time each one's results were written; no file is opened.
pub fn stamps(data: &Path) -> Vec<RunStamp> {
    let Ok(entries) = fs::read_dir(files::runs_folder(data)) else { return Vec::new() };
    let mut found: Vec<RunStamp> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            crate::api::routes::one_name(&name).ok()?;
            let folder = entry.path();
            if !folder.join(files::RUN).is_file() {
                return None;
            }
            Some(RunStamp { written: written(&folder.join(files::COMMUNITIES))?, name })
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// What the list needs of a run, read from its files.
pub fn summarise(data: &Path, name: &str) -> Result<Facts, RunFileError> {
    #[derive(serde::Deserialize)]
    struct Counts {
        papers: u64,
        communities: Vec<IgnoredAny>,
        modularity: f64,
    }
    let folder = files::folder_of(data, name)?;
    let path = folder.join(files::COMMUNITIES);
    let text =
        fs::read(&path).map_err(|source| crate::library::store::StoreError::Io { path: path.clone(), source })?;
    let counts: Counts = serde_json::from_slice(&text)
        .map_err(|error| RunFileError::Unreadable { path: path.clone(), reason: error.to_string() })?;
    let graph = fs::metadata(folder.join(files::GRAPH)).map(|meta| meta.len()).unwrap_or(0);
    Ok(Facts {
        title: files::title(&folder, name),
        papers: counts.papers,
        communities: counts.communities.len() as u64,
        modularity: counts.modularity,
        created: written(&path).unwrap_or_default(),
        bytes: text.len() as u64 + graph,
    })
}

fn load(data: &Path, name: &str) -> Result<Loaded, RunFileError> {
    let folder = files::folder_of(data, name)?;
    let read = |file: &str| {
        let path = folder.join(file);
        fs::read_to_string(&path).map_err(|source| match source.kind() {
            std::io::ErrorKind::NotFound => RunFileError::NotFound,
            _ => RunFileError::Store(crate::library::store::StoreError::Io { path, source }),
        })
    };
    Ok(Loaded { communities: read(files::COMMUNITIES)?, graph: read(files::GRAPH)? })
}

/// A run being made, from [`Runs::begin_making`] until it is dropped.
pub struct Making<'a>(&'a Runs);

impl Drop for Making<'_> {
    fn drop(&mut self) {
        *lock(&self.0.making) = None;
    }
}

impl Runs {
    /// A run is about to be made under `name`: while the answer is held, that run cannot be deleted. Refused
    /// (`None`) when one is being made already. Dropping it, however the run ends (a panic included), lets go.
    pub fn begin_making(&self, name: &str) -> Option<Making<'_>> {
        let mut making = lock(&self.making);
        if making.is_some() {
            return None;
        }
        *making = Some(name.to_owned());
        Some(Making(self))
    }

    /// The index, built the first time it is needed; with its build, the likeliest runs are loaded in the
    /// background.
    fn index<'a>(&'a self, data: &Path, library: &Library) -> Result<MutexGuard<'a, Indexed>, ServeError> {
        let mut indexed = lock(&self.indexed);
        if matches!(*indexed, Indexed::NotBuilt) {
            let found = stamps(data);
            let seen = found.iter().map(|stamp| (stamp.name.clone(), stamp.written)).collect();
            let index = RunIndex::new(found, library.run_summaries()?, library.run_usage()?);
            let folder = data.to_path_buf();
            drop(cache::warm(&index, &self.cache, SystemClock.now(), move |name| load(&folder, name)));
            *indexed = Indexed::Built { index, seen };
        }
        Ok(indexed)
    }

    /// The runs folder listed again: runs added, written again or removed on disk are told to the index.
    fn refresh(&self, data: &Path, library: &Library) -> Result<(), ServeError> {
        let found = stamps(data); // without the lock
        let mut indexed = self.index(data, library)?;
        let Indexed::Built { index, seen } = &mut *indexed else { return Ok(()) };
        let mut now: HashMap<String, Moment> = found.into_iter().map(|stamp| (stamp.name, stamp.written)).collect();
        // the folder was listed without the lock: a run saved or deleted since then is as it is now, not as listed
        let on_disk = |name: &str| written(&files::runs_folder(data).join(name).join(files::COMMUNITIES));
        now.retain(|name, _| on_disk(name).is_some());
        for (name, at) in seen.iter() {
            if !now.contains_key(name) && on_disk(name).is_some() {
                now.insert(name.clone(), *at);
            }
        }
        for (name, at) in &now {
            if seen.get(name) != Some(at) {
                index.written(name, *at);
                self.cache.invalidate(name);
            }
        }
        for name in seen.keys().filter(|name| !now.contains_key(*name)) {
            index.removed(name);
            self.cache.invalidate(name);
        }
        *seen = now;
        Ok(())
    }

    /// A page of the list, its runs not described yet read first (without holding the index's lock).
    fn page_of(&self, data: &Path, library: &Library, query: &Query) -> Result<Page, ServeError> {
        for _ in 0..MOST_ROUNDS {
            let due = match &*self.index(data, library)? {
                Indexed::Built { index, .. } => index.to_rebuild(query),
                Indexed::NotBuilt => Vec::new(),
            };
            if due.is_empty() {
                break;
            }
            let read: Vec<(RunStamp, Result<Facts, RunFileError>)> = due
                .into_iter()
                .map(|stamp| {
                    let facts = summarise(data, &stamp.name);
                    (stamp, facts)
                })
                .collect();
            let mut records: Vec<SummaryRecord> = Vec::new();
            if let Indexed::Built { index, .. } = &mut *self.index(data, library)? {
                for (stamp, facts) in read {
                    if let Err(error) = &facts {
                        eprintln!("runs: {} could not be read: {error}", stamp.name);
                    }
                    records.extend(index.rebuilt(&stamp, facts));
                }
            }
            library.keep_run_summaries(&records)?;
        }
        Ok(match &*self.index(data, library)? {
            Indexed::Built { index, .. } => index.page(query),
            Indexed::NotBuilt => Page { total: 0, offset: 0, rows: Vec::new() },
        })
    }

    /// One page of the runs, sorted, searched (`GET /api/runs?offset=&limit=&sort=&search=`).
    pub fn page(&self, data: &Path, library: &Library, query: &Query) -> Result<Page, ServeError> {
        self.refresh(data, library)?;
        self.page_of(data, library, query)
    }

    /// Every run, the most recently written first: the list the interface reads today (`GET /api/runs`). A run
    /// whose results cannot be read is left out of it (and said on the engine's error output).
    pub fn all(&self, data: &Path, library: &Library) -> Result<Vec<Listed>, ServeError> {
        self.refresh(data, library)?;
        let mut listed = Vec::new();
        let mut query = Query { offset: 0, limit: MAX_LIMIT, sort: Sort::Created, search: String::new() };
        loop {
            let page = self.page_of(data, library, &query)?;
            let count = page.rows.len();
            listed.extend(page.rows.into_iter().filter_map(|row| match row {
                Row::Run(run) => Some(Listed {
                    name: run.name,
                    title: run.title,
                    papers: run.papers,
                    communities: run.communities,
                    modularity: run.modularity,
                }),
                Row::Unreadable { .. } | Row::Unread { .. } => None,
            }));
            query.offset += count;
            if count == 0 || query.offset >= page.total {
                return Ok(listed);
            }
        }
    }

    /// A run's file, from memory or its folder.
    pub fn open(&self, data: &Path, library: &Library, name: &str, which: Which) -> Result<Arc<Loaded>, ServeError> {
        let folder = files::folder_of(data, name)?;
        let at = written(&folder.join(files::COMMUNITIES)).ok_or(ServeError::NotFound)?;
        let now = SystemClock.now();
        let usage = {
            let mut indexed = self.index(data, library)?;
            let Indexed::Built { index, seen } = &mut *indexed else { return Err(ServeError::NotFound) };
            if seen.get(name) != Some(&at) {
                index.written(name, at);
                seen.insert(name.to_owned(), at);
                self.cache.invalidate(name);
            }
            match which {
                Which::Communities => index.opened(name, now),
                Which::Graph => index.usage(name),
            }
            .unwrap_or_default()
        };
        if which == Which::Communities {
            library.keep_run_usage(name, &usage)?;
            let mut shown = lock(&self.shown);
            if shown.as_deref() != Some(name) {
                if let Some(before) = shown.take() {
                    self.cache.unpin(&before);
                }
                *shown = Some(name.to_owned());
            }
        }
        let run = self.cache.get(name, usage, now, |name| load(data, name)).map_err(|error| match error {
            cache::CacheError::Failed { source, .. } if matches!(*source, RunFileError::NotFound) => {
                ServeError::NotFound
            }
            other => ServeError::Files(other.to_string()),
        })?;
        if which == Which::Communities {
            // pinned only while it is still the run on screen: another run opened meanwhile has unpinned it
            let shown = lock(&self.shown);
            if shown.as_deref() == Some(name) {
                self.cache.pin(name);
            }
        }
        Ok(run)
    }

    /// Give a run another title (its folder keeps its name). Answers the title shown.
    pub fn rename(&self, data: &Path, library: &Library, name: &str, title: &str) -> Result<String, ServeError> {
        let folder = files::folder_of(data, name)?;
        let shown = files::retitle(&folder, name, title).map_err(|error| ServeError::Files(error.to_string()))?;
        let record = match &mut *self.index(data, library)? {
            Indexed::Built { index, .. } => index.retitled(name, &shown),
            Indexed::NotBuilt => None,
        };
        if let Some(record) = record {
            library.keep_run_summaries(&[record])?;
        }
        Ok(shown)
    }

    /// Delete a run: its folder, and what the library keeps only about it (its summary and how it was opened).
    /// The library's papers, embeddings and descriptions stay, for other runs and for making it again.
    pub fn delete(&self, data: &Path, library: &Library, name: &str) -> Result<(), ServeError> {
        let folder = files::folder_of(data, name)?;
        let making = lock(&self.making);
        if making.as_deref() == Some(name) {
            return Err(ServeError::BeingMade(name.to_owned()));
        }
        fs::remove_dir_all(&folder).map_err(|error| ServeError::Files(format!("{}: {error}", folder.display())))?;
        drop(making);
        library.forget_run(name)?;
        if let Indexed::Built { index, seen } = &mut *self.index(data, library)? {
            index.removed(name);
            seen.remove(name);
        }
        let mut shown = lock(&self.shown);
        if shown.as_deref() == Some(name) {
            self.cache.unpin(name);
            *shown = None;
        }
        self.cache.invalidate(name);
        Ok(())
    }

    /// A run's files were written again in place (its communities named): the copy in memory is dropped, so that
    /// the next read is of the files as they are now. The time a file was written is known to the second only, so
    /// a change within the second it was opened would otherwise go unseen.
    pub fn changed(&self, name: &str) {
        self.cache.invalidate(name);
    }

    /// A run was just made (or made again): its summary written, the old one dropped from memory.
    pub fn saved(&self, data: &Path, library: &Library, name: &str) -> Result<(), ServeError> {
        let facts = summarise(data, name)?;
        let record = SummaryRecord { name: name.to_owned(), of: facts.created, facts };
        if let Indexed::Built { index, seen } = &mut *self.index(data, library)? {
            seen.insert(name.to_owned(), record.of);
            index.saved(record.clone());
        }
        library.keep_run_summaries(&[record])?;
        self.cache.invalidate(name);
        Ok(())
    }
}
