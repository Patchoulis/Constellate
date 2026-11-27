//! Making a run, keeping it, and serving it: the heart of the app.
//!
//! A selection (conferences and years, or arXiv categories and dates) is gathered into papers (listings,
//! acceptance decisions, abstracts) kept in the library; each paper is embedded (once per paper, text and model);
//! the embeddings make a similarity graph, cut into communities and sub-communities; each community is summarised
//! (its terms, its nearest papers, its figures); and the run's files are written for the interface. The steps,
//! in order, are [`Step::ALL`]; what came of each is a [`StepOutcome`].
//!
//! # The seam with the other modules
//!
//! Two steps are other modules': scoring abstracts with the AI-text detector ([`Step::Detect`], [`crate::features::detector`])
//! and naming and describing the communities with the language model ([`Step::Describe`],
//! [`crate::features::writing::describe`]). The run calls them through [`seams::detect`] and [`seams::describe`]; [`seams`]
//! says what each is handed and must set. The model is asked only within what the person accepted with the run's
//! request ([`seams::Naming`]). Both steps are optional: a failure is told in the log and the run is made without
//! what it would have added.
//!
//! # A run is a job
//!
//! One at a time (the engine's one job, [`crate::jobs`]), with the step under way and its count on a
//! [`steps::Board`] the status route reads ("embedding 3,120 of 12,487 papers"). It can be stopped between
//! things (slices, batches of papers, megabytes of the model); everything done up to there is kept in the
//! library, so the same run started again goes on from there: nothing is fetched, embedded or read twice. Its
//! outcome is `Finished::Run` with the run's name.
//!
//! # Threads
//!
//! The graph, the clustering, the vocabulary and the records are computed on the engine's threads
//! ([`crate::threads`]); each result is gathered in a fixed order, so a run's files are the same byte for byte
//! whatever the number of threads (a test makes one run on one thread and on many).
//!
//! | module          | what lives there                                                       |
//! |-----------------|------------------------------------------------------------------------|
//! | [`steps`]       | the steps, their outcomes, the board of the step under way             |
//! | [`gather`]      | the selection fetched into the library, and its papers                 |
//! | [`model_files`] | the embedding model's files, downloaded once, checked, resumable       |
//! | [`embedding`]   | the papers the library has no vector for, embedded                     |
//! | [`filter`]      | what the settings leave out                                            |
//! | [`vocabulary`]  | the words of the papers, and the terms that set a group apart          |
//! | [`report`]      | the community records (`communities.json`) and the readable report     |
//! | [`drawing`]     | the papers and links of the graph view (`graph.json`)                  |
//! | [`files`]       | a run's folder and files, as the Python laid them out                  |
//! | [`served`]      | the runs: listed, opened, renamed, deleted                             |
//! | [`seams`]       | the detector's and the describer's steps, connected by the integrator  |

pub mod agreement;
pub mod drawing;
pub mod embedding;
pub mod files;
pub mod filter;
pub mod gather;
pub mod model_files;
pub mod report;
pub mod seams;
pub mod served;
pub mod steps;
pub mod vocabulary;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests;

use std::collections::BTreeSet;
use std::path::Path;
use std::str::FromStr;
use std::time::Instant;

use ndarray::{Array2, Axis};
use serde_json::json;

use crate::analysis::graph::cluster::{self, ClusterError, Limits, Method, Papers};
use crate::analysis::graph::eigen::EigenError;
use crate::analysis::graph::{self, GraphError, make_unit};
use crate::jobs::Tracker;
use crate::library::core::store::StoreError;
use crate::library::{Library, LibraryError, Paper};
use crate::selection::Selection;
use crate::settings::Settings;
use embedding::Embedding;
use gather::Sources;
use steps::{Board, thousands};
pub use steps::{Step, StepOutcome};

/// The version of how communities are cut, kept in every run's `run.json`: raised whenever the same papers and
/// settings would be cut otherwise, so that making a run again can say beforehand that its communities will
/// change ([`files::replacing`]). Runs the Python made have none: its k-means started from one random start where
/// the engine's keeps the best of several, so even with the same settings their communities are cut again.
/// Nothing remakes a run because this changed: a run is made again only when the person asks.
pub const CLUSTERING_VERSION: u32 = 1;

/// A step whose kept results a run can be told to ignore and produce again (`RedoStep` in the contracts).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RedoStep {
    Download,
    Embeddings,
    Descriptions,
}

impl RedoStep {
    pub const ALL: [RedoStep; 3] = [RedoStep::Download, RedoStep::Embeddings, RedoStep::Descriptions];

    pub fn id(self) -> &'static str {
        match self {
            RedoStep::Download => "download",
            RedoStep::Embeddings => "embeddings",
            RedoStep::Descriptions => "descriptions",
        }
    }
}

impl FromStr for RedoStep {
    type Err = String;

    fn from_str(id: &str) -> Result<Self, Self::Err> {
        RedoStep::ALL.into_iter().find(|step| step.id() == id).ok_or_else(|| {
            let steps: Vec<&str> = RedoStep::ALL.iter().map(|step| step.id()).collect();
            format!("cannot redo {id}; the steps are {}", steps.join(", "))
        })
    }
}

/// The steps a run is to do again despite what the library has.
pub type Redo = BTreeSet<RedoStep>;

/// What a run is asked to make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub selection: Selection,
    pub redo: Redo,
}

/// Why a run was not made.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// Too few papers, and the like: said for the person who asked.
    #[error("{0}")]
    Invalid(String),
    #[error("stopped while {}: everything done so far is kept, and starting the run again goes on from there", .0.title())]
    Cancelled(Step),
    #[error("{0}")]
    Sources(String),
    #[error("{0}")]
    Embed(String),
    #[error(transparent)]
    Library(#[from] LibraryError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error(transparent)]
    Cluster(#[from] ClusterError),
    #[error("the positions of the papers could not be worked out: {0}")]
    Eigen(#[from] EigenError),
}

/// Everything a run knows once its papers are embedded. Each step reads what earlier steps filled in and adds its
/// own: [`seams::detect`] sets `flagged`, [`report::community_tree`] makes `records`, [`seams::describe`] names
/// them.
pub struct Made {
    pub name: String,
    pub selection: Selection,
    pub settings: Settings,
    pub redo: Redo,
    /// The run's papers, ordered by id, and one embedding per paper (row `i` is paper `i`).
    pub papers: Vec<Paper>,
    pub embeddings: Array2<f32>,
    /// Per paper: flagged by the AI-text detector.
    pub flagged: Vec<bool>,
    /// The tree of community records; empty until the communities are summarised.
    pub records: Vec<report::Record>,
}

/// What making a run needs from the engine.
pub struct Context<'a> {
    pub data: &'a Path,
    pub library: &'a Library,
    pub settings: Settings,
    pub sources: Sources<'a>,
    pub embedding: &'a dyn Embedding,
    pub board: &'a Board,
    /// How the language model's requests reach its provider (a test's fake reaches nothing).
    pub transport: &'a dyn crate::model::language::Transport,
    /// What the person accepted for naming the run's communities.
    pub naming: &'a seams::Naming,
}

/// Fewer papers than this cannot be cut into communities that mean anything.
fn enough(papers: usize, settings: &Settings) -> Result<(), RunError> {
    let least = 4 * settings.min_community_size as usize;
    if papers < least {
        return Err(RunError::Invalid(format!(
            "only {papers} papers: at least {least} are needed to look for communities"
        )));
    }
    Ok(())
}

/// Do one step: say it in the log and on the board, time it, and say what came of it.
fn step<T>(
    which: Step,
    tracker: &Tracker<'_>,
    board: &Board,
    work: impl FnOnce() -> Result<(T, StepOutcome), RunError>,
) -> Result<T, RunError> {
    tracker.log(&format!("[{}/{}] {}", which.number(), Step::ALL.len(), which.title()));
    board.begin(which);
    if tracker.cancelled() {
        return Err(RunError::Cancelled(which));
    }
    let started = Instant::now();
    let (value, outcome) = work()?;
    tracker.log(&format!("  {outcome}"));
    tracker.log(&format!("  took {:.1}s", started.elapsed().as_secs_f64()));
    Ok(value)
}

/// An optional step: its failure is told, and the run goes on.
fn optional(
    which: Step,
    tracker: &Tracker<'_>,
    board: &Board,
    work: impl FnOnce() -> StepOutcome,
) -> Result<(), RunError> {
    step(which, tracker, board, || {
        let outcome = work();
        // what came of it, on the board too: "skipped: the AI-text detector is not trained"
        board.detail(outcome.to_string());
        if let StepOutcome::Failed(why) = &outcome {
            tracker.log(&format!("  {} failed ({why}): the run goes on without it", which.title()));
        }
        Ok(((), outcome))
    })
}

/// Make the run `request` asks for; answers its name.
pub fn make(context: &Context<'_>, request: &Request, tracker: &Tracker<'_>) -> Result<String, RunError> {
    let board = context.board;
    let _clearing = steps::Clearing(board);
    let library = context.library;
    let settings = &context.settings;
    let name = request.selection.name();
    let model = settings.embedding_model.clone();

    let papers = step(Step::Gather, tracker, board, || {
        let (papers, outcome) = gather::gather(
            library,
            &context.sources,
            &request.selection,
            request.redo.contains(&RedoStep::Download),
            tracker,
            board,
        )?;
        enough(papers.len(), settings)?;
        Ok((papers, outcome))
    })?;
    let redo_embeddings = request.redo.contains(&RedoStep::Embeddings);
    let todo = embedding::missing(library, &model, &papers, redo_embeddings)?;
    step(Step::Model, tracker, board, || {
        let outcome = embedding::prepare(context.embedding, &model, context.data, todo.len(), tracker, board)?;
        Ok(((), outcome))
    })?;
    let embeddings = step(Step::Embed, tracker, board, || {
        embedding::embed(context.embedding, library, &model, &papers, &todo, tracker, board)
    })?;
    let count = papers.len();
    let mut made = Made {
        name: name.clone(),
        selection: request.selection.clone(),
        settings: settings.clone(),
        redo: request.redo.clone(),
        papers,
        embeddings,
        flagged: vec![false; count],
        records: Vec::new(),
    };

    optional(Step::Detect, tracker, board, || {
        if !settings.detect_ai {
            return StepOutcome::Skipped(steps::Skip::SwitchedOff);
        }
        let outcome = seams::detect(library, &mut made, tracker);
        if made.flagged.len() != made.papers.len() {
            made.flagged = vec![false; made.papers.len()];
            return StepOutcome::Failed("the detector did not score every paper".to_owned());
        }
        outcome
    })?;

    step(Step::Filter, tracker, board, || {
        let accepted: Vec<_> = made.papers.iter().map(filter::accepted).collect();
        let declarations = filter::declarations(library, settings)?;
        let keep = filter::keep(&made.papers, &accepted, &made.flagged, settings, &declarations, tracker);
        let before = made.papers.len();
        if keep.iter().any(|kept| !kept) {
            let rows: Vec<usize> = (0..before).filter(|&index| keep[index]).collect();
            made.embeddings = made.embeddings.select(Axis(0), &rows);
            made.papers = rows.iter().map(|&index| made.papers[index].clone()).collect();
            made.flagged = rows.iter().map(|&index| made.flagged[index]).collect();
        }
        tracker.log(&format!("  {} papers go on", thousands(made.papers.len() as u64)));
        enough(made.papers.len(), settings)?;
        Ok(((), filter::outcome(before, made.papers.len())))
    })?;

    let adjacency = step(Step::Similarity, tracker, board, || {
        let adjacency = graph::adjacency_matrix(made.embeddings.view(), settings.center_embeddings)?;
        let pairs = adjacency.len().max(1) as f64;
        let positive = adjacency.iter().filter(|value| **value > 0.0).count() as f64 / pairs;
        let mean = adjacency.iter().map(|value| f64::from(*value)).sum::<f64>() / pairs;
        let count = made.papers.len();
        let said = format!("{count} x {count}, {:.1}% of pairs positive, mean weight {mean:.3}", 100.0 * positive);
        Ok((adjacency, StepOutcome::Done(said)))
    })?;

    let partition = step(Step::Communities, tracker, board, || {
        let method = Method::from_str(&settings.clustering)?;
        let mut vectors = made.embeddings.to_owned();
        vectors -= &report::overall_mean(made.embeddings.view());
        make_unit(&mut vectors);
        let limits = Limits {
            min_community_size: settings.min_community_size as usize,
            min_split_modularity: settings.min_split_modularity,
            sub_levels: settings.sub_levels as usize,
        };
        let count = (settings.max_communities as usize).min(made.papers.len());
        let papers = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
        let partition = cluster::communities(method, papers, count, limits)?;
        for cut in &partition.cuts {
            tracker.log(&format!(
                "    split {}: {} | {}  (modularity {:.3})",
                cut.parent, cut.sizes[0], cut.sizes[1], cut.modularity
            ));
        }
        let subs: usize = partition.communities.iter().map(cluster::Community::descendants).sum();
        let said = format!("{} communities, {subs} sub-communities", partition.communities.len());
        Ok((partition, StepOutcome::Done(said)))
    })?;

    let accepted: Vec<_> = made.papers.iter().map(filter::accepted).collect();
    step(Step::Summarise, tracker, board, || {
        let texts: Vec<String> =
            made.papers.iter().map(|paper| format!("{} {}", paper.title, paper.r#abstract)).collect();
        let vocabulary = vocabulary::Vocabulary::new(&texts);
        let tiers: Vec<_> = accepted.iter().map(|shown| shown.tier).collect();
        let corpus = report::Corpus {
            selection: &made.selection,
            papers: &made.papers,
            embeddings: made.embeddings.view(),
            adjacency: adjacency.view(),
            vocabulary: &vocabulary,
            flagged: &made.flagged,
            tiers: &tiers,
        };
        made.records = report::community_tree(&corpus, &partition.communities, None);
        Ok(((), StepOutcome::Done(format!("{} terms in the vocabulary", thousands(vocabulary.len() as u64)))))
    })?;

    optional(Step::Describe, tracker, board, || {
        if !settings.describe {
            return StepOutcome::Skipped(steps::Skip::SwitchedOff);
        }
        seams::describe(library, context.transport, context.naming, &mut made, tracker)
    })?;

    step(Step::Write, tracker, board, || {
        let groups: Vec<Vec<usize>> = partition.communities.iter().map(|community| community.papers.clone()).collect();
        let result = report::RunResult {
            name: name.clone(),
            papers: made.papers.len(),
            model: model.clone(),
            centered: settings.center_embeddings,
            clustering: settings.clustering.clone(),
            modularity: graph::modularity(adjacency.view(), &groups),
            ai_flagged: made.flagged.iter().filter(|flagged| **flagged).count(),
            splits: partition
                .cuts
                .iter()
                .map(|cut| report::Split {
                    parent: cut.parent.to_string(),
                    sizes: cut.sizes,
                    modularity: cut.modularity,
                })
                .collect(),
            communities: std::mem::take(&mut made.records),
        };
        let mut community = vec![0; made.papers.len()];
        for (index, group) in groups.iter().enumerate() {
            for &paper in group {
                community[paper] = index;
            }
        }
        let drawing = drawing::drawing(
            &made.papers,
            made.embeddings.view(),
            adjacency.view(),
            &community,
            &made.flagged,
            &accepted,
            settings.max_links as usize,
        )?;
        let mut centroids = Array2::<f32>::zeros((groups.len(), made.embeddings.ncols()));
        for (mut row, group) in centroids.axis_iter_mut(Axis(0)).zip(&groups) {
            row.assign(&report::centroid(made.embeddings.view(), group));
        }
        let urls: std::collections::HashMap<&str, &str> =
            made.papers.iter().map(|paper| (paper.id.as_str(), paper.url.as_deref().unwrap_or(""))).collect();
        let markdown = report::markdown(&result, &|id| urls.get(id).copied().unwrap_or("").to_owned());
        let run = json!({
            "selection": made.selection.to_json(),
            "settings": settings,
            "paper_ids": made.papers.iter().map(|paper| paper.id.as_str()).collect::<Vec<_>>(),
            "clustering_version": CLUSTERING_VERSION,
        });
        let folder = files::runs_folder(context.data).join(&name);
        files::write(
            &folder,
            &files::Written {
                run: &run,
                communities: &result,
                graph: &drawing,
                centroids: centroids.view(),
                report: &markdown,
            },
        )?;
        Ok(((), StepOutcome::Done(format!("modularity {:.3}, {} papers", result.modularity, result.papers))))
    })?;
    Ok(name)
}
