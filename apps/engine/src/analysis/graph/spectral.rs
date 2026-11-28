//! What the eigenvectors of the normalised similarity graph say: the leading ones, which spectral clustering
//! works on, and the second one, whose signs cut a group in two (the Fiedler vector).
//!
//! The normalised graph is `D^-1/2 A D^-1/2`, where `D` holds the degrees. Its largest eigenvalue is 1, once for
//! every piece of a graph that falls apart, and vectors of eigenvalues close to 1 are nearly constant on groups
//! of papers that are linked among themselves far more than to the rest.

use ndarray::{Array1, Array2, ArrayView2, Axis};
use rayon::prelude::*;

use super::eigen::{EigenError, leading};
use super::links::SparseGraph;
use crate::analysis::graph::degrees;

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

    /// Which connected piece each paper is in, the pieces numbered in order of their lowest-numbered paper; and how
    /// many pieces there are.
    pub fn pieces(&self) -> (Vec<usize>, usize) {
        match self {
            Self::Dense(matrix) => components(*matrix),
            Self::Sparse(graph) => {
                let papers = graph.papers();
                let mut piece = vec![usize::MAX; papers];
                let mut count = 0;
                for start in 0..papers {
                    if piece[start] != usize::MAX {
                        continue;
                    }
                    piece[start] = count;
                    let mut waiting = vec![start];
                    while let Some(paper) = waiting.pop() {
                        for (other, weight) in graph.links(paper) {
                            if weight > 0.0 && piece[other] == usize::MAX {
                                piece[other] = count;
                                waiting.push(other);
                            }
                        }
                    }
                    count += 1;
                }
                (piece, count)
            }
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

/// `count` communities made of whole pieces of `graph`, when it falls apart into more pieces of linked papers
/// than that; `None` when it does not, and spectral clustering is well defined on it.
///
/// Why: a graph in `p` pieces has the eigenvalue 1 `p` times, and when `p` is more than `count` the `count`
/// leading eigenvectors are not determined by the graph: any `count` directions of that `p`-dimensional space are
/// as good, and which ones the solver returns is decided by rounding. k-means on the rows then groups the pieces
/// one way on one processor and another way on the next (the nearest-neighbour graph of two topics of two
/// sub-topics each is in 4 pieces, one per sub-topic; asked for 2 communities, the grouping changed under noise of
/// 1e-7 in the embeddings in 13 runs of 20). With `p` at most `count` the eigenvectors span every piece's own
/// direction plus well-defined others, and k-means, which only measures distances, does not depend on which basis
/// of that space it is given: such graphs, connected ones above all, are cut exactly as before.
///
/// The pieces are joined by their mutual similarity in the whole similarity graph `similarity` (not in
/// `graph`, whose pieces have no links between them): again and again the two groups of pieces with the highest
/// average similarity between their papers are joined (average linkage), the first such pair in the order of
/// their lowest-numbered papers on a tie, until `count` remain. That uses every similarity between two pieces, not
/// a few links, so the answer moves only where two joins are within rounding of each other. The communities are
/// numbered in the order of their lowest-numbered paper. Papers linked to nobody (no similarity above 0 to any
/// paper) are not a piece of their own, which would make them a community: they join the smallest community, the
/// first of equal ones.
pub fn joined_pieces(graph: Graph, similarity: ArrayView2<f32>, count: usize) -> Option<Vec<usize>> {
    let (piece, pieces) = graph.pieces();
    let mut sizes = vec![0_usize; pieces];
    for &one in &piece {
        sizes[one] += 1;
    }
    let linked: Vec<usize> = (0..pieces).filter(|&one| sizes[one] > 1).collect();
    if linked.len() <= count || similarity.dim() != (piece.len(), piece.len()) {
        return None;
    }
    // each piece of linked papers by its place in `linked`; papers linked to nobody have none
    let mut place = vec![usize::MAX; pieces];
    for (index, &one) in linked.iter().enumerate() {
        place[one] = index;
    }
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); linked.len()];
    for (paper, &one) in piece.iter().enumerate() {
        if place[one] != usize::MAX {
            members[place[one]].push(paper);
        }
    }
    // the similarity between every two pieces, summed in 64 bits: each piece's row on one thread, its papers and
    // their similarities in order, so that it is the same on one thread or many
    let mut between: Vec<Vec<f64>> = members
        .par_iter()
        .map(|papers| {
            let mut row = vec![0.0; linked.len()];
            for &paper in papers {
                for (other, value) in similarity.row(paper).iter().enumerate() {
                    if let Some(sum) = row.get_mut(place[piece[other]]) {
                        *sum += f64::from(*value);
                    }
                }
            }
            row
        })
        .collect();
    let mut size: Vec<f64> = members.iter().map(|papers| papers.len() as f64).collect();
    // the groups still open, by their first piece, in order; each piece's group
    let mut open: Vec<usize> = (0..linked.len()).collect();
    let mut group: Vec<usize> = (0..linked.len()).collect();
    while open.len() > count {
        let mut best = (0, 0, f64::NEG_INFINITY);
        for (index, &first) in open.iter().enumerate() {
            for &second in &open[index + 1..] {
                let average = between[first][second] / (size[first] * size[second]);
                if average > best.2 {
                    best = (first, second, average);
                }
            }
        }
        let (kept, joined, _) = best;
        for other in 0..linked.len() {
            let moved = between[joined][other];
            between[kept][other] += moved;
            let moved = between[other][joined];
            between[other][kept] += moved;
        }
        size[kept] += size[joined];
        open.retain(|&one| one != joined);
        for one in &mut group {
            if *one == joined {
                *one = kept;
            }
        }
    }
    let mut labels = vec![usize::MAX; piece.len()];
    let mut community_sizes = vec![0_usize; open.len()];
    for (index, papers) in members.iter().enumerate() {
        let label = open.iter().position(|&one| one == group[index]).unwrap_or(0);
        community_sizes[label] += papers.len();
        for &paper in papers {
            labels[paper] = label;
        }
    }
    let smallest =
        (0..open.len()).fold(0, |best, one| if community_sizes[one] < community_sizes[best] { one } else { best });
    for label in &mut labels {
        if *label == usize::MAX {
            *label = smallest;
        }
    }
    Some(labels)
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
/// A group in several pieces of linked papers is cut into its largest piece (the first of equal ones) and the
/// rest: no eigenvector is needed to see that cut. A connected group is cut by the sign of its Fiedler vector (the
/// second eigenvector, in the random-walk scaling); if every paper has the same sign, by the vector's median.
///
/// Papers linked to nobody in the group (an outlier whose only similar papers went to the other half of an
/// earlier cut, an embedding with nothing in it) say nothing about where to cut. They are set aside, the linked
/// papers cut as above, and they join the smaller side. Counted as pieces of their own they would make every cut
/// "the largest piece and a few strays", which no size limit accepts, and the group could never be cut again.
pub fn fiedler_side(sub: ArrayView2<f32>) -> Result<Option<Vec<bool>>, EigenError> {
    let papers = sub.nrows();
    if papers < 2 {
        return Ok(None);
    }
    let (piece, pieces) = components(sub);
    let mut sizes = vec![0; pieces];
    for &one in &piece {
        sizes[one] += 1;
    }
    let linked: Vec<usize> = (0..papers).filter(|&paper| sizes[piece[paper]] > 1).collect();
    if linked.len() < papers {
        if linked.len() < 2 {
            return Ok(None);
        }
        let inner: Array2<f32> =
            Array2::from_shape_fn((linked.len(), linked.len()), |(i, j)| sub[[linked[i], linked[j]]]);
        let Some(inner_side) = fiedler_side(inner.view())? else { return Ok(None) };
        let first = inner_side.iter().filter(|&&here| here).count();
        let strays_first = 2 * first < inner_side.len();
        let mut side = vec![strays_first; papers];
        for (&paper, &here) in linked.iter().zip(&inner_side) {
            side[paper] = here;
        }
        return Ok(Some(side));
    }
    if pieces > 1 {
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
    use crate::analysis::graph::links::knn_graph;

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
    fn papers_linked_to_nobody_are_set_aside_and_join_the_smaller_side() {
        // two cliques of 30 and 20, weakly joined, and papers 50 and 51 linked to nobody
        let mut graph = Array2::<f32>::zeros((52, 52));
        for i in 0..50 {
            for j in 0..50 {
                if i != j {
                    graph[[i, j]] = if (i < 30) == (j < 30) { 1.0 } else { 0.01 };
                }
            }
        }
        let side = fiedler_side(graph.view()).unwrap().unwrap();
        assert!(side[..30].iter().all(|here| *here == side[0]));
        assert!(side[30..50].iter().all(|here| *here == side[30]));
        assert_ne!(side[0], side[30]);
        assert!(side[50] == side[30] && side[51] == side[30], "the strays join the smaller clique");
        // pieces of linked papers are still cut into the largest and the rest, the strays with the rest
        let mut pieces = Array2::<f32>::zeros((7, 7));
        for (a, b) in [(0, 1), (1, 2), (3, 4)] {
            pieces[[a, b]] = 0.5;
            pieces[[b, a]] = 0.5;
        }
        assert_eq!(fiedler_side(pieces.view()).unwrap().unwrap(), vec![true, true, true, false, false, false, false]);
        // nothing linked at all: nothing to cut by
        assert_eq!(fiedler_side(Array2::<f32>::zeros((5, 5)).view()).unwrap(), None);
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
        let mut random = crate::analysis::graph::random::Random::new(3);
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
