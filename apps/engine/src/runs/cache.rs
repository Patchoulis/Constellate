//! Loaded runs kept in memory, within a budget, and the warm-up that fills it at start.
//!
//! A run's place in the cache is one of a few states ([`Phase`]), each with what belongs to it:
//!
//! ```text
//!     absent ──get / prefetch──▶ loading ──loaded──▶ ready ──evicted / invalidated──▶ absent
//!                                        ──failed──▶ absent
//! ```
//!
//! While a run is `loading`, everyone who asks for it waits for that one load ([`Pending`]) instead of starting
//! their own: two requests for a run trigger one read of its tens of megabytes. A load that fails leaves the run
//! absent, so the next request tries again, and hands its failure to everyone who was waiting ("failed just now",
//! [`Outcome::Failed`]); a failure is never kept as if it were the run.
//!
//! # What is evicted
//!
//! When a run that was asked for does not fit, the runs least likely to be opened again go first, likelihood
//! being the usage score ([`super::usage`]), not how recently each was touched: a run used every day survives a
//! single visit to an old one, which plain least-recently-used would not allow.
//!
//! Runs are handed out as [`Arc`]s. Evicting one only drops the cache's reference: whoever is showing it keeps it.
//! A run someone is still holding is not evicted at all, since that would free nothing, and neither is one
//! [pinned](RunCache::pin) (the run on screen). Only those two can make the cache go over its budget, and they hold
//! that memory whatever the cache does.
//!
//! # Warm-up
//!
//! At start, [`warm`] loads on a thread of its own the few runs likeliest to be opened ([`choose`]). It only fills
//! room that is free: it never evicts anything. What it loaded is then evicted like any run, by its score.
//! A request during the warm-up is served by its own thread at once; if the warm-up is loading that very run, the
//! request waits for that load rather than repeating it.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use super::index::{Candidate, RunIndex};
use super::usage::{Moment, Usage, likelier};

/// The default budget: 1 GiB. A loaded run of twelve thousand papers (its graph and community tree) is some tens
/// of megabytes, so this keeps a few dozen such runs, or a handful of runs ten times larger, and is a small part
/// of the memory of a computer that can compute the similarity matrix of a large run (which is never cached).
pub const DEFAULT_BUDGET: u64 = 1 << 30;
/// The share of the budget the warm-up may fill: half, so that what the person opens first fits beside it.
pub const WARM_SHARE: f64 = 0.5;
/// The most runs the warm-up loads: a person starts with one run, or flicks between two or three.
pub const WARM_MOST: usize = 3;

/// A loaded run says how much memory it takes, approximately.
pub trait Weigh {
    fn bytes(&self) -> u64;
}

/// Why a run could not be had from the cache.
#[derive(Debug, thiserror::Error)]
pub enum CacheError<E: std::error::Error + 'static> {
    /// Loading it failed; everyone who was waiting for that load is told the same.
    #[error("run {name} could not be loaded: {source}")]
    Failed { name: String, source: Arc<E> },
    /// The load stopped unexpectedly (it panicked).
    #[error("loading run {name} stopped unexpectedly")]
    Stopped { name: String },
}

/// Where a run stands in the cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Absent,
    Loading,
    Ready,
}

/// How a load ended, for everyone waiting on it.
enum Outcome<R, E> {
    Waiting,
    Loaded(Arc<R>),
    Failed(Arc<E>),
    Stopped,
}

/// One load in progress, which those who ask for the same run wait on.
struct Pending<R, E> {
    outcome: Mutex<Outcome<R, E>>,
    settled: Condvar,
}

impl<R, E: std::error::Error + 'static> Pending<R, E> {
    fn new() -> Self {
        Self { outcome: Mutex::new(Outcome::Waiting), settled: Condvar::new() }
    }

    fn settle(&self, outcome: Outcome<R, E>) {
        *lock(&self.outcome) = outcome;
        self.settled.notify_all();
    }

    fn wait(&self, name: &str) -> Result<Arc<R>, CacheError<E>> {
        let mut outcome = lock(&self.outcome);
        loop {
            match &*outcome {
                Outcome::Waiting => outcome = self.settled.wait(outcome).unwrap_or_else(PoisonError::into_inner),
                Outcome::Loaded(run) => return Ok(Arc::clone(run)),
                Outcome::Failed(error) => {
                    return Err(CacheError::Failed { name: name.to_owned(), source: Arc::clone(error) });
                }
                Outcome::Stopped => return Err(CacheError::Stopped { name: name.to_owned() }),
            }
        }
    }
}

/// Who wanted a run in memory: the person (it may make room) or the warm-up (it may only fill free room).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wanted {
    Opened,
    Warmed,
}

/// Whether eviction may drop a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hold {
    Free,
    Pinned,
}

struct Ready<R> {
    run: Arc<R>,
    bytes: u64,
    usage: Usage,
    hold: Hold,
}

enum Slot<R, E> {
    Loading { pending: Arc<Pending<R, E>>, usage: Usage, wanted: Wanted },
    Ready(Ready<R>),
}

struct Inner<R, E> {
    slots: HashMap<String, Slot<R, E>>,
    /// The bytes of the ready runs.
    used: u64,
}

/// Every change to the cache is made whole under the lock, so a thread that panicked left nothing half-done.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Loaded runs by name, within a budget in bytes.
pub struct RunCache<R, E> {
    inner: Mutex<Inner<R, E>>,
    budget: u64,
}

impl<R, E> Default for RunCache<R, E>
where
    R: Weigh,
    E: std::error::Error + 'static,
{
    fn default() -> Self {
        Self::new(DEFAULT_BUDGET)
    }
}

impl<R, E> RunCache<R, E>
where
    R: Weigh,
    E: std::error::Error + 'static,
{
    pub fn new(budget: u64) -> Self {
        Self { inner: Mutex::new(Inner { slots: HashMap::new(), used: 0 }), budget }
    }

    pub fn budget(&self) -> u64 {
        self.budget
    }

    /// The bytes the cache holds now.
    pub fn used(&self) -> u64 {
        lock(&self.inner).used
    }

    pub fn phase(&self, name: &str) -> Phase {
        match lock(&self.inner).slots.get(name) {
            None => Phase::Absent,
            Some(Slot::Loading { .. }) => Phase::Loading,
            Some(Slot::Ready(_)) => Phase::Ready,
        }
    }

    /// The run `name`, from memory or loaded with `load`, for a person who opened it. `usage` is the run's usage
    /// with this open already counted (from [`RunIndex::opened`]) and `now` the time, which together say what is
    /// worth keeping. Whoever asks while the run is loading waits for that load.
    pub fn get<L>(&self, name: &str, usage: Usage, now: Moment, load: L) -> Result<Arc<R>, CacheError<E>>
    where
        L: FnOnce(&str) -> Result<R, E>,
    {
        let pending = {
            let mut inner = lock(&self.inner);
            match inner.slots.get_mut(name) {
                Some(Slot::Ready(ready)) => {
                    ready.usage = usage;
                    return Ok(Arc::clone(&ready.run));
                }
                Some(Slot::Loading { pending, usage: waiting, wanted }) => {
                    *waiting = usage;
                    *wanted = Wanted::Opened; // a run the warm-up is loading becomes one the person opened
                    let pending = Arc::clone(pending);
                    drop(inner);
                    return pending.wait(name);
                }
                None => {
                    let pending = Arc::new(Pending::new());
                    let slot = Slot::Loading { pending: Arc::clone(&pending), usage, wanted: Wanted::Opened };
                    inner.slots.insert(name.to_owned(), slot);
                    pending
                }
            }
        };
        self.load(name, &pending, now, load)
    }

    /// Load `name` for the warm-up, unless it is already there or on its way. Answers what happened.
    pub fn prefetch<L>(&self, name: &str, usage: Usage, now: Moment, load: L) -> Warmed
    where
        L: FnOnce(&str) -> Result<R, E>,
    {
        let pending = {
            let mut inner = lock(&self.inner);
            if inner.slots.contains_key(name) {
                return Warmed::AlreadyThere;
            }
            let pending = Arc::new(Pending::new());
            let slot = Slot::Loading { pending: Arc::clone(&pending), usage, wanted: Wanted::Warmed };
            inner.slots.insert(name.to_owned(), slot);
            pending
        };
        match self.load(name, &pending, now, load) {
            Ok(_) if self.phase(name) == Phase::Ready => Warmed::Loaded,
            Ok(_) => Warmed::NoRoom,
            Err(error) => Warmed::Failed(error.to_string()),
        }
    }

    /// Run `load` for the slot this thread put in `loading`, then settle it: keep the run if it is still wanted
    /// and there is room, and tell everyone waiting.
    fn load<L>(&self, name: &str, pending: &Arc<Pending<R, E>>, now: Moment, load: L) -> Result<Arc<R>, CacheError<E>>
    where
        L: FnOnce(&str) -> Result<R, E>,
    {
        // weighed here too: a panic while weighing must not leave anyone waiting
        let loaded = catch_unwind(AssertUnwindSafe(|| load(name).map(|run| (run.bytes(), run))));
        let mut inner = lock(&self.inner);
        // the slot is still this load's unless the run was invalidated (and perhaps asked for again) meanwhile
        let wanted = match inner.slots.remove(name) {
            Some(Slot::Loading { pending: there, usage, wanted }) if Arc::ptr_eq(&there, pending) => {
                Some((usage, wanted))
            }
            Some(someone_else) => {
                inner.slots.insert(name.to_owned(), someone_else);
                None
            }
            None => None,
        };
        let (outcome, answer) = match loaded {
            Ok(Ok((bytes, run))) => {
                let run = Arc::new(run);
                if let Some((usage, wanted)) = wanted {
                    let ready = Ready { run: Arc::clone(&run), bytes, usage, hold: Hold::Free };
                    self.admit(&mut inner, name, ready, wanted, now);
                }
                (Outcome::Loaded(Arc::clone(&run)), Ok(run))
            }
            Ok(Err(error)) => {
                let error = Arc::new(error);
                let answer = Err(CacheError::Failed { name: name.to_owned(), source: Arc::clone(&error) });
                (Outcome::Failed(error), answer)
            }
            Err(_) => (Outcome::Stopped, Err(CacheError::Stopped { name: name.to_owned() })),
        };
        drop(inner);
        pending.settle(outcome);
        answer
    }

    /// Keep a loaded run if it may be kept: one the person opened makes room by evicting the least likely runs
    /// nobody holds; one the warm-up loaded only takes room that is free. A run larger than the whole budget is
    /// handed out but not kept.
    fn admit(&self, inner: &mut Inner<R, E>, name: &str, ready: Ready<R>, wanted: Wanted, now: Moment) {
        if ready.bytes > self.budget {
            return;
        }
        let free = self.budget.saturating_sub(inner.used);
        if wanted == Wanted::Warmed && ready.bytes > free {
            return;
        }
        while inner.used + ready.bytes > self.budget {
            let least = inner
                .slots
                .iter()
                .filter_map(|(name, slot)| match slot {
                    Slot::Ready(kept) if kept.hold == Hold::Free && Arc::strong_count(&kept.run) == 1 => {
                        Some((name, kept))
                    }
                    Slot::Ready(_) | Slot::Loading { .. } => None,
                })
                .min_by(|(a_name, a), (b_name, b)| likelier(&a.usage, &b.usage, now).then_with(|| a_name.cmp(b_name)))
                .map(|(name, _)| name.clone());
            let Some(least) = least else { break }; // the rest are held or pinned: they stay, over the budget
            self.drop_slot(inner, &least);
        }
        inner.used += ready.bytes;
        inner.slots.insert(name.to_owned(), Slot::Ready(ready));
    }

    fn drop_slot(&self, inner: &mut Inner<R, E>, name: &str) {
        if let Some(Slot::Ready(ready)) = inner.slots.remove(name) {
            inner.used = inner.used.saturating_sub(ready.bytes);
        }
    }

    /// The run changed on disk (redone, retitled, deleted): forget it. Whoever holds it keeps their copy; a load in
    /// progress still answers those waiting for it but is not kept.
    pub fn invalidate(&self, name: &str) {
        let mut inner = lock(&self.inner);
        match inner.slots.get(name) {
            Some(Slot::Ready(_)) => self.drop_slot(&mut inner, name),
            Some(Slot::Loading { .. }) => {
                inner.slots.remove(name);
            }
            None => {}
        }
    }

    /// Keep `name` whatever else is asked for (the run on screen). Answers whether it is in memory to be pinned.
    pub fn pin(&self, name: &str) -> bool {
        self.set_hold(name, Hold::Pinned)
    }

    /// Let `name` be evicted again.
    pub fn unpin(&self, name: &str) -> bool {
        self.set_hold(name, Hold::Free)
    }

    /// How many requests are waiting on the load of `name`: the slot and the loading thread hold the others.
    #[cfg(test)]
    fn waiters(&self, name: &str) -> usize {
        match lock(&self.inner).slots.get(name) {
            Some(Slot::Loading { pending, .. }) => Arc::strong_count(pending).saturating_sub(2),
            Some(Slot::Ready(_)) | None => 0,
        }
    }

    fn set_hold(&self, name: &str, hold: Hold) -> bool {
        match lock(&self.inner).slots.get_mut(name) {
            Some(Slot::Ready(ready)) => {
                ready.hold = hold;
                true
            }
            Some(Slot::Loading { .. }) | None => false,
        }
    }
}

/// What the warm-up did with one run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warmed {
    Loaded,
    /// Already in memory or being loaded (the person asked for it first).
    AlreadyThere,
    /// Loaded, but by then the room it was chosen for had been taken.
    NoRoom,
    Failed(String),
}

/// The runs to warm, likeliest first: the highest scores that fit together in [`WARM_SHARE`] of `budget`, at most
/// [`WARM_MOST`], none that alone is larger than the share. With no run ever opened (a first start) it is the
/// most recently created run alone, if it fits.
pub fn choose(candidates: &[Candidate], now: Moment, budget: u64) -> Vec<String> {
    // budgets are far below 2^53 bytes, where a float stops counting bytes exactly
    let share = (budget as f64 * WARM_SHARE) as u64;
    let mut opened: Vec<&Candidate> =
        candidates.iter().filter(|candidate| candidate.usage.last_opened().is_some()).collect();
    if opened.is_empty() {
        let newest = candidates.iter().max_by(|a, b| a.created.cmp(&b.created).then_with(|| b.name.cmp(&a.name)));
        return newest.filter(|run| run.bytes <= share).map(|run| vec![run.name.clone()]).unwrap_or_default();
    }
    opened.sort_by(|a, b| likelier(&b.usage, &a.usage, now).then_with(|| a.name.cmp(&b.name)));
    let mut room = share;
    let mut chosen = Vec::new();
    for candidate in opened {
        if chosen.len() == WARM_MOST {
            break;
        }
        if candidate.bytes <= room {
            room -= candidate.bytes;
            chosen.push(candidate.name.clone());
        }
    }
    chosen
}

/// Start the warm-up: [`choose`] the runs from `index` and load them with `load` on a thread of its own, likeliest
/// first, while start-up carries on. The handle answers, when joined, what became of each.
pub fn warm<R, E, L>(
    index: &RunIndex,
    cache: &Arc<RunCache<R, E>>,
    now: Moment,
    load: L,
) -> JoinHandle<Vec<(String, Warmed)>>
where
    R: Weigh + Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
    L: Fn(&str) -> Result<R, E> + Send + 'static,
{
    let runs: Vec<(String, Usage)> = choose(&index.candidates(), now, cache.budget())
        .into_iter()
        .map(|name| {
            let usage = index.usage(&name).unwrap_or_default();
            (name, usage)
        })
        .collect();
    let cache = Arc::clone(cache);
    thread::spawn(move || {
        runs.into_iter()
            .map(|(name, usage)| {
                let warmed = cache.prefetch(&name, usage, now, &load);
                (name, warmed)
            })
            .collect()
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::runs::index::{Facts, SummaryRecord};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Barrier, mpsc};
    use std::time::Duration;

    const DAY: i64 = 24 * 60 * 60;
    const NOW: Moment = Moment(1_800_000_000);

    /// A run of a given size; its name says which it is.
    #[derive(Debug, PartialEq)]
    struct Run {
        name: String,
        bytes: u64,
    }

    impl Weigh for Run {
        fn bytes(&self) -> u64 {
            self.bytes
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("{0}")]
    struct Unreadable(String);

    type Cache = RunCache<Run, Unreadable>;

    fn sized(bytes: u64) -> impl Fn(&str) -> Result<Run, Unreadable> {
        move |name| Ok(Run { name: name.to_owned(), bytes })
    }

    fn opened(times: &[i64]) -> Usage {
        times.iter().fold(Usage::Never, |usage, days_ago| usage.opened(NOW.plus(-days_ago * DAY)))
    }

    #[test]
    fn a_run_is_loaded_once_and_then_served_from_memory() {
        let cache = Cache::new(100);
        let calls = AtomicUsize::new(0);
        let load = |name: &str| {
            calls.fetch_add(1, Ordering::SeqCst);
            sized(10)(name)
        };
        let first = cache.get("a", opened(&[0]), NOW, load).unwrap();
        let second = cache.get("a", opened(&[0, 0]), NOW, load).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!((cache.phase("a"), cache.used()), (Phase::Ready, 10));
    }

    #[test]
    fn the_least_likely_run_is_evicted_not_the_least_recent() {
        let cache = Cache::new(30);
        // opened every day this week; then two runs opened just now
        cache.get("daily", opened(&[0, 1, 2, 3, 4, 5, 6]), NOW, sized(10)).unwrap();
        cache.get("recent", opened(&[0]), NOW, sized(10)).unwrap();
        cache.get("other", opened(&[0, 0]), NOW, sized(10)).unwrap();
        // touching "recent" and "other" again makes "daily" the least recently used
        cache.get("recent", opened(&[0, 0]), NOW, sized(10)).unwrap();
        cache.get("other", opened(&[0, 0, 0]), NOW, sized(10)).unwrap();
        // one visit to a run last opened a year ago
        cache.get("old", opened(&[365, 0]), NOW, sized(10)).unwrap();
        assert_eq!(cache.phase("daily"), Phase::Ready, "the run used every day survives");
        assert_eq!(cache.phase("recent"), Phase::Absent, "the least likely one goes");
        assert_eq!((cache.phase("other"), cache.phase("old")), (Phase::Ready, Phase::Ready));
        assert_eq!(cache.used(), 30);
    }

    #[test]
    fn eviction_makes_as_much_room_as_the_new_run_needs_least_likely_first() {
        let cache = Cache::new(100);
        for (name, days_ago) in [("a", 30), ("b", 20), ("c", 10), ("d", 0)] {
            cache.get(name, opened(&[days_ago]), NOW, sized(25)).unwrap();
        }
        cache.get("big", opened(&[0]), NOW, sized(60)).unwrap();
        let phases: Vec<Phase> = ["a", "b", "c", "d", "big"].map(|name| cache.phase(name)).to_vec();
        assert_eq!(phases, [Phase::Absent, Phase::Absent, Phase::Absent, Phase::Ready, Phase::Ready]);
        assert_eq!(cache.used(), 85);
    }

    #[test]
    fn a_run_being_shown_is_never_taken_from_its_reader() {
        let cache = Cache::new(20);
        let shown = cache.get("shown", opened(&[30]), NOW, sized(10)).unwrap();
        cache.get("b", opened(&[0]), NOW, sized(10)).unwrap();
        cache.get("c", opened(&[0]), NOW, sized(10)).unwrap();
        // "shown" is the least likely, but someone holds it: "b" made way instead
        assert_eq!((cache.phase("shown"), cache.phase("b")), (Phase::Ready, Phase::Absent));
        cache.invalidate("shown");
        assert_eq!(cache.phase("shown"), Phase::Absent);
        assert_eq!(shown.name, "shown", "the reader's copy outlives the cache's");
        assert_eq!(cache.used(), 10);
    }

    #[test]
    fn a_pinned_run_stays_until_unpinned_even_over_the_budget() {
        let cache = Cache::new(20);
        cache.get("pinned", opened(&[60]), NOW, sized(10)).unwrap();
        assert!(cache.pin("pinned"));
        assert!(!cache.pin("not loaded"));
        cache.get("b", opened(&[0]), NOW, sized(10)).unwrap();
        cache.get("c", opened(&[0]), NOW, sized(20)).unwrap();
        assert_eq!(
            (cache.phase("pinned"), cache.phase("b"), cache.phase("c")),
            (Phase::Ready, Phase::Absent, Phase::Ready)
        );
        assert_eq!(cache.used(), 30); // over: the pinned run holds its memory whatever the cache does
        assert!(cache.unpin("pinned"));
        cache.get("d", opened(&[0]), NOW, sized(10)).unwrap();
        assert_eq!(cache.phase("pinned"), Phase::Absent);
    }

    #[test]
    fn a_run_larger_than_the_budget_is_served_but_not_kept() {
        let cache = Cache::new(50);
        cache.get("small", opened(&[0]), NOW, sized(10)).unwrap();
        let huge = cache.get("huge", opened(&[0]), NOW, sized(51)).unwrap();
        assert_eq!(huge.bytes, 51);
        assert_eq!((cache.phase("huge"), cache.phase("small"), cache.used()), (Phase::Absent, Phase::Ready, 10));
    }

    #[test]
    fn many_threads_asking_for_one_run_at_once_cause_one_load() {
        let cache = Arc::new(Cache::new(100));
        let calls = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (cache, calls, start) = (Arc::clone(&cache), Arc::clone(&calls), Arc::clone(&start));
                thread::spawn(move || {
                    start.wait();
                    cache
                        .get("a", opened(&[0]), NOW, |name| {
                            calls.fetch_add(1, Ordering::SeqCst);
                            thread::sleep(Duration::from_millis(100));
                            sized(10)(name)
                        })
                        .unwrap()
                })
            })
            .collect();
        let runs: Vec<Arc<Run>> = threads.into_iter().map(|thread| thread.join().unwrap()).collect();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(runs.iter().all(|run| Arc::ptr_eq(run, &runs[0])));
    }

    #[test]
    fn a_failed_load_is_told_to_everyone_waiting_and_not_kept() {
        let cache = Arc::new(Cache::new(100));
        let (started, loading) = mpsc::channel();
        let (release, wait) = mpsc::channel::<()>();
        let first = {
            let cache = Arc::clone(&cache);
            thread::spawn(move || {
                cache.get("a", opened(&[0]), NOW, move |_| {
                    started.send(()).unwrap();
                    wait.recv().unwrap();
                    Err(Unreadable("graph.json is cut short".to_owned()))
                })
            })
        };
        loading.recv().unwrap();
        assert_eq!(cache.phase("a"), Phase::Loading);
        let second = {
            let cache = Arc::clone(&cache);
            thread::spawn(move || {
                cache.get("a", opened(&[0]), NOW, |_| panic!("the second request must wait, not load"))
            })
        };
        while cache.waiters("a") == 0 {
            thread::sleep(Duration::from_millis(1));
        }
        release.send(()).unwrap();
        for outcome in [first.join().unwrap(), second.join().unwrap()] {
            let error = outcome.unwrap_err();
            assert!(matches!(&error, CacheError::Failed { name, .. } if name == "a"));
            assert_eq!(error.to_string(), "run a could not be loaded: graph.json is cut short");
        }
        assert_eq!(cache.phase("a"), Phase::Absent);
        assert_eq!(cache.get("a", opened(&[0]), NOW, sized(5)).unwrap().bytes, 5, "the next request tries again");
    }

    #[test]
    fn a_load_that_panics_leaves_nobody_waiting_for_ever() {
        let cache = Cache::new(100);
        let error = cache.get("a", opened(&[0]), NOW, |_| -> Result<Run, Unreadable> { panic!("bug") }).unwrap_err();
        assert!(matches!(error, CacheError::Stopped { .. }));
        assert_eq!(cache.phase("a"), Phase::Absent);
    }

    #[test]
    fn a_run_invalidated_while_loading_answers_its_waiters_but_is_not_kept() {
        let cache = Arc::new(Cache::new(100));
        let (started, loading) = mpsc::channel();
        let (release, wait) = mpsc::channel::<()>();
        let reader = {
            let cache = Arc::clone(&cache);
            thread::spawn(move || {
                cache.get("a", opened(&[0]), NOW, move |name| {
                    started.send(()).unwrap();
                    wait.recv().unwrap();
                    sized(10)(name)
                })
            })
        };
        loading.recv().unwrap();
        cache.invalidate("a"); // redone while it was being read
        release.send(()).unwrap();
        assert_eq!(reader.join().unwrap().unwrap().bytes, 10);
        assert_eq!((cache.phase("a"), cache.used()), (Phase::Absent, 0));
    }

    #[test]
    fn invalidating_a_run_frees_its_room() {
        let cache = Cache::new(100);
        cache.get("a", opened(&[0]), NOW, sized(40)).unwrap();
        cache.invalidate("a");
        cache.invalidate("never there");
        assert_eq!((cache.phase("a"), cache.used()), (Phase::Absent, 0));
    }

    fn candidate(name: &str, usage: Usage, bytes: u64, created_days_ago: i64) -> Candidate {
        Candidate { name: name.to_owned(), usage, bytes, created: NOW.plus(-created_days_ago * DAY) }
    }

    #[test]
    fn the_warm_up_chooses_the_likeliest_runs_that_fit_in_its_share() {
        let candidates = [
            candidate("daily", opened(&[0, 1, 2]), 30, 100),
            candidate("too big alone", opened(&[0, 0, 0, 0, 0]), 51, 100),
            candidate("weekly", opened(&[7]), 15, 100),
            candidate("last month", opened(&[30]), 5, 100),
            candidate("last year", opened(&[365]), 1, 100),
            candidate("never", Usage::Never, 1, 0),
        ];
        // a share of 50: "daily" (30) and "weekly" (15) fit, then "last month" (5) makes three
        assert_eq!(choose(&candidates, NOW, 100), ["daily", "weekly", "last month"]);
        assert_eq!(choose(&candidates, NOW, 1000), ["too big alone", "daily", "weekly"], "never more than three");
        assert_eq!(choose(&candidates, NOW, 40), ["weekly", "last month"]);
    }

    #[test]
    fn a_first_start_warms_only_the_newest_run() {
        let candidates = [
            candidate("old", Usage::Never, 1, 30),
            candidate("newest", Usage::Never, 1, 1),
            candidate("mid", Usage::Never, 1, 5),
        ];
        assert_eq!(choose(&candidates, NOW, 100), ["newest"]);
        assert_eq!(choose(&[candidate("huge", Usage::Never, 80, 1)], NOW, 100), Vec::<String>::new());
        assert_eq!(choose(&[], NOW, 100), Vec::<String>::new());
    }

    fn index_of(runs: &[(&str, Usage, u64)]) -> RunIndex {
        let mut index = RunIndex::default();
        for (name, usage, bytes) in runs {
            let facts = Facts {
                title: (*name).to_owned(),
                papers: 1,
                communities: 1,
                modularity: 0.0,
                created: NOW,
                bytes: *bytes,
            };
            index.saved(SummaryRecord { name: (*name).to_owned(), of: NOW, facts });
            if let Some(at) = usage.last_opened() {
                for _ in 0..usage.times() {
                    index.opened(name, at);
                }
            }
        }
        index
    }

    #[test]
    fn the_warm_up_does_not_hold_up_start_and_a_request_meanwhile_is_served_at_once() {
        let index = index_of(&[
            ("first", opened(&[0, 0, 0]), 10),
            ("second", opened(&[0, 0]), 10),
            ("third", opened(&[0]), 10),
        ]);
        let cache = Arc::new(Cache::new(100));
        let (started, loading) = mpsc::channel();
        let (release, wait) = mpsc::channel::<()>();
        let (started, wait) = (Mutex::new(started), Mutex::new(wait));
        let warming = warm(&index, &cache, NOW, move |name| {
            if name == "first" {
                lock(&started).send(()).unwrap();
                lock(&wait).recv().unwrap(); // the disk is slow today
            }
            sized(10)(name)
        });
        // warm has returned while its first load cannot finish: start-up goes on
        loading.recv().unwrap();
        assert_eq!(cache.phase("first"), Phase::Loading);
        let third = cache.get("third", opened(&[0, 0]), NOW, sized(10)).unwrap();
        assert_eq!(third.name, "third");
        release.send(()).unwrap();
        let outcome = warming.join().unwrap();
        assert_eq!(
            outcome,
            [
                ("first".to_owned(), Warmed::Loaded),
                ("second".to_owned(), Warmed::Loaded),
                ("third".to_owned(), Warmed::AlreadyThere)
            ]
        );
    }

    #[test]
    fn the_warm_up_never_evicts_a_run_the_person_opened() {
        let index = index_of(&[("likely", opened(&[0, 0, 0, 0]), 40)]);
        let cache = Arc::new(Cache::new(100));
        cache.get("opened", opened(&[300]), NOW, sized(70)).unwrap();
        let outcome = warm(&index, &cache, NOW, sized(40)).join().unwrap();
        assert_eq!(outcome, [("likely".to_owned(), Warmed::NoRoom)]);
        assert_eq!((cache.phase("opened"), cache.phase("likely"), cache.used()), (Phase::Ready, Phase::Absent, 70));
    }

    #[test]
    fn a_failed_warm_up_load_is_reported_and_leaves_the_run_absent() {
        let cache = Cache::new(30);
        let warmed = cache.prefetch("a", opened(&[0]), NOW, |_| Err(Unreadable("missing".to_owned())));
        assert_eq!(warmed, Warmed::Failed("run a could not be loaded: missing".to_owned()));
        assert_eq!(cache.phase("a"), Phase::Absent);
    }
}
