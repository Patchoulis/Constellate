//! Estimate which abstracts were written or rewritten by a language model: the Python backend's `rp/detect.py`,
//! in Rust.
//!
//! What this is NOT: watermark detection. A watermark can only be checked by whoever holds the generating model's
//! secret key, and most providers do not watermark text, so there is nothing in an arbitrary abstract to look up.
//!
//! What it is: a small version of the recipe Pangram Labs published for their detector (Emi & Spero 2024,
//! "Technical Report on the Pangram AI-Generated Text Classifier", arXiv:2402.14873):
//!
//! * **Synthetic mirrors.** For a human document, a model is asked for a document on the same topic and of the same
//!   length. Trained on such pairs, a classifier cannot use topic as a shortcut: only how the text is written
//!   differs. The human documents are abstracts submitted to arXiv in 2019, before such models were in use, and
//!   each gets two mirrors: one "written" from the title alone, one the human abstract "polished" by a model.
//! * **Hard negative mining.** Train; score a large pool of human documents; take those it most wrongly scores as
//!   machine-written; make mirrors for exactly those; add both and train again, while the classifier, measured on
//!   held-out documents, improves. The model is shown its own mistakes, which pushes false alarms down.
//! * **The classifier** is a logistic regression on word and word-pair frequencies ([`features`], [`logistic`]):
//!   language models favour certain words (Kobak et al. 2024, "Delving into ChatGPT usage in academic writing
//!   through excess vocabulary"). It explains itself: the words that raised a score can be listed. It is a linear
//!   model: scoring and training run on the processor, no graphics card is involved.
//!
//! Everything reported is measured on held-out papers ([`classifier::side`]): a fifth of the pool, chosen by id,
//! is never trained on and never mined; half of it decides when mining stops, and the other half, used for no
//! decision at all, is what the numbers come from. The threshold is set where [`FALSE_ALARMS`] of held-out human
//! abstracts score higher; the share of held-out mirrors above it is the catch rate, for "written" and "polished"
//! apart. What the numbers do not show: the mirrors come from the models that wrote them only (which, is kept with
//! every mirror and told with the detector); the scale is thousands of documents, in one domain; and how often
//! present-day human abstracts are flagged cannot be measured, since nobody knows which of them are purely human.
//! So a flag means "written the way these models write", an estimate, never proof.
//!
//! Building the training set asks the chosen language model for about 2,000 short generations, within a budget
//! the user accepted after seeing the estimate ([`training`]); every generation is kept as it arrives. Scoring a
//! run's papers afterwards is local and takes seconds ([`score_run`]). Until a training set is built, nothing is
//! flagged.
//!
//! | file            | what                                                                         |
//! |-----------------|------------------------------------------------------------------------------|
//! | [`features`]    | words and word pairs, weighted as scikit-learn's TF-IDF                      |
//! | [`logistic`]    | the logistic regression, solved by L-BFGS                                    |
//! | [`classifier`]  | the classifier, the held-out split, what is measured, the report             |
//! | [`training`]    | building the training set: the pool from arXiv, the mirrors, mining, the year check |

pub mod classifier;
pub mod features;
pub mod logistic;
#[cfg(test)]
mod parity;
pub mod training;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::jobs::Tracker;
use crate::library::detector::{ByYear, POOL};
use crate::library::{AiScore, Library, LibraryError, Paper};
use crate::settings::Settings;
use crate::threads::{self, Progress};
use classifier::{Classifier, Report};

/// The share of held-out human abstracts that score above the threshold, by construction.
pub const FALSE_ALARMS: f64 = 0.01;
/// Shorter abstracts are not scored and never flagged.
pub const MIN_WORDS: usize = 60;
/// Markers named for one paper.
pub const MARKERS: usize = 6;

/// What went wrong with the detector's own data.
#[derive(Debug, thiserror::Error)]
pub enum DetectorError {
    #[error(transparent)]
    Library(#[from] LibraryError),
    /// The kept classifier does not read as one (written by a newer version, or damaged).
    #[error("the AI-text detector kept in the library cannot be read: {0}")]
    Unreadable(String),
}

/// The classifier as kept, with its report and when it was fitted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    pub classifier: Classifier,
    pub report: Report,
}

/// A classifier ready to score, and what is known about it.
#[derive(Clone, Debug, PartialEq)]
pub struct Trained {
    pub classifier: Classifier,
    pub report: Report,
    pub trained_at: String,
}

impl Trained {
    /// Fitted by the code as it is now? A classifier fitted by an older version of the features or the training is
    /// stale: it still scores, and is fitted again only on request.
    pub fn current(&self) -> bool {
        self.classifier.features == features::FEATURES && self.classifier.training == logistic::TRAINING
    }
}

/// The classifier last read, by library file and fitting time, so that it is parsed once and not per request.
type Loaded = Option<(PathBuf, String, Arc<Trained>)>;
static LOADED: Mutex<Loaded> = Mutex::new(None);
/// Held while a classifier is fitted from a training set that has none, so that two requests fit it once.
static FITTING: Mutex<()> = Mutex::new(());

/// The classifier of the library, or None when no training set has been built. A training set built (or moved in
/// from the Python's files) without a classifier yet is fitted here, once, as the Python fitted its own whenever
/// its training file changed: locally, in seconds, at no cost.
pub fn model(library: &Library) -> Result<Option<Arc<Trained>>, DetectorError> {
    if let Some(found) = kept(library)? {
        return Ok(Some(found));
    }
    let _fitting = FITTING.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(found) = kept(library)? {
        return Ok(Some(found)); // fitted by whoever held the lock before
    }
    let (Some(built), _) = library.detector_built()? else { return Ok(None) };
    let pool = library.detector_humans(POOL)?;
    let mirrors = library.detector_mirrors()?;
    let Some((classifier, report)) = classifier::trained(&pool, &mirrors, &built) else { return Ok(None) };
    keep(library, classifier, report)?;
    kept(library)
}

/// Keep a classifier just fitted (replacing the one kept).
pub fn keep(library: &Library, classifier: Classifier, report: Report) -> Result<(), DetectorError> {
    let (features, training) = (classifier.features, classifier.training);
    let content = serde_json::to_string(&Kept { classifier, report }).map_err(LibraryError::from)?;
    library.keep_detector_model(features, training, &content)?;
    Ok(())
}

fn kept(library: &Library) -> Result<Option<Arc<Trained>>, DetectorError> {
    let Some(row) = library.detector_model()? else { return Ok(None) };
    // when it was fitted (to the second) and its length: a classifier fitted again in the same second from other
    // texts has other numbers in it, and so another length
    let stamp = format!("{} {}", row.trained_at, row.content.len());
    let mut loaded = LOADED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((path, at, trained)) = loaded.as_ref() {
        if path == library.path() && *at == stamp {
            return Ok(Some(Arc::clone(trained)));
        }
    }
    let Kept { classifier, report } =
        serde_json::from_str(&row.content).map_err(|error| DetectorError::Unreadable(error.to_string()))?;
    let trained = Arc::new(Trained { classifier: classifier.ready(), report, trained_at: row.trained_at.clone() });
    *loaded = Some((library.path().to_path_buf(), stamp, Arc::clone(&trained)));
    Ok(Some(trained))
}

/// Whether abstracts get scored, with what belongs to each situation. Exactly one holds; [`availability`] says
/// which, and [`Availability::why`] says it to the user.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Availability {
    /// Switched off in Settings: nothing is scored or tagged.
    Off,
    /// Switched on, but no classifier has been built on this computer: nothing is tagged.
    Untrained,
    /// The training set is being built: a job, so many generations done of how many.
    Building { done: u64, of: u64 },
    /// A classifier, fitted by this version of the code.
    Ready { trained_at: String },
    /// A classifier fitted by an older version of the features or the training: it scores (and every score is
    /// computed again with it), and is fitted again only on request.
    Stale { trained_at: String },
}

/// The situation, from the setting, a build job under way (its generations done of how many), and the classifier.
/// The switch wins: off is off whatever is built.
pub fn availability(enabled: bool, building: Option<(u64, u64)>, model: Option<&Trained>) -> Availability {
    match (enabled, building, model) {
        (false, _, _) => Availability::Off,
        (true, Some((done, of)), _) => Availability::Building { done, of },
        (true, None, None) => Availability::Untrained,
        (true, None, Some(trained)) if trained.current() => {
            Availability::Ready { trained_at: trained.trained_at.clone() }
        }
        (true, None, Some(trained)) => Availability::Stale { trained_at: trained.trained_at.clone() },
    }
}

impl Availability {
    /// The state's name as the interface knows it.
    pub fn name(&self) -> &'static str {
        match self {
            Availability::Off => "off",
            Availability::Untrained => "untrained",
            Availability::Building { .. } => "building",
            Availability::Ready { .. } => "ready",
            Availability::Stale { .. } => "stale",
        }
    }

    /// Does a run score abstracts in this state? (While building, the classifier kept before, if any, scores.)
    pub fn scores(&self) -> bool {
        matches!(self, Availability::Ready { .. } | Availability::Stale { .. })
    }

    /// A sentence for the interface. `cost` is what building would cost, as shown ("about USD 4.20 with
    /// claude-sonnet-5-5 of Anthropic"), or None when it cannot be estimated (then no cost is named at all).
    pub fn why(&self, cost: Option<&str>) -> String {
        match self {
            Availability::Off => "Switched off: abstracts are not scored and no paper is tagged. Papers tagged in \
                                  earlier runs keep their tag in those runs."
                .to_owned(),
            Availability::Untrained => {
                let cost = cost.map_or_else(String::new, |cost| format!(" Building it costs {cost}, once."));
                format!(
                    "Not available yet: no detector has been trained on this computer, so no paper is tagged. \
                     Training has the language model chosen in Settings write and polish about 2,000 abstracts \
                     from 2019 to learn its wording from; every one is kept, so stopping half way loses nothing.\
                     {cost} It does not need a graphics card."
                )
            }
            Availability::Building { done, of } => format!(
                "Being trained: {done} of {of} abstracts written so far. Until it is done, no paper is newly tagged."
            ),
            Availability::Ready { .. } => "On: every abstract is scored once (a fraction of a second per thousand, \
                                           on the processor: no graphics card needed) and papers above the \
                                           threshold are tagged."
                .to_owned(),
            Availability::Stale { .. } => "On, with a detector trained by an earlier version of the app: abstracts \
                                           are scored again with it and papers above its threshold are tagged. \
                                           Training it again uses the texts already kept: it costs nothing and \
                                           takes about a minute."
                .to_owned(),
        }
    }
}

/// The interface's `Library["ai_detector"]`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Told {
    pub state: &'static str,
    pub why: String,
}

/// The interface's `Library["ai_validation"]`: what was measured on papers the detector was not trained on.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Validation {
    /// The models whose text it was trained to recognise, as shown: "claude-sonnet-5-5 (Anthropic)".
    pub generators: Vec<String>,
    pub held_out: usize,
    pub false_alarms: f64,
    pub written_caught: Option<f64>,
    pub polished_caught: Option<f64>,
    pub machine_words: Vec<String>,
    pub by_year: Option<ByYear>,
}

/// A model as the information names it: "claude-sonnet-5-5 (Anthropic)", or the model alone when its provider
/// was not recorded.
pub fn generator_name(provider: Option<&str>, model: &str) -> String {
    match provider.and_then(crate::model::language::Provider::from_id) {
        Some(provider) => format!("{model} ({})", provider.about().label),
        None => model.to_owned(),
    }
}

impl Validation {
    pub fn of(trained: &Trained, by_year: Option<ByYear>) -> Self {
        let report = &trained.report;
        Validation {
            generators: report
                .written_by
                .iter()
                .map(|written| generator_name(written.provider.as_deref(), &written.model))
                .collect(),
            held_out: report.held_out,
            false_alarms: report.false_alarms,
            written_caught: report.measured.written_caught,
            polished_caught: report.measured.polished_caught,
            machine_words: report.machine_words.clone(),
            by_year,
        }
    }
}

/// What the AI-text detector found for one paper (`AiVerdict` in the contracts): an estimate, not proof.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AiVerdict {
    /// Higher: more machine-like; to three decimals, as the Python showed it.
    pub score: f64,
    pub threshold: f64,
    pub flagged: bool,
    /// The share of held-out human abstracts that score above the threshold.
    pub false_alarms: f64,
    /// The words and word pairs of this abstract that raised its score the most.
    pub markers: Vec<String>,
}

fn three(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// What the detector found for `paper`, for showing; None when its present text was never scored by the present
/// classifier, or is too short to judge (as the Python's `Detector.verdict`).
pub fn verdict(library: &Library, paper: &Paper) -> Result<Option<AiVerdict>, DetectorError> {
    let Some(trained) = model(library)? else { return Ok(None) };
    let classifier = &trained.classifier;
    let found = library.ai_scores(&classifier.key(), std::slice::from_ref(paper))?.pop().flatten();
    let Some(AiScore::Score(score)) = found else { return Ok(None) };
    Ok(Some(AiVerdict {
        score: three(score),
        threshold: three(classifier.threshold),
        flagged: score > classifier.threshold,
        false_alarms: FALSE_ALARMS,
        markers: classifier.markers(&paper.r#abstract, MARKERS),
    }))
}

/// One paper of a run, as the detector judged it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PaperScore {
    pub id: String,
    /// None: too short to judge, not in the library, or not scored (the detector is off or untrained).
    pub score: Option<f64>,
    /// Above the threshold: tagged. Never true without a score.
    pub flagged: bool,
}

/// Why a run's papers were not scored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Skipped {
    Off,
    Untrained,
}

/// What came of scoring a run's papers.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Scoring {
    /// `scored` papers were scored now (the rest were kept from before); `flagged` of all are above the threshold.
    Scored { scored: usize, kept: usize, flagged: usize },
    /// Every paper's score was kept from before, for its present text and this classifier.
    Kept { flagged: usize },
    /// Nothing was scored and nothing is flagged.
    Skipped { why: Skipped },
    /// Nothing is flagged: the detector's data could not be read or written, or the run was asked to stop.
    Failed { why: String },
}

/// A run's scores: the outcome, and one score per paper in the order asked.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunScores {
    pub outcome: Scoring,
    pub papers: Vec<PaperScore>,
}

impl RunScores {
    fn nothing(outcome: Scoring, ids: &[String]) -> Self {
        RunScores {
            outcome,
            papers: ids.iter().map(|id| PaperScore { id: id.clone(), score: None, flagged: false }).collect(),
        }
    }

    /// Per paper, in order: flagged or not.
    pub fn flags(&self) -> Vec<bool> {
        self.papers.iter().map(|paper| paper.flagged).collect()
    }
}

/// Score a run's papers: the one call the pipeline makes. Each paper is scored once per classifier and text (kept
/// in the library's `ai_scores` by the classifier's key and the paper's text hash), the rest on the computing
/// threads, each paper on its own so that the scores are the same at any thread count. A paper is flagged when
/// its score is above the threshold; one with no score never is.
pub fn score_run(library: &Library, settings: &Settings, ids: &[String], tracker: &Tracker<'_>) -> RunScores {
    if !settings.detect_ai {
        tracker.log(&format!("  {}", Availability::Off.why(None)));
        return RunScores::nothing(Scoring::Skipped { why: Skipped::Off }, ids);
    }
    match scored(library, ids, tracker) {
        Ok(Some(scores)) => scores,
        Ok(None) => {
            tracker.log("  no AI-text detector has been trained yet: nothing is flagged");
            RunScores::nothing(Scoring::Skipped { why: Skipped::Untrained }, ids)
        }
        Err(error) => {
            tracker.log(&format!("  the AI-text detector could not score: {error}"));
            RunScores::nothing(Scoring::Failed { why: error.to_string() }, ids)
        }
    }
}

fn scored(library: &Library, ids: &[String], tracker: &Tracker<'_>) -> Result<Option<RunScores>, DetectorError> {
    let Some(trained) = model(library)? else { return Ok(None) };
    let classifier = &trained.classifier;
    let key = classifier.key();
    let asked: Vec<&str> = ids.iter().map(String::as_str).collect();
    let records = library.papers(&asked)?;
    let known: Vec<Paper> = records.iter().flatten().cloned().collect();
    let mut scores: Vec<Option<AiScore>> = library.ai_scores(&key, &known)?;
    let missing: Vec<usize> = (0..known.len()).filter(|index| scores[*index].is_none()).collect();
    tracker.log(&format!("  {} scored before, {} to score", known.len() - missing.len(), missing.len()));
    let progress = Progress::new();
    let computed = threads::each(&missing, &progress, |&index| {
        if tracker.cancelled() {
            return None;
        }
        Some(classifier.score(&known[index].r#abstract).map_or(AiScore::TooShort, AiScore::Score))
    });
    let mut new: Vec<(&Paper, AiScore)> = Vec::new();
    let mut stopped = false;
    for (&index, outcome) in missing.iter().zip(computed) {
        match outcome.done().flatten() {
            Some(score) => {
                scores[index] = Some(score);
                new.push((&known[index], score));
            }
            None => stopped = true,
        }
    }
    library.keep_ai_scores(&key, &new)?;
    if stopped {
        // what was scored is kept; the run flags nothing rather than half its papers
        let why = "asked to stop before every paper was scored".to_owned();
        return Ok(Some(RunScores::nothing(Scoring::Failed { why }, ids)));
    }
    let by_id: std::collections::HashMap<&str, Option<f64>> = known
        .iter()
        .zip(&scores)
        .map(|(paper, score)| {
            (
                paper.id.as_str(),
                match score {
                    Some(AiScore::Score(score)) => Some(*score),
                    Some(AiScore::TooShort) | None => None,
                },
            )
        })
        .collect();
    let papers: Vec<PaperScore> = ids
        .iter()
        .map(|id| {
            let score = by_id.get(id.as_str()).copied().flatten();
            PaperScore { id: id.clone(), score, flagged: score.is_some_and(|score| score > classifier.threshold) }
        })
        .collect();
    let flagged = papers.iter().filter(|paper| paper.flagged).count();
    tracker.log(&format!(
        "  {flagged} of {} flagged: wording of the kind language models produce (the threshold flags {:.0}% of \
         pre-LLM abstracts)",
        ids.len(),
        100.0 * FALSE_ALARMS
    ));
    let outcome = if new.is_empty() {
        Scoring::Kept { flagged }
    } else {
        Scoring::Scored { scored: new.len(), kept: known.len() - new.len(), flagged }
    };
    Ok(Some(RunScores { outcome, papers }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests;
