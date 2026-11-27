//! The first step of a run: the library brought up to date for the selection, then the selection's papers.
//!
//! A selection is fetched in slices ([`crate::sources::download`]): one per conference, or one per day of arXiv.
//! A slice the library has as complete is not fetched again (unless the download is to be redone); one fetched in
//! part (a day that can still grow, a list without its abstracts, a conference found only in arXiv comments) is
//! fetched again, and a known paper keeps its record. A slice that cannot be fetched now is told in the log and the
//! others are fetched; the run then fails only if that slice was never fetched before, since it would be missing
//! from the run without the person knowing.

use std::time::Duration;

use chrono::NaiveDate;

use crate::jobs::Tracker;
use crate::library::{Known, Library, Paper, SliceOutcome};
use crate::pipeline::steps::{Board, Count, StepOutcome, Unit, thousands};
use crate::pipeline::{RunError, Step};
use crate::selection::Selection;
use crate::sources::download::{Gathered, Slice, gather as fetch_slice, slices};
use crate::sources::{self, Fetch};

/// A source's paper as the library keeps it.
pub fn kept(paper: sources::Paper) -> Paper {
    Paper {
        id: paper.id,
        title: paper.title,
        r#abstract: paper.abstract_text,
        authors: paper.authors,
        published: paper.published,
        updated: paper.updated,
        comment: paper.comment,
        primary_category: paper.primary_category,
        categories: paper.categories,
        venues: Vec::new(),
        url: Some(paper.url),
        pdf_url: paper.pdf_url,
    }
}

/// How a slice that came back whole is kept: a conference found only through arXiv comments is fetched again
/// next time, in case its list appears.
fn outcome_of(slice: &Slice, papers: &[Paper]) -> SliceOutcome {
    match slice {
        Slice::Conference { .. }
            if !papers.iter().any(|paper| paper.origin() == crate::library::Origin::Conference) =>
        {
            SliceOutcome::ArxivComments
        }
        Slice::Conference { .. } | Slice::Day { .. } => SliceOutcome::Complete,
    }
}

/// What the gather step needs from outside: where papers come from, how to wait between arXiv's requests, and
/// which day it is (recent days can still grow).
pub struct Sources<'a> {
    pub fetch: &'a (dyn Fetch + Sync),
    pub pause: &'a (dyn Fn(Duration) + Sync),
    pub today: NaiveDate,
}

/// Bring the library up to date for `selection` and answer its papers, ordered by id.
pub fn gather(
    library: &Library,
    sources: &Sources<'_>,
    selection: &Selection,
    redo: bool,
    tracker: &Tracker<'_>,
    board: &Board,
) -> Result<(Vec<Paper>, StepOutcome), RunError> {
    let every = slices(selection);
    let mut todo = Vec::new();
    for slice in &every {
        if redo || library.needs_fetching(&slice.key())? {
            todo.push(slice);
        }
    }
    let skipped = every.len() - todo.len();
    if skipped > 0 {
        tracker.log(&format!("  {skipped} of {} already downloaded: skipped", every.len()));
    }
    let known = if redo { Known::Replace } else { Known::Keep };
    let log = |line: &str| tracker.log(line);
    let mut missing: Vec<String> = Vec::new();
    let mut fetched = 0;
    for (index, slice) in todo.iter().enumerate() {
        if tracker.cancelled() {
            return Err(RunError::Cancelled(Step::Gather));
        }
        board.count(Count { done: index as u64, of: todo.len() as u64, unit: Unit::Slices });
        tracker.log(&format!("  {}", slice.label()));
        let (papers, outcome) = match fetch_slice(slice, sources.fetch, sources.pause, sources.today, &log) {
            Gathered::Finished(papers) => {
                let papers: Vec<Paper> = papers.into_iter().map(kept).collect();
                let outcome = outcome_of(slice, &papers);
                (papers, outcome)
            }
            Gathered::Unfinished { papers, why } => {
                tracker.log(&format!("    not complete yet ({why:?}): fetched again next time"));
                (papers.into_iter().map(kept).collect(), SliceOutcome::Incomplete)
            }
            Gathered::NotNow(why) => {
                tracker.log(&format!("    could not be fetched now: {why}"));
                if library.slice_outcome(&slice.key())?.is_none() {
                    missing.push(format!("{}: {why}", slice.label()));
                }
                continue;
            }
        };
        let added = library.add_slice(&slice.key(), outcome, &papers, slice.venue(), known)?;
        fetched += 1;
        tracker.log(&format!("    {} new, {} already in the library", added.new, added.known));
    }
    if !missing.is_empty() {
        return Err(RunError::Sources(format!(
            "{} could not be fetched; what was fetched is kept, run again to go on",
            missing.join("; ")
        )));
    }
    let papers = library.select(selection)?;
    tracker.log(&format!(
        "  {} papers selected ({} in the library)",
        thousands(papers.len() as u64),
        thousands(library.paper_count()? as u64)
    ));
    let outcome = if fetched == 0 {
        StepOutcome::Kept(format!("{} papers, all in the library", thousands(papers.len() as u64)))
    } else {
        StepOutcome::Done(format!("{fetched} of {} slices fetched", every.len()))
    };
    Ok((papers, outcome))
}
