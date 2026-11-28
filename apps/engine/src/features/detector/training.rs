//! Building the training set, as the Python's `Trainer.build` did, through the engine's rules for money:
//!
//! 1. **The pool**: [`POOL`] abstracts submitted to arXiv in May and June 2019 (cs.LG, stat.ML, cs.CV, cs.CL),
//!    fetched a day at a time at arXiv's pace ([`crate::sources::arxiv`]), each day kept as it arrives and never
//!    fetched again. The Python asked the whole range at once, oldest first, and took the first [`POOL`] of at least
//!    [`MIN_WORDS`] words: a day at a time, oldest first, is the same papers in the same order.
//! 2. **Mirrors** of the first [`HELD_OUT_MIRRORS`] held-out papers (to measure on), then of the first [`START`]
//!    others (to train on).
//! 3. **Rounds of mining**, at most [`ROUNDS`]: the [`MINED`] training-side papers the classifier most wrongly takes
//!    for a model's get mirrors and are added, while the catch rate on the validation side improves.
//!
//! Every mirror is asked of the model chosen in Settings ([`Chosen`]) through [`batch::ask_all`], under the one
//! budget the user accepted after seeing [`estimate`]: what is kept is never asked again, each answer is kept as it
//! arrives (in the batch's store, then in the detector's table with the provider and model that wrote it), every
//! call is recorded in the ledger with its purpose, and stopping half way (a cancel, the budget) loses nothing:
//! building again sends only what is missing. The Python wrote with two fixed Claude models in turn; here the
//! models are the user's choice, and which wrote each mirror is kept and told ("trained on text from ...").
//!
//! Training a classifier on what is kept costs nothing: once the training set is built, building again only fits it
//! again (what a stale classifier needs).

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use chrono::{Days, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::classifier::{self, Classifier, Held, Side, held, held_out, measure};
use super::{DetectorError, MIN_WORDS, keep};
use crate::features::detector::store::{Generator, Human, Kind, Mirror, POOL as POOL_SAMPLE, YearFlagged};
use crate::jobs::{Progress, Tracker};
use crate::library::{Feature, Library, Slot, Subject};
use crate::model::batch::{self, Asking, BatchEstimate, IN_FLIGHT, Item, Outcome};
use crate::model::chosen::Chosen;
use crate::model::language::{Ask, Budget, Effort, Endpoint, Form, Transport, Unavailable};
use crate::sources::{Fetch, arxiv};

/// Human abstracts from 2019 to draw on.
pub const POOL: usize = 5_000;
/// Of them, how many get mirrors before any mining.
pub const START: usize = 400;
/// Hard negatives added (with mirrors) per round of mining.
pub const MINED: usize = 150;
/// At most this many rounds of mining.
pub const ROUNDS: usize = 3;
/// Held-out papers that get mirrors, to measure the catch rate on.
pub const HELD_OUT_MIRRORS: usize = 150;
/// The most generations a build may ask for: every paper above, both mirrors.
pub const MOST_GENERATIONS: usize = 2 * (HELD_OUT_MIRRORS + START + ROUNDS * MINED);
/// The pool's arXiv categories and days, as the Python's `POOL_QUERY` had them.
pub const CATEGORIES: [&str; 4] = ["cs.LG", "stat.ML", "cs.CV", "cs.CL"];
const POOL_DAYS: (NaiveDate, NaiveDate) = (date(2019, 5, 1), date(2019, 6, 30));
/// One week of arXiv submissions per year for the year check, as the Python's `YEARS` had them.
pub const YEARS: [(&str, NaiveDate, NaiveDate); 6] = [
    ("2019", date(2019, 8, 5), date(2019, 8, 9)),
    ("2021", date(2021, 10, 4), date(2021, 10, 8)),
    ("2022", date(2022, 10, 3), date(2022, 10, 7)),
    ("2023", date(2023, 10, 2), date(2023, 10, 6)),
    ("2024", date(2024, 10, 7), date(2024, 10, 11)),
    ("2025", date(2025, 10, 6), date(2025, 10, 10)),
];
/// Abstracts of each year's week scored by the year check.
pub const YEAR_SAMPLE: usize = 1_000;
/// The most a build may be allowed to cost, in US dollars, whatever is asked: 2,000 short abstracts cost a few
/// dollars with most models.
pub const MOST_USD: f64 = 100.0;
/// The same for a model with no price, in tokens.
pub const MOST_TOKENS: u64 = 20_000_000;
/// The version of the prompts: in the key each mirror is kept under.
pub const PROMPT: u32 = 1;
/// What the ledger shows the calls as.
pub const PURPOSE: &str = "training the AI-text detector";
/// What the job is called while it runs.
pub const KIND: &str = "training the AI-text detector";
/// What a mirror is likely to take, in tokens (150 to 250 words), for the estimate.
const LIKELY_TOKENS: u32 = 450;
const MAX_TOKENS: u32 = 2_000;

const fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    match NaiveDate::from_ymd_opt(year, month, day) {
        Some(date) => date,
        None => NaiveDate::MIN,
    }
}

/// The prompts, as the Python had them.
pub fn prompt(kind: Kind) -> &'static str {
    match kind {
        Kind::Written => {
            "Write the abstract of a research paper with the given title, as it would appear on arXiv: one \
             paragraph of 150 to 250 words. Output only the abstract."
        }
        Kind::Polished => {
            "Improve the wording, clarity and flow of this research abstract. Keep every claim, number and technical \
             term, and about the same length. Output only the rewritten abstract."
        }
    }
}

/// What a model answers: the abstract alone, in JSON, as every provider can be held to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Written {
    #[serde(rename = "abstract")]
    pub text: String,
}

fn schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["abstract"],
        "properties": {"abstract": {"type": "string"}}
    })
}

fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// The mirrors to ask for: each paper's two, less those kept.
fn wanted<'a>(papers: &[&'a Human], have: &HashSet<(String, Kind)>) -> Vec<(&'a Human, Kind)> {
    papers
        .iter()
        .flat_map(|paper| Kind::ALL.map(|kind| (*paper, kind)))
        .filter(|(paper, kind)| !have.contains(&(paper.id.clone(), *kind)))
        .collect()
}

fn have(mirrors: &[Mirror]) -> HashSet<(String, Kind)> {
    mirrors.iter().map(|mirror| (mirror.id.clone(), mirror.kind)).collect()
}

fn items<'a>(chosen: &'a Chosen, schema: &'a Value, asked: &[(&'a Human, Kind)]) -> Vec<Item<'a>> {
    let effort = chosen.effort_at_most(Effort::Low);
    asked
        .iter()
        .map(|&(paper, kind)| Item {
            slot: Slot {
                feature: Feature::Detector,
                key: chosen.key(Feature::Detector, PROMPT, &[paper.id.as_str()], kind.name()),
                subject: Subject::Paper(paper.id.clone()),
            },
            prompt: PROMPT,
            ask: Ask {
                purpose: PURPOSE,
                model: chosen.model(),
                system: prompt(kind),
                shared: None,
                question: match kind {
                    Kind::Written => &paper.title,
                    Kind::Polished => &paper.r#abstract,
                },
                form: Form::Json(schema),
                effort,
                max_tokens: MAX_TOKENS,
            },
            likely_tokens: LIKELY_TOKENS,
        })
        .collect()
}

/// The recipe's sizes: [`Sizes::RECIPE`] in the app; smaller in a test, as the Python's tests shrank them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sizes {
    pub pool: usize,
    pub start: usize,
    pub mined: usize,
    pub rounds: usize,
    pub held_out_mirrors: usize,
}

impl Sizes {
    pub const RECIPE: Sizes =
        Sizes { pool: POOL, start: START, mined: MINED, rounds: ROUNDS, held_out_mirrors: HELD_OUT_MIRRORS };
}

/// A paper of the pool not fetched yet, for the estimate: a title and an abstract of a usual length.
fn stand_in(index: usize) -> Human {
    Human { id: format!("planned-{index}"), title: "t".repeat(90), r#abstract: "word ".repeat(260) }
}

/// What building would ask and cost, before anything is asked (`DetectorEstimate` in the contracts).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Estimate {
    /// The pool's abstracts kept, of how many it needs.
    pub pool_kept: usize,
    pub pool: usize,
    /// Mirrors kept already (they cost nothing).
    pub mirrors_kept: usize,
    /// The most generations a build could still ask for: an upper bound, since which papers mining picks is known
    /// only as it goes.
    pub to_generate: usize,
    /// Whether a training set has been built: building again then asks nothing and fits the classifier again.
    pub built: bool,
    /// What asking `to_generate` would cost with the chosen model ([`BatchEstimate`]).
    pub cost: BatchEstimate,
}

/// What building would ask for: the papers whose mirrors are missing (stand-ins for papers not known yet: the
/// pool not fetched, or those mining will pick), in the order a build asks them.
struct Plan {
    /// The papers whose mirrors may be asked for (stand-ins where they are not known yet).
    papers: Vec<Human>,
    /// Of them, the mirrors not kept: (paper, kind).
    asked: Vec<(usize, Kind)>,
    pool_kept: usize,
    mirrors_kept: usize,
    built: bool,
}

fn plan(library: &Library, sizes: Sizes) -> Result<Plan, DetectorError> {
    let pool = first(library.detector_humans(POOL_SAMPLE)?, sizes);
    let mirrors = library.detector_mirrors()?;
    let have = have(&mirrors);
    let (built, _) = library.detector_built()?;
    let mut papers: Vec<Human> = Vec::new();
    if built.is_some() {
        // built: building again fits the classifier again on what is kept, and asks nothing
    } else if pool.len() >= sizes.pool {
        let held = pool.iter().filter(|human| held_out(&human.id)).take(sizes.held_out_mirrors);
        // the first START to train on, then the next ones as stand-ins for those mining will pick
        let free = pool.iter().filter(|human| !held_out(&human.id)).take(sizes.start + sizes.rounds * sizes.mined);
        papers.extend(held.chain(free).cloned());
    } else {
        papers.extend((0..sizes.held_out_mirrors + sizes.start + sizes.rounds * sizes.mined).map(stand_in));
    }
    let asked: Vec<(usize, Kind)> = papers
        .iter()
        .enumerate()
        .flat_map(|(index, paper)| Kind::ALL.map(|kind| (index, kind, paper)))
        .filter(|(_, kind, paper)| !have.contains(&(paper.id.clone(), *kind)))
        .map(|(index, kind, _)| (index, kind))
        .collect();
    Ok(Plan { papers, asked, pool_kept: pool.len(), mirrors_kept: mirrors.len(), built: built.is_some() })
}

/// What building would cost with `chosen`; nothing is sent.
pub fn estimate(library: &Library, chosen: &Chosen, sizes: Sizes) -> Result<Estimate, DetectorError> {
    let Plan { papers, asked, pool_kept, mirrors_kept, built } = plan(library, sizes)?;
    let schema = schema();
    let asked: Vec<(&Human, Kind)> = asked.iter().map(|(index, kind)| (&papers[*index], *kind)).collect();
    let items = items(chosen, &schema, &asked);
    let cost = batch::estimate_batch::<Written>(library, &items, chosen)?;
    let pool = sizes.pool;
    Ok(Estimate { pool_kept: pool_kept.min(pool), pool, mirrors_kept, to_generate: cost.to_ask, built, cost })
}

/// The pool a classifier is trained and measured on: the first [`Sizes::pool`] abstracts kept (a day is kept
/// whole, so more may be kept), as the Python's pool was exactly that many.
pub fn pool(library: &Library, sizes: Sizes) -> Result<Vec<Human>, crate::library::LibraryError> {
    Ok(first(library.detector_humans(POOL_SAMPLE)?, sizes))
}

/// The pool as used: its first [`Sizes::pool`] abstracts.
fn first(mut pool: Vec<Human>, sizes: Sizes) -> Vec<Human> {
    pool.truncate(sizes.pool);
    pool
}

/// What a build is handed.
pub struct Building<'a> {
    pub library: &'a Library,
    /// arXiv, for the pool and the year check.
    pub fetch: &'a dyn Fetch,
    /// Waits between arXiv's requests (a real sleep outside the tests).
    pub pause: &'a dyn Fn(Duration),
    pub chosen: &'a Chosen,
    pub transport: &'a dyn Transport,
    /// Where the model is asked, or why it cannot be: needed only when a mirror is missing.
    pub endpoint: Result<Endpoint, Unavailable>,
    pub budget: &'a Budget,
    pub sizes: Sizes,
}

/// What came of a build.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Built {
    /// The training set is built and a classifier fitted on it: `generated` mirrors were asked for now.
    Trained { generated: usize, trained_on: usize },
    /// It stopped before the end (asked to stop, the budget, the provider): everything generated is kept, and
    /// building again asks only for the rest.
    Stopped { generated: usize, why: String },
}

/// Why a build could not go on.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error(transparent)]
    Detector(#[from] DetectorError),
    #[error(transparent)]
    Arxiv(#[from] arxiv::ArxivError),
    #[error("too few 2019 abstracts could be fetched from arXiv to train on ({0})")]
    TooFew(usize),
}

impl From<crate::library::LibraryError> for BuildError {
    fn from(error: crate::library::LibraryError) -> Self {
        Self::Detector(error.into())
    }
}

/// Where a build is, as it goes.
enum Step {
    Going,
    Stopped(String),
}

/// The generations of one build: how many were asked for, and of how many at most, told as the job's progress.
struct Counter<'a> {
    tracker: &'a Tracker<'a>,
    done: u64,
    of: u64,
    generated: usize,
}

/// Build the training set and fit its classifier: the job behind `POST /api/detector/build`.
pub fn build(building: &Building<'_>, tracker: &Tracker<'_>) -> Result<Built, BuildError> {
    let library = building.library;
    if let (Some(built), _) = library.detector_built()? {
        // built before: nothing to ask; the classifier is fitted again on what is kept (a stale one asked for this)
        tracker.log("The training set was built before: fitting the classifier again on the texts kept.");
        let trained_on = refit(library, &built, building.sizes)?;
        return Ok(Built::Trained { generated: 0, trained_on });
    }
    let sizes = building.sizes;
    let asked = plan(library, sizes)?.asked;
    let mut counter = Counter { tracker, done: 0, of: asked.len() as u64, generated: 0 };
    tracker.progress(Progress { done: 0, of: counter.of, usd: building.budget.spent().1 });

    if let Step::Stopped(why) = fetch_pool(building, tracker)? {
        return Ok(Built::Stopped { generated: 0, why });
    }
    let pool = first(library.detector_humans(POOL_SAMPLE)?, sizes);
    if pool.len() < sizes.start + sizes.held_out_mirrors {
        return Err(BuildError::TooFew(pool.len()));
    }
    let held_papers: Vec<&Human> = pool.iter().filter(|human| held_out(&human.id)).collect();
    let free: Vec<&Human> = pool.iter().filter(|human| !held_out(&human.id)).collect();

    let measured_on = &held_papers[..sizes.held_out_mirrors.min(held_papers.len())];
    tracker.log(&format!("Writing the mirrors of {} held-out abstracts, to measure on ...", measured_on.len()));
    if let Step::Stopped(why) = mirror(building, &mut counter, measured_on)? {
        return Ok(Built::Stopped { generated: counter.generated, why });
    }
    let mut train: Vec<String> = free.iter().take(sizes.start).map(|human| human.id.clone()).collect();
    tracker.log(&format!("Writing the mirrors of {} abstracts to train on ...", train.len()));
    if let Step::Stopped(why) = mirror(building, &mut counter, &free[..sizes.start.min(free.len())])? {
        return Ok(Built::Stopped { generated: counter.generated, why });
    }
    let mirrors = library.detector_mirrors()?;
    let validation = held(&pool, &mirrors, Side::Validation);
    let mut model = classifier::fit_on(&pool, &mirrors, &train);
    let mut measured = headline(&model, &validation);
    let mut rounds = vec![train.len()];
    tracker.log(&format!("  round 0: {} abstracts with mirrors: {}", train.len(), said(measured)));
    for number in 1..=sizes.rounds {
        // the human abstracts the classifier most wrongly takes for a model's
        let taken: HashSet<&str> = train.iter().map(String::as_str).collect();
        let mut rest: Vec<(f64, usize, &Human)> = free
            .iter()
            .filter(|human| !taken.contains(human.id.as_str()))
            .enumerate()
            .map(|(index, human)| (model.score(&human.r#abstract).unwrap_or(f64::NEG_INFINITY), index, *human))
            .collect();
        rest.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let hardest: Vec<&Human> = rest.into_iter().take(sizes.mined).map(|(_, _, human)| human).collect();
        if hardest.is_empty() {
            break;
        }
        tracker.log(&format!("Round {number} of mining: writing the mirrors of {} abstracts ...", hardest.len()));
        if let Step::Stopped(why) = mirror(building, &mut counter, &hardest)? {
            return Ok(Built::Stopped { generated: counter.generated, why });
        }
        let mirrors = library.detector_mirrors()?;
        let validation = held(&pool, &mirrors, Side::Validation);
        let candidate: Vec<String> =
            train.iter().cloned().chain(hardest.iter().map(|human| human.id.clone())).collect();
        let next = classifier::fit_on(&pool, &mirrors, &candidate);
        let next_measured = headline(&next, &validation);
        tracker.log(&format!("  round {number}: +{} hard negatives: {}", hardest.len(), said(next_measured)));
        if next_measured <= measured {
            tracker.log("  no better than the round before: stopping there");
            break;
        }
        (train, model, measured) = (candidate, next, next_measured);
        rounds.push(train.len());
    }
    library.keep_detector_built(&train, &rounds)?;
    let (Some(built), _) = library.detector_built()? else { return Err(BuildError::TooFew(0)) };
    let trained_on = refit(library, &built, sizes)?;
    tracker.log(&format!("Trained on {trained_on} abstracts from 2019 and their mirrors."));
    Ok(Built::Trained { generated: counter.generated, trained_on })
}

/// The mean of the two catch rates on the validation side: what mining is judged by.
fn headline(model: &Classifier, validation: &Held<'_>) -> f64 {
    measure(model, validation).map_or(0.0, |measured| {
        (measured.written_caught.unwrap_or(0.0) + measured.polished_caught.unwrap_or(0.0)) / 2.0
    })
}

fn said(headline: f64) -> String {
    format!("{:.0}% caught on average, at 1% false alarms", 100.0 * headline)
}

/// Fit the classifier of the built training set and keep it, with the year check measured again when its abstracts
/// are kept. Answers how many abstracts it was trained on.
fn refit(
    library: &Library,
    built: &crate::features::detector::store::Built,
    sizes: Sizes,
) -> Result<usize, BuildError> {
    let pool = pool(library, sizes)?;
    let mirrors = library.detector_mirrors()?;
    let Some((classifier, report)) = classifier::trained(&pool, &mirrors, built) else {
        return Err(BuildError::TooFew(pool.len()));
    };
    let years = years_flagged(library, &classifier)?;
    keep(library, classifier, report)?;
    if !years.is_empty() {
        library.keep_detector_years(&years)?;
    }
    Ok(built.train.len())
}

/// Ask for the mirrors of `papers` that are not kept, and keep each with the model that wrote it.
fn mirror(building: &Building<'_>, counter: &mut Counter<'_>, papers: &[&Human]) -> Result<Step, BuildError> {
    let library = building.library;
    let asked = wanted(papers, &have(&library.detector_mirrors()?));
    if asked.is_empty() {
        return Ok(Step::Going);
    }
    let endpoint = match &building.endpoint {
        Ok(endpoint) => endpoint.clone(),
        Err(why) => return Ok(Step::Stopped(format!("the model cannot be asked: {why}"))),
    };
    let schema = schema();
    let items = items(building.chosen, &schema, &asked);
    let tracker = counter.tracker;
    let (base, of) = (counter.done, counter.of);
    // the batch counts its own items; the job's progress counts the whole build's
    let log = |line: &str| tracker.log(line);
    let progress = |progress: Progress| {
        let done = base + progress.done;
        tracker.progress(Progress { done, of: of.max(done), usd: progress.usd });
    };
    let inner = Tracker::new(&log, &progress, tracker.cancel());
    let asking = Asking { library, transport: building.transport, endpoint, recorder: library, in_flight: IN_FLIGHT };
    let outcomes = batch::ask_all::<Written>(asking, &items, building.budget, &inner)?;
    counter.done = base + asked.len() as u64;
    counter.of = of.max(counter.done);
    let provider = Some(building.chosen.provider());
    let mut made = Vec::new();
    let mut stopped = None;
    let mut failed = 0;
    for ((paper, kind), outcome) in asked.iter().zip(outcomes) {
        let (model, text) = match outcome {
            Outcome::Answered(answered) => {
                counter.generated += 1;
                (answered.model, answered.answer.text)
            }
            Outcome::Kept(kept) => (kept.model, kept.answer.text),
            Outcome::Failed(_) => {
                failed += 1;
                continue;
            }
            Outcome::NotStarted(why) => {
                stopped.get_or_insert(why.to_string());
                continue;
            }
        };
        // a mirror too short to learn from is left out, as the Python left it out (it stays kept in the batch's
        // store, so asking again costs nothing)
        if words(&text) >= MIN_WORDS {
            made.push(Mirror { id: paper.id.clone(), kind: *kind, generator: Generator { provider, model }, text });
        }
    }
    library.keep_detector_mirrors(&made)?;
    if failed > 0 {
        tracker.log(&format!(
            "  {failed} could not be written: they are left out, and asked for again only if this build stops and is \
             started again before it finishes"
        ));
    }
    Ok(stopped.map_or(Step::Going, Step::Stopped))
}

/// Every day from `from` to `to`, as arXiv's queries name them.
fn days(from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    let mut days = Vec::new();
    let mut day = from;
    while day <= to {
        days.push(day);
        day = day.checked_add_days(Days::new(1)).unwrap_or(NaiveDate::MAX);
    }
    days
}

/// Fetch days of arXiv for `sample` until it has `wanted` abstracts of at least [`MIN_WORDS`] words, each day
/// kept as it arrives and never fetched again.
fn fetch_sample(
    building: &Building<'_>,
    tracker: &Tracker<'_>,
    sample: &str,
    (from, to): (NaiveDate, NaiveDate),
    wanted: usize,
) -> Result<Step, BuildError> {
    let library = building.library;
    let fetched: HashSet<String> = library.detector_days(sample)?.into_iter().collect();
    let mut have = library.detector_humans(sample)?.len();
    let categories: Vec<String> = CATEGORIES.iter().map(|category| (*category).to_owned()).collect();
    for day in days(from, to) {
        if have >= wanted {
            break;
        }
        let name = day.format("%Y-%m-%d").to_string();
        if fetched.contains(&name) {
            continue;
        }
        if tracker.cancelled() {
            return Ok(Step::Stopped("asked to stop".to_owned()));
        }
        tracker.log(&format!("  arXiv, {name}: {have} of {wanted} abstracts so far"));
        let papers = arxiv::query(building.fetch, building.pause, &arxiv::submitted_on(day, &categories), &|_| {})?;
        let humans: Vec<Human> = papers
            .into_iter()
            .filter(|paper| words(&paper.abstract_text) >= MIN_WORDS)
            .map(|paper| Human { id: paper.id, title: paper.title, r#abstract: paper.abstract_text })
            .collect();
        have += humans.len();
        library.keep_detector_day(sample, &name, &humans)?;
    }
    Ok(Step::Going)
}

fn fetch_pool(building: &Building<'_>, tracker: &Tracker<'_>) -> Result<Step, BuildError> {
    let wanted = building.sizes.pool;
    if building.library.detector_counts(POOL_SAMPLE)?.0 >= wanted {
        return Ok(Step::Going);
    }
    tracker.log(&format!("Fetching {wanted} abstracts from 2019 from arXiv (one request every few seconds) ..."));
    fetch_sample(building, tracker, POOL_SAMPLE, POOL_DAYS, wanted)
}

/// The share flagged of each year's kept abstracts (the years whose abstracts are kept).
fn years_flagged(library: &Library, classifier: &Classifier) -> Result<BTreeMap<String, YearFlagged>, BuildError> {
    let mut shares = BTreeMap::new();
    for (year, ..) in YEARS {
        let mut texts = library.detector_humans(year)?;
        texts.truncate(YEAR_SAMPLE);
        let scores: Vec<f64> = texts.iter().filter_map(|human| classifier.score(&human.r#abstract)).collect();
        if scores.is_empty() {
            continue;
        }
        let flagged = scores.iter().filter(|score| **score > classifier.threshold).count();
        shares.insert(
            year.to_owned(),
            YearFlagged { abstracts: scores.len(), flagged: flagged as f64 / scores.len() as f64 },
        );
    }
    Ok(shares)
}

/// The year check, as the Python's `over_the_years`: the share flagged of a week of arXiv in each year. Its human
/// examples are all from 2019, so if writing had merely drifted, 2021 and 2022 (before ChatGPT, at the end of
/// November 2022) would already be flagged more often. It asks arXiv only, never a model: free, a few minutes.
pub fn check_years(
    building: &Building<'_>,
    classifier: &Classifier,
    tracker: &Tracker<'_>,
) -> Result<Option<BTreeMap<String, YearFlagged>>, BuildError> {
    for (year, from, to) in YEARS {
        if let Step::Stopped(_) = fetch_sample(building, tracker, year, (from, to), YEAR_SAMPLE)? {
            return Ok(None);
        }
    }
    let shares = years_flagged(building.library, classifier)?;
    for (year, found) in &shares {
        tracker.log(&format!("  {year}: {:.1}% of {} abstracts flagged", 100.0 * found.flagged, found.abstracts));
    }
    building.library.keep_detector_years(&shares)?;
    Ok(Some(shares))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::super::tests::{HUMAN, MACHINE, text};
    use super::*;
    use crate::api::other_providers::{MODEL, Scripted};
    use crate::jobs::Cancel;
    use crate::model::language::{Limit, Provider, fake};
    use crate::sources::fake::{Fake as FakeFetch, ok};
    use crate::usage::Usd;

    /// The Python test's sizes: a pool of 120, 20 to start with, one round of 5, 10 held-out papers mirrored.
    const SMALL: Sizes = Sizes { pool: 120, start: 20, mined: 5, rounds: 1, held_out_mirrors: 10 };

    fn chosen() -> Chosen {
        Chosen::of(Provider::OpenAi, MODEL, Effort::Low, Ok(fake::endpoint(Provider::OpenAi)))
    }

    /// A model that writes in the machine style: polished or written, each a little different.
    fn writer() -> Arc<Scripted> {
        let count = Mutex::new(0usize);
        Scripted::new(move |request| {
            let mut count = count.lock().unwrap();
            *count += 1;
            let polished = request.contains("Improve the wording");
            json!({"abstract": text(MACHINE, *count + if polished { 10_000 } else { 0 })}).to_string()
        })
    }

    /// arXiv's answer for the first day of the pool: `count` abstracts in the human style.
    fn first_day(count: usize) -> FakeFetch {
        let entries: String = (0..count)
            .map(|n| {
                format!(
                    "<entry><id>http://arxiv.org/abs/1905.{n:05}v1</id><title>Title {n}</title><summary>{}</summary>\
                     <published>2019-05-01T00:00:00Z</published><updated>2019-05-01T00:00:00Z</updated></entry>",
                    text(HUMAN, n)
                )
            })
            .collect();
        let feed = format!(
            r#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom" xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/"
               xmlns:arxiv="http://arxiv.org/schemas/atom"><opensearch:totalResults>{count}</opensearch:totalResults>{entries}</feed>"#
        );
        let categories: Vec<String> = CATEGORIES.iter().map(|category| (*category).to_owned()).collect();
        let url = arxiv::page_url(&arxiv::submitted_on(POOL_DAYS.0, &categories), 0);
        FakeFetch::new().with(&url, ok("application/atom+xml", &feed))
    }

    struct Setup {
        _folder: tempfile::TempDir,
        library: Library,
        transport: Arc<Scripted>,
        fetch: FakeFetch,
        chosen: Chosen,
    }

    fn setup() -> Setup {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        Setup { _folder: folder, library, transport: writer(), fetch: first_day(130), chosen: chosen() }
    }

    fn plenty() -> Budget {
        Budget::new(Limit::Dollars(Usd::from_picodollars(10_000_000_000_000)))
    }

    impl Setup {
        fn build(&self, budget: &Budget, cancel: &Cancel) -> Built {
            let building = Building {
                library: &self.library,
                fetch: &self.fetch,
                pause: &|_| {},
                chosen: &self.chosen,
                transport: &*self.transport,
                endpoint: self.chosen.endpoint(),
                budget,
                sizes: SMALL,
            };
            let (log, progress) = (|_: &str| {}, |_: Progress| {});
            build(&building, &Tracker::new(&log, &progress, cancel)).unwrap()
        }
    }

    #[test]
    fn a_build_fetches_the_pool_mirrors_mines_the_hardest_humans_and_trains() {
        let setup = setup();
        let shown = estimate(&setup.library, &setup.chosen, SMALL).unwrap();
        assert_eq!((shown.to_generate, shown.pool_kept), (2 * (10 + 20 + 5), 0), "stand-ins for the pool");
        assert!(shown.cost.cost.usd.is_some() && setup.transport.sent().is_empty(), "an estimate sends nothing");

        let built = setup.build(&plenty(), &Cancel::new());
        let Built::Trained { generated, trained_on } = built else { panic!("{built:?}") };
        assert_eq!(setup.fetch.asked().len(), 1, "one day of arXiv was enough");
        let pool = setup.library.detector_humans(POOL_SAMPLE).unwrap();
        assert_eq!(pool.len(), 130, "the whole day is kept; the first 120 are the pool");
        let (Some(kept), _) = setup.library.detector_built().unwrap() else { panic!() };
        let mirrored: HashSet<String> = setup.library.detector_mirrors().unwrap().into_iter().map(|m| m.id).collect();
        assert!(kept.train.iter().all(|id| mirrored.contains(id)), "every training human has its mirrors");
        assert!(!kept.train.iter().any(|id| held_out(id)));
        let free: Vec<&str> = pool[..120].iter().filter(|human| !held_out(&human.id)).map(|h| h.id.as_str()).collect();
        assert_eq!(kept.rounds[0], 20);
        assert_eq!(&kept.train[..20], &free[..20]);
        assert_eq!(trained_on, kept.train.len());
        assert_eq!(generated, setup.transport.sent().len(), "one call per mirror written");
        let mirrors = setup.library.detector_mirrors().unwrap();
        assert!(mirrors.iter().all(|mirror| mirror.generator.provider == Some(Provider::OpenAi)));
        assert_eq!(Kind::ALL.map(|kind| mirrors.iter().any(|mirror| mirror.kind == kind)), [true, true]);
        let spending = setup.library.spending().unwrap();
        assert_eq!(spending.purposes[PURPOSE].calls, generated as i64, "recorded in the ledger with its purpose");
        let of_a_run: i64 = setup
            .library
            .connection()
            .query_row("SELECT COUNT(*) FROM spending WHERE run IS NOT NULL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(of_a_run, 0, "the detector's training is no run's cost");

        let trained = super::super::model(&setup.library).unwrap().unwrap();
        let report = &trained.report;
        assert_eq!((report.rounds[0].round, report.rounds[0].humans), (0, 20));
        assert_eq!((report.rounds[0].written_caught, report.rounds[0].polished_caught), (Some(1.0), Some(1.0)));
        assert_eq!(report.written_by[0].model, MODEL);

        // building again asks nothing: it fits the classifier again on what is kept
        let sent = setup.transport.sent().len();
        assert_eq!(estimate(&setup.library, &setup.chosen, SMALL).unwrap().to_generate, 0);
        assert!(matches!(setup.build(&plenty(), &Cancel::new()), Built::Trained { generated: 0, .. }));
        assert_eq!(setup.transport.sent().len(), sent, "a mirror that exists is not paid for again");
        assert_eq!(setup.fetch.asked().len(), 1, "a day fetched is not fetched again");
    }

    #[test]
    fn a_build_stopped_by_its_budget_keeps_what_it_paid_for_and_asks_only_the_rest_next_time() {
        let setup = setup();
        // a few calls' worth of tokens: the batch stops before it would pass them
        let small = Budget::new(Limit::Tokens(12_000));
        let stopped = setup.build(&small, &Cancel::new());
        let Built::Stopped { generated, why } = stopped else { panic!("{stopped:?}") };
        assert!(generated > 0 && generated < 70, "{generated}");
        assert!(why.contains("budget") || why.contains("token"), "{why}");
        assert!(small.spent().0 <= 12_000);
        assert_eq!(setup.library.detector_mirrors().unwrap().len(), generated, "everything paid for is kept");
        assert_eq!(setup.library.detector_built().unwrap().0, None, "not built");

        let first = setup.transport.sent().len();
        assert!(matches!(setup.build(&plenty(), &Cancel::new()), Built::Trained { .. }));
        let mirrors = setup.library.detector_mirrors().unwrap().len();
        assert_eq!(setup.transport.sent().len() - first, mirrors - generated, "only what was missing was asked");
    }

    #[test]
    fn a_cancelled_build_asks_nothing_and_fetches_nothing() {
        let setup = setup();
        let cancel = Cancel::new();
        cancel.cancel();
        assert!(matches!(setup.build(&plenty(), &cancel), Built::Stopped { generated: 0, .. }));
        assert!(setup.transport.sent().is_empty() && setup.fetch.asked().is_empty());
    }

    #[test]
    fn a_build_without_a_key_stops_before_asking_and_says_why() {
        let setup = setup();
        let building = Building {
            library: &setup.library,
            fetch: &setup.fetch,
            pause: &|_| {},
            chosen: &setup.chosen,
            transport: &*setup.transport,
            endpoint: Err(Unavailable::NoKey),
            budget: &plenty(),
            sizes: SMALL,
        };
        let (log, progress, cancel) = (|_: &str| {}, |_: Progress| {}, Cancel::new());
        let built = build(&building, &Tracker::new(&log, &progress, &cancel)).unwrap();
        assert!(matches!(&built, Built::Stopped { why, .. } if why.contains("cannot be asked")), "{built:?}");
        assert!(setup.transport.sent().is_empty());
    }

    #[test]
    fn the_pool_days_and_the_year_weeks_are_those_the_python_asked() {
        let pool = days(POOL_DAYS.0, POOL_DAYS.1);
        assert_eq!(
            (pool.len(), pool[0].to_string(), pool[60].to_string()),
            (61, "2019-05-01".to_owned(), "2019-06-30".to_owned())
        );
        assert!(YEARS.iter().all(|(_, from, to)| days(*from, *to).len() == 5));
        assert_eq!(MOST_GENERATIONS, 2_000);
    }
}
