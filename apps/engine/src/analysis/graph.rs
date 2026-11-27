//! The similarity graph over papers, and what is read off it directly.
//!
//! `adjacency[i, j] = max(0, cosine similarity of papers i and j)`, zero on the diagonal.
//!
//! The graph is one dense `n x n` matrix of 32-bit numbers, and it is the only thing of that size ever made:
//! 12,000 papers are 0.6 GB, 30,000 are 3.6 GB, which is where [`DENSE_LIMIT`] stops.
//!
//! # On every core
//!
//! Everything here that reads the whole graph does it row by row on the engine's threads ([`crate::threads`]):
//! the matrix product that makes it, its degrees, modularity. Each row (or block of rows) is worked out by one
//! thread from inputs nobody writes, and is written by that thread alone (rayon hands out disjoint rows), so
//! there is nothing to share. Sums of many rows are added up afterwards in the order of the rows, never in the
//! order threads finished, so every number is the same bit for bit on one thread or on 32.

use ndarray::linalg::general_mat_mul;
use ndarray::{Array1, Array2, ArrayView1, ArrayView2, Axis, Zip, s};
use rayon::prelude::*;

/// The most papers a dense similarity matrix is made for.
pub const DENSE_LIMIT: usize = 30_000;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    #[error(
        "{papers} papers is too many for a dense similarity matrix (limit {DENSE_LIMIT}); narrow the date range or categories."
    )]
    TooMany { papers: usize },
    #[error("the embedding of paper {paper} holds a number that is not finite; embed it again")]
    NotFinite { paper: usize },
}

/// Rows of the similarity matrix made together by one thread: a block of 128 rows of 1,024 numbers is half a
/// megabyte, which stays in a core's cache while it is multiplied with every paper.
const ROWS_PER_BLOCK: usize = 128;

/// Scale every row to length 1, in place. A row of zeros stays zeros.
pub fn make_unit(rows: &mut Array2<f32>) {
    rows.axis_iter_mut(Axis(0)).into_par_iter().for_each(|mut row| {
        let length = row.dot(&row).sqrt();
        if length > 0.0 {
            row /= length;
        }
    });
}

/// `max(0, cosine similarity)` between every two papers, zero on the diagonal.
///
/// With `center`, the mean embedding is removed first: text embeddings all share a large common direction, which
/// otherwise makes every pair of papers similar and the clip at 0 do nothing.
///
/// An embedding with a NaN or an infinity in it is refused: centred, it would make every paper's vector NaN, and
/// the clip at 0 would turn the whole graph into papers with no links at all, silently.
pub fn adjacency_matrix(embeddings: ArrayView2<f32>, center: bool) -> Result<Array2<f32>, GraphError> {
    let papers = embeddings.nrows();
    if papers > DENSE_LIMIT {
        return Err(GraphError::TooMany { papers });
    }
    if let Some(paper) = embeddings.rows().into_iter().position(|row| row.iter().any(|value| !value.is_finite())) {
        return Err(GraphError::NotFinite { paper });
    }
    let mut vectors = embeddings.to_owned();
    if center && let Some(mean) = vectors.mean_axis(Axis(0)) {
        vectors -= &mean;
    }
    make_unit(&mut vectors);
    Ok(similarities(vectors.view()))
}

/// `max(0, a b^T)` of unit rows, zero on the diagonal: the product cut into blocks of rows, each block made,
/// clipped and given its zero by one thread. The matrix library adds up each entry over the same columns in the
/// same order whichever block it is in, so the matrix is the one the whole product gives.
fn similarities(vectors: ArrayView2<f32>) -> Array2<f32> {
    let papers = vectors.nrows();
    let mut similarity = Array2::<f32>::zeros((papers, papers));
    let all = vectors.t();
    similarity.axis_chunks_iter_mut(Axis(0), ROWS_PER_BLOCK).into_par_iter().enumerate().for_each(
        |(block, mut rows)| {
            let first = block * ROWS_PER_BLOCK;
            let mine = vectors.slice(s![first..first + rows.nrows(), ..]);
            general_mat_mul(1.0, &mine, &all, 0.0, &mut rows);
            rows.mapv_inplace(|value| value.max(0.0));
            for (offset, mut row) in rows.outer_iter_mut().enumerate() {
                row[first + offset] = 0.0;
            }
        },
    );
    similarity
}

/// Each paper's degree: the sum of its row, added up in 64 bits (a row is thousands of small numbers), one row
/// per thread at a time.
pub fn degrees(matrix: ArrayView2<f32>) -> Array1<f64> {
    let sums: Vec<f64> =
        matrix.axis_iter(Axis(0)).into_par_iter().map(|row| row.iter().map(|value| f64::from(*value)).sum()).collect();
    Array1::from(sums)
}

/// Newman modularity of `groups` (each a list of paper indexes) on the graph `matrix`: how much more weight
/// lies inside the groups than would if the same degrees were wired at random. 0 is no structure.
pub fn modularity(matrix: ArrayView2<f32>, groups: &[Vec<usize>]) -> f64 {
    let degree = degrees(matrix);
    let total: f64 = degree.sum();
    if total == 0.0 {
        return 0.0;
    }
    groups
        .iter()
        .map(|group| {
            // each paper's weight inside its group on a thread of its own, then added up in the group's order
            let rows: Vec<f64> = group
                .par_iter()
                .map(|&i| {
                    let row = matrix.row(i);
                    group.iter().map(|&j| f64::from(row[j])).sum::<f64>()
                })
                .collect();
            let inside: f64 = rows.iter().sum();
            let share: f64 = group.iter().map(|&i| degree[i]).sum::<f64>() / total;
            inside / total - share * share
        })
        .sum()
}

/// The mean of the rows of `embeddings` listed in `group`.
pub fn mean_of(embeddings: ArrayView2<f32>, group: &[usize]) -> Array1<f32> {
    let mut mean = Array1::<f32>::zeros(embeddings.ncols());
    for &paper in group {
        mean += &embeddings.row(paper);
    }
    if !group.is_empty() {
        mean /= group.len() as f32;
    }
    mean
}

/// Cosine similarity of two vectors; 0 when either is all zeros.
pub fn cosine(a: ArrayView1<f32>, b: ArrayView1<f32>) -> f32 {
    let lengths = (a.dot(&a) * b.dot(&b)).sqrt();
    if lengths > 0.0 { a.dot(&b) / lengths } else { 0.0 }
}

/// Is the matrix the same read by rows and by columns, to within `tolerance`?
pub fn is_symmetric(matrix: ArrayView2<f32>, tolerance: f32) -> bool {
    matrix.nrows() == matrix.ncols()
        && Zip::from(&matrix).and(&matrix.t()).all(|here, there| (here - there).abs() <= tolerance)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ndarray::{array, s};

    /// Two groups of points: the first near one direction, the second near another, with a little spread.
    fn two_groups() -> Array2<f32> {
        let mut rows = Array2::<f32>::zeros((8, 6));
        for row in 0..8 {
            let centre = if row < 4 { 0 } else { 3 };
            rows[[row, centre]] = 4.0;
            rows[[row, centre + 1 + row % 2]] = 1.0;
        }
        rows
    }

    #[test]
    fn rows_are_scaled_to_length_one_and_a_row_of_zeros_is_left_alone() {
        let mut rows = array![[3.0, 4.0], [0.0, 0.0], [0.0, -2.0]];
        make_unit(&mut rows);
        assert_eq!(rows, array![[0.6, 0.8], [0.0, 0.0], [0.0, -1.0]]);
    }

    #[test]
    fn adjacency_is_clipped_symmetric_and_has_no_self_links() {
        let embeddings = two_groups();
        for center in [true, false] {
            let adjacency = adjacency_matrix(embeddings.view(), center).unwrap();
            assert_eq!(adjacency.dim(), (8, 8));
            assert!(adjacency.iter().all(|value| (0.0..=1.0001).contains(value)), "clipped to [0, 1]");
            assert!(adjacency.diag().iter().all(|value| *value == 0.0), "no self-links");
            assert!(is_symmetric(adjacency.view(), 1e-6));
        }
        // centred, papers of different groups point away from each other: clipped to nothing
        let centred = adjacency_matrix(embeddings.view(), true).unwrap();
        assert!(centred.slice(s![..4, 4..]).iter().all(|value| *value == 0.0));
        assert!(centred.slice(s![..4, ..4]).sum() > 6.0, "papers of one group stay linked");
        // uncentred, with the direction every embedding shares, everything links to everything
        let raw = adjacency_matrix((&embeddings + 5.0).view(), false).unwrap();
        assert!(raw.slice(s![..4, 4..]).iter().all(|value| *value > 0.5));
    }

    #[test]
    fn identical_papers_are_as_similar_as_can_be() {
        let embeddings = array![[1.0, 2.0, 3.0], [1.0, 2.0, 3.0], [-3.0, 0.5, 1.0]];
        let adjacency = adjacency_matrix(embeddings.view(), false).unwrap();
        assert!((adjacency[[0, 1]] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn too_many_papers_are_refused_with_a_reason() {
        let embeddings = Array2::<f32>::zeros((DENSE_LIMIT + 1, 1));
        let error = adjacency_matrix(embeddings.view(), true).unwrap_err();
        assert_eq!(error, GraphError::TooMany { papers: DENSE_LIMIT + 1 });
        assert!(error.to_string().contains("narrow the date range"));
    }

    #[test]
    fn an_embedding_that_is_not_finite_is_refused_and_named() {
        let mut embeddings = two_groups();
        embeddings[[5, 2]] = f32::NAN;
        assert_eq!(adjacency_matrix(embeddings.view(), true), Err(GraphError::NotFinite { paper: 5 }));
        embeddings[[5, 2]] = 0.0;
        embeddings[[1, 0]] = f32::INFINITY;
        assert_eq!(adjacency_matrix(embeddings.view(), false), Err(GraphError::NotFinite { paper: 1 }));
    }

    #[test]
    fn no_papers_give_an_empty_graph() {
        let adjacency = adjacency_matrix(Array2::<f32>::zeros((0, 4)).view(), true).unwrap();
        assert_eq!(adjacency.dim(), (0, 0));
        assert_eq!(modularity(adjacency.view(), &[]), 0.0);
    }

    /// Two cliques of four joined by one weak link.
    fn two_cliques() -> Array2<f32> {
        let mut matrix = Array2::<f32>::zeros((8, 8));
        for i in 0..8 {
            for j in 0..8 {
                if i != j && (i < 4) == (j < 4) {
                    matrix[[i, j]] = 1.0;
                }
            }
        }
        matrix[[3, 4]] = 0.1;
        matrix[[4, 3]] = 0.1;
        matrix
    }

    #[test]
    fn modularity_is_high_for_the_true_groups_and_zero_for_one_group() {
        let matrix = two_cliques();
        let truth = modularity(matrix.view(), &[(0..4).collect(), (4..8).collect()]);
        let mixed = modularity(matrix.view(), &[vec![0, 1, 4, 5], vec![2, 3, 6, 7]]);
        let whole = modularity(matrix.view(), &[(0..8).collect()]);
        assert!((truth - 0.4917).abs() < 1e-3, "{truth}");
        assert!(mixed < 0.0, "{mixed}");
        assert!(whole.abs() < 1e-9, "{whole}");
        assert!((degrees(matrix.view())[3] - 3.1).abs() < 1e-6);
        assert!((degrees(matrix.view())[0] - 3.0).abs() < 1e-9);
    }

    #[test]
    fn the_mean_of_a_group_and_the_cosine_of_two_vectors() {
        let embeddings = array![[1.0, 0.0], [3.0, 0.0], [0.0, 5.0]];
        assert_eq!(mean_of(embeddings.view(), &[0, 1]), array![2.0, 0.0]);
        assert_eq!(mean_of(embeddings.view(), &[]), array![0.0, 0.0]);
        assert_eq!(cosine(embeddings.row(0), embeddings.row(1)), 1.0);
        assert_eq!(cosine(embeddings.row(0), embeddings.row(2)), 0.0);
        assert_eq!(cosine(embeddings.row(0), array![0.0, 0.0].view()), 0.0);
    }

    /// Embeddings with no structure to them, of a size that spans several blocks of rows (and a short last one).
    fn noise(papers: usize, width: usize) -> Array2<f32> {
        let mut random = crate::analysis::random::Random::new(7);
        Array2::from_shape_simple_fn((papers, width), || random.normal() as f32)
    }

    #[test]
    fn the_graph_its_degrees_and_modularity_are_the_same_bit_for_bit_on_one_thread_and_on_many() {
        let embeddings = noise(3 * ROWS_PER_BLOCK + 17, 48);
        let groups: Vec<Vec<usize>> = vec![(0..150).collect(), (150..embeddings.nrows()).rev().collect()];
        let work = || {
            let adjacency = adjacency_matrix(embeddings.view(), true).unwrap();
            let score = modularity(adjacency.view(), &groups);
            (degrees(adjacency.view()), score.to_bits(), adjacency)
        };
        let (one, many) = crate::threads::on_one_and_many(work);
        assert_eq!(one, many);
    }

    #[test]
    fn the_graph_made_in_blocks_is_the_whole_product_clipped() {
        let mut vectors = noise(2 * ROWS_PER_BLOCK + 5, 33);
        make_unit(&mut vectors);
        let mut whole = vectors.dot(&vectors.t());
        whole.mapv_inplace(|value| value.max(0.0));
        whole.diag_mut().fill(0.0);
        assert_eq!(similarities(vectors.view()), whole);
    }

    /// How long a conference-sized graph takes on this computer: `cargo test --release -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, not a check: 12,487 papers of 1,024 numbers each"]
    fn a_conference_sized_graph_is_made_in_seconds() {
        let (papers, width) = (12_487, 1_024);
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let embeddings = Array2::from_shape_fn((papers, width), |_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 2_000) as f32 / 1_000.0 - 1.0
        });
        let started = std::time::Instant::now();
        let adjacency = adjacency_matrix(embeddings.view(), true).unwrap();
        let made = started.elapsed();
        let started = std::time::Instant::now();
        let halves: Vec<Vec<usize>> = vec![(0..papers / 2).collect(), (papers / 2..papers).collect()];
        let score = modularity(adjacency.view(), &halves);
        println!(
            "{papers} papers: graph in {:.2}s ({} MB), modularity of two halves in {:.2}s ({score:.4})",
            made.as_secs_f32(),
            adjacency.len() * 4 / 1_048_576,
            started.elapsed().as_secs_f32()
        );
        assert!(is_symmetric(adjacency.view(), 1e-4));
    }
}
