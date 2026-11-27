//! The list of runs the sidebar shows, answered without loading a single run.
//!
//! A run's results are tens of megabytes; what the list needs of it (title, counts, modularity, dates, size) is a
//! few hundred bytes, its [`Facts`]. The facts are written when a run is saved ([`RunIndex::saved`]), kept by the
//! database as a [`SummaryRecord`], and handed to [`RunIndex::new`] at start. A listing ([`RunIndex::list`]) then
//! sorts, searches and pages over those small records only: ten thousand runs cost one page of rows.
//!
//! A run whose summary is missing (saved before summaries existed) or older than the run (redone by another
//! program, copied in by hand) is still listed, sorted with what is known of it. Its summary is rebuilt from the
//! run only when it lands on the page being shown, through [`Summarise`], and handed back in
//! [`Listing::rebuilt`] for the database to keep, so it is never rebuilt twice. A rebuilt summary can move its
//! run (a size or a date becomes known), so the page is chosen again after rebuilding until every row on it is
//! described: the page shown is always in its own order. While some runs are not described, an order by size
//! or date is exact only among those that are (a run not yet read sorts as size 0, created when last written),
//! so a run read for one page may move onto another; [`RunIndex::unsummarised`] lists them for whoever wants to
//! read them all in the background once, after which every order is exact.
//!
//! Reading a run is slow, and the index is shared behind a lock with what serves runs. So a listing can be done
//! in steps, the slow one without the lock: [`RunIndex::to_rebuild`] says which runs on the page need reading,
//! the caller reads them, [`RunIndex::rebuilt`] writes each result back, and [`RunIndex::page`] answers the page.
//! [`RunIndex::list`] does the same in one call, for a caller with the index to itself.
//!
//! A run that cannot be read is not read again on every listing: only once its results have been written again.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::usage::{Moment, Usage};

/// How many times a listing reads the runs on its page and chooses the page again, at most. Each round reads only
/// runs not yet described, so two suffice unless rebuilding moves many runs around; after the last, the page is
/// shown with what is known.
pub const MOST_ROUNDS: usize = 4;
/// Rows in a page when the request does not say.
pub const DEFAULT_LIMIT: usize = 50;
/// The most rows one page holds, whatever is asked: a page is meant to be small.
pub const MAX_LIMIT: usize = 500;

/// What a run says about itself, read from its results when it is saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Facts {
    /// The name the person gave it, or the run's name when they gave none.
    pub title: String,
    pub papers: u64,
    pub communities: u64,
    pub modularity: f64,
    pub created: Moment,
    /// About how much memory the run takes once loaded, for the cache's budget.
    pub bytes: u64,
}

/// A run's summary as the database keeps it: the facts, and which version of the run's results they describe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SummaryRecord {
    pub name: String,
    /// When the results described were written; older than the run's own time means the summary is stale.
    pub of: Moment,
    pub facts: Facts,
}

/// That a run exists, and when its results were last written: what a listing of the runs' folders (or table)
/// gives cheaply, without opening any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunStamp {
    pub name: String,
    pub written: Moment,
}

/// One run as the sidebar shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunSummary {
    pub name: String,
    pub title: String,
    pub papers: u64,
    pub communities: u64,
    pub modularity: f64,
    pub created: Moment,
    pub last_opened: Option<Moment>,
    pub times_opened: u32,
    pub bytes: u64,
}

/// A row of a page: a run; a run whose summary could not be rebuilt (its results unreadable), so that the person
/// sees it is there and can remove it; or a run not described yet (only after a listing that stopped reading).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Row {
    Run(RunSummary),
    Unreadable { name: String, reason: String },
    Unread { name: String },
}

/// The order of a listing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    /// Most recently opened first; runs never opened after them, newest first.
    #[default]
    Opened,
    /// By title, A to Z, ignoring case.
    Name,
    /// Newest first.
    Created,
    /// Largest first.
    Size,
}

/// What the list endpoint is asked: `?offset=&limit=&sort=&search=`, each optional.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Query {
    pub offset: usize,
    pub limit: usize,
    pub sort: Sort,
    /// Words that must all appear, in any case, in a run's title or name.
    pub search: String,
}

impl Default for Query {
    fn default() -> Self {
        Self { offset: 0, limit: DEFAULT_LIMIT, sort: Sort::default(), search: String::new() }
    }
}

/// One page of the list, and how many runs match in all (for "41–90 of 1,203").
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Page {
    pub total: usize,
    pub offset: usize,
    pub rows: Vec<Row>,
}

/// A page, and the summaries rebuilt to show it, which the database should keep.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    pub page: Page,
    pub rebuilt: Vec<SummaryRecord>,
}

/// Reads a run to describe it: the expensive way, used only for a run on the page whose summary is missing or
/// stale. Whoever wires the index up implements it over the run's results.
pub trait Summarise {
    type Error: fmt::Display;
    fn summarise(&self, name: &str) -> Result<Facts, Self::Error>;
}

/// What is known of a run's summary.
#[derive(Clone, Debug, PartialEq)]
enum Summary {
    /// Never written: only the run's name and time are known.
    Missing,
    /// Written for the results of `of`, current when that is the run's own time.
    Written { of: Moment, facts: Facts },
    /// Never written, and reading the run failed.
    Unreadable { reason: String },
}

/// Whether the summary may be rebuilt from the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Attempt {
    /// Never tried, or the last try succeeded.
    Open,
    /// Reading the results written at this time failed: not tried again until the run is written again.
    FailedFor(Moment),
}

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    written: Moment,
    summary: Summary,
    attempt: Attempt,
    usage: Usage,
    /// Title and name in lower case, so that a search does not lower-case ten thousand titles each time.
    haystack: String,
    /// The title in lower case, which the order by name compares.
    title_key: String,
}

impl Entry {
    fn new(name: String, written: Moment) -> Self {
        let haystack = name.to_lowercase();
        let title_key = haystack.clone();
        Self {
            name,
            written,
            summary: Summary::Missing,
            attempt: Attempt::Open,
            usage: Usage::Never,
            haystack,
            title_key,
        }
    }

    fn facts(&self) -> Option<&Facts> {
        match &self.summary {
            Summary::Written { facts, .. } => Some(facts),
            Summary::Missing | Summary::Unreadable { .. } => None,
        }
    }

    fn describe(&mut self, of: Moment, facts: Facts) {
        self.title_key = facts.title.to_lowercase();
        self.haystack = format!("{} {}", self.title_key, self.name.to_lowercase());
        self.summary = Summary::Written { of, facts };
    }

    /// Missing or older than the run.
    fn out_of_date(&self) -> bool {
        match &self.summary {
            Summary::Written { of, .. } => *of < self.written,
            Summary::Missing | Summary::Unreadable { .. } => true,
        }
    }

    fn needs_rebuilding(&self) -> bool {
        let given_up = matches!(self.attempt, Attempt::FailedFor(at) if at >= self.written);
        self.out_of_date() && !given_up
    }

    /// Without a summary, the run's results are as old as the run.
    fn created(&self) -> Moment {
        self.facts().map_or(self.written, |facts| facts.created)
    }

    fn bytes(&self) -> u64 {
        self.facts().map_or(0, |facts| facts.bytes)
    }

    fn row(&self) -> Row {
        match &self.summary {
            Summary::Written { facts, .. } => Row::Run(RunSummary {
                name: self.name.clone(),
                title: facts.title.clone(),
                papers: facts.papers,
                communities: facts.communities,
                modularity: facts.modularity,
                created: facts.created,
                last_opened: self.usage.last_opened(),
                times_opened: self.usage.times(),
                bytes: facts.bytes,
            }),
            Summary::Unreadable { reason } => Row::Unreadable { name: self.name.clone(), reason: reason.clone() },
            Summary::Missing => Row::Unread { name: self.name.clone() },
        }
    }
}

/// The order of `sort`, made total by the name, so that pages never overlap or skip a run.
fn order(sort: Sort, a: &Entry, b: &Entry) -> Ordering {
    let first = match sort {
        Sort::Opened => b.usage.last_opened().cmp(&a.usage.last_opened()).then_with(|| b.created().cmp(&a.created())),
        Sort::Name => a.title_key.cmp(&b.title_key),
        Sort::Created => b.created().cmp(&a.created()),
        Sort::Size => b.bytes().cmp(&a.bytes()),
    };
    first.then_with(|| a.name.cmp(&b.name))
}

/// A run the warm-up may choose: one whose size is known.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub name: String,
    pub usage: Usage,
    pub bytes: u64,
    pub created: Moment,
}

/// Every run's summary and usage, in memory: a few hundred bytes a run.
#[derive(Clone, Debug, Default)]
pub struct RunIndex {
    entries: HashMap<String, Entry>,
}

impl RunIndex {
    /// The runs that exist (`stamps`), with the summaries and usage the database kept. A summary or usage for a
    /// run that no longer exists is left out; whoever stored it may delete it.
    pub fn new(
        stamps: impl IntoIterator<Item = RunStamp>,
        summaries: impl IntoIterator<Item = SummaryRecord>,
        usage: impl IntoIterator<Item = (String, Usage)>,
    ) -> Self {
        let mut entries: HashMap<String, Entry> =
            stamps.into_iter().map(|stamp| (stamp.name.clone(), Entry::new(stamp.name, stamp.written))).collect();
        for record in summaries {
            if let Some(entry) = entries.get_mut(&record.name) {
                entry.describe(record.of, record.facts);
            }
        }
        for (name, kept) in usage {
            if let Some(entry) = entries.get_mut(&name) {
                entry.usage = kept;
            }
        }
        Self { entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A run was saved (new or redone): its summary is current. How it was used before stays.
    pub fn saved(&mut self, record: SummaryRecord) {
        let entry = self.entries.entry(record.name.clone()).or_insert_with(|| Entry::new(record.name, record.of));
        entry.written = entry.written.max(record.of);
        entry.describe(record.of, record.facts);
    }

    /// A run's results changed on disk without a new summary: it is rebuilt when next shown.
    pub fn written(&mut self, name: &str, at: Moment) {
        match self.entries.get_mut(name) {
            Some(entry) => entry.written = entry.written.max(at),
            None => {
                self.entries.insert(name.to_owned(), Entry::new(name.to_owned(), at));
            }
        }
    }

    /// The person gave a run another title. Answers the summary to keep, or `None` when the run has none yet
    /// (it is then read, with the new title, when next shown).
    pub fn retitled(&mut self, name: &str, title: &str) -> Option<SummaryRecord> {
        let entry = self.entries.get_mut(name)?;
        let Summary::Written { of, facts } = &entry.summary else {
            return None;
        };
        let (of, mut facts) = (*of, facts.clone());
        facts.title = title.to_owned();
        entry.describe(of, facts.clone());
        Some(SummaryRecord { name: name.to_owned(), of, facts })
    }

    pub fn removed(&mut self, name: &str) {
        self.entries.remove(name);
    }

    /// A run was opened at `at`: answers its new usage, for the database to keep, or `None` for a run not here.
    pub fn opened(&mut self, name: &str, at: Moment) -> Option<Usage> {
        let entry = self.entries.get_mut(name)?;
        entry.usage = entry.usage.opened(at);
        Some(entry.usage)
    }

    pub fn usage(&self, name: &str) -> Option<Usage> {
        self.entries.get(name).map(|entry| entry.usage)
    }

    /// The runs whose summary is missing or stale, for whoever wants to rebuild them all in the background (a
    /// search finds such a run only by its name until then).
    pub fn unsummarised(&self) -> Vec<String> {
        self.entries.values().filter(|entry| entry.out_of_date()).map(|entry| entry.name.clone()).collect()
    }

    /// The runs the warm-up can weigh: those with a summary, since a run of unknown size cannot be said to fit.
    pub fn candidates(&self) -> Vec<Candidate> {
        self.entries
            .values()
            .filter_map(|entry| {
                entry.facts().map(|facts| Candidate {
                    name: entry.name.clone(),
                    usage: entry.usage,
                    bytes: facts.bytes,
                    created: facts.created,
                })
            })
            .collect()
    }

    /// The runs on the page for `query`, in its order, and how many match in all.
    fn select(&self, query: &Query) -> (usize, usize, Vec<&Entry>) {
        let words: Vec<String> = query.search.split_whitespace().map(str::to_lowercase).collect();
        let mut found: Vec<&Entry> =
            self.entries.values().filter(|entry| words.iter().all(|word| entry.haystack.contains(word))).collect();
        let total = found.len();
        let offset = query.offset.min(total);
        let end = offset.saturating_add(query.limit.min(MAX_LIMIT)).min(total);
        let compare = |a: &&Entry, b: &&Entry| order(query.sort, a, b);
        // only the first `end` need be in order: a selection, then a sort of those, not a sort of every run
        if end > 0 && end < found.len() {
            found.select_nth_unstable_by(end - 1, compare);
        }
        found.truncate(end);
        found.sort_unstable_by(compare);
        found.drain(..offset);
        (total, offset, found)
    }

    /// The runs on the page for `query` whose summary must be read from the run before the page is shown.
    pub fn to_rebuild(&self, query: &Query) -> Vec<RunStamp> {
        let (_, _, shown) = self.select(query);
        shown
            .into_iter()
            .filter(|entry| entry.needs_rebuilding())
            .map(|entry| RunStamp { name: entry.name.clone(), written: entry.written })
            .collect()
    }

    /// What reading the run `stamp` gave. Answers the summary for the database to keep, or `None` when there is
    /// nothing to keep: the read failed, or the run was written again (or removed) while it was being read, in
    /// which case it is read again when next shown.
    pub fn rebuilt<E: fmt::Display>(&mut self, stamp: &RunStamp, read: Result<Facts, E>) -> Option<SummaryRecord> {
        let entry = self.entries.get_mut(&stamp.name).filter(|entry| entry.written == stamp.written)?;
        match read {
            Ok(facts) => {
                entry.attempt = Attempt::Open;
                entry.describe(stamp.written, facts.clone());
                Some(SummaryRecord { name: stamp.name.clone(), of: stamp.written, facts })
            }
            Err(error) => {
                entry.attempt = Attempt::FailedFor(stamp.written);
                // a stale summary is still better than none: it stays until the run can be read again
                if entry.facts().is_none() {
                    entry.summary = Summary::Unreadable { reason: error.to_string() };
                }
                None
            }
        }
    }

    /// The page for `query` with what is known now; nothing is read.
    pub fn page(&self, query: &Query) -> Page {
        let (total, offset, shown) = self.select(query);
        Page { total, offset, rows: shown.into_iter().map(Entry::row).collect() }
    }

    /// One page of the runs matching `query`, in its order, its out-of-date rows read with `summarise` first. A
    /// caller that shares the index behind a lock does these steps itself and reads without the lock (see the
    /// module's documentation).
    pub fn list(&mut self, query: &Query, summarise: &impl Summarise) -> Listing {
        let mut rebuilt = Vec::new();
        for _ in 0..MOST_ROUNDS {
            let due = self.to_rebuild(query);
            if due.is_empty() {
                break;
            }
            for stamp in due {
                let read = summarise.summarise(&stamp.name);
                rebuilt.extend(self.rebuilt(&stamp, read));
            }
        }
        Listing { page: self.page(query), rebuilt }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::{Mutex, MutexGuard};
    use std::time::Instant;

    fn lock_index(index: &Mutex<RunIndex>) -> MutexGuard<'_, RunIndex> {
        index.lock().unwrap()
    }

    /// Reads runs that are all readable except those named "broken…", and counts what it was asked.
    #[derive(Default)]
    struct Reader {
        asked: RefCell<Vec<String>>,
    }

    impl Summarise for Reader {
        type Error = String;
        fn summarise(&self, name: &str) -> Result<Facts, String> {
            self.asked.borrow_mut().push(name.to_owned());
            if name.starts_with("broken") {
                return Err("communities.json is not valid JSON".to_owned());
            }
            // "run-042" was created at 420 and is 42 bytes; any other name at 1, of 7 bytes
            let number = name.rsplit('-').next().and_then(|number| number.parse::<u64>().ok());
            let (created, bytes) = number.map_or((1, 7), |number| (i64::try_from(number).unwrap() * 10, number));
            Ok(facts(&format!("read {name}"), created, bytes))
        }
    }

    fn facts(title: &str, created: i64, bytes: u64) -> Facts {
        Facts { title: title.to_owned(), papers: 100, communities: 5, modularity: 0.4, created: Moment(created), bytes }
    }

    fn record(name: &str, title: &str, created: i64, bytes: u64) -> SummaryRecord {
        SummaryRecord { name: name.to_owned(), of: Moment(created), facts: facts(title, created, bytes) }
    }

    fn stamp(name: &str, written: i64) -> RunStamp {
        RunStamp { name: name.to_owned(), written: Moment(written) }
    }

    const TOPICS: [&str; 5] =
        ["Graph Neural Networks", "Diffusion Models", "Reinforcement Learning", "Robotics", "Vision"];

    /// `count` runs, every one summarised, a third of them opened.
    fn generated(count: usize) -> RunIndex {
        let mut index = RunIndex::default();
        for number in 0..count {
            let created = i64::try_from(number).unwrap() * 60;
            let title = format!("{} {}", TOPICS[number % TOPICS.len()], 2000 + number % 26);
            let bytes = u64::try_from((number * 7919) % 1000).unwrap();
            index.saved(record(&format!("run-{number:06}"), &title, created, bytes));
            if number % 3 == 0 {
                index.opened(&format!("run-{number:06}"), Moment(1_000_000_000 - created));
            }
        }
        index
    }

    fn names(page: &Page) -> Vec<String> {
        page.rows
            .iter()
            .map(|row| match row {
                Row::Run(run) => run.name.clone(),
                Row::Unreadable { name, .. } | Row::Unread { name } => name.clone(),
            })
            .collect()
    }

    fn query(offset: usize, limit: usize, sort: Sort, search: &str) -> Query {
        Query { offset, limit, sort, search: search.to_owned() }
    }

    #[test]
    fn pages_follow_on_without_overlap_or_gap_and_say_the_total() {
        let mut index = generated(5000);
        let reader = Reader::default();
        for sort in [Sort::Opened, Sort::Name, Sort::Created, Sort::Size] {
            let mut seen = Vec::new();
            let mut offset = 0;
            loop {
                let listing = index.list(&query(offset, 437, sort, ""), &reader);
                assert_eq!(listing.page.total, 5000);
                assert_eq!(listing.page.offset, offset);
                if listing.page.rows.is_empty() {
                    break;
                }
                offset += listing.page.rows.len();
                seen.extend(names(&listing.page));
            }
            let mut every = seen.clone();
            every.sort();
            every.dedup();
            assert_eq!((seen.len(), every.len()), (5000, 5000), "{sort:?}");
        }
        assert!(reader.asked.borrow().is_empty(), "listing summarised runs must never read a run");
    }

    #[test]
    fn a_page_is_the_same_slice_a_full_sort_would_give() {
        let mut index = generated(3000);
        let reader = Reader::default();
        for sort in [Sort::Opened, Sort::Name, Sort::Created, Sort::Size] {
            let everything = names(&index.list(&query(0, MAX_LIMIT, sort, ""), &reader).page);
            let mut whole = Vec::new();
            for offset in (0..3000).step_by(MAX_LIMIT) {
                whole.extend(names(&index.list(&query(offset, MAX_LIMIT, sort, ""), &reader).page));
            }
            assert_eq!(&whole[..MAX_LIMIT], &everything[..], "{sort:?}");
            let middle = names(&index.list(&query(1234, 20, sort, ""), &reader).page);
            assert_eq!(&middle[..], &whole[1234..1254], "{sort:?}");
        }
    }

    #[test]
    fn each_order_is_the_one_asked_for() {
        let mut index = generated(2000);
        let reader = Reader::default();
        let rows = |index: &mut RunIndex, sort| -> Vec<RunSummary> {
            let pages =
                (0..2000).step_by(MAX_LIMIT).map(|offset| index.list(&query(offset, MAX_LIMIT, sort, ""), &reader));
            let rows: Vec<Row> = pages.flat_map(|listing| listing.page.rows).collect();
            rows.into_iter().map(|row| if let Row::Run(run) = row { run } else { panic!() }).collect()
        };
        let opened = rows(&mut index, Sort::Opened);
        let (were, never) = opened.split_at(opened.iter().position(|run| run.last_opened.is_none()).unwrap());
        assert!(were.windows(2).all(|pair| pair[0].last_opened >= pair[1].last_opened));
        assert!(never.iter().all(|run| run.last_opened.is_none()));
        assert!(never.windows(2).all(|pair| pair[0].created >= pair[1].created));
        let by_name = rows(&mut index, Sort::Name);
        assert!(by_name.windows(2).all(|pair| pair[0].title.to_lowercase() <= pair[1].title.to_lowercase()));
        assert!(rows(&mut index, Sort::Created).windows(2).all(|pair| pair[0].created >= pair[1].created));
        assert!(rows(&mut index, Sort::Size).windows(2).all(|pair| pair[0].bytes >= pair[1].bytes));
    }

    #[test]
    fn a_search_needs_every_word_in_title_or_name_whatever_the_case() {
        let mut index = generated(4000);
        let reader = Reader::default();
        let listing = index.list(&query(0, MAX_LIMIT, Sort::Name, "  diffusion   MODELS 2003 "), &reader);
        // titles cycle through 5 topics and 26 years: 4000 / 130 runs, give or take one
        assert!((30..=31).contains(&listing.page.total), "{}", listing.page.total);
        for row in &listing.page.rows {
            let Row::Run(run) = row else { panic!() };
            assert_eq!(run.title, "Diffusion Models 2003");
        }
        assert_eq!(index.list(&query(0, 10, Sort::Name, "run-000042"), &reader).page.total, 1);
        assert_eq!(index.list(&query(0, 10, Sort::Name, "diffusion robotics"), &reader).page.total, 0);
        assert_eq!(index.list(&query(0, 10, Sort::Name, "   "), &reader).page.total, 4000);
    }

    #[test]
    fn a_page_past_the_end_is_empty_and_a_huge_limit_is_cut_to_the_most_a_page_holds() {
        let mut index = generated(1000);
        let reader = Reader::default();
        let past = index.list(&query(5000, 10, Sort::Opened, ""), &reader).page;
        assert_eq!((past.total, past.offset, past.rows.len()), (1000, 1000, 0));
        assert_eq!(index.list(&query(0, usize::MAX, Sort::Opened, ""), &reader).page.rows.len(), MAX_LIMIT);
        assert_eq!(index.list(&query(990, 50, Sort::Opened, ""), &reader).page.rows.len(), 10);
        let counted = index.list(&query(0, 0, Sort::Opened, ""), &reader).page;
        assert_eq!((counted.total, counted.rows.len()), (1000, 0));
    }

    #[test]
    fn missing_and_stale_summaries_are_rebuilt_only_for_the_page_shown_and_once() {
        let mut summaries: Vec<SummaryRecord> = Vec::new();
        let mut stamps = Vec::new();
        for number in 0..100 {
            let name = format!("run-{number:03}");
            stamps.push(stamp(&name, number * 10 + if number % 2 == 0 { 5 } else { 0 }));
            // the even ones were redone after their summary was written; every tenth has none at all
            if number % 10 != 0 {
                summaries.push(record(&name, &format!("old {number}"), number * 10, 1));
            }
        }
        let mut index = RunIndex::new(stamps, summaries, []);
        assert_eq!(index.unsummarised().len(), 40 + 10); // 40 stale, 10 missing
        let reader = Reader::default();
        let listing = index.list(&query(0, 10, Sort::Created, ""), &reader);
        // newest first: run-099 … run-090; the even ones are stale and run-090 has no summary
        assert_eq!(*reader.asked.borrow(), ["run-098", "run-096", "run-094", "run-092", "run-090"]);
        assert_eq!(listing.rebuilt.len(), 5);
        assert!(listing.rebuilt.iter().all(|record| record.facts.title.starts_with("read ")));
        assert_eq!(listing.rebuilt[0].of, Moment(985)); // describes the run as it is now
        let Row::Run(run) = &listing.page.rows[1] else { panic!() };
        assert_eq!(run.title, "read run-098");

        let again = index.list(&query(0, 10, Sort::Created, "read"), &reader);
        assert_eq!(again.page.total, 5);
        assert_eq!(reader.asked.borrow().len(), 5, "rebuilt summaries are kept");
        assert_eq!(index.unsummarised().len(), 45);
    }

    #[test]
    fn a_run_that_cannot_be_read_is_listed_as_unreadable_and_read_again_only_once_rewritten() {
        let mut index = RunIndex::new([stamp("broken-run", 1), stamp("fine", 2)], [], []);
        let reader = Reader::default();
        let asked = |reader: &Reader| reader.asked.borrow().iter().filter(|name| *name == "broken-run").count();
        let page = index.list(&Query::default(), &reader).page;
        assert_eq!(page.rows.len(), 2);
        assert!(page.rows.iter().any(|row| matches!(row, Row::Unreadable { name, reason }
            if name == "broken-run" && reason.contains("not valid JSON"))));
        index.list(&Query::default(), &reader);
        assert_eq!(asked(&reader), 1, "not read again on every listing");
        index.written("broken-run", Moment(10));
        index.list(&Query::default(), &reader);
        assert_eq!(asked(&reader), 2, "read again once its results were written again");
    }

    #[test]
    fn a_page_of_runs_never_described_is_shown_in_its_own_order_once_they_are() {
        // written in the order of their names, so by size they come out in another order once read
        let stamps: Vec<RunStamp> =
            (0..200).map(|number| stamp(&format!("run-{:03}", (number * 37) % 200), 5)).collect();
        let mut index = RunIndex::new(stamps, [], []);
        let reader = Reader::default();
        for offset in [0, 10, 50] {
            let page = index.list(&query(offset, 10, Sort::Size, ""), &reader).page;
            let sizes: Vec<u64> = page
                .rows
                .iter()
                .map(|row| if let Row::Run(run) = row { run.bytes } else { panic!("{row:?} not described") })
                .collect();
            assert_eq!(sizes.len(), 10);
            assert!(sizes.windows(2).all(|pair| pair[0] >= pair[1]), "{sizes:?}");
        }
        assert!(reader.asked.borrow().len() < 200, "only runs that came onto a page were read");
    }

    #[test]
    fn a_listing_can_read_runs_without_holding_the_index() {
        let index = Mutex::new(RunIndex::new([stamp("run-001", 5), stamp("run-002", 5)], [], []));
        let reader = Reader::default();
        let due = lock_index(&index).to_rebuild(&Query::default());
        assert_eq!(due.len(), 2);
        let read: Vec<_> = due.iter().map(|stamp| reader.summarise(&stamp.name)).collect(); // no lock held
        lock_index(&index).written("run-002", Moment(9)); // redone meanwhile: its read is out of date
        let mut index = lock_index(&index);
        let kept: Vec<_> = due.iter().zip(read).filter_map(|(stamp, read)| index.rebuilt(stamp, read)).collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(index.to_rebuild(&Query::default()), [stamp("run-002", 9)]);
        assert!(matches!(
            &index.page(&Query::default()).rows[..],
            [Row::Run(_), Row::Unread { .. }] | [Row::Unread { .. }, Row::Run(_)]
        ));
    }

    #[test]
    fn a_stale_summary_that_cannot_be_rebuilt_is_still_shown() {
        let mut index = RunIndex::new([stamp("broken-run", 50)], [record("broken-run", "Kept title", 10, 3)], []);
        let page = index.list(&Query::default(), &Reader::default()).page;
        assert!(matches!(&page.rows[0], Row::Run(run) if run.title == "Kept title"));
    }

    #[test]
    fn opening_saving_retitling_and_removing_keep_the_index_true() {
        let mut index =
            RunIndex::new([stamp("a", 1), stamp("b", 2)], [record("a", "A", 1, 1)], [("gone".into(), Usage::Never)]);
        assert_eq!(index.len(), 2);
        assert_eq!(index.opened("a", Moment(100)).unwrap().times(), 1);
        assert_eq!(index.opened("nothing", Moment(100)), None);
        let kept = index.retitled("a", "Better title").unwrap();
        assert_eq!((kept.facts.title.as_str(), kept.of), ("Better title", Moment(1)));
        assert_eq!(index.retitled("b", "No summary yet"), None);
        assert_eq!(index.list(&query(0, 10, Sort::Name, "better"), &Reader::default()).page.total, 1);

        index.saved(record("a", "Redone", 500, 9)); // redone: the usage stays
        assert_eq!(index.usage("a").unwrap().times(), 1);
        index.written("a", Moment(600));
        assert!(index.unsummarised().contains(&"a".to_owned()));
        index.removed("a");
        assert_eq!(index.usage("a"), None);
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn a_query_is_read_from_the_request_with_defaults_for_what_it_leaves_out() {
        let asked: Query = serde_json::from_str(r#"{"sort": "size", "search": "gnn"}"#).unwrap();
        assert_eq!(asked, Query { offset: 0, limit: DEFAULT_LIMIT, sort: Sort::Size, search: "gnn".into() });
        let page = Page { total: 1, offset: 0, rows: vec![Row::Unreadable { name: "x".into(), reason: "y".into() }] };
        let told = serde_json::to_value(page).unwrap();
        assert_eq!(told["rows"][0]["kind"], "unreadable");
    }

    #[test]
    #[ignore = "a measurement: cargo test --release -- --ignored --nocapture listing_a_page"]
    fn listing_a_page_out_of_a_hundred_thousand_runs_is_quick() {
        let mut index = generated(100_000);
        let reader = Reader::default();
        let asked =
            [(5000, Sort::Opened, ""), (5000, Sort::Name, ""), (5000, Sort::Size, ""), (0, Sort::Opened, "graph 2010")];
        for (offset, sort, search) in asked {
            let started = Instant::now();
            let listing = index.list(&query(offset, 50, sort, search), &reader);
            assert_eq!(listing.page.rows.len(), 50);
            let took = started.elapsed();
            let bytes = serde_json::to_vec(&listing.page).unwrap().len();
            println!(
                "{sort:?} {search:?}: {took:?} for {} rows of {} ({bytes} bytes)",
                listing.page.rows.len(),
                listing.page.total
            );
        }
    }
}
