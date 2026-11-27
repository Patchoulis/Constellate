//! What the eigenvectors of the normalised similarity graph say: the leading ones, which spectral clustering
//! works on, and the second one, whose signs cut a group in two (the Fiedler vector).
//!
//! The normalised graph is `D^-1/2 A D^-1/2`, where `D` holds the degrees. Its largest eigenvalue is 1, once for
//! every piece of a graph that falls apart, and vectors of eigenvalues close to 1 are nearly constant on groups
//! of papers that are linked among themselves far more than to the rest.

use ndarray::{Array1, Array2, ArrayView2, Axis};
use rayon::prelude::*;

use super::eigen::{EigenError, leading};
use super::graph::degrees;
use super::links::SparseGraph;

/// A similarity graph, held in one of the two ways the clustering methods make one.
#[derive(Clone, Copy, Debug)]
pub enum Graph<'a> {
    /// Every pair of papers: the similarity matrix itself.
    Dense(ArrayView2<'a, f32>),
    /// A few links per paper: the nearest-neighbour graph.
    Sparse(&'a SparseGraph),
}

impl Graph<'_> {
    fn papers(&self) -> usize {
        match self {
            Self::Dense(matrix) => matrix.nrows(),
            Self::Sparse(graph) => graph.papers(),
        }
    }

    fn degrees(&self) -> Array1<f64> {
        match self {
            Self::Dense(matrix) => degrees(*matrix),
            Self::Sparse(graph) => Array1::from(graph.degrees()),
        }
    }

    /// The graph's matrix times a block of vectors. A dense graph is multiplied in 32 bits, as it is stored.
    fn product(&self, block: ArrayView2<f64>) -> Array2<f64> {
        match self {
            Self::Dense(matrix) => dense_product(*matrix, block.mapv(|value| value as f32)).mapv(f64::from),
            Self::Sparse(graph) => graph.product(block),
        }
    }
}

/// `matrix` times `block`, both 32-bit.
///
/// The matrix library copies the whole matrix into its own layout for every product, which for a graph of 12,000
/// papers is 600 MB copied before any arithmetic: for the blocks of the eigensolver that copy is most of the time,
/// and the solver needs dozens of products. So the matrix is read once instead, row by row on every core, and each
/// row is multiplied with the block while it is in the cache. (Measured on 12,487 papers: 10 ms for a block of 8
/// columns against 75 ms, 60 ms for 64 columns against 150 ms.) A matrix that is not laid out row by row, which
/// this crate never makes, goes to the library.
fn dense_product(matrix: ArrayView2<f32>, block: Array2<f32>) -> Array2<f32> {
    // shapes that do not fit go to the library too, which says so rather than multiplying the wrong numbers
    let fits = block.nrows() == matrix.ncols();
    match matrix.as_slice() {
        Some(entries) if fits && block.ncols() <= 8 => streamed_product::<8>(entries, matrix.ncols(), &block),
        Some(entries) if fits => streamed_product::<16>(entries, matrix.ncols(), &block),
        _ => matrix.dot(&block),
    }
}

/// `matrix` (its entries row by row, `size` to a row) times `block`. The block's columns are taken `W` at a time,
/// padded with zeros, so that the innermost loop has a fixed length the compiler turns into vector instructions.
fn streamed_product<const W: usize>(entries: &[f32], size: usize, block: &Array2<f32>) -> Array2<f32> {
    let (rows, width) = block.dim();
    let groups = width.div_ceil(W);
    // group g holds, for every row of the block, its columns g*W .. g*W + W
    let mut lanes = vec![vec![[0.0_f32; W]; rows]; groups];
    for (row, values) in block.rows().into_iter().enumerate() {
        for (column, value) in values.iter().enumerate() {
            lanes[column / W][row][column % W] = *value;
        }
    }
    let size = size.max(1);
    // Rows are taken ROWS at a time, so that each value of the block read from the cache serves ROWS rows.
    let sums: Vec<Vec<[[f32; W]; ROWS]>> = entries
        .par_chunks(ROWS * size)
        .map(|weights| {
            let mut rows: [&[f32]; ROWS] = [&[]; ROWS];
            for (slot, row) in rows.iter_mut().zip(weights.chunks(size)) {
                *slot = row;
            }
            lanes.iter().map(|group| row_sums(rows, group)).collect()
        })
        .collect();
    Array2::from_shape_fn((entries.len() / size, width), |(row, column)| {
        sums[row / ROWS][column / W][row % ROWS][column % W]
    })
}

/// Rows of the matrix multiplied together in [`streamed_product`].
const ROWS: usize = 4;

/// Each of up to `ROWS` rows of the matrix times one group of `W` columns of the block. A row left empty (the last
/// piece of a matrix can have fewer rows) gives zeros.
fn row_sums<const W: usize>(rows: [&[f32]; ROWS], group: &[[f32; W]]) -> [[f32; W]; ROWS] {
    let mut sums = [[0.0_f32; W]; ROWS];
    if rows.iter().all(|row| row.len() == group.len()) {
        for (index, values) in group.iter().enumerate() {
            for (sum, row) in sums.iter_mut().zip(rows) {
                let weight = row[index];
                for (total, value) in sum.iter_mut().zip(values) {
                    *total += weight * value;
                }
            }
        }
    } else {
        for (sum, row) in sums.iter_mut().zip(rows) {
            for (weight, values) in row.iter().zip(group) {
                for (total, value) in sum.iter_mut().zip(values) {
                    *total += weight * value;
                }
            }
        }
    }
    sums
}

/// The leading eigenvectors of a normalised graph.
#[derive(Clone, Debug)]
pub struct Spectrum {
    /// Largest first.
    pub values: Array1<f64>,
    /// One column per eigenvalue, each of length one.
    pub vectors: Array2<f64>,
    /// `D^-1/2` as a vector: an eigenvector of the normalised graph times this is the matching vector of the
    /// random-walk form `D^-1 A`, which is what the Fiedler cut reads.
    pub scale: Array1<f64>,
}

/// The `count` leading eigenvectors of `D^-1/2 A D^-1/2`, largest eigenvalue first.
///
/// The normalised matrix is never formed: the solver only needs its product with vectors, which is
/// `D^-1/2 (A (D^-1/2 V))`, so nothing the size of the graph is made. The same graph always gives the same vectors.
/// A paper with no links has degree 0; it is given a tiny degree instead, which leaves its row of the normalised
/// matrix empty (eigenvalue 0) rather than undefined.
pub fn leading_eigenvectors(graph: Graph, count: usize) -> Result<Spectrum, EigenError> {
    let scale = graph.degrees().mapv(|degree| 1.0 / degree.max(1e-12).sqrt());
    let column = scale.view().insert_axis(Axis(1));
    let eigen = leading(graph.papers(), count, |block| {
        let scaled = &block * &column;
        graph.product(scaled.view()) * column
    })?;
    Ok(Spectrum { values: eigen.values, vectors: eigen.vectors, scale })
}

/// Which connected piece each paper is in (links of weight above 0), the pieces numbered in order of their
/// lowest-numbered paper; and how many pieces there are.
fn components(matrix: ArrayView2<f32>) -> (Vec<usize>, usize) {
    let papers = matrix.nrows();
    let mut piece = vec![usize::MAX; papers];
    let mut count = 0;
    for start in 0..papers {
        if piece[start] != usize::MAX {
            continue;
        }
        piece[start] = count;
        let mut waiting = vec![start];
        while let Some(paper) = waiting.pop() {
            for (other, weight) in matrix.row(paper).iter().enumerate() {
                if *weight > 0.0 && piece[other] == usize::MAX {
                    piece[other] = count;
                    waiting.push(other);
                }
            }
        }
        count += 1;
    }
    (piece, count)
}

/// The middle value; for an even count, halfway between the two middle ones.
fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    match sorted.len() {
        0 => 0.0,
        length if length % 2 == 1 => sorted[middle],
        _ => (sorted[middle - 1] + sorted[middle]) / 2.0,
    }
}

/// Two-way cut of one group of papers (its similarity graph `sub`): `true` for the papers on one side. `None`
/// when the group cannot be cut.
///
/// A group in several pieces is cut into its largest piece (the first of equal ones) and the rest: no
/// eigenvector is needed to see that cut. A connected group is cut by the sign of its Fiedler vector (the second
/// eigenvector, in the random-walk scaling); if every paper has the same sign, by the vector's median.
pub fn fiedler_side(sub: ArrayView2<f32>) -> Result<Option<Vec<bool>>, EigenError> {
    let papers = sub.nrows();
    if papers < 2 {
        return Ok(None);
    }
    let (piece, pieces) = components(sub);
    if pieces > 1 {
        let mut sizes = vec![0; pieces];
        for &one in &piece {
            sizes[one] += 1;
        }
        let largest = (0..pieces).fold(0, |best, one| if sizes[one] > sizes[best] { one } else { best });
        return Ok(Some(piece.iter().map(|&one| one == largest).collect()));
    }
    let spectrum = leading_eigenvectors(Graph::Dense(sub), 2)?;
    let fiedler: Vec<f64> = (&spectrum.vectors.column(1) * &spectrum.scale).to_vec();
    let one_sided = |side: &[bool]| side.iter().all(|&here| here) || !side.iter().any(|&here| here);
    let mut side: Vec<bool> = fiedler.iter().map(|&value| value > 0.0).collect();
    if one_sided(&side) {
        let middle = median(&fiedler);
        side = fiedler.iter().map(|&value| value > middle).collect();
    }
    Ok(if one_sided(&side) { None } else { Some(side) })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::analysis::links::knn_graph;

    /// `cliques` groups of `size` papers, every pair within a group linked with weight 1, and `between` between
    /// papers of different groups.
    fn cliques(cliques: usize, size: usize, between: f32) -> Array2<f32> {
        let papers = cliques * size;
        Array2::from_shape_fn((papers, papers), |(i, j)| match (i == j, i / size == j / size) {
            (true, _) => 0.0,
            (false, true) => 1.0,
            (false, false) => between,
        })
    }

    #[test]
    fn disjoint_cliques_have_eigenvalue_one_once_per_clique() {
        let graph = cliques(4, 10, 0.0);
        let spectrum = leading_eigenvectors(Graph::Dense(graph.view()), 5).unwrap();
        for index in 0..4 {
            assert!((spectrum.values[index] - 1.0).abs() < 1e-6, "{:?}", spectrum.values);
        }
        // the next eigenvalue of a clique of 10 normalised is -1/9
        assert!((spectrum.values[4] + 1.0 / 9.0).abs() < 1e-5, "{:?}", spectrum.values);
        // the four leading vectors are constant on each clique: rows of one clique are parallel
        let rows = spectrum.vectors.slice(ndarray::s![.., ..4]);
        for clique in 0..4 {
            let first = rows.row(clique * 10);
            for paper in clique * 10..clique * 10 + 10 {
                assert!((&rows.row(paper) - &first).iter().all(|value| value.abs() < 1e-5));
            }
        }
    }

    #[test]
    fn the_second_vector_of_two_weakly_joined_cliques_splits_them_by_sign() {
        let graph = cliques(2, 30, 0.01);
        let spectrum = leading_eigenvectors(Graph::Dense(graph.view()), 2).unwrap();
        assert!((spectrum.values[0] - 1.0).abs() < 1e-6);
        let second = spectrum.vectors.column(1);
        assert!(second[0] != 0.0);
        assert!(second.iter().take(30).all(|value| value.signum() == second[0].signum()));
        assert!(second.iter().skip(30).all(|value| value.signum() == -second[0].signum()));
    }

    #[test]
    fn a_sparse_graph_has_the_same_spectrum_as_its_dense_matrix() {
        let graph = cliques(3, 8, 0.05);
        let sparse = knn_graph(graph.view(), 9);
        let dense = sparse.to_dense();
        let from_sparse = leading_eigenvectors(Graph::Sparse(&sparse), 3).unwrap();
        let from_dense = leading_eigenvectors(Graph::Dense(dense.view()), 3).unwrap();
        assert!((&from_sparse.values - &from_dense.values).iter().all(|value| value.abs() < 1e-5));
    }

    #[test]
    fn the_fiedler_vector_separates_two_cliques() {
        let graph = cliques(2, 6, 0.01);
        let side = fiedler_side(graph.view()).unwrap().unwrap();
        assert!(side[..6].iter().all(|here| *here == side[0]));
        assert!(side[6..].iter().all(|here| *here == side[6]));
        assert_ne!(side[0], side[6]);
    }

    #[test]
    fn a_large_group_is_cut_the_same_way() {
        let graph = cliques(2, 300, 0.01);
        let side = fiedler_side(graph.view()).unwrap().unwrap();
        assert!(side[..300].iter().all(|here| *here == side[0]));
        assert!(side[300..].iter().all(|here| *here != side[0]));
    }

    #[test]
    fn a_group_in_pieces_is_cut_into_its_largest_piece_and_the_rest() {
        // pieces {0, 2}, {1, 3, 4}, {5}
        let mut graph = Array2::<f32>::zeros((6, 6));
        for (a, b) in [(0, 2), (1, 3), (3, 4)] {
            graph[[a, b]] = 0.5;
            graph[[b, a]] = 0.5;
        }
        let side = fiedler_side(graph.view()).unwrap().unwrap();
        assert_eq!(side, vec![false, true, false, true, true, false]);
    }

    #[test]
    fn a_single_paper_or_a_group_with_no_cut_is_not_cut() {
        assert_eq!(fiedler_side(Array2::<f32>::zeros((1, 1)).view()).unwrap(), None);
        // two linked papers: the Fiedler vector has one paper on each side
        let pair = ndarray::array![[0.0, 1.0], [1.0, 0.0]];
        let side = fiedler_side(pair.view()).unwrap().unwrap();
        assert_ne!(side[0], side[1]);
    }

    #[test]
    fn a_graph_larger_than_the_basis_still_finds_each_piece() {
        // 600 papers, so the solver has to restart; six pieces, so eigenvalue 1 is repeated six times
        let graph = cliques(6, 100, 0.0);
        let spectrum = leading_eigenvectors(Graph::Dense(graph.view()), 6).unwrap();
        assert!(spectrum.values.iter().all(|value| (value - 1.0).abs() < 1e-5), "{:?}", spectrum.values);
        let joined = cliques(6, 100, 0.001);
        let spectrum = leading_eigenvectors(Graph::Dense(joined.view()), 7).unwrap();
        assert!((spectrum.values[0] - 1.0).abs() < 1e-6);
        assert!(spectrum.values[5] > 0.95 && spectrum.values[6] < 0.05, "{:?}", spectrum.values);
    }

    #[test]
    fn the_streamed_product_agrees_with_the_matrix_library() {
        let mut random = crate::analysis::random::Random::new(3);
        // 23 rows: not a whole number of the rows taken together
        let matrix = Array2::from_shape_simple_fn((23, 23), || random.uniform() as f32);
        for width in [1, 3, 8, 9, 16, 20, 40] {
            let block = Array2::from_shape_simple_fn((23, width), || random.normal() as f32);
            let streamed = dense_product(matrix.view(), block.clone());
            let library = matrix.dot(&block);
            assert_eq!(streamed.dim(), (23, width));
            assert!((&streamed - &library).iter().all(|value| value.abs() < 1e-4), "width {width}");
        }
        // a matrix not laid out row by row goes to the library
        let transposed = matrix.t();
        let block = Array2::<f32>::ones((23, 2));
        assert!((&dense_product(transposed, block.clone()) - &transposed.dot(&block)).iter().all(|v| v.abs() < 1e-4));
    }

    #[test]
    fn the_median_of_odd_and_even_counts() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), 2.5);
    }
}
