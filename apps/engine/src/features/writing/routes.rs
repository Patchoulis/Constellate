//! What the language model writes about a run's communities ([`crate::features::writing`]): their names and descriptions,
//! and a community's overview.
//!
//! ```text
//! GET  /api/describe/estimate                 what naming a run made with the settings would cost at most, before
//!                                             it is made (`DescribeBeforeRun`); nothing is sent
//! GET  /api/runs/{run}/describe/estimate      what naming a made run's communities would cost (`DescribeEstimate`);
//!      ?add_sections=true                     ... those described plainly before sections, again with sections
//!      ?redo=true                             ... all of them again ("Describe all again")
//! POST /api/runs/{run}/describe               {provider, model, accepted_usd | accepted_tokens, add_sections?,
//!                                             redo?}: name them, as the one job (`DescribeStarted`)
//! GET  /api/runs/{run}/overview/{path}        where the community's overview stands for the model chosen
//!                                             (`OverviewStatus`, with its estimate when it is to be written)
//! POST /api/runs/{run}/overview/{path}        {provider, model, accepted_usd | accepted_tokens}: write it, as the
//!                                             one job (`OverviewStatus`)
//! GET  /api/runs/{run}/closest/{path}         each statement of the community's description with its papers closest
//!                                             in meaning, when found (`ClosestPapers`)
//! POST /api/runs/{run}/closest/{path}         find them, as the one job (`ClosestStarted`); nothing when kept
//! ```
//!
//! The closest papers cost nothing and send nothing anywhere: they are found with the run's embedding model on this
//! computer ([`crate::features::writing::closest`]). Without the model's files here, the answer says so
//! (`unavailable`) and nothing is started.
//!
//! A community described before sections (a plain description) is never asked again by an ordinary naming:
//! only by one with `add_sections` (or `redo`), whose own estimate the interface shows first.
//!
//! Money is spent only by the two POSTs, which the interface sends when the user clicks a button showing the
//! estimate, and which carry the most the user was shown: if that has grown since (the papers or the model
//! changed), or the request names another model than the one chosen, nothing is asked. The Python wrote an overview
//! at a click that showed no cost, and named a run's communities as part of making it without asking: here the run
//! form shows `GET /api/describe/estimate` and sends what was accepted as the run request's `describe`. The
//! pipeline (ported on its own) is to name a new run only with that: [`crate::features::writing::describe::budget_before_run`]
//! on it, then [`crate::features::writing::describe::describe_run`]; with no `describe`, the run is made without names.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::routing::{MethodRouter, get, post};
use ndarray::Array2;
use serde::{Deserialize, Serialize};

use crate::api::routes::{ApiError, Shared, on_the_side, one_name};
use crate::features::writing::closest::{self, Statement};
use crate::features::writing::describe::{self, Again, Group, Outcome, Run};
use crate::features::writing::overview::{self, Community, Graph, NotWritten, Overview, Prepared, Standing};
use crate::features::writing::{Stored, StoredTree, read_tree, tree_path};
use crate::jobs::{About, Finished, Phase, Tracker};
use crate::library::core::store::read_json;
use crate::library::{Feature, Library, Paper, Subject};
use crate::model::batch::{Asking, BatchEstimate, IN_FLIGHT};
use crate::model::chosen::{Accepted, Chosen, Terms, budget};
use crate::model::language::{Estimate, Provider};
use crate::pipeline::steps::model_files::{self, QWEN3};
use crate::settings;

/// What the job naming a run's communities is called in `/api/status`.
pub const DESCRIBE_JOB: &str = "naming communities";

/// The routes of this module, for [`crate::api::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![
        ("/api/describe/estimate", get(before_run)),
        ("/api/runs/{run}/describe/estimate", get(run_estimate)),
        ("/api/runs/{run}/describe", post(describe_now)),
        ("/api/runs/{run}/overview/{path}", get(overview_status).post(write_overview)),
        ("/api/runs/{run}/closest/{path}", get(closest_papers).post(find_closest)),
    ]
}

/// A run's `run.json`, as much as writing needs.
#[derive(Deserialize)]
struct RunFile {
    #[serde(default)]
    paper_ids: Vec<String>,
    #[serde(default)]
    settings: RunSettings,
}

#[derive(Default, Deserialize)]
struct RunSettings {
    #[serde(default)]
    embedding_model: String,
}

fn run_folder(engine: &Shared, run: &str) -> Result<PathBuf, ApiError> {
    let folder = engine.data.join("runs").join(one_name(run)?);
    if folder.is_dir() { Ok(folder) } else { Err(ApiError::NotFound) }
}

/// A run's files: its papers' ids and its tree (refused for a run whose tree repeats a path).
fn run_files(engine: &Shared, run: &str) -> Result<(PathBuf, RunFile, StoredTree), ApiError> {
    let folder = run_folder(engine, run)?;
    let file: RunFile = read_json(&folder.join("run.json"))?.ok_or(ApiError::NotFound)?;
    let tree = read_tree(&folder)?.ok_or(ApiError::NotFound)?;
    if tree.has_repeated_paths() {
        return Err(ApiError::Invalid(
            "This run was made by an older version, in which two communities could share a name. Run it again \
             (everything is reused) and ask then."
                .to_owned(),
        ));
    }
    Ok((folder, file, tree))
}

/// The library's records of `ids`, by id (those it has).
fn records_of(library: &Library, ids: &[String]) -> Result<HashMap<String, Paper>, ApiError> {
    let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
    Ok(library.papers(&borrowed)?.into_iter().flatten().map(|paper| (paper.id.clone(), paper)).collect())
}

// ------------------------------------------------------------------ names and descriptions

/// `DescribeBeforeRun` in the contracts.
#[derive(Serialize)]
pub struct BeforeRun {
    pub provider: Provider,
    pub model: String,
    pub communities_at_most: u64,
    pub calls_at_most: u64,
    /// At most: as many communities as the settings allow, every prompt as long as it may be (an estimate).
    pub estimate: Estimate,
    /// The chosen provider's key is set (never the key).
    pub api_key_set: bool,
    /// The settings' "describe communities".
    pub describe: bool,
}

async fn before_run(State(engine): State<Shared>) -> Result<Json<BeforeRun>, ApiError> {
    let told = on_the_side(move || {
        let chosen = engine.chosen()?;
        let settings = settings::load(&engine.settings_file(), &engine.choices)?;
        let at_most = describe::estimate_before_run(&chosen, &settings);
        Ok(BeforeRun {
            provider: chosen.provider(),
            model: chosen.model().to_owned(),
            communities_at_most: at_most.communities_at_most,
            calls_at_most: at_most.calls_at_most,
            estimate: at_most.cost,
            api_key_set: chosen.can_ask(),
            describe: settings.describe,
        })
    })
    .await?;
    Ok(Json(told))
}

/// A made run, as describing it needs: its papers (a paper the library no longer has is an empty record, so that
/// members still index the run's papers), its tree, and its papers' embeddings when the library has them all.
struct Loaded {
    folder: PathBuf,
    papers: Vec<Paper>,
    embeddings: Option<Array2<f32>>,
    groups: Vec<Group>,
    /// Communities with a name in the run's files now.
    named: usize,
    /// Of those, with a plain description and no sections.
    plain: usize,
    count: usize,
}

fn loaded(engine: &Shared, library: &Library, run: &str) -> Result<Loaded, ApiError> {
    let (folder, file, tree) = run_files(engine, run)?;
    let mut records = records_of(library, &file.paper_ids)?;
    let complete = records.len() == file.paper_ids.len();
    let papers: Vec<Paper> = file
        .paper_ids
        .iter()
        .map(|id| {
            records.remove(id).unwrap_or_else(|| Paper {
                id: id.clone(),
                title: String::new(),
                r#abstract: String::new(),
                authors: Vec::new(),
                published: String::new(),
                updated: String::new(),
                comment: String::new(),
                primary_category: String::new(),
                categories: Vec::new(),
                venues: Vec::new(),
                url: None,
                pdf_url: None,
            })
        })
        .collect();
    // nearest the mean embedding: from the library's vectors, else as the run's file lists them
    let embeddings = if complete && !file.settings.embedding_model.is_empty() {
        library.vectors(&file.settings.embedding_model, &papers).ok()
    } else {
        None
    };
    let index_of: HashMap<&str, usize> =
        file.paper_ids.iter().enumerate().map(|(index, id)| (id.as_str(), index)).collect();
    let groups = ordered(&tree.communities, &Group::from_stored(&tree.communities), &index_of);
    fn counted(stored: &[Stored], of: &dyn Fn(&Stored) -> bool) -> usize {
        stored.iter().map(|community| usize::from(of(community)) + counted(&community.children, of)).sum()
    }
    let named =
        counted(&tree.communities, &|community| community.name.as_deref().is_some_and(|name| !name.trim().is_empty()));
    let plain = counted(&tree.communities, &Stored::plainly_described);
    let count = tree.paths().len();
    Ok(Loaded { folder, papers, embeddings, groups, named, plain, count })
}

/// `groups` with each one's members ordered as its file lists them nearest its mean embedding, then the rest in
/// their order: what is sent when the library has not the run's embeddings.
fn ordered(stored: &[Stored], groups: &[Group], index_of: &HashMap<&str, usize>) -> Vec<Group> {
    stored
        .iter()
        .zip(groups)
        .map(|(community, group)| {
            let near: Vec<usize> = community
                .nearest_to_centroid
                .iter()
                .filter(|nearest| nearest.in_community)
                .filter_map(|nearest| index_of.get(nearest.id.as_str()).copied())
                .filter(|index| group.members.contains(index))
                .collect();
            let mut members = near.clone();
            members.extend(group.members.iter().copied().filter(|index| !near.contains(index)));
            Group {
                tree_path: group.tree_path.clone(),
                members,
                keywords: group.keywords.clone(),
                children: ordered(&community.children, &group.children, index_of),
            }
        })
        .collect()
}

/// What an estimate is asked for: naming as usual, or asking again those described plainly, or all.
#[derive(Deserialize)]
struct Asked {
    #[serde(default)]
    redo: bool,
    #[serde(default)]
    add_sections: bool,
}

/// What `redo` and `add_sections` ask again: all of it wins over the plain ones.
fn again_of(redo: bool, add_sections: bool) -> Again {
    match (redo, add_sections) {
        (true, _) => Again::All,
        (false, true) => Again::Plain,
        (false, false) => Again::Nothing,
    }
}

/// `DescribeEstimate` in the contracts.
#[derive(Serialize)]
pub struct RunEstimate {
    pub provider: Provider,
    pub model: String,
    /// Communities and sub-communities of the run.
    pub communities: usize,
    /// Of those, with a name in its files now.
    pub named: usize,
    /// Of those named, with a plain description and no sections: described before sections were asked for.
    pub plain: usize,
    /// Calls kept from before cost nothing (`kept`); an estimate.
    pub estimate: BatchEstimate,
    pub api_key_set: bool,
}

fn estimate_of(
    engine: &Shared,
    library: &Library,
    chosen: &Chosen,
    run: &str,
    again: Again,
) -> Result<(Loaded, BatchEstimate), ApiError> {
    let loaded = loaded(engine, library, run)?;
    let estimate = {
        let described = Run {
            name: run,
            folder: Some(&loaded.folder),
            papers: &loaded.papers,
            embeddings: loaded.embeddings.as_ref().map(|rows| rows.view()),
            communities: &loaded.groups,
            again,
        };
        describe::estimate_run(library, &described, chosen)?
    };
    Ok((loaded, estimate))
}

async fn run_estimate(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    query: Result<Query<Asked>, QueryRejection>,
) -> Result<Json<RunEstimate>, ApiError> {
    let Query(Asked { redo, add_sections }) = query?;
    let told = on_the_side(move || {
        let library = engine.library()?;
        let chosen = engine.chosen()?;
        let (loaded, estimate) = estimate_of(&engine, &library, &chosen, &run, again_of(redo, add_sections))?;
        Ok(RunEstimate {
            provider: chosen.provider(),
            model: chosen.model().to_owned(),
            communities: loaded.count,
            named: loaded.named,
            plain: loaded.plain,
            estimate,
            api_key_set: chosen.can_ask(),
        })
    })
    .await?;
    Ok(Json(told))
}

/// What the user accepted, for the model they were shown.
#[derive(Deserialize)]
struct Accepting {
    provider: Option<Provider>,
    model: String,
    accepted_usd: Option<f64>,
    accepted_tokens: Option<u64>,
    #[serde(default)]
    redo: bool,
    /// Describe again, with sections, those described plainly before (at the estimate `?add_sections=true` showed).
    #[serde(default)]
    add_sections: bool,
}

/// `DescribeStarted` in the contracts: the job's number, for following it in `/api/status`.
#[derive(Serialize)]
pub struct Started {
    pub ok: bool,
    pub id: u64,
}

fn invalid(why: impl ToString) -> ApiError {
    ApiError::Invalid(why.to_string())
}

async fn describe_now(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    body: Result<Json<Accepting>, JsonRejection>,
) -> Result<Json<Started>, ApiError> {
    let Json(request) = body?;
    let id = on_the_side(move || {
        let library = engine.library()?;
        let chosen = engine.chosen()?;
        chosen.as_shown(request.provider, &request.model).map_err(invalid)?;
        chosen.endpoint().map_err(invalid)?;
        let again = again_of(request.redo, request.add_sections);
        let (loaded, estimate) = estimate_of(&engine, &library, &chosen, &run, again)?;
        let accepted = Accepted::of(request.accepted_usd, request.accepted_tokens).map_err(invalid)?;
        let budget = describe::budget_of_run(&estimate, accepted).map_err(invalid)?;
        let transport = Arc::clone(&engine.transport);
        let served = Arc::clone(&engine);
        let work = move |tracker: &Tracker<'_>| {
            tracker.log(&match again {
                Again::Plain => format!("Describing the communities of {run} again with sections, with {chosen}."),
                Again::Nothing | Again::All => format!("Naming the communities of {run} with {chosen}."),
            });
            let Loaded { folder, papers, embeddings, groups, .. } = loaded;
            let described = Run {
                name: &run,
                folder: Some(&folder),
                papers: &papers,
                embeddings: embeddings.as_ref().map(|rows| rows.view()),
                communities: &groups,
                again,
            };
            let done = describe::describe_run(&library, &*transport, &described, &chosen, &budget, tracker);
            // its communities.json was written again: the next read is of the new file, not of one kept in memory
            served.runs.changed(&run);
            match done.outcome {
                Outcome::Described { .. } | Outcome::Kept { .. } => Ok(Finished::Run { run: run.clone() }),
                outcome => Err(outcome.told()),
            }
        };
        Ok(engine.job.start_tracked(DESCRIBE_JOB, work)?)
    })
    .await?;
    Ok(Json(Started { ok: true, id }))
}

// ------------------------------------------------------------------ overviews

/// `OverviewStatus` in the contracts, with what asking would cost when it is to be written.
#[derive(Serialize)]
pub struct Status {
    pub state: Phase,
    pub overview: Option<Box<Overview>>,
    pub error: Option<String>,
    /// The model the overview shown is (or would be) written by, and its provider.
    pub provider: Provider,
    pub model: String,
    /// What writing it would cost, when it is neither kept nor being written; an estimate.
    pub estimate: Option<BatchEstimate>,
    pub api_key_set: bool,
}

/// A community of a made run, as its overview needs it: the light part, enough to know whether it is kept.
struct Light {
    folder: PathBuf,
    tree: StoredTree,
    /// Its papers' ids, in its order.
    members: Vec<String>,
}

fn light(engine: &Shared, run: &str, path: &str) -> Result<Light, ApiError> {
    let path = tree_path(path).ok_or(ApiError::NotFound)?;
    let (folder, file, tree) = run_files(engine, run)?;
    let community = tree.find(path).ok_or(ApiError::NotFound)?;
    let members = community.members.iter().filter_map(|&index| file.paper_ids.get(index)).cloned().collect();
    Ok(Light { folder, tree, members })
}

/// The overview's prompt, read from the run's graph and the library: what estimating, writing and showing need.
fn prepared(
    library: &Library,
    chosen: &Chosen,
    run: &str,
    path: &str,
    light: &Light,
) -> Result<(Prepared, HashMap<String, Paper>), ApiError> {
    let graph: Graph = read_json(&light.folder.join("graph.json"))?.ok_or(ApiError::NotFound)?;
    let records = records_of(library, &light.members)?;
    let community = light.tree.find(path).ok_or(ApiError::NotFound)?;
    let about = Community { run, tree: &light.tree, community, graph: &graph, records: &records };
    let prepared = overview::prepare(chosen, &about);
    // the graph's papers and run.json's must be the same, or what is kept would be looked for under another key
    if prepared.members != light.members {
        return Err(ApiError::Invalid(
            "This run's files disagree about its papers. Run it again (everything is reused) and ask then.".to_owned(),
        ));
    }
    Ok((prepared, records))
}

fn status(engine: &Shared, run: &str, path: &str) -> Result<Status, ApiError> {
    let library = engine.library()?;
    let chosen = engine.chosen()?;
    let light = light(engine, run, path)?;
    let ids: Vec<&str> = light.members.iter().map(String::as_str).collect();
    let key = overview::key_of(&chosen, &ids);
    let subject = crate::library::Subject::Community { run: run.to_owned(), community: path.to_owned() };
    let prefix = overview::job_prefix(&subject);
    let line = format!("{prefix}{}.", key.as_str());
    let job = engine.job.snapshot(0);
    let kept = library.kept::<overview::Said>(Feature::Overview, &key)?;
    // while it is being written, nothing heavier than this is read at each look
    let standing = match &kept {
        None => Standing::of(None, &job, &prefix, &line),
        Some(_) => Standing::Idle,
    };
    let (standing, estimate) = match (kept, standing) {
        (None, Standing::Running) => (Standing::Running, None),
        (kept, standing) => {
            let (prepared, records) = prepared(&library, &chosen, run, path, &light)?;
            match kept {
                Some(kept) => (Standing::Done(Box::new(overview::shown(&kept, &prepared, &records))), None),
                None => (standing, Some(prepared.estimate(&library)?)),
            }
        }
    };
    let (overview, error) = match &standing {
        Standing::Done(overview) => (Some(overview.clone()), None),
        Standing::Failed(why) => (None, Some(why.clone())),
        Standing::Idle | Standing::Running => (None, None),
    };
    Ok(Status {
        state: standing.phase(),
        overview,
        error,
        provider: chosen.provider(),
        model: chosen.model().to_owned(),
        estimate,
        api_key_set: chosen.can_ask(),
    })
}

async fn overview_status(
    State(engine): State<Shared>,
    Path((run, path)): Path<(String, String)>,
) -> Result<Json<Status>, ApiError> {
    Ok(Json(on_the_side(move || status(&engine, &run, &path)).await?))
}

/// What writing an overview asks for: the model the estimate was shown for and its most.
#[derive(Deserialize)]
struct Writing {
    provider: Option<Provider>,
    model: String,
    accepted_usd: Option<f64>,
    accepted_tokens: Option<u64>,
}

async fn write_overview(
    State(engine): State<Shared>,
    Path((run, path)): Path<(String, String)>,
    body: Result<Json<Writing>, JsonRejection>,
) -> Result<Json<Status>, ApiError> {
    let Json(request) = body?;
    let told = on_the_side(move || {
        let now = status(&engine, &run, &path)?;
        // kept, or being written (for this model or the one chosen when it began): nothing more is started
        if matches!(now.state, Phase::Done | Phase::Running) {
            return Ok(now);
        }
        let library = engine.library()?;
        let chosen = engine.chosen()?;
        chosen.as_shown(request.provider, &request.model).map_err(invalid)?;
        let endpoint = chosen.endpoint().map_err(invalid)?;
        let light = light(&engine, &run, &path)?;
        let (prepared, _) = prepared(&library, &chosen, &run, &path, &light)?;
        // no point can be shown with quotes of fewer papers: nothing is paid for that could show nothing
        if prepared.given.len() < overview::MIN_CITED {
            return Err(ApiError::Invalid(format!(
                "an overview needs at least {} papers the library has; this community has {}",
                overview::MIN_CITED,
                prepared.given.len()
            )));
        }
        let estimate = prepared.estimate(&library)?;
        let accepted = Accepted::of(request.accepted_usd, request.accepted_tokens).map_err(invalid)?;
        let budget = budget(&estimate.cost, accepted, Terms::AsShown).map_err(invalid)?;
        let transport = Arc::clone(&engine.transport);
        let worker = Arc::clone(&library);
        let subject = prepared.subject.clone();
        engine.job.start_tracked(overview::JOB, move |tracker| {
            let asking =
                Asking { library: &worker, transport: &*transport, endpoint, recorder: &*worker, in_flight: IN_FLIGHT };
            match overview::ask(asking, &prepared, &budget, tracker) {
                Ok(_) => {
                    tracker.log("Written; its quotes are checked whenever it is shown.");
                    Ok(Finished::Feature { feature: Feature::Overview, subject })
                }
                Err(NotWritten::Library(error)) => {
                    eprintln!("overview: {error}");
                    Err("the engine could not read or write its files".to_owned())
                }
                Err(why) => Err(why.to_string()),
            }
        })?;
        // told as running at once: the job's first log line, which says whose job it is, may not be written yet
        Ok(Status { state: Phase::Running, overview: None, error: None, estimate: None, ..now })
    })
    .await?;
    Ok(Json(told))
}

// ------------------------------------------------------------------ closest papers

/// `ClosestPapers` in the contracts.
#[derive(Serialize)]
pub struct ClosestStatus {
    pub state: Phase,
    /// Every statement with its papers, once all are found (`done`).
    pub statements: Option<Vec<closest::Shown>>,
    pub error: Option<String>,
    /// Why they cannot be found on this computer now (the embedding model is not here, say): nothing is started.
    pub unavailable: Option<String>,
    /// The embedding model they are (or would be) found with.
    pub model: String,
    /// No paper below this cosine similarity is shown.
    pub floor: f32,
}

/// `ClosestStarted` in the contracts: the job's number, or none when nothing needed finding.
#[derive(Serialize)]
pub struct ClosestStarted {
    pub ok: bool,
    pub id: Option<u64>,
}

/// A community of a made run, as finding its closest papers needs it.
struct Described {
    /// Its papers' ids, in its order.
    members: Vec<String>,
    /// The run's embedding model.
    model: String,
    statements: Vec<Statement>,
    subject: Subject,
}

fn described(engine: &Shared, run: &str, path: &str) -> Result<Described, ApiError> {
    let path = tree_path(path).ok_or(ApiError::NotFound)?;
    let (_, file, tree) = run_files(engine, run)?;
    let community = tree.find(path).ok_or(ApiError::NotFound)?;
    Ok(Described {
        members: community.members.iter().filter_map(|&index| file.paper_ids.get(index)).cloned().collect(),
        model: file.settings.embedding_model,
        statements: closest::statements_of(community),
        subject: Subject::Community { run: run.to_owned(), community: path.to_owned() },
    })
}

/// Why `model` cannot embed on this computer now, in words for the interface; `None` when it can.
fn unavailable(engine: &Shared, model: &str) -> Option<String> {
    if model.is_empty() {
        return Some("This run does not say which embedding model embedded its papers.".to_owned());
    }
    if model != engine.runs.embedding.model() {
        return Some(format!("This run's papers were embedded with {model}, which this version cannot run."));
    }
    model_files::present(&QWEN3, &engine.data).is_none().then(|| {
        "The embedding model is not on this computer: it is downloaded the first time a run is made here.".to_owned()
    })
}

fn closest_status(engine: &Shared, run: &str, path: &str) -> Result<ClosestStatus, ApiError> {
    let library = engine.library()?;
    let about = described(engine, run, path)?;
    let ids: Vec<&str> = about.members.iter().map(String::as_str).collect();
    let kept = closest::kept_of(&library, &about.model, &ids, &about.statements)?;
    let told = |state: Phase| ClosestStatus {
        state,
        statements: None,
        error: None,
        unavailable: None,
        model: about.model.clone(),
        floor: closest::FLOOR,
    };
    if kept.iter().all(Option::is_some) {
        // only the papers shown are read
        let mut shown_ids: Vec<String> =
            kept.iter().flatten().flat_map(|kept| kept.papers.iter().map(|found| found.paper.clone())).collect();
        shown_ids.sort_unstable();
        shown_ids.dedup();
        let records = records_of(&library, &shown_ids)?;
        let statements = about
            .statements
            .iter()
            .zip(kept.iter().flatten())
            .map(|(statement, kept)| closest::shown(statement, kept, &records))
            .collect();
        return Ok(ClosestStatus { statements: Some(statements), ..told(Phase::Done) });
    }
    let job = engine.job.snapshot(usize::MAX);
    let ours = job.about.as_ref().is_some_and(|job| job.feature == Feature::Closest && job.subject == about.subject);
    Ok(match (job.state, ours) {
        (Phase::Running, true) => told(Phase::Running),
        (Phase::Failed, true) => ClosestStatus { error: job.error, ..told(Phase::Failed) },
        _ => ClosestStatus { unavailable: unavailable(engine, &about.model), ..told(Phase::Idle) },
    })
}

async fn closest_papers(
    State(engine): State<Shared>,
    Path((run, path)): Path<(String, String)>,
) -> Result<Json<ClosestStatus>, ApiError> {
    Ok(Json(on_the_side(move || closest_status(&engine, &run, &path)).await?))
}

async fn find_closest(
    State(engine): State<Shared>,
    Path((run, path)): Path<(String, String)>,
) -> Result<Json<ClosestStarted>, ApiError> {
    let told = on_the_side(move || {
        let now = closest_status(&engine, &run, &path)?;
        match now.state {
            Phase::Done => return Ok(ClosestStarted { ok: true, id: None }),
            Phase::Running => return Ok(ClosestStarted { ok: true, id: Some(engine.job.snapshot(usize::MAX).id) }),
            Phase::Idle | Phase::Failed => {}
        }
        if let Some(why) = now.unavailable.or_else(|| unavailable(&engine, &now.model)) {
            return Err(ApiError::Invalid(why));
        }
        let folder = model_files::present(&QWEN3, &engine.data)
            .ok_or_else(|| ApiError::Invalid("The embedding model is not on this computer.".to_owned()))?;
        engine.runs.embedding.use_folder(folder);
        let library = engine.library()?;
        let Described { members, model, statements, subject } = described(&engine, &run, &path)?;
        let embedding = Arc::clone(&engine.runs.embedding);
        let about = About { feature: Feature::Closest, subject: subject.clone(), item: None };
        let id = engine.job.start_about(closest::JOB, about, move |tracker| {
            let started = std::time::Instant::now();
            tracker.log(&format!(
                "Finding the papers of {path} of {run} closest to its description, with {model} on this computer: \
                 nothing is sent anywhere."
            ));
            let failed = |error: &dyn std::fmt::Display| {
                eprintln!("closest papers: {error}");
                "the engine could not read the community's papers or their embeddings".to_owned()
            };
            let ids: Vec<&str> = members.iter().map(String::as_str).collect();
            let kept = closest::kept_of(&library, &model, &ids, &statements).map_err(|error| failed(&error))?;
            let todo: Vec<Statement> =
                statements.iter().zip(&kept).filter(|(_, kept)| kept.is_none()).map(|(statement, _)| statement.clone()).collect();
            let papers: Vec<Paper> = library.papers(&ids).map_err(|error| failed(&error))?.into_iter().flatten().collect();
            let vectors = library.vectors(&model, &papers).map_err(|error| {
                eprintln!("closest papers: {error}");
                format!("some of this community's papers have no embedding by {model}: make the run again (nothing is redone)")
            })?;
            let rows: Vec<Vec<f32>> = vectors.outer_iter().map(|row| row.to_vec()).collect();
            let asked = closest::Asked { model: &model, statements: &todo, members: &papers, vectors: &rows };
            closest::find(embedding.as_ref(), &asked, tracker, &mut |_, found| {
                closest::keep(&library, &subject, &model, &ids, &found)
            })?;
            tracker.log(&format!(
                "Found the closest papers of {} statements among {} papers in {:.1} s.",
                todo.len(),
                papers.len(),
                started.elapsed().as_secs_f64()
            ));
            Ok(Finished::Feature { feature: Feature::Closest, subject })
        })?;
        Ok(ClosestStarted { ok: true, id: Some(id) })
    })
    .await?;
    Ok(Json(told))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::Path as FilePath;
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    use axum::Router;
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use crate::api::guard::config::tests::STRONG;
    use crate::api::guard::config::{Environment, Listen};
    use crate::api::other_providers::{
        ACCEPTED, Chosen as Picked, LOCAL, Scripted, accepting, call, choices, choose, finished, priced_for, sent_to,
    };
    use crate::api::routes::Engine;
    use crate::api::tests::served;
    use crate::features::writing::describe::tests::{answering, described_before_sections};
    use crate::library::Subject;
    use crate::library::papers::tests::paper;
    use crate::library::{Known, Library};
    use crate::model::language::Effort;
    use crate::model::language::{ApiKey, KeyHeader, Providers, Reply, Transport, TransportError};
    use crate::pipeline::steps::model_files::QWEN3;

    const AT: &str = "127.0.0.1:8765";

    /// The fields of `name` in the contracts, its parents' included: what a route's answer must have, no more.
    fn contract(name: &str) -> Vec<String> {
        let source = include_str!("../../../../../packages/contracts/src/index.ts");
        let start = source.find(&format!("export interface {name} ")).unwrap_or_else(|| panic!("no {name}"));
        let open = start + source[start..].find('{').unwrap();
        let mut fields = Vec::new();
        if let Some(parents) = source[start..open].split(" extends ").nth(1) {
            for parent in parents.split(',') {
                fields.extend(contract(parent.trim()));
            }
        }
        for line in source[open + 1..].lines() {
            if line.starts_with('}') {
                break;
            }
            let Some(rest) = line.strip_prefix('\t') else { continue };
            if rest.starts_with(['\t', '/', '*', ' ', '|']) {
                continue;
            }
            let field = rest.split([':', '?']).next().unwrap().trim();
            if !field.is_empty() {
                fields.push(field.to_owned());
            }
        }
        fields.sort();
        fields
    }

    /// `value`'s fields are `name`'s in the contracts, exactly.
    fn matches(value: &Value, name: &str) {
        let mut keys: Vec<String> =
            value.as_object().unwrap_or_else(|| panic!("{name}: {value}")).keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, contract(name), "{name}: {value}");
    }

    /// What every paper says, so that a quote of it can be found.
    fn said_by(index: usize) -> String {
        format!("Paper {index} reports that the balanced router improves accuracy on every benchmark.")
    }

    /// The model, as the fake provider plays it: an overview quoting the first three papers of its prompt, or names.
    fn model(body: &str) -> String {
        if !body.contains("Write the overview") {
            return answering(body);
        }
        let papers: Vec<usize> = (0..6).filter(|index| body.contains(&format!("[paper p{index}]"))).collect();
        let quotes: Vec<Value> = papers
            .iter()
            .take(3)
            .map(|index| json!({"paper": format!("p{index}"), "quote": format!("Paper {index} reports that the balanced router improves accuracy")}))
            .collect();
        let point = json!({"title": "A balanced router improves accuracy", "insight": "Three papers report it.", "quotes": quotes});
        let thin = json!({"title": "One paper only", "insight": "Not a pattern.", "quotes": [quotes[0].clone()]});
        json!({
            "title": "Routers", "summary": {"text": "The papers balance routers.", "quotes": quotes},
            "common_methods": [point, thin], "recurring_findings": [], "promising_directions": [],
            "structure_patterns": [], "open_questions": []
        })
        .to_string()
    }

    /// A data folder with six papers in the library and a run "r1" of them: c1 (papers 0-2, cut into c1.0 and
    /// c1.1) and c10 (papers 3-5).
    fn data() -> tempfile::TempDir {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(&data.path().join("library")).unwrap();
        let papers: Vec<_> = (0..6)
            .map(|index| {
                let mut made = paper(&format!("p{index}"));
                made.title = format!("Title {index}");
                made.r#abstract = said_by(index);
                made
            })
            .collect();
        library.add_papers(&papers, None, Known::Keep).unwrap();
        let run = data.path().join("runs").join("r1");
        std::fs::create_dir_all(&run).unwrap();
        let ids: Vec<String> = (0..6).map(|index| format!("p{index}")).collect();
        let file = json!({"selection": {}, "settings": {"embedding_model": "not-embedded"}, "paper_ids": ids});
        std::fs::write(run.join("run.json"), file.to_string()).unwrap();
        let community = |path: &str, members: Vec<usize>, children: Vec<Value>| {
            json!({"id": 0, "tree_path": path, "size": members.len(), "name": null, "description": null,
                   "keywords": [format!("t{path}")], "members": members, "children": children, "nearest_to_centroid": []})
        };
        let tree = json!({"name": "r1", "papers": 6, "modularity": 0.1, "communities": [
            community("c1", vec![0, 1, 2], vec![community("c1.0", vec![0, 1], vec![]), community("c1.1", vec![2], vec![])]),
            community("c10", vec![3, 4, 5], vec![]),
        ]});
        std::fs::write(run.join("communities.json"), tree.to_string()).unwrap();
        let nodes: Vec<Value> =
            (0..6).map(|index| json!({"id": format!("p{index}"), "title": format!("Title {index}")})).collect();
        let graph = json!({"nodes": nodes, "edges": [[0, 1, 0.9], [1, 2, 0.8], [2, 3, 0.5], [3, 4, 0.7]]});
        std::fs::write(run.join("graph.json"), graph.to_string()).unwrap();
        data
    }

    /// The routes of an engine on `data` choosing `picked`, every request to a provider answered by `transport`.
    fn router(data: &FilePath, picked: Picked, transport: Arc<dyn Transport>) -> Router {
        choose(data, picked);
        let mut engine = Engine::new(data.to_path_buf(), choices());
        engine.providers = Providers::from_lookup(|name| match name {
            "OPENAI_API_KEY" => Some("test-key-not-real".to_owned()),
            "CONSTELLATE_MODEL_BASE_URL" => Some(LOCAL.to_owned()),
            _ => None,
        });
        engine.transport = transport;
        let pairs = [("CONSTELLATE_TOKEN", STRONG), ("CONSTELLATE_ORIGIN", "http://127.0.0.1:3000")];
        served(&Listen::from_environment(&Environment::from_pairs(pairs)).unwrap(), AT, engine).0
    }

    async fn settled(router: &Router, path: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let (_, now) = call(router, "GET", path, None).await;
            if now["state"] != "running" {
                return now;
            }
            assert!(Instant::now() < deadline, "the overview was never written: {now}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[test]
    fn the_contracts_are_read_as_they_are_written() {
        assert_eq!(contract("OverviewCounts"), ["malformed", "not_found", "shown", "summary_left_out", "written"]);
        assert!(contract("BatchCost").contains(&"most_usd".to_owned()), "a parent's fields too");
    }

    #[tokio::test]
    async fn an_overview_is_looked_up_by_exact_path() {
        let data = data();
        let router = router(data.path(), Picked::OpenAi, Scripted::new(model));
        for path in ["/api/runs/r1/overview/c10", "/api/runs/r1/overview/c1.0"] {
            let (status, told) = call(&router, "GET", path, None).await;
            assert_eq!((status, told["state"].clone()), (StatusCode::OK, json!("idle")), "{told}");
        }
        for path in [
            "/api/runs/r1/overview/c100",
            "/api/runs/r1/overview/C1",
            "/api/runs/none/overview/c1",
            "/api/runs/..%2Fr1/overview/c1",
            "/api/runs/r1/overview/..%2F..%2Fsettings.json",
        ] {
            assert_eq!(call(&router, "GET", path, None).await.0, StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[tokio::test]
    async fn an_overview_costs_what_was_shown_is_written_once_with_its_evidence_and_is_the_models_own() {
        let data = data();
        let transport = Scripted::new(model);
        let router = router(data.path(), Picked::OpenAi, transport.clone());
        let at = "/api/runs/r1/overview/c1";
        let (_, idle) = call(&router, "GET", at, None).await;
        matches(&idle, "OverviewStatus");
        matches(&idle["estimate"], "BatchCost");
        priced_for(&idle["estimate"], Picked::OpenAi);
        assert_eq!((idle["state"].clone(), idle["api_key_set"].clone()), (json!("idle"), json!(true)));
        // nothing is sent without the cost accepted, or for another model than the one shown
        assert_eq!(call(&router, "POST", at, None).await.0, StatusCode::BAD_REQUEST);
        let mut other = accepting(&idle["estimate"], ACCEPTED);
        other["model"] = json!("gpt-something-else");
        assert_eq!(call(&router, "POST", at, Some(other)).await.0, StatusCode::BAD_REQUEST);
        let mut less = accepting(&idle["estimate"], ACCEPTED);
        less["accepted_usd"] = json!(idle["estimate"]["most_usd"].as_f64().unwrap() / 3.0);
        assert_eq!(call(&router, "POST", at, Some(less)).await.0, StatusCode::BAD_REQUEST);
        assert!(transport.sent().is_empty());

        let (status, started) = call(&router, "POST", at, Some(accepting(&idle["estimate"], ACCEPTED))).await;
        assert_eq!((status, started["state"].clone()), (StatusCode::OK, json!("running")), "{started}");
        let done = settled(&router, at).await;
        assert_eq!(done["state"], "done", "{done}");
        matches(&done, "OverviewStatus");
        let overview = &done["overview"];
        matches(overview, "Overview");
        matches(&overview["counts"], "OverviewCounts");
        assert_eq!(
            overview["counts"],
            json!({"written": 2, "shown": 1, "not_found": 1, "malformed": 0, "summary_left_out": false})
        );
        assert_eq!((overview["papers"].clone(), overview["papers_given"].clone()), (json!(3), json!(3)));
        let point = &overview["sections"][0]["points"][0];
        matches(point, "OverviewPoint");
        assert_eq!(point["papers"].as_array().unwrap().len(), 3);
        assert_eq!(point["evidence"][0]["established"]["kind"], "abstract");
        assert_eq!(point["evidence"][0]["quote"], "Paper 0 reports that the balanced router improves accuracy");
        assert_eq!(done["estimate"], Value::Null);
        sent_to(&transport.sent(), Picked::OpenAi);
        let prompt = transport.sent()[0].body.to_string();
        assert!(prompt.contains("BEGIN PAPERS") && prompt.contains("[paper p2]") && !prompt.contains("[paper p3]"));

        // asking again costs nothing: it is kept
        let (_, again) = call(&router, "POST", at, Some(accepting(&idle["estimate"], ACCEPTED))).await;
        assert_eq!(again["state"], "done");
        assert_eq!(transport.sent().len(), 1);

        // another provider's model: not this overview, until it writes its own
        choose(data.path(), Picked::ThisComputer);
        let (_, fresh) = call(&router, "GET", at, None).await;
        assert_eq!((fresh["state"].clone(), fresh["overview"].clone()), (json!("idle"), Value::Null), "{fresh}");
        priced_for(&fresh["estimate"], Picked::ThisComputer);
        call(&router, "POST", at, Some(accepting(&fresh["estimate"], ACCEPTED))).await;
        let done = settled(&router, at).await;
        assert_eq!((done["state"].clone(), done["provider"].clone()), (json!("done"), json!("compatible")), "{done}");
        assert_eq!(transport.sent().len(), 2);
        sent_to(&transport.sent()[1..], Picked::ThisComputer);
        // and the first is still kept for the first
        choose(data.path(), Picked::OpenAi);
        assert_eq!(call(&router, "GET", at, None).await.1["state"], "done");
    }

    /// A provider that answers only once let go.
    struct Held {
        inner: Arc<Scripted>,
        open: Mutex<bool>,
        opened: Condvar,
    }

    impl Held {
        fn release(&self) {
            *self.open.lock().unwrap() = true;
            self.opened.notify_all();
        }

        fn wait(&self) {
            let mut open = self.open.lock().unwrap();
            while !*open {
                open = self.opened.wait(open).unwrap();
            }
        }
    }

    impl Transport for Held {
        fn post(
            &self,
            url: &str,
            key: &ApiKey,
            headers: &[(&str, &str)],
            body: &[u8],
            patience: Duration,
        ) -> Result<Reply, TransportError> {
            self.wait();
            self.inner.post(url, key, headers, body, patience)
        }

        fn post_keyed(
            &self,
            url: &str,
            key: &ApiKey,
            sent_as: KeyHeader,
            headers: &[(&str, &str)],
            body: &[u8],
            patience: Duration,
        ) -> Result<Reply, TransportError> {
            self.wait();
            self.inner.post_keyed(url, key, sent_as, headers, body, patience)
        }

        fn wait(&self, _duration: Duration) {}
    }

    #[tokio::test]
    async fn changing_the_model_while_an_overview_is_written_starts_no_second_one() {
        let data = data();
        let held = Arc::new(Held { inner: Scripted::new(model), open: Mutex::new(false), opened: Condvar::new() });
        let router = router(data.path(), Picked::OpenAi, held.clone());
        let at = "/api/runs/r1/overview/c1";
        let (_, idle) = call(&router, "GET", at, None).await;
        let (_, started) = call(&router, "POST", at, Some(accepting(&idle["estimate"], ACCEPTED))).await;
        assert_eq!(started["state"], "running");
        choose(data.path(), Picked::ThisComputer);
        // the first one's, still going: shown as running, and nothing else starts
        let deadline = Instant::now() + Duration::from_secs(10);
        while call(&router, "GET", at, None).await.1["state"] != "running" {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let (_, fresh) = call(&router, "GET", at, None).await;
        assert_eq!(fresh["estimate"], Value::Null);
        let shown = json!({"provider": "compatible", "model": idle["model"], "accepted_tokens": 1_000_000});
        let (status, second) = call(&router, "POST", at, Some(shown)).await;
        assert_eq!((status, second["state"].clone()), (StatusCode::OK, json!("running")), "{second}");
        held.release();
        settled(&router, at).await;
        assert_eq!(held.inner.sent().len(), 1, "one overview was paid for");
    }

    #[tokio::test]
    async fn a_run_is_named_at_the_cost_shown_and_named_again_only_when_asked() {
        let data = data();
        let transport = Scripted::new(model);
        let router = router(data.path(), Picked::OpenAi, transport.clone());
        let (status, before) = call(&router, "GET", "/api/describe/estimate", None).await;
        assert_eq!(status, StatusCode::OK, "{before}");
        matches(&before, "DescribeBeforeRun");
        matches(&before["estimate"], "AskingCost");
        priced_for(&before["estimate"], Picked::OpenAi);
        assert_eq!(
            (before["communities_at_most"].clone(), before["calls_at_most"].clone()),
            (json!(24 * 7), json!(24 * 4))
        );

        let at = "/api/runs/r1/describe";
        let (_, shown) = call(&router, "GET", &format!("{at}/estimate"), None).await;
        matches(&shown, "DescribeEstimate");
        matches(&shown["estimate"], "BatchCost");
        assert_eq!((shown["communities"].clone(), shown["named"].clone()), (json!(4), json!(0)));
        assert_eq!((shown["estimate"]["to_ask"].clone(), shown["estimate"]["kept"].clone()), (json!(3), json!(0)));
        // nothing sent for another model, or below the most shown
        let mut other = accepting(&shown["estimate"], ACCEPTED);
        other["model"] = json!("gpt-something-else");
        assert_eq!(call(&router, "POST", at, Some(other)).await.0, StatusCode::BAD_REQUEST);
        assert!(transport.sent().is_empty());

        // the run is open, so kept in memory: what it is served as must follow its file once it is named
        let (_, opened) = call(&router, "GET", "/api/runs/r1", None).await;
        assert_eq!(opened["communities"][0]["name"], Value::Null);
        let (status, job) = call(&router, "POST", at, Some(accepting(&shown["estimate"], ACCEPTED))).await;
        assert_eq!(status, StatusCode::OK, "{job}");
        matches(&job, "DescribeStarted");
        let ended = finished(&router, &job["id"]).await;
        assert_eq!(ended["state"], "done", "{ended}");
        let (_, opened) = call(&router, "GET", "/api/runs/r1", None).await;
        assert_eq!(opened["communities"][0]["name"], "Topic tc1", "served as named, not as it was in memory");
        assert_eq!(ended["outcome"], json!({"kind": "run", "run": "r1"}));
        assert_eq!(transport.sent().len(), 3);
        sent_to(&transport.sent(), Picked::OpenAi);
        let file: Value =
            serde_json::from_slice(&std::fs::read(data.path().join("runs/r1/communities.json")).unwrap()).unwrap();
        assert_eq!(file["communities"][0]["name"], "Topic tc1");
        assert_eq!(file["communities"][0]["children"][1]["name"], "Topic tc1.1");
        let c10 = &file["communities"][1];
        assert_eq!(c10["description"], "Papers on tc10. They share one method.", "the shown sections' texts");
        assert_eq!(c10["sections"][0]["heading"], "Problem");
        assert_eq!(c10["sections"][1]["heading"], "Shared methods");
        matches(&c10["sections"][0], "CommunitySection");
        let cite = &c10["sections"][0]["cites"][0];
        matches(cite, "CommunityCite");
        matches(&cite["evidence"], "Evidence");
        assert_eq!(cite["title"], "Title 3");
        assert_eq!(cite["evidence"]["paper"], "p3");
        assert_eq!(cite["evidence"]["established"]["kind"], "abstract");
        assert_eq!(c10["sections_left_out"], 1, "what sets it apart cited one paper");
        assert_eq!(file["modularity"], 0.1, "the rest of the file as it was");

        let (_, now) = call(&router, "GET", &format!("{at}/estimate"), None).await;
        assert_eq!((now["named"].clone(), now["estimate"]["to_ask"].clone()), (json!(4), json!(0)));
        assert_eq!(now["plain"], 0, "described with sections");
        let (_, sections) = call(&router, "GET", &format!("{at}/estimate?add_sections=true"), None).await;
        assert_eq!(sections["estimate"]["to_ask"], 0, "nothing described plainly: nothing to rewrite");
        let (_, again) = call(&router, "GET", &format!("{at}/estimate?redo=true"), None).await;
        assert_eq!(again["estimate"]["to_ask"], 3);
        let mut redo = accepting(&again["estimate"], ACCEPTED);
        redo["redo"] = json!(true);
        let (_, job) = call(&router, "POST", at, Some(redo)).await;
        assert_eq!(finished(&router, &job["id"]).await["state"], "done");
        assert_eq!(transport.sent().len(), 6, "described all again, as asked");
    }

    #[tokio::test]
    async fn a_run_described_before_sections_is_rewritten_only_when_asked_and_at_the_cost_shown() {
        use crate::features::writing::describe::{self, Again, Group, Run};
        use crate::features::writing::read_tree;
        use crate::library::Paper;
        use crate::model::chosen::Chosen;
        use crate::model::language::Provider;
        let data = data();
        let transport = Scripted::new(model);
        let router = router(data.path(), Picked::OpenAi, transport.clone());
        let at = "/api/runs/r1/describe";
        // described by the plain version: its answers kept, its names and descriptions in the run's files
        let (_, shown) = call(&router, "GET", &format!("{at}/estimate"), None).await;
        let chosen = Chosen::of(
            Provider::OpenAi,
            shown["model"].as_str().unwrap(),
            Effort::Medium,
            Err(crate::model::language::Unavailable::NoKey),
        );
        let library = Library::open(&data.path().join("library")).unwrap();
        let folder = data.path().join("runs/r1");
        let tree = read_tree(&folder).unwrap().unwrap();
        let groups = Group::from_stored(&tree.communities);
        let ids: Vec<String> = (0..6).map(|index| format!("p{index}")).collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let papers: Vec<Paper> = library.papers(&ids).unwrap().into_iter().map(Option::unwrap).collect();
        let run = Run {
            name: "r1",
            folder: Some(&folder),
            papers: &papers,
            embeddings: None,
            communities: &groups,
            again: Again::Nothing,
        };
        let names = described_before_sections(&library, &run, &chosen);
        describe::write_names(&folder, &names).unwrap();
        drop(library);

        let (_, plain) = call(&router, "GET", &format!("{at}/estimate"), None).await;
        assert_eq!((plain["named"].clone(), plain["plain"].clone()), (json!(4), json!(4)));
        assert_eq!(plain["estimate"]["to_ask"], 0, "an ordinary naming asks nothing of them");
        let (_, job) = call(&router, "POST", at, Some(accepting(&plain["estimate"], ACCEPTED))).await;
        assert_eq!(finished(&router, &job["id"]).await["state"], "done");
        assert!(transport.sent().is_empty());

        // the rewrite: its own estimate, and nothing sent without its cost accepted
        let (_, rewrite) = call(&router, "GET", &format!("{at}/estimate?add_sections=true"), None).await;
        matches(&rewrite, "DescribeEstimate");
        assert_eq!((rewrite["estimate"]["to_ask"].clone(), rewrite["estimate"]["kept"].clone()), (json!(3), json!(0)));
        priced_for(&rewrite["estimate"], Picked::OpenAi);
        let unaccepted = json!({"provider": "openai", "model": rewrite["model"], "add_sections": true});
        assert_eq!(call(&router, "POST", at, Some(unaccepted)).await.0, StatusCode::BAD_REQUEST);
        // the ordinary estimate's most (nothing) does not pay for the rewrite
        let mut too_little = accepting(&plain["estimate"], ACCEPTED);
        too_little["add_sections"] = json!(true);
        assert_eq!(call(&router, "POST", at, Some(too_little)).await.0, StatusCode::BAD_REQUEST);
        assert!(transport.sent().is_empty());
        let mut accepted = accepting(&rewrite["estimate"], ACCEPTED);
        accepted["add_sections"] = json!(true);
        let (status, job) = call(&router, "POST", at, Some(accepted)).await;
        assert_eq!(status, StatusCode::OK, "{job}");
        assert_eq!(finished(&router, &job["id"]).await["state"], "done");
        assert_eq!(transport.sent().len(), 3);
        let file: Value = serde_json::from_slice(&std::fs::read(folder.join("communities.json")).unwrap()).unwrap();
        assert_eq!(file["communities"][1]["name"], "Topic tc10");
        assert_eq!(file["communities"][1]["sections"][0]["heading"], "Problem");
        let (_, after) = call(&router, "GET", &format!("{at}/estimate?add_sections=true"), None).await;
        assert_eq!((after["plain"].clone(), after["estimate"]["to_ask"].clone()), (json!(0), json!(0)));
    }

    #[tokio::test]
    async fn the_closest_papers_cost_nothing_say_when_the_model_is_not_here_and_are_shown_once_kept() {
        use crate::features::writing::closest::{self, Kept, KeptPaper};
        let data = data();
        // the run's description, plain: two sentences
        let file = data.path().join("runs/r1/communities.json");
        let mut tree: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        tree["communities"][1]["name"] = json!("Routers");
        tree["communities"][1]["description"] = json!("Papers balance routers. They report accuracy.");
        std::fs::write(&file, tree.to_string()).unwrap();
        let run_file = data.path().join("runs/r1/run.json");
        let mut run: Value = serde_json::from_slice(&std::fs::read(&run_file).unwrap()).unwrap();
        run["settings"]["embedding_model"] = json!(QWEN3.model);
        std::fs::write(&run_file, run.to_string()).unwrap();
        let transport = Scripted::new(model);
        let router = router(data.path(), Picked::OpenAi, transport.clone());
        let at = "/api/runs/r1/closest/c10";
        let (status, idle) = call(&router, "GET", at, None).await;
        assert_eq!(status, StatusCode::OK, "{idle}");
        matches(&idle, "ClosestPapers");
        assert_eq!(idle["state"], "idle");
        assert_eq!(idle["model"], QWEN3.model);
        assert!(
            idle["unavailable"].as_str().unwrap().starts_with("The embedding model is not on this computer"),
            "{idle}"
        );
        // asked anyway: refused with the same reason, and nothing started
        let (status, refused) = call(&router, "POST", at, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(refused["error"].as_str().unwrap().contains("not on this computer"), "{refused}");
        assert!(transport.sent().is_empty(), "no language model is asked");

        // found before: shown at once, the sentence located in the paper, without the model
        let library = Library::open(&data.path().join("library")).unwrap();
        let members = ["p3", "p4", "p5"];
        let subject = Subject::Community { run: "r1".into(), community: "c10".into() };
        for (statement, papers) in
            [("Papers balance routers.", vec![("p4", 0.612_f32)]), ("They report accuracy.", vec![])]
        {
            let kept = Kept {
                statement: statement.into(),
                papers: papers
                    .into_iter()
                    .map(|(paper, cosine)| KeptPaper { paper: paper.into(), cosine, sentence: said_by(4) })
                    .collect(),
            };
            closest::keep(&library, &subject, QWEN3.model, &members, &kept).unwrap();
        }
        drop(library);
        let (_, done) = call(&router, "GET", at, None).await;
        matches(&done, "ClosestPapers");
        assert_eq!(done["state"], "done", "{done}");
        let first = &done["statements"][0];
        matches(first, "ClosestStatement");
        assert_eq!(first["statement"], "Papers balance routers.");
        assert_eq!(first["section"], Value::Null);
        matches(&first["papers"][0], "ClosestPaper");
        assert_eq!(first["papers"][0]["id"], "p4");
        assert_eq!(first["papers"][0]["title"], "Title 4");
        assert_eq!(first["papers"][0]["evidence"]["quote"], said_by(4).trim_end_matches('.'));
        assert_eq!(first["papers"][0]["evidence"]["established"]["kind"], "abstract");
        assert_eq!(done["statements"][1]["papers"], json!([]), "nothing close: none, and said so");
        let (_, again) = call(&router, "POST", at, None).await;
        assert_eq!(again, json!({"ok": true, "id": null}), "kept: nothing to start");
        // the description changes: its new sentence is not found yet
        let mut tree: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        tree["communities"][1]["description"] = json!("Papers balance routers. A new sentence now.");
        std::fs::write(&file, tree.to_string()).unwrap();
        assert_eq!(call(&router, "GET", at, None).await.1["state"], "idle");
        assert_eq!(call(&router, "GET", "/api/runs/r1/closest/c100", None).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_run_whose_tree_repeats_a_path_is_refused_with_what_to_do() {
        let data = data();
        let router = router(data.path(), Picked::OpenAi, Scripted::new(model));
        let old = json!({"communities": [
            {"tree_path": "c1", "members": [0], "children": [{"tree_path": "c10", "members": [0]}]},
            {"tree_path": "c10", "members": [1]}]});
        std::fs::write(data.path().join("runs/r1/communities.json"), old.to_string()).unwrap();
        let (status, told) = call(&router, "GET", "/api/runs/r1/overview/c1", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(told["error"].as_str().unwrap().contains("Run it again"), "{told}");
        assert_eq!(call(&router, "GET", "/api/runs/r1/describe/estimate", None).await.0, StatusCode::BAD_REQUEST);
    }
}
