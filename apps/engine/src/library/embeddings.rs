//! Embeddings: one vector per model and paper, kept with the hash of the text it was computed from.
//!
//! An embedding is reused only for the same model, the same paper and exactly the same text: a paper whose
//! abstract changed on arXiv is embedded again, one that did not is not. A new vector for a paper replaces the
//! old one (the Python appended and left the old row orphaned in its file).
//!
//! A vector is a `BLOB` of little-endian `f32`, the layout of a row of the Python's `vectors.npy`, so moving one
//! in is a copy of its bytes. Reading back the vectors of a selection looks each paper up by its primary key and
//! decodes straight into one contiguous matrix, in the order asked.

use ndarray::{Array2, ArrayView2};
use rusqlite::{Connection, OptionalExtension, params};

use super::papers::exists;
use super::{Library, LibraryError, Paper};

/// Why a paper's vector cannot be handed out.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum NotEmbedded {
    #[error("paper {paper} has no embedding by {model}")]
    Missing { model: String, paper: String },
    /// The paper's text changed since it was embedded.
    #[error("the embedding of paper {paper} by {model} is of an earlier version of its text")]
    Stale { model: String, paper: String },
}

/// The bytes kept for a vector.
fn encode(row: ndarray::ArrayView1<'_, f32>) -> Vec<u8> {
    row.iter().flat_map(|value| value.to_le_bytes()).collect()
}

/// Keep one vector, as raw little-endian bytes, replacing any the paper had from the model.
pub(super) fn put(
    connection: &Connection,
    model: &str,
    paper_id: &str,
    text_hash: &str,
    vector: &[u8],
) -> rusqlite::Result<usize> {
    let mut statement = connection.prepare_cached(
        "INSERT INTO embeddings (model, paper_id, text_hash, vector) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (model, paper_id) DO UPDATE SET text_hash = excluded.text_hash, vector = excluded.vector",
    )?;
    statement.execute(params![model, paper_id, text_hash, vector])
}

/// The length in bytes of the model's vectors, if it has any.
pub(super) fn width(connection: &Connection, model: &str) -> rusqlite::Result<Option<usize>> {
    let bytes: Option<i64> = connection
        .query_row("SELECT length(vector) FROM embeddings WHERE model = ?1 LIMIT 1", [model], |row| row.get(0))
        .optional()?;
    Ok(bytes.and_then(|bytes| usize::try_from(bytes).ok()))
}

impl Library {
    /// Keep the vectors `model` computed for `papers`, one row of `vectors` per paper, in one transaction.
    pub fn add_embeddings(
        &self,
        model: &str,
        papers: &[Paper],
        vectors: ArrayView2<'_, f32>,
    ) -> Result<(), LibraryError> {
        if vectors.nrows() != papers.len() {
            return Err(LibraryError::Shape(format!("{} vectors for {} papers", vectors.nrows(), papers.len())));
        }
        self.write(|transaction| {
            let bytes = vectors.ncols() * size_of::<f32>();
            if let Some(kept) = width(transaction, model)?
                && kept != bytes
            {
                return Err(LibraryError::Shape(format!(
                    "{model} keeps vectors of {} numbers, not {}",
                    kept / size_of::<f32>(),
                    vectors.ncols()
                )));
            }
            for (paper, vector) in papers.iter().zip(vectors.rows()) {
                if !exists(transaction, &paper.id)? {
                    return Err(LibraryError::UnknownPaper(paper.id.clone()));
                }
                put(transaction, model, &paper.id, &paper.text_hash(), &encode(vector))?;
            }
            Ok(())
        })
    }

    /// For each paper, whether `model` has embedded exactly its present text.
    pub fn embedded(&self, model: &str, papers: &[Paper]) -> Result<Vec<bool>, LibraryError> {
        self.read(|connection| {
            let mut statement =
                connection.prepare_cached("SELECT text_hash FROM embeddings WHERE model = ?1 AND paper_id = ?2")?;
            papers
                .iter()
                .map(|paper| {
                    let kept: Option<String> = statement.query_row([model, &paper.id], |row| row.get(0)).optional()?;
                    Ok(kept.is_some_and(|kept| kept == paper.text_hash()))
                })
                .collect()
        })
    }

    /// The vectors of `papers` by `model`, one row per paper in the order given. Every paper must have been
    /// embedded with its present text ([`NotEmbedded`] otherwise): a vector of an older text is never handed out.
    pub fn vectors(&self, model: &str, papers: &[Paper]) -> Result<Array2<f32>, LibraryError> {
        self.read(|connection| {
            let width = width(connection, model)?.unwrap_or(0) / size_of::<f32>();
            let mut numbers = Vec::with_capacity(papers.len() * width);
            let mut statement = connection
                .prepare_cached("SELECT text_hash, vector FROM embeddings WHERE model = ?1 AND paper_id = ?2")?;
            for paper in papers {
                let mut rows = statement.query([model, &paper.id])?;
                let Some(row) = rows.next()? else {
                    return Err(NotEmbedded::Missing { model: model.into(), paper: paper.id.clone() }.into());
                };
                if row.get_ref(0)?.as_str().map_err(rusqlite::Error::from)? != paper.text_hash() {
                    return Err(NotEmbedded::Stale { model: model.into(), paper: paper.id.clone() }.into());
                }
                // borrowed from SQLite's page: decoded without a copy in between
                let bytes = row.get_ref(1)?.as_blob().map_err(rusqlite::Error::from)?;
                if bytes.len() != width * size_of::<f32>() {
                    return Err(LibraryError::Shape(format!(
                        "paper {}'s vector by {model} is not {width} long",
                        paper.id
                    )));
                }
                numbers
                    .extend(bytes.chunks_exact(4).map(|four| f32::from_le_bytes([four[0], four[1], four[2], four[3]])));
            }
            Array2::from_shape_vec((papers.len(), width), numbers)
                .map_err(|error| LibraryError::Shape(error.to_string()))
        })
    }

    /// How many papers each model has embedded.
    pub fn embedding_counts(&self) -> Result<std::collections::BTreeMap<String, usize>, LibraryError> {
        self.read(|connection| {
            let mut statement = connection.prepare("SELECT model, COUNT(*) FROM embeddings GROUP BY model")?;
            let counts = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, usize::try_from(row.get::<_, i64>(1)?).unwrap_or(0)))
            })?;
            Ok(counts.collect::<rusqlite::Result<_>>()?)
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::library::Known;
    use crate::library::papers::tests::paper;
    use ndarray::array;
    use std::time::Instant;

    fn library_with(papers: &[Paper]) -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        library.add_papers(papers, None, Known::Keep).unwrap();
        (folder, library)
    }

    #[test]
    fn vectors_come_back_exactly_and_in_the_order_asked() {
        let (a, b) = (paper("a"), paper("b"));
        let (_folder, library) = library_with(&[a.clone(), b.clone()]);
        let written = array![[1.0_f32, -0.5, f32::MIN_POSITIVE], [0.25, 1e-30, -7.0]];
        library.add_embeddings("test/model", &[a.clone(), b.clone()], written.view()).unwrap();
        let read = library.vectors("test/model", &[b.clone(), a.clone(), b.clone()]).unwrap();
        assert_eq!(read, array![[0.25_f32, 1e-30, -7.0], [1.0, -0.5, f32::MIN_POSITIVE], [0.25, 1e-30, -7.0]]);
        assert_eq!(library.vectors("test/model", &[]).unwrap().dim(), (0, 3));
        assert_eq!(library.embedding_counts().unwrap().get("test/model"), Some(&2));
    }

    #[test]
    fn an_embedding_is_reused_only_for_the_same_model_paper_and_text() {
        let first = paper("p");
        let (_folder, library) = library_with(&[first.clone(), paper("q")]);
        library.add_embeddings("test/model", std::slice::from_ref(&first), array![[1.0_f32; 4]].view()).unwrap();
        let mut revised = first.clone();
        revised.r#abstract = "revised".into();
        let mut same_text_other_id = paper("q");
        same_text_other_id.title.clone_from(&first.title);
        same_text_other_id.r#abstract.clone_from(&first.r#abstract);
        assert_eq!(
            library.embedded("test/model", &[first.clone(), revised.clone(), same_text_other_id.clone()]).unwrap(),
            [true, false, false]
        );
        assert_eq!(library.embedded("other/model", std::slice::from_ref(&first)).unwrap(), [false]);
        assert!(matches!(
            library.vectors("test/model", &[revised.clone()]),
            Err(LibraryError::NotEmbedded(NotEmbedded::Stale { .. }))
        ));
        assert!(matches!(
            library.vectors("test/model", &[same_text_other_id]),
            Err(LibraryError::NotEmbedded(NotEmbedded::Missing { .. }))
        ));
        // embedded again for the new text: the old vector is replaced, not kept beside it
        library.add_embeddings("test/model", std::slice::from_ref(&revised), array![[2.0_f32; 4]].view()).unwrap();
        assert_eq!(library.vectors("test/model", &[revised]).unwrap(), array![[2.0_f32; 4]]);
        assert_eq!(library.embedding_counts().unwrap().get("test/model"), Some(&1));
    }

    #[test]
    fn vectors_that_do_not_fit_are_refused_and_nothing_is_kept() {
        let (_folder, library) = library_with(&[paper("a"), paper("b")]);
        let two = [paper("a"), paper("b")];
        assert!(matches!(
            library.add_embeddings("m", &two, array![[1.0_f32, 2.0]].view()),
            Err(LibraryError::Shape(_))
        ));
        library.add_embeddings("m", &two[..1], array![[1.0_f32, 2.0]].view()).unwrap();
        assert!(matches!(
            library.add_embeddings("m", &two[1..], array![[1.0_f32]].view()),
            Err(LibraryError::Shape(_))
        ));
        let unknown = [paper("b"), paper("not in the library")];
        let refused = library.add_embeddings("m", &unknown, array![[1.0_f32, 2.0], [3.0, 4.0]].view());
        assert!(matches!(refused, Err(LibraryError::UnknownPaper(id)) if id == "not in the library"));
        assert_eq!(library.embedded("m", &two).unwrap(), [true, false]); // "b" went with the transaction
    }

    /// The size of a large selection with the default model. Run in release to see the real time:
    /// `cargo test --release -- --ignored twelve_thousand --nocapture`
    #[test]
    #[ignore = "a timing, to be run in release"]
    fn twelve_thousand_vectors_of_1024_are_read_in_well_under_a_second() {
        let papers: Vec<Paper> = (0..12_000).map(|number| paper(&format!("2603.{number:05}"))).collect();
        let (_folder, library) = library_with(&papers);
        let vectors = Array2::from_shape_fn((papers.len(), 1024), |(row, column)| (row * 1024 + column) as f32);
        library.add_embeddings("Qwen/Qwen3-Embedding-0.6B", &papers, vectors.view()).unwrap();
        let mut order = papers.clone();
        order.reverse();
        let started = Instant::now();
        let read = library.vectors("Qwen/Qwen3-Embedding-0.6B", &order).unwrap();
        let took = started.elapsed();
        println!("read 12,000 x 1,024 vectors in {took:?}");
        assert_eq!(read.dim(), (12_000, 1024));
        assert_eq!(read[[0, 5]], (11_999 * 1024 + 5) as f32);
        assert!(took.as_secs_f64() < 0.5, "{took:?}");
    }
}
