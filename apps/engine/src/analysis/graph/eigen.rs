//! Eigenvectors of symmetric matrices: all of them for a small dense matrix, and the leading few of a large one
//! that is known only by its product with vectors.
//!
//! The large case is the normalised similarity graph, `D^-1/2 A D^-1/2`, at up to 30,000 papers. Forming it would
//! be a second matrix the size of the graph, so the solver never sees a matrix at all: it is handed a function that
//! multiplies a block of vectors, and builds everything from those products.
//!
//! The method is a thick-restart block Krylov method with Rayleigh-Ritz (in exact arithmetic, block Lanczos with
//! full reorthogonalisation):
//!
//! 1. Start from a block of random vectors (a fixed seed, so the same matrix always gives the same vectors).
//! 2. Grow an orthonormal basis `Q` by multiplying the newest block and removing from the result everything already
//!    in the basis, keeping the products `W = M Q` as they are made.
//! 3. When the basis is full, solve the small problem `Q^T W` exactly (the dense solver below): its eigenpairs give
//!    Ritz pairs, the best approximations to the eigenpairs of `M` within the basis, and `W` gives their products
//!    without another multiplication.
//! 4. Stop when every wanted pair has a small residual `|M x - t x|`. Otherwise keep the best Ritz vectors as the
//!    start of the next basis ("thick restart", so nothing learned is lost) and grow it from their residuals.
//!
//! Working in blocks rather than one vector at a time matters twice: an eigenvalue that is repeated (one per part of
//! a graph in several pieces) is found as many times as it is repeated, as long as the block is wider than the
//! repetition, which is why a block is wider than the number of vectors wanted; and reading a graph of 12,000 papers
//! once costs about as much as the arithmetic of a block of 16 vectors, so narrower blocks would waste the reads.

use ndarray::{Array1, Array2, ArrayView2, Axis, Zip, s};
use rayon::prelude::*;

use super::random::Random;

/// Extra vectors in a block beyond those wanted: the eigenvalues just below the wanted ones then converge too,
/// which keeps the last wanted ones from converging slowly.
const EXTRA: usize = 8;
/// The basis holds this many blocks before it is restarted. A deeper basis needs fewer products in all (the graph
/// of 12,487 random papers: 1,400 columns for 50 eigenvectors at 8 blocks, 2,300 at 4) but makes each restart's
/// small problem larger; 8 was the fastest of those measured.
const DEPTH: usize = 8;
/// A wanted pair is found when `|M x - t x|` is at most this. The graph is held in 32-bit numbers, whose products are
/// good to about 1e-6, so asking for much less would only be asking for noise.
pub const TOLERANCE: f64 = 1e-5;
/// Restarts before giving up. Each restart keeps everything the last one found, so this is far more than any graph
/// of papers needs; it is here so that a matrix that is not symmetric cannot loop for ever.
const MOST_RESTARTS: usize = 500;
/// A new direction shorter than this, relative to its length before what the basis holds was removed from it, is
/// already in the basis. Not much smaller: the Gram matrix squares lengths, so 1e-6 is already 1e-12 there.
const DEFLATION: f64 = 1e-6;
/// The seed of the start vectors.
const SEED: u64 = 0;

/// Eigenpairs, largest eigenvalue first; the vectors are the columns, each of length one.
#[derive(Clone, Debug)]
pub struct Eigen {
    pub values: Array1<f64>,
    pub vectors: Array2<f64>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EigenError {
    #[error("{wanted} eigenvectors were asked of a matrix of size {size}")]
    TooMany { wanted: usize, size: usize },
    #[error("the eigenvectors did not settle: {converged} of {wanted} after {restarts} restarts")]
    NotConverged { wanted: usize, converged: usize, restarts: usize },
}

/// Every eigenpair of the symmetric matrix `matrix`, largest eigenvalue first.
///
/// Householder reduction to a tridiagonal matrix, then the implicit QL method on it: the procedures `tred2` and
/// `tql2` of EISPACK, as written out in JAMA. It is meant for the small problems of the Krylov method (a few
/// hundred rows), where it takes milliseconds; only the lower triangle of `matrix` is read.
pub fn symmetric_eigen(matrix: ArrayView2<f64>) -> Eigen {
    let size = matrix.nrows();
    if size == 0 {
        return Eigen { values: Array1::zeros(0), vectors: Array2::zeros((0, 0)) };
    }
    let mut vectors = matrix.to_owned();
    let (mut diagonal, mut off_diagonal) = tridiagonalise(&mut vectors);
    diagonalise_tridiagonal(&mut vectors, &mut diagonal, &mut off_diagonal);
    let mut order: Vec<usize> = (0..size).collect();
    order.sort_by(|&a, &b| diagonal[b].total_cmp(&diagonal[a]));
    Eigen { values: order.iter().map(|&index| diagonal[index]).collect(), vectors: vectors.select(Axis(1), &order) }
}

/// Householder reduction of the symmetric matrix held in `v` to tridiagonal form. Returns the diagonal and the
/// off-diagonal (`off[i]` joins rows `i - 1` and `i`; `off[0]` is 0), and leaves in `v` the orthogonal matrix that
/// does the reduction.
fn tridiagonalise(v: &mut Array2<f64>) -> (Vec<f64>, Vec<f64>) {
    let n = v.nrows();
    let mut d: Vec<f64> = v.row(n - 1).to_vec();
    let mut e = vec![0.0; n];
    for i in (1..n).rev() {
        // scaled, so that squaring cannot overflow or underflow
        let scale: f64 = d[..i].iter().map(|value| value.abs()).sum();
        let mut h = 0.0;
        if scale == 0.0 {
            e[i] = d[i - 1];
            for j in 0..i {
                d[j] = v[[i - 1, j]];
                v[[i, j]] = 0.0;
                v[[j, i]] = 0.0;
            }
        } else {
            // the Householder vector
            for value in &mut d[..i] {
                *value /= scale;
                h += *value * *value;
            }
            let f = d[i - 1];
            let g = if f > 0.0 { -h.sqrt() } else { h.sqrt() };
            e[i] = scale * g;
            h -= f * g;
            d[i - 1] = f - g;
            e[..i].fill(0.0);
            // applied to the columns not yet reduced
            for j in 0..i {
                let f = d[j];
                v[[j, i]] = f;
                let mut g = e[j] + v[[j, j]] * f;
                for k in j + 1..i {
                    g += v[[k, j]] * d[k];
                    e[k] += v[[k, j]] * f;
                }
                e[j] = g;
            }
            let mut f = 0.0;
            for j in 0..i {
                e[j] /= h;
                f += e[j] * d[j];
            }
            let hh = f / (h + h);
            for j in 0..i {
                e[j] -= hh * d[j];
            }
            for j in 0..i {
                let (f, g) = (d[j], e[j]);
                for k in j..i {
                    v[[k, j]] -= f * e[k] + g * d[k];
                }
                d[j] = v[[i - 1, j]];
                v[[i, j]] = 0.0;
            }
        }
        d[i] = h;
    }
    // the transformations, multiplied together
    for i in 0..n - 1 {
        v[[n - 1, i]] = v[[i, i]];
        v[[i, i]] = 1.0;
        let h = d[i + 1];
        if h != 0.0 {
            for k in 0..=i {
                d[k] = v[[k, i + 1]] / h;
            }
            for j in 0..=i {
                let g: f64 = (0..=i).map(|k| v[[k, i + 1]] * v[[k, j]]).sum();
                for k in 0..=i {
                    v[[k, j]] -= g * d[k];
                }
            }
        }
        for k in 0..=i {
            v[[k, i + 1]] = 0.0;
        }
    }
    for j in 0..n {
        d[j] = v[[n - 1, j]];
        v[[n - 1, j]] = 0.0;
    }
    v[[n - 1, n - 1]] = 1.0;
    e[0] = 0.0;
    (d, e)
}

/// The implicit QL method with shifts on the tridiagonal matrix (`d`, `e`) from [`tridiagonalise`]. Leaves the
/// eigenvalues in `d` (in no particular order) and the eigenvectors in the columns of `v`.
fn diagonalise_tridiagonal(v: &mut Array2<f64>, d: &mut [f64], e: &mut [f64]) {
    /// Sweeps for one eigenvalue; it takes two or three, so this only stops a matrix of NaN from looping.
    const MOST_SWEEPS: usize = 64;
    let n = d.len();
    // the rotations below combine two eigenvectors at a time: held as rows, each is contiguous in memory
    let mut vectors = v.t().as_standard_layout().into_owned();
    for i in 1..n {
        e[i - 1] = e[i];
    }
    e[n - 1] = 0.0;
    let mut shift_total = 0.0;
    let mut largest = 0.0_f64;
    for l in 0..n {
        largest = largest.max(d[l].abs() + e[l].abs());
        // the first negligible off-diagonal element at or after l (e[n - 1] is 0, so there is one)
        let mut m = l;
        while m + 1 < n && e[m].abs() > f64::EPSILON * largest {
            m += 1;
        }
        if m > l {
            for _ in 0..MOST_SWEEPS {
                // the implicit shift
                let g = d[l];
                let mut p = (d[l + 1] - g) / (2.0 * e[l]);
                let r = if p < 0.0 { -p.hypot(1.0) } else { p.hypot(1.0) };
                d[l] = e[l] / (p + r);
                d[l + 1] = e[l] * (p + r);
                let dl1 = d[l + 1];
                let h = g - d[l];
                for value in &mut d[l + 2..n] {
                    *value -= h;
                }
                shift_total += h;
                // the QL transformation
                p = d[m];
                let (mut c, mut c2, mut c3) = (1.0, 1.0, 1.0);
                let el1 = e[l + 1];
                let (mut s, mut s2) = (0.0, 0.0);
                for i in (l..m).rev() {
                    c3 = c2;
                    c2 = c;
                    s2 = s;
                    let g = c * e[i];
                    let h = c * p;
                    let r = p.hypot(e[i]);
                    e[i + 1] = s * r;
                    s = e[i] / r;
                    c = p / r;
                    p = c * d[i] - s * g;
                    d[i + 1] = h + s * (c * g + s * d[i]);
                    let (mut low, mut high) = vectors.multi_slice_mut((s![i, ..], s![i + 1, ..]));
                    Zip::from(&mut low).and(&mut high).for_each(|low, high| {
                        let h = *high;
                        *high = s * *low + c * h;
                        *low = c * *low - s * h;
                    });
                }
                p = -s * s2 * c3 * el1 * e[l] / dl1;
                e[l] = s * p;
                d[l] = c * p;
                if e[l].abs() <= f64::EPSILON * largest {
                    break;
                }
            }
        }
        d[l] += shift_total;
        e[l] = 0.0;
    }
    v.assign(&vectors.t());
}

/// The `wanted` leading eigenpairs (largest eigenvalue first) of the symmetric `size x size` matrix `M` whose
/// product with a block of vectors (`size x b`) is `product(block)`.
///
/// The result depends only on `M` and `wanted`, and is the same on every run (on one kind of processor to the last
/// bit): the start vectors come from a fixed seed, every sum shared between cores is added up in a fixed order, and
/// each vector's sign is chosen so that its largest entry is positive.
pub fn leading<P>(size: usize, wanted: usize, product: P) -> Result<Eigen, EigenError>
where
    P: Fn(ArrayView2<f64>) -> Array2<f64>,
{
    if wanted > size {
        return Err(EigenError::TooMany { wanted, size });
    }
    if wanted == 0 {
        return Ok(Eigen { values: Array1::zeros(0), vectors: Array2::zeros((size, 0)) });
    }
    let block = (wanted + EXTRA).min(size);
    let capacity = (DEPTH * block).min(size);
    // What is kept across a restart. When the basis is smaller than the matrix it is DEPTH blocks, so keeping half
    // still leaves several blocks of room; when it is the whole space, the first Rayleigh-Ritz is exact and there
    // is no restart.
    let kept = (capacity / 2).max(wanted);
    let mut random = Random::new(SEED);
    let mut basis = Array2::<f64>::zeros((size, capacity));
    let mut images = Array2::<f64>::zeros((size, capacity));
    let mut filled = 0;
    let mut next = random_block(size, block, &mut random);
    let mut converged = 0;
    for restart in 0..=MOST_RESTARTS {
        filled = grow(&mut basis, &mut images, filled, next, &product, &mut random);
        let ritz = rayleigh_ritz(basis.slice(s![.., ..filled]), images.slice(s![.., ..filled]), kept.min(filled));
        let residuals = ritz.residuals();
        converged = residuals.iter().take(wanted).take_while(|residual| **residual <= TOLERANCE).count();
        if converged == wanted || filled == size {
            return Ok(ritz.into_leading(wanted));
        }
        if restart == MOST_RESTARTS {
            break;
        }
        // the restart: the best Ritz vectors begin the new basis, and their residuals point where it must grow
        let count = ritz.values.len();
        basis.slice_mut(s![.., ..count]).assign(&ritz.vectors);
        images.slice_mut(s![.., ..count]).assign(&ritz.images);
        filled = count;
        // a full block of them, from the pairs not yet settled, best first
        let unsettled: Vec<usize> = (0..count).filter(|&index| residuals[index] > TOLERANCE).take(block).collect();
        next = ritz.residual_vectors(&unsettled);
    }
    Err(EigenError::NotConverged { wanted, converged, restarts: MOST_RESTARTS })
}

/// Rows per piece when a tall matrix is cut into pieces for the cores.
const ROWS_PER_PIECE: usize = 512;

/// `a b` for a tall `a` (one row per paper) and a small `b`: the rows of `a` cut into pieces, each multiplied on
/// its own core. The basis of the solver is thousands of rows by a few hundred columns, and the matrix library
/// multiplies on one thread (it is built without threads of its own, so that these pieces do not each start
/// more: [`crate::threads`]). Each piece is written by one thread and the pieces are put together in order, so
/// the product does not depend on how they were shared out.
fn tall_dot(a: ArrayView2<f64>, b: ArrayView2<f64>) -> Array2<f64> {
    let pieces: Vec<Array2<f64>> =
        a.axis_chunks_iter(Axis(0), ROWS_PER_PIECE).into_par_iter().map(|piece| piece.dot(&b)).collect();
    let views: Vec<ArrayView2<f64>> = pieces.iter().map(|piece| piece.view()).collect();
    ndarray::concatenate(Axis(0), &views).unwrap_or_else(|_| a.dot(&b))
}

/// `a^T b` for two tall matrices of the same rows: the pieces' products are made on every core and then added in
/// order, so that the sum is the same however the work was shared out.
fn tall_transpose_dot(a: ArrayView2<f64>, b: ArrayView2<f64>) -> Array2<f64> {
    let pieces: Vec<Array2<f64>> = a
        .axis_chunks_iter(Axis(0), ROWS_PER_PIECE)
        .into_par_iter()
        .zip(b.axis_chunks_iter(Axis(0), ROWS_PER_PIECE))
        .map(|(left, right)| left.t().dot(&right))
        .collect();
    pieces.into_iter().reduce(|sum, piece| sum + piece).unwrap_or_else(|| Array2::zeros((a.ncols(), b.ncols())))
}

/// A `size x count` block of numbers from the standard normal distribution.
fn random_block(size: usize, count: usize, random: &mut Random) -> Array2<f64> {
    Array2::from_shape_simple_fn((size, count), || random.normal())
}

/// Fill the basis from column `filled` on: each new block is what is left of the products of the last one once
/// the basis is taken out. Returns how many columns are filled.
fn grow<P>(
    basis: &mut Array2<f64>,
    images: &mut Array2<f64>,
    mut filled: usize,
    mut candidates: Array2<f64>,
    product: &P,
    random: &mut Random,
) -> usize
where
    P: Fn(ArrayView2<f64>) -> Array2<f64>,
{
    let (size, capacity) = basis.dim();
    while filled < capacity {
        let room = capacity - filled;
        let mut fresh = complement(basis.slice(s![.., ..filled]), candidates, room);
        if fresh.ncols() == 0 {
            // the basis holds all that the products lead to (an invariant subspace): look somewhere new
            fresh = complement(basis.slice(s![.., ..filled]), random_block(size, room, random), room);
            if fresh.ncols() == 0 {
                break;
            }
        }
        let count = fresh.ncols();
        let image = product(fresh.view());
        basis.slice_mut(s![.., filled..filled + count]).assign(&fresh);
        images.slice_mut(s![.., filled..filled + count]).assign(&image);
        filled += count;
        candidates = image;
    }
    filled
}

/// The columns of `candidates` with everything in `basis` (orthonormal columns) taken out, made orthonormal among
/// themselves, at most `most` of them. A column that was already (nearly) in the span is dropped.
///
/// Everything is done with matrix products, which run on every core: a column-by-column Gram-Schmidt was most of
/// the solver's time.
fn complement(basis: ArrayView2<f64>, candidates: Array2<f64>, most: usize) -> Array2<f64> {
    let mut block = unit_columns(candidates);
    // twice: once is not enough when the candidates lay mostly inside the basis ("twice is enough", Kahan)
    for pass in 0..2 {
        if basis.ncols() > 0 {
            let along = tall_transpose_dot(basis, block.view());
            block -= &tall_dot(basis, along.view());
        }
        // the first pass keeps every direction that survives, so that the second chooses among all of them
        block = orthonormal_columns(block, if pass == 0 { usize::MAX } else { most });
    }
    block
}

/// The columns scaled to length one; columns of zeros are dropped.
fn unit_columns(block: Array2<f64>) -> Array2<f64> {
    let lengths: Vec<f64> = block.columns().into_iter().map(|column| column.dot(&column).sqrt()).collect();
    let kept: Vec<usize> = (0..lengths.len()).filter(|&index| lengths[index] > 0.0).collect();
    let mut unit = block.select(Axis(1), &kept);
    for (mut column, &index) in unit.columns_mut().into_iter().zip(&kept) {
        column /= lengths[index];
    }
    unit
}

/// An orthonormal basis of the span of the columns, at most `most` of them, the most significant directions first
/// (SVQB, Stathopoulos and Wu, 2002): from the eigenvectors `V` and eigenvalues `L` of the columns' Gram matrix,
/// `block V L^-1/2` is orthonormal. A direction whose eigenvalue says less than [`DEFLATION`] of a column's length
/// is left of it is dropped: it was in the span already.
fn orthonormal_columns(block: Array2<f64>, most: usize) -> Array2<f64> {
    let gram = tall_transpose_dot(block.view(), block.view());
    let eigen = symmetric_eigen(gram.view());
    let kept: Vec<usize> =
        (0..eigen.values.len()).filter(|&index| eigen.values[index] > DEFLATION * DEFLATION).take(most).collect();
    let mut rotation = eigen.vectors.select(Axis(1), &kept);
    for (mut column, &index) in rotation.columns_mut().into_iter().zip(&kept) {
        column /= eigen.values[index].sqrt();
    }
    tall_dot(block.view(), rotation.view())
}

/// The best approximations to eigenpairs within a basis.
struct Ritz {
    values: Array1<f64>,
    vectors: Array2<f64>,
    /// `M` times each vector, worked out from the products already made.
    images: Array2<f64>,
}

/// The `count` leading Ritz pairs of the basis `q` whose products are `w`.
fn rayleigh_ritz(q: ArrayView2<f64>, w: ArrayView2<f64>, count: usize) -> Ritz {
    let projected = tall_transpose_dot(q, w);
    // the products carry rounding, so the small matrix is made exactly symmetric before it is solved
    let symmetric = (&projected + &projected.t()) / 2.0;
    let small = symmetric_eigen(symmetric.view());
    let leading = small.vectors.slice(s![.., ..count]);
    Ritz {
        values: small.values.slice(s![..count]).to_owned(),
        vectors: tall_dot(q, leading),
        images: tall_dot(w, leading),
    }
}

impl Ritz {
    /// `M x - t x` for each listed pair, as the columns of a block.
    fn residual_vectors(&self, pairs: &[usize]) -> Array2<f64> {
        let mut block = self.images.select(Axis(1), pairs);
        for (mut column, &pair) in block.columns_mut().into_iter().zip(pairs) {
            column.scaled_add(-self.values[pair], &self.vectors.column(pair));
        }
        block
    }

    /// `|M x - t x|` for every pair.
    fn residuals(&self) -> Vec<f64> {
        let all: Vec<usize> = (0..self.values.len()).collect();
        self.residual_vectors(&all).columns().into_iter().map(|column| column.dot(&column).sqrt()).collect()
    }

    /// The first `wanted` pairs, each vector signed so that its largest entry is positive.
    fn into_leading(self, wanted: usize) -> Eigen {
        let mut vectors = self.vectors.slice(s![.., ..wanted]).to_owned();
        for mut column in vectors.columns_mut() {
            let largest =
                column.iter().copied().fold(0.0_f64, |best, value| if value.abs() > best.abs() { value } else { best });
            if largest < 0.0 {
                column.mapv_inplace(|value| -value);
            }
        }
        Eigen { values: self.values.slice(s![..wanted]).to_owned(), vectors }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ndarray::array;

    /// A random symmetric matrix.
    fn symmetric(size: usize, seed: u64) -> Array2<f64> {
        let mut random = Random::new(seed);
        let half = Array2::from_shape_simple_fn((size, size), || random.normal());
        (&half + &half.t()) / 2.0
    }

    /// `|M V - V diag(values)|`, largest over all entries, and `|V^T V - I|`.
    fn errors(matrix: &Array2<f64>, eigen: &Eigen) -> (f64, f64) {
        let product = matrix.dot(&eigen.vectors);
        let scaled = &eigen.vectors * &eigen.values.view().insert_axis(Axis(0));
        let fit = (&product - &scaled).iter().fold(0.0_f64, |most, value| most.max(value.abs()));
        let gram = eigen.vectors.t().dot(&eigen.vectors);
        let identity = Array2::<f64>::eye(gram.nrows());
        let orthogonal = (&gram - &identity).iter().fold(0.0_f64, |most, value| most.max(value.abs()));
        (fit, orthogonal)
    }

    #[test]
    fn a_small_symmetric_matrix_is_solved_exactly_with_the_largest_eigenvalue_first() {
        let matrix = array![[2.0, 1.0], [1.0, 2.0]];
        let eigen = symmetric_eigen(matrix.view());
        assert!((eigen.values[0] - 3.0).abs() < 1e-12 && (eigen.values[1] - 1.0).abs() < 1e-12);
        let (fit, orthogonal) = errors(&matrix, &eigen);
        assert!(fit < 1e-12 && orthogonal < 1e-12);
    }

    #[test]
    fn a_random_symmetric_matrix_is_solved_to_rounding() {
        for size in [1, 3, 17, 120] {
            let matrix = symmetric(size, size as u64);
            let eigen = symmetric_eigen(matrix.view());
            let (fit, orthogonal) = errors(&matrix, &eigen);
            assert!(fit < 1e-10 && orthogonal < 1e-10, "size {size}: {fit} {orthogonal}");
            assert!(eigen.values.windows(2).into_iter().all(|pair| pair[0] >= pair[1]), "largest first");
        }
    }

    #[test]
    fn repeated_and_zero_eigenvalues_are_found_as_often_as_they_repeat() {
        let matrix = Array2::from_diag(&array![0.0, 5.0, 0.0, 5.0, -1.0]);
        let eigen = symmetric_eigen(matrix.view());
        assert_eq!(eigen.values, array![5.0, 5.0, 0.0, 0.0, -1.0]);
        assert_eq!(symmetric_eigen(Array2::<f64>::zeros((0, 0)).view()).values.len(), 0);
    }

    #[test]
    fn the_leading_eigenpairs_are_found_from_products_alone() {
        let matrix = symmetric(300, 9);
        let exact = symmetric_eigen(matrix.view());
        let found = leading(300, 6, |block| matrix.dot(&block)).unwrap();
        for index in 0..6 {
            assert!((found.values[index] - exact.values[index]).abs() < 1e-8, "{index}");
            let overlap = found.vectors.column(index).dot(&exact.vectors.column(index)).abs();
            assert!((overlap - 1.0).abs() < 1e-6, "{index}: {overlap}");
        }
        let (fit, orthogonal) = errors(&matrix, &found);
        assert!(fit < TOLERANCE && orthogonal < 1e-8, "{fit} {orthogonal}");
    }

    #[test]
    fn the_leading_eigenpairs_are_the_same_every_time() {
        let matrix = symmetric(200, 4);
        let first = leading(200, 3, |block| matrix.dot(&block)).unwrap();
        let again = leading(200, 3, |block| matrix.dot(&block)).unwrap();
        assert_eq!(first.vectors, again.vectors);
        for column in first.vectors.columns() {
            let largest =
                column.iter().copied().fold(0.0_f64, |best, value| if value.abs() > best.abs() { value } else { best });
            assert!(largest > 0.0, "each vector's largest entry is positive");
        }
    }

    #[test]
    fn a_matrix_smaller_than_a_block_is_solved_whole() {
        let matrix = symmetric(5, 1);
        let found = leading(5, 5, |block| matrix.dot(&block)).unwrap();
        let (fit, _) = errors(&matrix, &found);
        assert!(fit < 1e-10);
        assert_eq!(leading(5, 6, |block| matrix.dot(&block)).unwrap_err(), EigenError::TooMany { wanted: 6, size: 5 });
        assert_eq!(leading(5, 0, |block| matrix.dot(&block)).unwrap().vectors.dim(), (5, 0));
    }
}
