//! How many threads the engine computes on, decided in one place, and the work of many papers run on them.
//!
//! # How many threads
//!
//! The engine computes on one pool of threads: rayon's global pool, which every parallel section of the engine
//! uses (`par_iter` and the like). [`start`] builds it once, as the engine starts, with the count [`decide`]
//! gives: every core the system lets the engine use ([`std::thread::available_parallelism`], which counts a
//! process's CPU affinity and container quota), or fewer when `CONSTELLATE_THREADS` caps it. Nothing else in the
//! engine starts threads that compute, so nothing multiplies that count:
//!
//! * the matrix library (`matrixmultiply`) is built without threads of its own; the large products are cut into
//!   rows on the pool instead ([`crate::analysis::graph`], [`crate::analysis::graph::eigen`]);
//! * ONNX Runtime's own threads (intra-op) on the processor are as many as the pool has
//!   ([`crate::model::embed`]), and the model runs on one thread outside the pool, one batch at a time: the
//!   model and the pool take turns, they do not run on top of each other;
//! * a parallel section inside another (each community's eigenvectors while the communities are cut in
//!   parallel) is more tasks for the same threads, never more threads;
//! * downloads wait on the network, not on a core: they have a few threads of their own
//!   ([`crate::sources::polite`]), so that a slow host never holds a computing thread.
//!
//! The HTTP layer's async runtime keeps threads of its own, and routes hand anything long to `spawn_blocking`
//! (`api::routes::on_the_side`), whose threads only wait for the pool. The interface is therefore answered while
//! a run computes, even when every computing thread is busy.
//!
//! # Many papers at once
//!
//! [`each`] runs one function over many papers on the pool and gives back one [`Outcome`] per paper, in the
//! papers' order: what the function returned, the panic it died of, or that the batch was cancelled before it
//! was begun. A paper that cannot be read is that paper's outcome, never the batch's end. [`Progress`] is how far
//! the batch is and whether it was asked to stop, read by whoever watches without taking any lock.

use std::any::Any;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rayon::prelude::*;

/// The environment variable that caps the computing threads.
pub const VARIABLE: &str = "CONSTELLATE_THREADS";

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ThreadsError {
    #[error("{VARIABLE} must be a whole number of threads, 1 or more (unset: every core)")]
    NotACount,
    #[error("the engine's computing threads could not be started: {0}")]
    NotStarted(String),
}

/// The cap `CONSTELLATE_THREADS` sets, read from its value (none when it is unset).
pub fn cap(value: Option<&str>) -> Result<Option<NonZeroUsize>, ThreadsError> {
    value.map(|text| text.trim().parse::<NonZeroUsize>().map_err(|_| ThreadsError::NotACount)).transpose()
}

/// How many threads the engine computes on: every core it may use, or `cap` when that is fewer.
pub fn decide(cap: Option<NonZeroUsize>) -> NonZeroUsize {
    let cores = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
    cap.map_or(cores, |cap| cap.min(cores))
}

/// Build the pool every parallel section computes on, with `threads` threads. Once per process, before anything
/// computes: a second call, or one after a parallel section already started the default pool, is refused.
pub fn start(threads: NonZeroUsize) -> Result<(), ThreadsError> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads.get())
        .thread_name(|index| format!("constellate-compute-{index}"))
        .build_global()
        .map_err(|error| ThreadsError::NotStarted(error.to_string()))
}

/// How many threads the code that calls this computes on: the engine's pool, or the pool a test installed.
pub fn computing() -> usize {
    rayon::current_num_threads()
}

/// What a panic said, for a person: its message when it had one.
pub fn panic_message(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "the work stopped unexpectedly".to_owned())
}

/// How far a batch is, and whether it was asked to stop: shared by the threads doing the work and whoever
/// watches it, with no lock at all.
///
/// The counters are plain atomic counts and the flag a plain atomic switch. Nothing is handed from one thread to
/// another through them (each paper's result travels in the batch's answer), so the cheapest ordering,
/// `Relaxed`, is enough: a watcher may see a count a moment old, never a wrong one, and a cancel is seen by every
/// thread at its next paper.
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    cancelled: AtomicBool,
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    /// `count` more papers to do.
    pub fn expect(&self, count: usize) {
        self.total.fetch_add(count, Ordering::Relaxed);
    }

    /// One more paper done, whatever came of it.
    pub fn advance(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Ask the batch to stop: papers already begun finish, the others are not begun.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// What became of one paper's work in a batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<T> {
    /// What the work returned (its own `Result`, when it can fail).
    Done(T),
    /// The work panicked, with this message. The other papers are not affected.
    Panicked(String),
    /// The batch was cancelled before this paper was begun.
    Cancelled,
}

impl<T> Outcome<T> {
    /// What the work returned, if it returned.
    pub fn done(self) -> Option<T> {
        match self {
            Self::Done(value) => Some(value),
            Self::Panicked(_) | Self::Cancelled => None,
        }
    }
}

/// Run `work` on one paper, turning a panic into that paper's outcome.
///
/// Catching the panic is sound here because nothing half-changed can be seen afterwards: `work` is given its
/// paper by shared reference, and what it returns is its own. Anything it shares with other papers' work must
/// itself survive a panic in the middle of a change (the library's writes roll back; a poisoned lock is taken
/// again only where every change under it is a single assignment), which is true of everything in this crate.
pub fn caught<T>(work: impl FnOnce() -> T) -> Outcome<T> {
    match catch_unwind(AssertUnwindSafe(work)) {
        Ok(value) => Outcome::Done(value),
        Err(panic) => Outcome::Panicked(panic_message(&*panic)),
    }
}

/// `work` on every one of `papers`, on the computing threads; one outcome per paper, in the papers' order.
///
/// Why the answer does not depend on how the threads shared the work: each paper's outcome depends on that paper
/// alone, and rayon's `collect` puts each in its paper's place whatever order they finished in. A cancel is
/// looked at before each paper, so a cancelled batch ends after the papers already begun.
pub fn each<P, T, F>(papers: &[P], progress: &Progress, work: F) -> Vec<Outcome<T>>
where
    P: Sync,
    T: Send,
    F: Fn(&P) -> T + Sync,
{
    progress.expect(papers.len());
    papers
        .par_iter()
        .map(|paper| {
            if progress.is_cancelled() {
                return Outcome::Cancelled;
            }
            let outcome = caught(|| work(paper));
            progress.advance();
            outcome
        })
        .collect()
}

/// A pool of `threads` threads for a test, so that a result can be computed on one thread and on many and the
/// two compared.
#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap()
}

/// The same work on one thread and on many: both answers, for a test to compare.
#[cfg(test)]
pub(crate) fn on_one_and_many<T: Send>(work: impl Fn() -> T + Sync) -> (T, T) {
    let one = pool(1).install(&work);
    let many = pool(16).install(&work);
    (one, many)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn the_cap_is_a_whole_number_of_threads_or_unset() {
        assert_eq!(cap(None), Ok(None));
        assert_eq!(cap(Some(" 4 ")), Ok(NonZeroUsize::new(4)));
        for wrong in ["0", "-2", "four", "1.5", ""] {
            assert_eq!(cap(Some(wrong)), Err(ThreadsError::NotACount), "{wrong:?}");
        }
        assert!(ThreadsError::NotACount.to_string().contains(VARIABLE));
    }

    #[test]
    fn the_engine_computes_on_every_core_unless_capped_and_never_on_more() {
        let cores = std::thread::available_parallelism().unwrap();
        assert_eq!(decide(None), cores);
        assert_eq!(decide(NonZeroUsize::new(1)), NonZeroUsize::MIN);
        assert_eq!(decide(NonZeroUsize::new(100_000)), cores);
    }

    #[test]
    fn every_paper_has_its_outcome_in_its_place_whatever_the_threads() {
        let papers: Vec<u64> = (0..2_000).collect();
        let work = || each(&papers, &Progress::new(), |paper| paper * paper);
        let (one, many) = on_one_and_many(work);
        assert_eq!(one, many);
        assert_eq!(many[1_999], Outcome::Done(1_999 * 1_999));
    }

    #[test]
    fn a_paper_whose_work_panics_is_that_papers_outcome_and_the_others_are_done() {
        let papers: Vec<u32> = (0..100).collect();
        let progress = Progress::new();
        let outcomes = pool(8).install(|| {
            each(&papers, &progress, |&paper| {
                assert!(paper % 10 != 3, "paper {paper} is unreadable");
                paper
            })
        });
        for (paper, outcome) in papers.iter().zip(&outcomes) {
            match outcome {
                Outcome::Panicked(said) => {
                    assert_eq!(paper % 10, 3);
                    assert!(said.contains(&format!("paper {paper} is unreadable")), "{said}");
                }
                other => assert_eq!(other, &Outcome::Done(*paper)),
            }
        }
        assert_eq!((progress.done(), progress.total()), (100, 100));
        // and the pool goes on as before
        assert_eq!(pool(8).install(|| each(&papers, &Progress::new(), |&paper| paper)).len(), 100);
    }

    #[test]
    fn a_cancelled_batch_begins_no_more_papers() {
        let papers: Vec<u32> = (0..1_000).collect();
        let progress = Progress::new();
        // one thread, so that which papers were begun before the cancel is known
        let outcomes = pool(1).install(|| {
            each(&papers, &progress, |&paper| {
                if paper == 10 {
                    progress.cancel();
                }
                paper
            })
        });
        assert_eq!(outcomes[..=10], (0..=10).map(Outcome::Done).collect::<Vec<_>>()[..]);
        assert!(outcomes[11..].iter().all(|outcome| *outcome == Outcome::Cancelled));
        assert_eq!((progress.done(), progress.total()), (11, 1_000));
    }

    #[test]
    fn progress_counted_from_many_threads_at_once_loses_no_paper() {
        const THREADS: usize = 16;
        const ROUNDS: usize = 500;
        let progress = Progress::new();
        let barrier = Barrier::new(THREADS);
        std::thread::scope(|scope| {
            for _ in 0..THREADS {
                scope.spawn(|| {
                    barrier.wait();
                    for _ in 0..ROUNDS {
                        progress.expect(1);
                        progress.advance();
                    }
                });
            }
        });
        assert_eq!((progress.done(), progress.total()), (THREADS * ROUNDS, THREADS * ROUNDS));
    }

    #[test]
    fn a_panic_says_what_it_said() {
        assert_eq!(caught(|| -> u32 { panic!("plain") }), Outcome::Panicked("plain".to_owned()));
        assert_eq!(caught(|| -> u32 { panic!("{} words", 2) }), Outcome::Panicked("2 words".to_owned()));
        assert_eq!(caught(|| 7).done(), Some(7));
    }
}
