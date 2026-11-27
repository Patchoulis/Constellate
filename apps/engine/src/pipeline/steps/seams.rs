//! The two steps of a run that other modules do: scoring every abstract with the AI-text detector
//! ([`crate::features::detector`]), and writing a name and a description for every community with the language model chosen
//! in the settings ([`crate::features::writing::describe`]).
//!
//! * [`detect`] is called after the papers are embedded and before they are filtered (a setting can leave the
//!   flagged papers out). It sets `run.flagged[i]` for paper `run.papers[i]` from [`crate::features::detector::score_run`],
//!   which keeps every score in the library by classifier and text, so that no abstract is scored twice: a run made
//!   again scores nothing it scored before.
//! * [`describe`] is called once every community's record is made (`run.records`, the whole tree, with its terms
//!   and nearest papers) and before the files are written, so that `communities.json` and `report.md` carry the
//!   names. It asks the model only within what the person accepted for this run ([`Naming::Within`]: the most
//!   `GET /api/describe/estimate` showed, sent back with the run request); with nothing accepted
//!   ([`Naming::KeptOnly`]) nothing is sent, and only the communities named before for the same model and papers
//!   are named. `run.redo` with [`crate::pipeline::RedoStep::Descriptions`] asks again what was kept.
//!
//! Either may fail ([`StepOutcome::Failed`]): the run goes on without what it would have added.

use crate::features::detector::{self, Scoring, Skipped};
use crate::features::writing::describe::{self, Group, Outcome, Run};
use crate::jobs::Tracker;
use crate::library::Library;
use crate::model::chosen::Chosen;
use crate::model::language::{Budget, Transport};
use crate::pipeline::Made;
use crate::pipeline::steps::{Skip, StepOutcome};

/// What the person accepted for naming the run's communities, decided before the run starts.
pub enum Naming {
    /// Nothing was accepted (the request had no `describe`): nothing is sent; what was kept for these papers and
    /// the chosen model is used.
    KeptOnly(Chosen),
    /// Name them with `chosen`, within `budget` (what was accepted of the estimate shown).
    Within { chosen: Chosen, budget: Budget },
    /// Which model would name them cannot be known (the settings name a provider with no model): nothing named.
    Unknown(String),
}

/// A record's tree as the describer takes it.
fn groups(records: &[crate::pipeline::results::report::Record]) -> Vec<Group> {
    records
        .iter()
        .map(|record| Group {
            tree_path: record.tree_path.clone(),
            members: record.members.clone(),
            keywords: record.keywords.clone(),
            children: groups(&record.children),
        })
        .collect()
}

/// Name and describe the communities of `run.records`, as `naming` allows.
pub fn describe(
    library: &Library,
    transport: &dyn Transport,
    naming: &Naming,
    run: &mut Made,
    tracker: &Tracker<'_>,
) -> StepOutcome {
    let redo = run.redo.contains(&crate::pipeline::RedoStep::Descriptions);
    let tree = groups(&run.records);
    let described = Run {
        name: &run.name,
        // the pipeline writes the names with the rest of the run's files: an older run's files in the same folder
        // are not touched before the new ones replace them
        folder: None,
        papers: &run.papers,
        embeddings: Some(run.embeddings.view()),
        communities: &tree,
        redo,
    };
    let done = match naming {
        Naming::Unknown(why) => return StepOutcome::Failed(why.clone()),
        Naming::KeptOnly(_) if redo => return StepOutcome::Skipped(Skip::NotAccepted),
        Naming::KeptOnly(chosen) => describe::kept_only(library, &described, chosen, tracker),
        Naming::Within { chosen, budget } => {
            tracker.log(&format!("  naming with {chosen}, within what was accepted"));
            describe::describe_run(library, transport, &described, chosen, budget, tracker)
        }
    };
    let asked = described.communities_count();
    for record in &mut run.records {
        record.each_mut(&mut |record| {
            if let Some(named) = done.names.get(&record.tree_path) {
                record.name = Some(named.name.clone());
                record.description = Some(named.description.clone());
            }
        });
    }
    let named = done.names.len();
    match done.outcome {
        Outcome::Described { written, kept } => {
            StepOutcome::Done(format!("{written} named now, {kept} kept from before, of {asked}"))
        }
        Outcome::Kept { kept } => StepOutcome::Kept(format!("{kept} of {asked} names")),
        Outcome::NotAsked { kept: 0 } => StepOutcome::Skipped(Skip::NotAccepted),
        Outcome::NotAsked { kept } => StepOutcome::Kept(format!(
            "{kept} of {asked} names; no cost was accepted for this run, so the others are left unnamed"
        )),
        Outcome::NoKey { .. } if named == 0 => StepOutcome::Skipped(Skip::NoKey { provider: provider_of(naming) }),
        Outcome::NoKey { kept, .. } => {
            StepOutcome::Kept(format!("{kept} of {asked} names; no key is set, so the others are left unnamed"))
        }
        Outcome::Stopped { why, written, kept, not_asked } => StepOutcome::Done(format!(
            "stopped ({why}): {written} named now, {kept} kept from before, {not_asked} not asked, of {asked}"
        )),
        Outcome::Failed { reason, .. } if named == 0 => StepOutcome::Failed(reason),
        Outcome::Failed { reason, written, kept, failed } => StepOutcome::Done(format!(
            "{written} named now, {kept} kept from before; {failed} could not be named ({reason})"
        )),
    }
}

fn provider_of(naming: &Naming) -> String {
    match naming {
        Naming::KeptOnly(chosen) | Naming::Within { chosen, .. } => chosen.provider().about().label.to_owned(),
        Naming::Unknown(_) => String::new(),
    }
}

/// Score the abstracts of `run.papers`, setting `run.flagged`.
pub fn detect(library: &Library, run: &mut Made, tracker: &Tracker<'_>) -> StepOutcome {
    let ids: Vec<String> = run.papers.iter().map(|paper| paper.id.clone()).collect();
    let scores = detector::score_run(library, &run.settings, &ids, tracker);
    run.flagged = scores.flags();
    let of = ids.len();
    match scores.outcome {
        Scoring::Scored { scored, kept, flagged } => {
            StepOutcome::Done(format!("{scored} scored now, {kept} kept from before; {flagged} of {of} flagged"))
        }
        Scoring::Kept { flagged } => StepOutcome::Kept(format!("every score; {flagged} of {of} flagged")),
        Scoring::Skipped { why: Skipped::Off } => StepOutcome::Skipped(Skip::SwitchedOff),
        Scoring::Skipped { why: Skipped::Untrained } => StepOutcome::Skipped(Skip::DetectorNotTrained),
        Scoring::Failed { why } => StepOutcome::Failed(why),
    }
}
