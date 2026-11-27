//! Many runs: listing them without loading them, keeping the likely ones in memory, warming those at start.
//!
//! A run (one selection of papers, clustered) is large once loaded: for twelve thousand papers its graph and
//! community tree are tens of megabytes. A person may have thousands of runs. So nothing here loads every run,
//! or even reads every run's file, to show the list; and what is kept in memory is chosen by how the person uses
//! their runs.
//!
//! | piece       | what it does                                                                     |
//! |-------------|----------------------------------------------------------------------------------|
//! | [`usage`]   | how likely a run is to be opened: recency and frequency in one decaying score    |
//! | [`index`]   | the sidebar's list from small summaries: sorted, searched, paged; no run loaded |
//! | [`cache`]   | loaded runs within a byte budget, each loaded once, least likely evicted first  |
//! | [`cache::warm`] | at start, the likeliest few loaded in the background                         |
//!
//! None of it stores anything. What must outlive the app is handed in and handed back, for the database:
//!
//! * **Summaries** ([`index::SummaryRecord`]: a run's [`index::Facts`] and which version of its results they
//!   describe) are written when a run is saved: call [`index::RunIndex::saved`] and store the record. A listing
//!   that had to rebuild summaries returns them in [`index::Listing::rebuilt`]: store those too. A retitle
//!   returns the record to store from [`index::RunIndex::retitled`].
//! * **Usage** ([`usage::Usage`], one small record a run) changes whenever a run is opened: store what
//!   [`index::RunIndex::opened`] returns.
//!
//! # Wiring it up
//!
//! At start, read the runs that exist with their times ([`index::RunStamp`], a listing of folders or a table, no
//! run opened), the stored summaries and the stored usage, and build the [`index::RunIndex`] from them. Make one
//! [`cache::RunCache`] (budget [`cache::DEFAULT_BUDGET`] unless the settings say otherwise) and call
//! [`cache::warm`] with the same loader the requests use; do not wait for it.
//!
//! When a run is **served** (the interface asks for a run's communities or graph):
//!
//! ```text
//! let usage = index.opened(name, clock.now())?;   // record_open: store `usage`
//! let run = cache.get(name, usage, clock.now(), load)?;
//! ```
//!
//! Count an open once per time the person opens a run, not once per file the interface fetches for it: the
//! interface asks for the communities and the graph of one run together, so record the open on the first of
//! the two (the communities), and serve the graph with the usage as it stands ([`index::RunIndex::usage`]).
//! Pin the run on screen ([`cache::RunCache::pin`]) and unpin the one it replaces, so that it is never evicted
//! while shown.
//!
//! When a run is **saved, redone, retitled or deleted**, update the index ([`index::RunIndex::saved`],
//! [`index::RunIndex::retitled`], [`index::RunIndex::removed`]) and call [`cache::RunCache::invalidate`] with its
//! name. A run changed on disk by something else: [`index::RunIndex::written`] and `invalidate`.
//!
//! # The list endpoint
//!
//! `GET /api/runs` takes, all optional, `offset` (default 0), `limit` (default [`index::DEFAULT_LIMIT`], at most
//! [`index::MAX_LIMIT`]), `sort` (`opened`, the default, most recently opened first; `name`; `created`; `size`)
//! and `search` (words that must all appear in a run's title or name, any case): [`index::Query`] reads exactly
//! these. It answers an [`index::Page`]: `total` (how many runs match), `offset`, and `rows`, each a run
//! (`"kind": "run"`), a run that could not be read (`"kind": "unreadable"`, with a reason), or, rarely, a run not
//! read yet (`"kind": "unread"`). The index is behind one lock, which serving a run needs too ([`index::RunIndex::opened`]),
//! so the endpoint must not read runs while holding it:
//!
//! ```text
//! for _ in 0..MOST_ROUNDS {
//!     let due = index.lock().to_rebuild(&query);       // which rows on the page need reading
//!     if due.is_empty() { break }
//!     let read = due.iter().map(|run| summarise(&run.name));  // without the lock
//!     let mut index = index.lock();
//!     for (run, read) in due.iter().zip(read) { store(index.rebuilt(run, read)) }
//! }
//! let page = index.lock().page(&query);
//! ```

pub mod cache;
pub mod index;
pub mod usage;
