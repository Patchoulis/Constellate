//! How good a clustering of a run's papers is, by scores that cover each other's blind spots (what
//! `examples/compare.rs` reports, as the Python's `rp/compare.py` did):
//!
//! * agreement with a reference labelling: adjusted mutual information ([`adjusted_mutual_information`],
//!   corrected for chance, the arithmetic mean of the entropies as scikit-learn's default) and the adjusted Rand
//!   index ([`adjusted_rand_index`]);
//! * coherence of a community's wording: the mean normalised pointwise mutual information of its top terms over
//!   the abstracts they appear in ([`coherence`]);
//! * internal quality: the silhouette with cosine distance ([`silhouette_cosine`]).
//!
//! Each is the textbook definition, written to give the numbers scikit-learn gives (the tests hold values computed
//! with it).

use std::collections::HashMap;

use ndarray::ArrayView2;
use rayon::prelude::*;

use super::vocabulary::Vocabulary;

/// Labels of any kind turned into numbers from 0, in the order first met.
pub fn numbered<T: std::hash::Hash + Eq + Clone>(labels: &[T]) -> Vec<usize> {
    let mut seen: HashMap<T, usize> = HashMap::new();
    labels
        .iter()
        .map(|label| {
            let next = seen.len();
            *seen.entry(label.clone()).or_insert(next)
        })
        .collect()
}

/// The table of how many items have label `i` in `a` and `j` in `b`, with its row and column sums.
fn contingency(a: &[usize], b: &[usize]) -> (Vec<Vec<u64>>, Vec<u64>, Vec<u64>) {
    let rows = a.iter().max().map_or(0, |most| most + 1);
    let columns = b.iter().max().map_or(0, |most| most + 1);
    let mut table = vec![vec![0_u64; columns]; rows];
    for (&i, &j) in a.iter().zip(b) {
        table[i][j] += 1;
    }
    let row_sums: Vec<u64> = table.iter().map(|row| row.iter().sum()).collect();
    let column_sums: Vec<u64> = (0..columns).map(|j| table.iter().map(|row| row[j]).sum()).collect();
    (table, row_sums, column_sums)
}

fn entropy(sums: &[u64], total: f64) -> f64 {
    sums.iter()
        .filter(|count| **count > 0)
        .map(|&count| {
            let share = count as f64 / total;
            -share * share.ln()
        })
        .sum()
}

/// Adjusted mutual information of two labellings of the same items (scikit-learn's
/// `adjusted_mutual_info_score`, arithmetic mean): 1 for the same grouping, about 0 for unrelated ones.
pub fn adjusted_mutual_information(a: &[usize], b: &[usize]) -> f64 {
    let items = a.len();
    let (table, rows, columns) = contingency(a, b);
    let (used_rows, used_columns) =
        (rows.iter().filter(|c| **c > 0).count(), columns.iter().filter(|c| **c > 0).count());
    if items == 0 || (used_rows == used_columns && (used_rows <= 1 || a == b)) {
        return 1.0; // as scikit-learn: one group on both sides, or the same groups
    }
    let total = items as f64;
    let mut mutual = 0.0;
    for (i, row) in table.iter().enumerate() {
        for (j, &count) in row.iter().enumerate() {
            if count > 0 {
                let count = count as f64;
                mutual += count / total * (total * count / (rows[i] as f64 * columns[j] as f64)).ln();
            }
        }
    }
    // the mutual information two labellings with these group sizes have on average, by chance
    let mut log_factorial = vec![0.0_f64; items + 1];
    for n in 1..=items {
        log_factorial[n] = log_factorial[n - 1] + (n as f64).ln();
    }
    let mut expected = 0.0;
    for &row in rows.iter().filter(|count| **count > 0) {
        for &column in columns.iter().filter(|count| **count > 0) {
            let (ai, bj) = (row as usize, column as usize);
            let low = (ai + bj).saturating_sub(items).max(1);
            for nij in low..=ai.min(bj) {
                let share = nij as f64 / total;
                let term = share * (total * nij as f64 / (ai as f64 * bj as f64)).ln();
                let chance =
                    log_factorial[ai] + log_factorial[bj] + log_factorial[items - ai] + log_factorial[items - bj]
                        - log_factorial[items]
                        - log_factorial[nij]
                        - log_factorial[ai - nij]
                        - log_factorial[bj - nij]
                        - log_factorial[items - ai - bj + nij];
                expected += term * chance.exp();
            }
        }
    }
    let normaliser = (entropy(&rows, total) + entropy(&columns, total)) / 2.0;
    let mut denominator = normaliser - expected;
    let tiny = f64::EPSILON;
    denominator = if denominator < 0.0 { denominator.min(-tiny) } else { denominator.max(tiny) };
    (mutual - expected) / denominator
}

/// The adjusted Rand index of two labellings (scikit-learn's `adjusted_rand_score`).
pub fn adjusted_rand_index(a: &[usize], b: &[usize]) -> f64 {
    let (table, rows, columns) = contingency(a, b);
    let pairs = |count: u64| (count * count.saturating_sub(1) / 2) as f64;
    let together: f64 = table.iter().flatten().map(|&count| pairs(count)).sum();
    let (in_rows, in_columns): (f64, f64) =
        (rows.iter().map(|&c| pairs(c)).sum(), columns.iter().map(|&c| pairs(c)).sum());
    let all = pairs(a.len() as u64);
    let expected = if all > 0.0 { in_rows * in_columns / all } else { 0.0 };
    let most = (in_rows + in_columns) / 2.0;
    if (most - expected).abs() < f64::EPSILON {
        return 1.0; // as scikit-learn: nothing to adjust (one group, or every item alone, on both sides)
    }
    (together - expected) / (most - expected)
}

/// The mean silhouette of `labels` over the rows of `points`, with cosine distance (scikit-learn's
/// `silhouette_score(..., metric="cosine")`): how much nearer each point is to its own group than to the next.
pub fn silhouette_cosine(points: ArrayView2<f32>, labels: &[usize]) -> f64 {
    let count = points.nrows();
    let groups = labels.iter().max().map_or(0, |most| most + 1);
    let sizes: Vec<usize> = (0..groups).map(|group| labels.iter().filter(|label| **label == group).count()).collect();
    let lengths: Vec<f64> =
        points.outer_iter().map(|row| row.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt()).collect();
    let scores: Vec<f64> = (0..count)
        .into_par_iter()
        .map(|i| {
            if sizes[labels[i]] <= 1 {
                return 0.0;
            }
            let mut distance = vec![0.0_f64; groups];
            for j in 0..count {
                if i == j {
                    continue;
                }
                let dot: f64 =
                    points.row(i).iter().zip(points.row(j)).map(|(a, b)| f64::from(*a) * f64::from(*b)).sum();
                let lengths = lengths[i] * lengths[j];
                let cosine = if lengths > 0.0 { dot / lengths } else { 0.0 };
                distance[labels[j]] += 1.0 - cosine;
            }
            let own = distance[labels[i]] / (sizes[labels[i]] - 1) as f64;
            let nearest = (0..groups)
                .filter(|&group| group != labels[i] && sizes[group] > 0)
                .map(|group| distance[group] / sizes[group] as f64)
                .fold(f64::INFINITY, f64::min);
            if !nearest.is_finite() { 0.0 } else { (nearest - own) / own.max(nearest) }
        })
        .collect();
    scores.iter().sum::<f64>() / count.max(1) as f64
}

/// The mean normalised pointwise mutual information between the `top` terms of each group (the Python's
/// `compare._coherence`): +1 when the terms always appear together, -1 when two never share an abstract.
pub fn coherence(labels: &[usize], vocabulary: &Vocabulary, top: usize) -> f64 {
    let documents = labels.len() as f64;
    let groups = labels.iter().max().map_or(0, |most| most + 1);
    let mut scores = Vec::new();
    for group in 0..groups {
        let members: Vec<usize> = (0..labels.len()).filter(|&paper| labels[paper] == group).collect();
        if members.is_empty() {
            continue;
        }
        let terms: Vec<usize> =
            vocabulary.keywords(&members, None, top).iter().filter_map(|term| vocabulary.number(term)).collect();
        let mut single = vec![0.0_f64; terms.len()];
        let mut joint = vec![vec![0.0_f64; terms.len()]; terms.len()];
        for paper in 0..labels.len() {
            let has: Vec<bool> = terms
                .iter()
                .map(|term| {
                    vocabulary.counts(paper).binary_search_by(|(number, _)| (*number as usize).cmp(term)).is_ok()
                })
                .collect();
            for a in 0..terms.len() {
                if has[a] {
                    single[a] += 1.0;
                    for b in a + 1..terms.len() {
                        if has[b] {
                            joint[a][b] += 1.0;
                        }
                    }
                }
            }
        }
        for a in 0..terms.len() {
            for b in a + 1..terms.len() {
                let together = joint[a][b] / documents;
                scores.push(if together > 0.0 {
                    (together / ((single[a] / documents) * (single[b] / documents))).ln() / -together.ln()
                } else {
                    -1.0
                });
            }
        }
    }
    if scores.is_empty() { f64::NAN } else { scores.iter().sum::<f64>() / scores.len() as f64 }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Values computed with scikit-learn 1.9.1 for the same labels.
    #[test]
    fn agreement_scores_are_scikit_learns() {
        let (a, b) = ([0, 0, 0, 1, 1, 1, 2, 2, 2, 2], [0, 0, 1, 1, 1, 2, 2, 2, 0, 0]);
        assert!((adjusted_mutual_information(&a, &b) - 0.17152423540072853).abs() < 1e-12);
        assert!((adjusted_rand_index(&a, &b) - 0.09090909090909091).abs() < 1e-12);
        let c = numbered(&["x", "x", "y", "y", "y", "z", "z", "x", "y", "z", "z", "z"]);
        let d = numbered(&[1, 1, 1, 2, 2, 2, 3, 3, 3, 1, 2, 3]);
        assert!((adjusted_mutual_information(&c, &d) - -0.09327039292868426).abs() < 1e-12);
        assert!((adjusted_rand_index(&c, &d) - -0.08873720136518772).abs() < 1e-12);
        assert_eq!(adjusted_mutual_information(&[0, 0, 1, 1], &[0, 0, 1, 1]), 1.0);
        assert_eq!(adjusted_mutual_information(&[0, 0, 0], &[0, 0, 0]), 1.0);
    }

    #[test]
    fn the_silhouette_is_scikit_learns() {
        let points = ndarray::array![[1.0_f32, 0.0], [0.9, 0.1], [0.0, 1.0], [0.1, 0.9], [1.0, 1.0], [0.8, 0.7]];
        assert!((silhouette_cosine(points.view(), &[0, 0, 1, 1, 2, 2]) - 0.9804902672767639).abs() < 1e-6);
    }

    #[test]
    fn terms_that_always_appear_together_are_coherent_and_ones_that_never_do_are_not() {
        let texts: Vec<String> = (0..20)
            .map(|i| if i < 10 { "graph neural message".to_owned() } else { "diffusion score noise".to_owned() })
            .collect();
        let vocabulary = Vocabulary::new(&texts);
        let labels: Vec<usize> = (0..20).map(|i| usize::from(i >= 10)).collect();
        assert!(coherence(&labels, &vocabulary, 3) > 0.99);
        let mixed: Vec<usize> = (0..20).map(|i| i % 2).collect();
        assert!(coherence(&mixed, &vocabulary, 6) < coherence(&labels, &vocabulary, 6));
    }
}
