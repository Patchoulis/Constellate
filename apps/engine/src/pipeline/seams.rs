//! The two steps of a run that other modules do: writing a name and a description for every community (with the
//! language model chosen in the settings), and scoring every abstract with the AI-text detector.
//!
//! They are declared here with the exact signature the run calls them with, and in this build they do nothing:
//! each answers "skipped: not available in this build", and the run is made without descriptions and with no
//! paper flagged, as the Python made a run without an API key or a trained detector. Whoever brings in those
//! modules replaces the bodies below with calls into them (and nothing else of the run changes):
//!
//! * [`detect`] is called after the papers are embedded and before they are filtered (a setting can leave the
//!   flagged papers out). It sets `run.flagged[i]` for paper `run.papers[i]`, keeping its scores in the library
//!   by paper, text hash and classifier version so that no abstract is scored twice.
//! * [`describe`] is called once every community's record is made (`run.records`, the whole tree, with its terms
//!   and nearest papers) and before the files are written. It sets `name` and `description` of the records it
//!   describes. `run.redo` says whether descriptions are to be written again (`RedoStep::Descriptions`). It must
//!   ask the model only for a cost the person has seen and accepted (the engine's `Chosen` and budget rule): when
//!   the settings' provider has no key, it answers [`Skip::NoKey`].
//!
//! Either may fail ([`StepOutcome::Failed`]): the run goes on without what it would have added.

use serde::Serialize;

use super::Made;
use super::steps::{Skip, StepOutcome};
use crate::jobs::Tracker;
use crate::library::Library;

/// Name and describe the communities of `run.records`.
///
/// In this build, only with what the library already has: a community whose model and papers were described
/// before (by the Python, whose descriptions the library moved in, or by the describer once it is in) gets that
/// description back, as the Python's describer gave it without asking again; nothing is asked of any model.
pub fn describe(library: &Library, run: &mut Made, tracker: &Tracker<'_>) -> StepOutcome {
    if run.redo.contains(&super::RedoStep::Descriptions) {
        return StepOutcome::Skipped(Skip::NotInThisBuild);
    }
    let (mut found, mut asked) = (0, 0);
    let model = run.settings.describe_model.clone();
    let papers = &run.papers;
    let mut failed = None;
    for record in &mut run.records {
        record.each_mut(&mut |record| {
            asked += 1;
            let key =
                crate::library::community_key(&model, record.members.iter().map(|&paper| papers[paper].id.as_str()));
            match library.description(&key) {
                Ok(Some(kept)) => {
                    record.name = Some(kept.name);
                    record.description = Some(kept.description);
                    found += 1;
                }
                Ok(None) => {}
                Err(error) => failed = Some(error.to_string()),
            }
        });
    }
    if let Some(why) = failed {
        return StepOutcome::Failed(why);
    }
    if found == 0 {
        return StepOutcome::Skipped(Skip::NotInThisBuild);
    }
    tracker
        .log(&format!("  {found} of {asked} communities described before; the others are not described in this build"));
    StepOutcome::Kept(format!("{found} of {asked} descriptions"))
}

/// Score the abstracts of `run.papers`, setting `run.flagged`.
pub fn detect(library: &Library, run: &mut Made, tracker: &Tracker<'_>) -> StepOutcome {
    let _ = (library, run, tracker);
    StepOutcome::Skipped(Skip::NotInThisBuild)
}

/// Whether abstracts get scored in the next run, and why (`Library.ai_detector` in the contracts).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DetectorState {
    /// `off`, `untrained` or `ready`.
    pub state: &'static str,
    pub why: String,
}

/// What the interface is told of the detector; the detector's module answers this once it is in.
pub fn detector_state(switched_on: bool) -> DetectorState {
    if switched_on {
        DetectorState {
            state: "untrained",
            why: "The AI-text detector is not part of this version of the engine yet, so no abstract is scored."
                .to_owned(),
        }
    } else {
        DetectorState { state: "off", why: "Switched off in the settings.".to_owned() }
    }
}

/// What was measured about the detector on papers it was not trained on (`Library.ai_validation`); none until
/// the detector's module is in and trained.
pub fn detector_validation() -> Option<serde_json::Value> {
    None
}
