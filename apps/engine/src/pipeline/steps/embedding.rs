//! The embedding steps of a run: the model's files (downloaded once), then a vector for every paper the library has
//! none for (same model, same paper, same text). Nothing is embedded twice: vectors are kept in the library chunk
//! by chunk, so a run stopped half-way keeps what it embedded and the next one goes on from there.
//!
//! The model runs one batch at a time on whatever [`crate::model::embed`] found (a graphics card, or the
//! processor); the progress line says which.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use ndarray::Array2;

use super::model_files::{self, Download, ModelRepo, QWEN3};
use crate::jobs::Tracker;
use crate::library::{Library, Paper};
use crate::model::embed::{Device, Encoder};
use crate::pipeline::steps::{Board, Count, Skip, StepOutcome, Unit, thousands};
use crate::pipeline::{RunError, Step};

/// Texts handed to the model at once (the Python's batch size).
pub const BATCH: usize = 16;
/// Papers embedded between two writes to the library (the Python's 25 batches).
pub const CHUNK: usize = BATCH * 25;

/// What embeds papers: the real model, or a test's stand-in.
pub trait Embedding: Send + Sync {
    /// Make the model `model` ready to embed (its files downloaded, say). Answers what that step came to.
    fn prepare(&self, model: &str, data: &Path, tracker: &Tracker<'_>, board: &Board) -> Result<StepOutcome, RunError>;
    /// One unit vector per text, in order.
    fn encode(&self, model: &str, texts: &[&str]) -> Result<Vec<Vec<f32>>, RunError>;
    /// What it runs on, in words for the progress line.
    fn device(&self) -> String;
}

/// The model this engine can run: `Qwen/Qwen3-Embedding-0.6B` under ONNX Runtime. Its files are downloaded once;
/// the loaded model is kept for the next run.
pub struct Onnx {
    download: Arc<dyn Download>,
    repo: ModelRepo,
    /// The model loaded, and from which folder.
    loaded: Mutex<Loaded>,
}

enum Loaded {
    Not,
    /// The files are here, not loaded yet.
    Ready(PathBuf),
    Open {
        encoder: Box<Encoder>,
    },
}

impl Onnx {
    pub fn new(download: Arc<dyn Download>) -> Self {
        Self { download, repo: QWEN3, loaded: Mutex::new(Loaded::Not) }
    }

    /// Which device the loaded model runs on, if it has been loaded.
    pub fn loaded_device(&self) -> Option<Device> {
        match &*self.loaded.lock().unwrap_or_else(PoisonError::into_inner) {
            Loaded::Open { encoder } => Some(encoder.device),
            Loaded::Not | Loaded::Ready(_) => None,
        }
    }

    fn not_this_model(&self, model: &str) -> RunError {
        RunError::Embed(format!(
            "this version of Constellate embeds with {} only; some papers have no embedding by {model}. Choose {} \
             in the settings (papers embedded by {model} before are still used)",
            self.repo.model, self.repo.model
        ))
    }
}

/// The words for a device.
pub fn device_words(device: Device) -> &'static str {
    match device {
        Device::GraphicsCard => "the graphics card",
        Device::Processor => "the processor (no graphics card could be used: many times slower)",
    }
}

impl Embedding for Onnx {
    fn prepare(&self, model: &str, data: &Path, tracker: &Tracker<'_>, board: &Board) -> Result<StepOutcome, RunError> {
        if model != self.repo.model {
            return Err(self.not_this_model(model));
        }
        if matches!(&*self.loaded.lock().unwrap_or_else(PoisonError::into_inner), Loaded::Open { .. }) {
            return Ok(StepOutcome::Kept("loaded already".to_owned()));
        }
        let (folder, outcome) =
            model_files::ensure(&self.repo, data, self.download.as_ref(), tracker, &|count| board.count(count))
                .map_err(|error| match error {
                    model_files::ModelError::Cancelled => RunError::Cancelled(Step::Model),
                    other => RunError::Embed(other.to_string()),
                })?;
        *self.loaded.lock().unwrap_or_else(PoisonError::into_inner) = Loaded::Ready(folder);
        Ok(outcome)
    }

    fn encode(&self, model: &str, texts: &[&str]) -> Result<Vec<Vec<f32>>, RunError> {
        if model != self.repo.model {
            return Err(self.not_this_model(model));
        }
        let mut loaded = self.loaded.lock().unwrap_or_else(PoisonError::into_inner);
        if let Loaded::Ready(folder) = &*loaded {
            let encoder = Encoder::open(folder).map_err(|error| RunError::Embed(error.to_string()))?;
            *loaded = Loaded::Open { encoder: Box::new(encoder) };
        }
        match &mut *loaded {
            Loaded::Open { encoder } => encoder.encode(texts).map_err(|error| RunError::Embed(error.to_string())),
            Loaded::Not | Loaded::Ready(_) => Err(RunError::Embed("the embedding model is not ready".to_owned())),
        }
    }

    fn device(&self) -> String {
        self.loaded_device().map_or("the processor or a graphics card", device_words).to_owned()
    }
}

/// Which of `papers` are to be embedded: those `model` has not embedded with their present text, or all of them.
pub fn missing(library: &Library, model: &str, papers: &[Paper], redo: bool) -> Result<Vec<usize>, RunError> {
    if redo {
        return Ok((0..papers.len()).collect());
    }
    let embedded = library.embedded(model, papers)?;
    Ok(embedded.iter().enumerate().filter(|(_, done)| !**done).map(|(index, _)| index).collect())
}

/// The model step: ready the model when something is to be embedded.
pub fn prepare(
    embedding: &dyn Embedding,
    model: &str,
    data: &Path,
    to_embed: usize,
    tracker: &Tracker<'_>,
    board: &Board,
) -> Result<StepOutcome, RunError> {
    if to_embed == 0 {
        return Ok(StepOutcome::Skipped(Skip::NothingToDo("every paper is embedded already".to_owned())));
    }
    embedding.prepare(model, data, tracker, board)
}

/// The embed step: the papers listed in `todo` embedded and kept, chunk by chunk; then every paper's vector.
pub fn embed(
    embedding: &dyn Embedding,
    library: &Library,
    model: &str,
    papers: &[Paper],
    todo: &[usize],
    tracker: &Tracker<'_>,
    board: &Board,
) -> Result<(Array2<f32>, StepOutcome), RunError> {
    tracker.log(&format!("  {} found in the library, {} to embed with {model}", papers.len() - todo.len(), todo.len()));
    let outcome = if todo.is_empty() {
        StepOutcome::Kept(format!("all {} papers were embedded before", thousands(papers.len() as u64)))
    } else {
        // similar lengths together: less padding (the Python's order)
        let texts: Vec<String> = todo.iter().map(|&index| papers[index].text()).collect();
        let mut order: Vec<usize> = (0..todo.len()).collect();
        order.sort_by_key(|&index| std::cmp::Reverse(texts[index].chars().count()));
        let total = todo.len() as u64;
        let mut done = 0_u64;
        board.count(Count { done, of: total, unit: Unit::Papers });
        let mut said_device = false;
        for chunk in order.chunks(CHUNK) {
            let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(chunk.len());
            for batch in chunk.chunks(BATCH) {
                if tracker.cancelled() {
                    return Err(RunError::Cancelled(Step::Embed));
                }
                let batch_texts: Vec<&str> = batch.iter().map(|&index| texts[index].as_str()).collect();
                vectors.extend(embedding.encode(model, &batch_texts)?);
                if !said_device {
                    said_device = true;
                    let device = embedding.device();
                    tracker.log(&format!("  embedding on {device}"));
                    board.detail(format!("embedding on {device}"));
                }
                done += batch.len() as u64;
                board.count(Count { done, of: total, unit: Unit::Papers });
            }
            let width = vectors.first().map_or(0, Vec::len);
            if vectors.iter().any(|vector| vector.len() != width) {
                return Err(RunError::Embed("the model answered vectors of different lengths".to_owned()));
            }
            let matrix = Array2::from_shape_vec((vectors.len(), width), vectors.concat())
                .map_err(|error| RunError::Embed(error.to_string()))?;
            let chunk_papers: Vec<Paper> = chunk.iter().map(|&index| papers[todo[index]].clone()).collect();
            library.add_embeddings(model, &chunk_papers, matrix.view())?; // kept chunk by chunk
            tracker.log(&format!("    {}/{}", thousands(done), thousands(total)));
        }
        StepOutcome::Done(format!("{} papers embedded", thousands(total)))
    };
    Ok((library.vectors(model, papers)?, outcome))
}
