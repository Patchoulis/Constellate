//! One background job at a time, as an explicit state machine.
//!
//! ```text
//!     idle ──start──▶ running ──work returned──▶ done
//!                             ──work failed────▶ failed
//!     done / failed ──start──▶ running
//! ```
//!
//! The state is one value ([`State`]), and what belongs to a state lives inside it: only a finished job has a
//! result, only a failed one an error. A job that is "done" without a result cannot be written down, and whoever
//! looks ([`Job::snapshot`]) sees the state and what belongs to it together, never one without the other.
//!
//! Every job that starts gets a number and says what kind of work it is. Whoever follows a job can therefore tell
//! "the job I was following finished" from "some other job finished", and "a run finished" from "reading the
//! declarations finished".
//!
//! A job that does many things of one kind (asking the model about 739 papers) says how far it has got
//! ([`Progress`]: n of m done, so many dollars spent), and can be asked to stop ([`Job::cancel`]). Stopping is
//! the work's to do: it is told through [`Tracker::cancelled`], starts nothing new, and returns what it has. The
//! job stays `running` until then, so whoever follows it waits for what was done, not for a guess.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use serde::Serialize;

use crate::library::{Feature, Subject};
use crate::threads::{Progress as Counter, panic_message};
use crate::usage::Usd;

/// What a finished job of the engine's made, for whoever follows it (`JobOutcome` in the contracts). Only a run is
/// opened by the interface's job follower; a feature's component recognises its own outcome and shows its answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Finished {
    /// A run was made (or made again): its name.
    Run { run: String },
    /// A feature finished asking about this subject: its answers are kept and can be shown.
    Feature { feature: Feature, subject: Subject },
    /// Nothing to open (citing sentences read for the whole library, say).
    Nothing,
}

/// What a feature's job is about, set as it starts ([`Job::start_about`]) and kept with it until the next one: so
/// that whoever looks, from any window and at any moment of the job, can tell which section of the interface a
/// running job belongs to, without waiting for the job to say so in its log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct About {
    pub feature: Feature,
    pub subject: Subject,
    /// The one item of the subject the job asks about, when it is one (a question's id among a run's questions).
    pub item: Option<String>,
}

/// How far a job that does many things has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub done: u64,
    pub of: u64,
    /// What it has spent so far on the language model's API, in US dollars as the interface is told them (of the
    /// calls whose price is known).
    pub usd: Usd,
}

/// A request to stop, shared by the work and whoever may ask for it. Once asked, it stays asked.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// What the work of a job is handed: where its log lines and its progress go, and whether it was asked to stop.
/// `Sync`, so that the threads a job runs on can all report through it.
pub struct Tracker<'a> {
    log: &'a (dyn Fn(&str) + Sync),
    progress: &'a (dyn Fn(Progress) + Sync),
    cancel: &'a Cancel,
}

impl<'a> Tracker<'a> {
    /// A tracker of one's own, for work that is not run as a job (a test, a command-line tool).
    pub fn new(log: &'a (dyn Fn(&str) + Sync), progress: &'a (dyn Fn(Progress) + Sync), cancel: &'a Cancel) -> Self {
        Self { log, progress, cancel }
    }

    pub fn log(&self, line: &str) {
        (self.log)(line);
    }

    pub fn progress(&self, progress: Progress) {
        (self.progress)(progress);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub fn cancel(&self) -> &Cancel {
        self.cancel
    }
}

/// [`Job::cancel`] refused: the job named is not the one running (it finished, or another started since).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("job {id} is not running")]
pub struct NotRunning {
    pub id: u64,
}

/// Where a job is, with what belongs to that state.
#[derive(Clone, Debug, PartialEq)]
pub enum State<T> {
    Idle,
    Running,
    Done(T),
    Failed(String),
}

/// A state's name alone, as the interface is told it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Idle,
    Running,
    Done,
    Failed,
}

impl<T> State<T> {
    pub fn phase(&self) -> Phase {
        match self {
            State::Idle => Phase::Idle,
            State::Running => Phase::Running,
            State::Done(_) => Phase::Done,
            State::Failed(_) => Phase::Failed,
        }
    }
}

/// A job is already running: the one refusal [`Job::start`] gives. It names what is running, so that the person
/// who asked is told the truth ("the declarations are being read"), not a guess.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{kind} is already in progress")]
pub struct Busy {
    pub kind: String,
}

/// The most lines a job's log keeps: a job over thousands of papers may log a line for each, and the log is held
/// in memory and sent to whoever follows the job. The earliest go first; the line numbers `since` counts in stay
/// those of every line logged.
const LOG_LINES: usize = 5_000;

/// How a job says how far it has got: one of the two, never both, since a job is started one way or the other.
enum Measure {
    /// Through its tracker ([`Job::start_tracked`], [`Job::start`]): the last progress it reported, if any.
    Tracked(Option<Progress>),
    /// By the papers its threads count ([`Job::start_counted`]), without this lock.
    Counted(Arc<Counter>),
}

struct Inner<T> {
    /// Counts the jobs started; 0 until the first.
    id: u64,
    kind: String,
    /// What a feature's job is about ([`Job::start_about`]); none for other jobs.
    about: Option<About>,
    state: State<T>,
    /// The log's last [`LOG_LINES`] lines.
    log: VecDeque<String>,
    /// Lines logged before those kept.
    dropped: usize,
    measure: Measure,
    /// The running job's request to stop; a new one for every job.
    cancel: Cancel,
}

impl<T> Inner<T> {
    fn log(&mut self, line: String) {
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
            self.dropped += 1;
        }
        self.log.push_back(line);
    }
}

/// What a job looks like at one moment.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Snapshot<T> {
    pub id: u64,
    pub kind: String,
    /// What a feature's job is (or was) about, when it was started with it.
    pub about: Option<About>,
    pub state: Phase,
    pub result: Option<T>,
    pub error: Option<String>,
    /// The log's lines from `since` on.
    pub log: Vec<String>,
    /// How many lines were logged in all: the earliest of a very long log are no longer kept ([`LOG_LINES`]).
    pub total: usize,
    pub progress: Option<Progress>,
    /// How many of its papers the job has done, of how many, when it counts them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub papers: Option<Counted>,
}

/// Papers done, of how many.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Counted {
    pub done: usize,
    pub of: usize,
}

pub struct Job<T> {
    inner: Arc<Mutex<Inner<T>>>,
}

impl<T> Clone for Job<T> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

impl<T> Default for Job<T> {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                id: 0,
                kind: String::new(),
                about: None,
                state: State::Idle,
                log: VecDeque::new(),
                dropped: 0,
                measure: Measure::Tracked(None),
                cancel: Cancel::new(),
            })),
        }
    }
}

/// The job's data, whatever happened to a thread that held it: every change to it is a single assignment, so
/// there is no half-changed state to protect anyone from.
fn open<T>(inner: &Mutex<Inner<T>>) -> MutexGuard<'_, Inner<T>> {
    inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl<T: Clone + Send + 'static> Job<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `work` on a thread of its own; what it returns becomes the job's result, an `Err` (or a panic) its
    /// failure. `work` is given the function that adds a line to the job's log. Answers the job's number, or
    /// [`Busy`] while another is running.
    pub fn start<F>(&self, kind: &str, work: F) -> Result<u64, Busy>
    where
        F: FnOnce(&dyn Fn(&str)) -> Result<T, String> + Send + 'static,
    {
        self.start_tracked(kind, move |tracker| work(&|line: &str| tracker.log(line)))
    }

    /// [`Job::start`] for work over many papers: `work` is also given the job's [`Counter`], to count its papers
    /// on ([`crate::threads::each`] does) and to look at between papers for a cancel ([`Job::cancel`]). The
    /// snapshot then says how many papers are done, read from the counters without the job's lock being taken
    /// for each paper.
    pub fn start_counted<F>(&self, kind: &str, work: F) -> Result<u64, Busy>
    where
        F: FnOnce(&dyn Fn(&str), &Counter) -> Result<T, String> + Send + 'static,
    {
        let counted = Arc::new(Counter::new());
        let lent = Arc::clone(&counted);
        self.launch(kind, None, Measure::Counted(counted), move |tracker| work(&|line: &str| tracker.log(line), &lent))
    }

    /// [`Job::start`] for work that reports its progress and can be asked to stop: `work` is given a [`Tracker`].
    pub fn start_tracked<F>(&self, kind: &str, work: F) -> Result<u64, Busy>
    where
        F: FnOnce(&Tracker<'_>) -> Result<T, String> + Send + 'static,
    {
        self.launch(kind, None, Measure::Tracked(None), work)
    }

    /// [`Job::start_tracked`] for a feature's work, with what it is about set as it starts ([`About`]): a snapshot
    /// taken at any moment of the job says so.
    pub fn start_about<F>(&self, kind: &str, about: About, work: F) -> Result<u64, Busy>
    where
        F: FnOnce(&Tracker<'_>) -> Result<T, String> + Send + 'static,
    {
        self.launch(kind, Some(about), Measure::Tracked(None), work)
    }

    /// Begin `work` as the one running job, saying how far it has got by `measure`. A thread that cannot be
    /// started for it (the system has none left) makes the job failed at once, never running for ever.
    fn launch<F>(&self, kind: &str, about: Option<About>, measure: Measure, work: F) -> Result<u64, Busy>
    where
        F: FnOnce(&Tracker<'_>) -> Result<T, String> + Send + 'static,
    {
        let (id, cancel) = {
            let mut job = open(&self.inner);
            if job.state.phase() == Phase::Running {
                return Err(Busy { kind: job.kind.clone() });
            }
            job.id += 1;
            job.kind = kind.to_owned();
            job.about = about;
            job.state = State::Running;
            job.log.clear();
            job.dropped = 0;
            job.measure = measure;
            job.cancel = Cancel::new();
            (job.id, job.cancel.clone())
        };
        let inner = Arc::clone(&self.inner);
        let spawned = thread::Builder::new().name("job".to_owned()).spawn(move || {
            let log = |line: &str| open(&inner).log(line.to_owned());
            let progress = |progress: Progress| {
                if let Measure::Tracked(last) = &mut open(&inner).measure {
                    *last = Some(progress);
                }
            };
            let tracker = Tracker::new(&log, &progress, &cancel);
            let outcome = catch_unwind(AssertUnwindSafe(|| work(&tracker)));
            let mut job = open(&inner);
            job.state = match outcome {
                Ok(Ok(result)) => State::Done(result),
                Ok(Err(error)) => {
                    job.log(format!("ERROR: {error}"));
                    State::Failed(error)
                }
                Err(panic) => {
                    let said = panic_message(&*panic);
                    job.log(format!("ERROR: {said}"));
                    State::Failed(said)
                }
            };
        });
        if spawned.is_err() {
            let mut job = open(&self.inner);
            let said = "no thread could be started for the job".to_owned();
            job.log(format!("ERROR: {said}"));
            job.state = State::Failed(said);
        }
        Ok(id)
    }

    /// The job as it is now, with the log's lines from `since` on.
    pub fn snapshot(&self, since: usize) -> Snapshot<T> {
        let job = open(&self.inner);
        let (result, error) = match &job.state {
            State::Done(result) => (Some(result.clone()), None),
            State::Failed(error) => (None, Some(error.clone())),
            State::Idle | State::Running => (None, None),
        };
        let (progress, papers) = match &job.measure {
            Measure::Tracked(progress) => (*progress, None),
            Measure::Counted(counted) => {
                (None, (counted.total() > 0).then(|| Counted { done: counted.done(), of: counted.total() }))
            }
        };
        Snapshot {
            id: job.id,
            kind: job.kind.clone(),
            about: job.about.clone(),
            state: job.state.phase(),
            result,
            error,
            log: job.log.iter().skip(since.saturating_sub(job.dropped)).cloned().collect(),
            total: job.dropped + job.log.len(),
            progress,
            papers,
        }
    }

    /// Ask job `id` to stop, if it is the one running. The work stops starting new things and returns what it
    /// has; until then the job is still running. Work that never looks ([`Job::start`]) runs to its end.
    pub fn cancel(&self, id: u64) -> Result<(), NotRunning> {
        let mut job = open(&self.inner);
        if job.id != id || job.state.phase() != Phase::Running {
            return Err(NotRunning { id });
        }
        if !job.cancel.is_cancelled() {
            job.cancel.cancel();
            if let Measure::Counted(counted) = &job.measure {
                counted.cancel();
            }
            job.log("Asked to stop.".to_owned());
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// Wait until the job has left `running`.
    fn finished<T: Clone + Send + 'static>(job: &Job<T>) -> Snapshot<T> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let now = job.snapshot(0);
            if now.state != Phase::Running {
                return now;
            }
            assert!(Instant::now() < deadline, "the job never finished");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_feature_s_job_says_what_it_is_about_from_the_moment_it_starts_until_the_next_one() {
        let job: Job<String> = Job::new();
        let (go, wait) = mpsc::channel::<()>();
        let about = About {
            feature: Feature::Answers,
            subject: Subject::Run("r1".to_owned()),
            item: Some("question".to_owned()),
        };
        job.start_about("answering a question", about.clone(), move |_| {
            wait.recv().ok();
            Ok(String::new())
        })
        .unwrap();
        // before the work has run a single line
        let now = job.snapshot(0);
        assert_eq!((now.state, now.about.as_ref(), now.log.len()), (Phase::Running, Some(&about), 0));
        go.send(()).unwrap();
        assert_eq!(finished(&job).about, Some(about), "still said once it is done");
        job.start("a run", |_| Ok(String::new())).unwrap();
        assert_eq!(job.snapshot(0).about, None, "another job is about nothing it did not say");
    }

    #[test]
    fn a_new_job_is_idle_and_has_nothing() {
        let job: Job<String> = Job::new();
        let now = job.snapshot(0);
        assert_eq!((now.id, now.state, now.result, now.error, now.total), (0, Phase::Idle, None, None, 0));
    }

    #[test]
    fn work_that_returns_makes_the_job_done_with_its_result_and_log() {
        let job = Job::new();
        let id = job
            .start("a run", |log| {
                log("[1/2] download");
                log("[2/2] embed");
                Ok("neurips2025".to_owned())
            })
            .unwrap();
        let now = finished(&job);
        assert_eq!((now.id, now.kind.as_str(), now.state), (id, "a run", Phase::Done));
        assert_eq!(now.result.as_deref(), Some("neurips2025"));
        assert_eq!(now.error, None);
        assert_eq!(now.log, ["[1/2] download", "[2/2] embed"]);
        assert_eq!(job.snapshot(1).log, ["[2/2] embed"]); // from a line on
        assert_eq!(job.snapshot(99).log, Vec::<String>::new());
        assert_eq!(job.snapshot(99).total, 2);
    }

    #[test]
    fn work_that_fails_or_panics_makes_the_job_failed_with_the_reason() {
        let job: Job<String> = Job::new();
        job.start("a run", |_| Err("only 3 papers".to_owned())).unwrap();
        let now = finished(&job);
        assert_eq!((now.state, now.result, now.error.as_deref()), (Phase::Failed, None, Some("only 3 papers")));
        assert_eq!(now.log, ["ERROR: only 3 papers"]);

        job.start("a run", |_| panic!("index out of range")).unwrap();
        let now = finished(&job);
        assert_eq!((now.state, now.error.as_deref()), (Phase::Failed, Some("index out of range")));
    }

    #[test]
    fn a_second_job_is_refused_while_one_runs_and_told_what_is_running() {
        let job: Job<u32> = Job::new();
        let (release, wait) = mpsc::channel::<()>();
        job.start("reading the declarations", move |_| {
            wait.recv().map_err(|error| error.to_string())?;
            Ok(1)
        })
        .unwrap();
        let refused = job.start("a run", |_| Ok(2)).unwrap_err();
        assert_eq!(refused.to_string(), "reading the declarations is already in progress");
        assert_eq!(job.snapshot(0).kind, "reading the declarations"); // the refused one changed nothing
        release.send(()).unwrap();
        assert_eq!(finished(&job).result, Some(1));
    }

    #[test]
    fn a_finished_or_failed_job_can_be_followed_by_another_with_a_new_number_and_a_fresh_log() {
        let job: Job<u32> = Job::new();
        let first = job.start("a run", |log| {
            log("old line");
            Ok(1)
        });
        finished(&job);
        let second = job.start("a run", |_| Err("no".to_owned()));
        let failed = finished(&job);
        assert_eq!(failed.log, ["ERROR: no"]); // the first job's lines are gone
        let third = job.start("reading the declarations", |_| Ok(3));
        let now = finished(&job);
        assert_eq!((first, second, third), (Ok(1), Ok(2), Ok(3)));
        assert_eq!((now.id, now.kind.as_str(), now.result, now.error), (3, "reading the declarations", Some(3), None));
    }

    #[test]
    fn whoever_looks_never_sees_done_without_a_result() {
        let job: Job<u32> = Job::new();
        for round in 0..200 {
            job.start("a run", move |_| Ok(round)).unwrap();
            loop {
                let now = job.snapshot(0);
                match now.state {
                    Phase::Running => assert_eq!(now.result, None),
                    Phase::Done => {
                        assert_eq!(now.result, Some(round));
                        break;
                    }
                    other => panic!("unexpected state {other:?}"),
                }
            }
        }
    }

    #[test]
    fn a_tracked_job_reports_its_progress_and_stops_when_asked_with_what_it_has() {
        let job: Job<u64> = Job::new();
        let (reported, wait) = mpsc::channel::<()>();
        let id = job
            .start_tracked("asking about papers", move |tracker| {
                let mut done = 0;
                for paper in 0..1_000 {
                    if tracker.cancelled() {
                        break;
                    }
                    tracker.progress(Progress { done: paper, of: 1_000, usd: Usd::from_picodollars(paper) });
                    done = paper;
                    if paper == 2 {
                        reported.send(()).map_err(|error| error.to_string())?;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Ok(done)
            })
            .unwrap();
        wait.recv().unwrap();
        assert!(job.snapshot(0).progress.is_some_and(|progress| progress.of == 1_000 && progress.done >= 2));
        assert_eq!(job.cancel(id + 1), Err(NotRunning { id: id + 1 }), "only the job named is asked");
        job.cancel(id).unwrap();
        job.cancel(id).unwrap(); // asking twice is asking once
        let now = finished(&job);
        assert_eq!(now.state, Phase::Done);
        assert!(now.result.unwrap() < 999, "it stopped early");
        assert_eq!(now.log.iter().filter(|line| line.starts_with("Asked to stop")).count(), 1);
        assert_eq!(job.cancel(id), Err(NotRunning { id }), "a finished job is not running");
        let told = serde_json::to_value(now.progress).unwrap();
        assert_eq!(told["of"], 1_000);

        // the next job starts with no progress and is not asked to stop
        job.start_tracked("again", |tracker| Ok(u64::from(tracker.cancelled()))).unwrap();
        let next = finished(&job);
        assert_eq!((next.result, next.progress), (Some(0), None));
    }

    #[test]
    fn a_job_over_many_papers_counts_them_and_stops_between_papers_when_cancelled() {
        let job: Job<usize> = Job::new();
        assert!(job.cancel(1).is_err(), "nothing is running");
        let (started, wait) = mpsc::channel::<()>();
        let (release, go) = mpsc::channel::<()>();
        let go = Mutex::new(go); // a receiver is not shared between threads as it is
        let papers: Vec<u32> = (0..10_000).collect();
        let id = job
            .start_counted("reading the declarations", move |_, progress| {
                // one thread, so that no paper but the first is begun before the cancel
                let outcomes = crate::threads::pool(1).install(|| {
                    crate::threads::each(&papers, progress, |&paper| {
                        if paper == 0 {
                            started.send(()).map_err(|error| error.to_string())?;
                            go.lock().map_err(|error| error.to_string())?.recv().map_err(|error| error.to_string())?;
                        }
                        Ok::<u32, String>(paper)
                    })
                });
                Ok(outcomes.iter().filter(|outcome| matches!(outcome, crate::threads::Outcome::Done(_))).count())
            })
            .unwrap();
        wait.recv().unwrap();
        job.cancel(id).unwrap();
        release.send(()).unwrap();
        let now = finished(&job);
        let papers = now.papers.unwrap();
        assert_eq!(papers.of, 10_000);
        assert_eq!(Some(papers.done), now.result);
        assert_eq!(papers.done, 1, "the papers not begun before the cancel were not done");
        let told = serde_json::to_value(job.snapshot(0)).unwrap();
        assert_eq!(told["papers"]["of"], 10_000);
        job.start("a run", |_| Ok(1)).unwrap();
        assert_eq!(finished(&job).papers, None, "a job that counts nothing says nothing about papers");
    }

    #[test]
    fn a_long_log_keeps_its_last_lines_and_counts_every_line_logged() {
        let job: Job<u32> = Job::new();
        job.start("a run", |log| {
            for line in 0..LOG_LINES + 10 {
                log(&format!("line {line}"));
            }
            Err("stopped".to_owned())
        })
        .unwrap();
        let now = finished(&job);
        assert_eq!(now.total, LOG_LINES + 11);
        assert_eq!(now.log.len(), LOG_LINES, "the first lines went");
        assert_eq!(now.log[0], "line 11");
        assert_eq!(now.log.last().map(String::as_str), Some("ERROR: stopped"), "the failure is always kept");
        // `since` counts every line logged, kept or not
        assert_eq!(job.snapshot(LOG_LINES + 9).log, ["line 5009", "ERROR: stopped"]);
        assert_eq!(job.snapshot(LOG_LINES + 11).log, Vec::<String>::new());
    }

    #[test]
    fn a_job_says_how_far_it_has_got_in_one_way_only() {
        let job: Job<u32> = Job::new();
        let (reported, wait) = mpsc::channel::<()>();
        let (release, go) = mpsc::channel::<()>();
        job.start_tracked("asking", move |tracker| {
            tracker.progress(Progress { done: 1, of: 3, usd: Usd::ZERO });
            reported.send(()).map_err(|error| error.to_string())?;
            go.recv().map_err(|error| error.to_string())?;
            Ok(1)
        })
        .unwrap();
        wait.recv().unwrap();
        let tracked = job.snapshot(0);
        assert_eq!((tracked.progress.map(|progress| progress.done), tracked.papers), (Some(1), None));
        release.send(()).unwrap();
        finished(&job);
        job.start_counted("reading", |_, counter| {
            counter.expect(4);
            counter.advance();
            Ok(1)
        })
        .unwrap();
        let counted = finished(&job);
        assert_eq!((counted.progress, counted.papers), (None, Some(Counted { done: 1, of: 4 })));
    }

    #[test]
    fn a_finished_job_says_what_it_made_in_the_interfaces_words() {
        let made = |finished: Finished| serde_json::to_value(finished).unwrap();
        assert_eq!(
            made(Finished::Run { run: "neurips2025".to_owned() }),
            serde_json::json!({"kind": "run", "run": "neurips2025"})
        );
        assert_eq!(
            made(Finished::Feature { feature: Feature::Critique, subject: Subject::Paper("p1".to_owned()) }),
            serde_json::json!({"kind": "feature", "feature": "critique", "subject": {"paper": "p1"}})
        );
        let community = Subject::Community { run: "r1".to_owned(), community: "c0.1".to_owned() };
        assert_eq!(
            made(Finished::Feature { feature: Feature::CitationKinds, subject: community }),
            serde_json::json!({"kind": "feature", "feature": "citation_kinds", "subject": {"community": {"run": "r1", "community": "c0.1"}}})
        );
        assert_eq!(made(Finished::Nothing), serde_json::json!({"kind": "nothing"}));
    }

    #[test]
    fn a_snapshot_is_told_to_the_interface_in_its_words() {
        let job: Job<String> = Job::new();
        job.start("a run", |_| Ok("x".to_owned())).unwrap();
        let told = serde_json::to_value(finished(&job)).unwrap();
        assert_eq!(told["state"], "done");
        assert_eq!(told["result"], "x");
        assert_eq!(told["error"], serde_json::Value::Null);
    }
}
