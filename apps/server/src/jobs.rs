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

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use serde::Serialize;

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

struct Inner<T> {
    /// Counts the jobs started; 0 until the first.
    id: u64,
    kind: String,
    state: State<T>,
    log: Vec<String>,
}

/// What a job looks like at one moment.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Snapshot<T> {
    pub id: u64,
    pub kind: String,
    pub state: Phase,
    pub result: Option<T>,
    pub error: Option<String>,
    /// The log's lines from `since` on.
    pub log: Vec<String>,
    /// How many lines the log has in all.
    pub total: usize,
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
        Self { inner: Arc::new(Mutex::new(Inner { id: 0, kind: String::new(), state: State::Idle, log: Vec::new() })) }
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
        let id = {
            let mut job = open(&self.inner);
            if job.state.phase() == Phase::Running {
                return Err(Busy { kind: job.kind.clone() });
            }
            job.id += 1;
            job.kind = kind.to_owned();
            job.state = State::Running;
            job.log.clear();
            job.id
        };
        let inner = Arc::clone(&self.inner);
        thread::spawn(move || {
            let log = |line: &str| open(&inner).log.push(line.to_owned());
            let outcome = catch_unwind(AssertUnwindSafe(|| work(&log)));
            let mut job = open(&inner);
            job.state = match outcome {
                Ok(Ok(result)) => State::Done(result),
                Ok(Err(error)) => {
                    job.log.push(format!("ERROR: {error}"));
                    State::Failed(error)
                }
                Err(panic) => {
                    let said = panic
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_owned())
                        .or_else(|| panic.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "the work stopped unexpectedly".to_owned());
                    job.log.push(format!("ERROR: {said}"));
                    State::Failed(said)
                }
            };
        });
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
        Snapshot {
            id: job.id,
            kind: job.kind.clone(),
            state: job.state.phase(),
            result,
            error,
            log: job.log.iter().skip(since).cloned().collect(),
            total: job.log.len(),
        }
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
    fn a_snapshot_is_told_to_the_interface_in_its_words() {
        let job: Job<String> = Job::new();
        job.start("a run", |_| Ok("x".to_owned())).unwrap();
        let told = serde_json::to_value(finished(&job)).unwrap();
        assert_eq!(told["state"], "done");
        assert_eq!(told["result"], "x");
        assert_eq!(told["error"], serde_json::Value::Null);
    }
}
