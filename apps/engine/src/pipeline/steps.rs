//! The steps a run is made in, in order, what came of each, and how far the one under way has got.
//!
//! Every step is one [`Step`], and what came of it one [`StepOutcome`]: done, kept from before, skipped (with
//! why), or failed (with why). A step that is not [`Step::optional`] and fails ends the run; an optional one
//! (describing the communities, scoring the abstracts) that fails is told in the log and the run goes on without
//! it, as a run without descriptions or AI-text flags.
//!
//! The step under way, and how many of its things it has done, is kept on a [`Board`] that the status route reads
//! without taking the job's lock: "embedding 3,120 of 12,487 papers", "downloading the embedding model, 412 of
//! 1,200 MB".

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;

/// The steps of a run, in the order they are done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// The selection's papers: listings, acceptance decisions, abstracts; kept in the library.
    Gather,
    /// The embedding model's files, downloaded once into the data folder.
    Model,
    /// One vector per paper, each kept in the library by paper, text and model.
    Embed,
    /// The AI-text estimate of every abstract (another module's; see [`super::seams`]).
    Detect,
    /// The papers the settings leave out, left out.
    Filter,
    /// The similarity graph.
    Similarity,
    /// Communities and sub-communities.
    Communities,
    /// What each community is made of: its terms, its nearest papers, its figures.
    Summarise,
    /// A name and a description for every community, by the language model (another module's; see
    /// [`super::seams`]).
    Describe,
    /// The run's files.
    Write,
}

impl Step {
    pub const ALL: [Step; 10] = [
        Step::Gather,
        Step::Model,
        Step::Embed,
        Step::Detect,
        Step::Filter,
        Step::Similarity,
        Step::Communities,
        Step::Summarise,
        Step::Describe,
        Step::Write,
    ];

    /// What the log calls it.
    pub fn title(self) -> &'static str {
        match self {
            Step::Gather => "gather the papers",
            Step::Model => "the embedding model",
            Step::Embed => "embed",
            Step::Detect => "detect model-written abstracts",
            Step::Filter => "filter",
            Step::Similarity => "similarity -> adjacency",
            Step::Communities => "communities",
            Step::Summarise => "summarise communities",
            Step::Describe => "describe communities",
            Step::Write => "write results",
        }
    }

    /// Its place, from 1.
    pub fn number(self) -> usize {
        Step::ALL.iter().position(|step| *step == self).map_or(0, |index| index + 1)
    }

    /// Can the run go on when this step fails? Only for what another module adds on top of the run.
    pub fn optional(self) -> bool {
        matches!(self, Step::Detect | Step::Describe)
    }
}

/// Why a step did nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    /// No cost was accepted for it with the run's request (naming the communities): nothing was sent.
    NotAccepted,
    /// The settings switch it off.
    SwitchedOff,
    /// The language model's provider chosen has no key set.
    NoKey { provider: String },
    /// The AI-text detector has not been trained on this computer.
    DetectorNotTrained,
    /// There is nothing for it to do (every paper is embedded already).
    NothingToDo(String),
}

impl fmt::Display for Skip {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Skip::NotAccepted => formatter.write_str(
                "no cost was accepted for naming this run, so nothing was sent; the run is made without names, which \
                 can be added from it later",
            ),
            Skip::SwitchedOff => formatter.write_str("switched off in the settings"),
            Skip::NoKey { provider } => write!(formatter, "no key is set for the chosen provider ({provider})"),
            Skip::DetectorNotTrained => formatter.write_str("the AI-text detector is not trained"),
            Skip::NothingToDo(why) => formatter.write_str(why),
        }
    }
}

/// What came of one step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepOutcome {
    /// Done now; what it came to, in a few words.
    Done(String),
    /// Nothing needed doing: everything it makes was kept from before.
    Kept(String),
    Skipped(Skip),
    Failed(String),
}

impl fmt::Display for StepOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StepOutcome::Done(what) => write!(formatter, "done: {what}"),
            StepOutcome::Kept(what) => write!(formatter, "kept from before: {what}"),
            StepOutcome::Skipped(why) => write!(formatter, "skipped: {why}"),
            StepOutcome::Failed(why) => write!(formatter, "failed: {why}"),
        }
    }
}

/// What a count of a step counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Papers,
    /// Days of arXiv, or conferences.
    Slices,
    Bytes,
    Communities,
}

impl Unit {
    fn word(self) -> &'static str {
        match self {
            Unit::Papers => "papers",
            Unit::Slices => "slices",
            Unit::Bytes => "bytes",
            Unit::Communities => "communities",
        }
    }
}

/// So many of so many things done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Count {
    pub done: u64,
    pub of: u64,
    pub unit: Unit,
}

impl fmt::Display for Count {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.unit == Unit::Bytes {
            let megabytes = |bytes: u64| bytes as f64 / 1_048_576.0;
            return write!(formatter, "{:.0} of {:.0} MB", megabytes(self.done), megabytes(self.of));
        }
        write!(formatter, "{} of {} {}", thousands(self.done), thousands(self.of), self.unit.word())
    }
}

/// A whole number with a comma between thousands: 12,487.
pub fn thousands(number: u64) -> String {
    let digits = number.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The step under way, as the status route tells it (`RunStep` in the contracts).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StepReport {
    pub step: Step,
    pub title: &'static str,
    /// Its place, from 1, of how many steps there are.
    pub number: usize,
    pub of: usize,
    /// How far it has got, for a step that counts.
    pub count: Option<Count>,
    /// What it is doing in words ("embedding on the processor").
    pub detail: Option<String>,
}

/// Where the run under way is, shared between the run (which writes it) and the status route (which reads it).
/// Empty when no run is being made.
#[derive(Clone, Debug, Default)]
pub struct Board(Arc<Mutex<Option<StepReport>>>);

impl Board {
    fn change(&self, change: impl FnOnce(&mut Option<StepReport>)) {
        // every change is one assignment: a panic elsewhere cannot leave it half-changed
        change(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner));
    }

    /// The step under way, if a run is being made.
    pub fn now(&self) -> Option<StepReport> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn begin(&self, step: Step) {
        self.change(|now| {
            *now = Some(StepReport {
                step,
                title: step.title(),
                number: step.number(),
                of: Step::ALL.len(),
                count: None,
                detail: None,
            });
        });
    }

    pub fn count(&self, count: Count) {
        self.change(|now| {
            if let Some(report) = now {
                report.count = Some(count);
            }
        });
    }

    pub fn detail(&self, detail: String) {
        self.change(|now| {
            if let Some(report) = now {
                report.detail = Some(detail);
            }
        });
    }

    pub fn clear(&self) {
        self.change(|now| *now = None);
    }

    /// Is a run being made (of any name)?
    pub fn busy(&self) -> bool {
        self.now().is_some()
    }
}

/// Clears the board when the run ends, whichever way it ends (a panic included).
pub struct Clearing<'a>(pub &'a Board);

impl Drop for Clearing<'_> {
    fn drop(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_steps_are_numbered_in_their_order_and_only_the_other_modules_steps_are_optional() {
        assert_eq!(Step::Gather.number(), 1);
        assert_eq!(Step::Write.number(), Step::ALL.len());
        let optional: Vec<Step> = Step::ALL.into_iter().filter(|step| step.optional()).collect();
        assert_eq!(optional, [Step::Detect, Step::Describe]);
    }

    #[test]
    fn a_count_is_said_with_its_unit_and_thousands_apart() {
        let count = Count { done: 3_120, of: 12_487, unit: Unit::Papers };
        assert_eq!(count.to_string(), "3,120 of 12,487 papers");
        let bytes = Count { done: 412 * 1_048_576, of: 1_200 * 1_048_576, unit: Unit::Bytes };
        assert_eq!(bytes.to_string(), "412 of 1200 MB");
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000_000), "1,000,000");
    }

    #[test]
    fn the_board_shows_the_step_under_way_and_is_empty_once_the_run_ends() {
        let board = Board::default();
        assert!(!board.busy());
        {
            let _clearing = Clearing(&board);
            board.begin(Step::Embed);
            board.count(Count { done: 1, of: 2, unit: Unit::Papers });
            let now = board.now().unwrap();
            assert_eq!((now.number, now.of, now.count.unwrap().done), (3, 10, 1));
        }
        assert_eq!(board.now(), None);
    }

    #[test]
    fn every_outcome_says_what_it_was() {
        assert_eq!(
            StepOutcome::Skipped(Skip::NoKey { provider: "OpenAI".into() }).to_string(),
            "skipped: no key is set for the chosen provider (OpenAI)"
        );
        assert!(StepOutcome::Skipped(Skip::NotAccepted).to_string().starts_with("skipped: no cost was accepted"));
        assert_eq!(
            StepOutcome::Skipped(Skip::DetectorNotTrained).to_string(),
            "skipped: the AI-text detector is not trained"
        );
        assert_eq!(StepOutcome::Failed("x".into()).to_string(), "failed: x");
    }
}
