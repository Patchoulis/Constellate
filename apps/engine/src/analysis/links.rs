//! Each paper's closest papers: the few links worth drawing, and the nearest-neighbour graph one clustering method
//! works on.
//!
//! Both read a paper's row of the similarity graph and want only its first few entries in order of similarity.
//! Sorting a row of 12,000 to keep 15 would be wasted work, so the row is partly ordered instead: a selection puts
//! the best few first (in linear time), and only those are sorted.
//!
//! Each paper's row is read on whichever thread rayon gives it, and its answer depends on that row alone (ties go
//! to the lower index, so not even the selection's inner order shows); the answers are collected in the papers'
//! order and joined afterwards on one thread. The links and the graph are the same however many threads did it.

use ndarray::{Array2, ArrayView1, ArrayView2};
use rayon::prelude::*;

/// How many neighbours each paper keeps in the nearest-neighbour graph.
pub const KNN: usize = 15;

/// A link to draw between two papers (`papers.0 < papers.1`), with their similarity rounded to three decimals,
/// which is all a drawing needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    pub papers: (usize, usize),
    pub similarity: f32,
}

/// The `count` papers most similar to `paper` (never itself), most similar first; ties go to the lower index, so
/// the answer does not depend on how the selection happened to run.
///
/// `others` is room to select in, a row long, kept by the thread from one paper to the next (rayon's
/// `map_init`): it is cleared before use, so what it held before changes nothing. Without it every paper would
/// allocate a row's worth of indexes, and keep it in its answer (a truncated vector keeps its room): for 15,000
/// papers that was 1.8 GB allocated and freed again, which took as long as the selections.
fn nearest(row: ArrayView1<f32>, paper: usize, count: usize, others: &mut Vec<usize>) -> Vec<usize> {
    let closer = |a: &usize, b: &usize| row[*b].total_cmp(&row[*a]).then(a.cmp(b));
    others.clear();
    others.extend((0..row.len()).filter(|&other| other != paper));
    let count = count.min(others.len());
    if count == 0 {
        return Vec::new();
    }
    if count < others.len() {
        others.select_nth_unstable_by(count - 1, closer);
    }
    let mut best = others[..count].to_vec();
    best.sort_unstable_by(closer);
    best
}

/// How many of a paper's best matches, given most similar first, come before the largest drop in similarity: where
/// "close" ends. With fewer than two there is no drop, and all are kept.
fn before_the_largest_drop(similarities: &[f32]) -> usize {
    let mut largest = (0, f32::NEG_INFINITY);
    for (index, pair) in similarities.windows(2).enumerate() {
        let drop = pair[0] - pair[1];
        if drop > largest.1 {
            largest = (index, drop);
        }
    }
    if similarities.len() < 2 { similarities.len() } else { largest.0 + 1 }
}

/// The few links worth drawing, each pair once.
///
/// A paper is linked to its most similar papers, as many as come before the largest drop in its own ranking: among
/// its `most + 1` best matches, the biggest gap between one similarity and the next is where "close" ends. One
/// near-duplicate gives one link; a tight group of five gives five; nothing gets more than `most`. Links of no
/// similarity are not drawn. The links come in the order the papers are read, each where it is first found.
pub fn natural_neighbours(adjacency: ArrayView2<f32>, most: usize) -> Vec<Link> {
    let papers = adjacency.nrows();
    let count = (most + 1).min(papers.saturating_sub(1));
    let per_paper: Vec<Vec<Link>> = (0..papers)
        .into_par_iter()
        .map_init(Vec::new, |others, paper| {
            let row = adjacency.row(paper);
            let best = nearest(row, paper, count, others);
            let similarities: Vec<f32> = best.iter().map(|&other| row[other]).collect();
            let kept = before_the_largest_drop(&similarities);
            best[..kept]
                .iter()
                .zip(&similarities)
                .filter(|(_, similarity)| **similarity > 0.0)
                .map(|(&other, &similarity)| Link {
                    papers: (paper.min(other), paper.max(other)),
                    similarity: (similarity * 1000.0).round() / 1000.0,
                })
                .collect()
        })
        .collect();
    let mut seen = std::collections::HashSet::new();
    per_paper.into_iter().flatten().filter(|link| seen.insert(link.papers)).collect()
}

/// A graph that keeps only a few links per paper, stored by rows: the links of paper `i` are
/// `targets[starts[i]..starts[i + 1]]` with their `weights`, in order of target.
#[derive(Clone, Debug, PartialEq)]
pub struct SparseGraph {
    starts: Vec<usize>,
    targets: Vec<usize>,
    weights: Vec<f32>,
}

impl SparseGraph {
    /// A graph from each paper's links as `(target, weight)`, in order of target, one link per target.
    fn from_rows(rows: Vec<Vec<(usize, f32)>>) -> Self {
        let mut graph = Self { starts: vec![0], targets: Vec::new(), weights: Vec::new() };
        for row in rows {
            for (target, weight) in row {
                graph.targets.push(target);
                graph.weights.push(weight);
            }
            graph.starts.push(graph.targets.len());
        }
        graph
    }

    /// How many papers the graph is over.
    pub fn papers(&self) -> usize {
        self.starts.len() - 1
    }

    /// The links of one paper, as `(target, weight)`.
    pub fn links(&self, paper: usize) -> impl Iterator<Item = (usize, f32)> + '_ {
        let range = self.starts[paper]..self.starts[paper + 1];
        self.targets[range.clone()].iter().copied().zip(self.weights[range].iter().copied())
    }

    /// Each paper's degree: the sum of its links' weights.
    pub fn degrees(&self) -> Vec<f64> {
        (0..self.papers()).map(|paper| self.links(paper).map(|(_, weight)| f64::from(weight)).sum()).collect()
    }

    /// The graph's matrix times a block of vectors (`papers x b`).
    pub fn product(&self, block: ArrayView2<f64>) -> Array2<f64> {
        let mut result = Array2::<f64>::zeros(block.dim());
        result.outer_iter_mut().into_par_iter().enumerate().for_each(|(paper, mut row)| {
            for (target, weight) in self.links(paper) {
                row.scaled_add(f64::from(weight), &block.row(target));
            }
        });
        result
    }

    /// The graph as a dense matrix; for tests and small groups.
    pub fn to_dense(&self) -> Array2<f32> {
        let mut dense = Array2::<f32>::zeros((self.papers(), self.papers()));
        for paper in 0..self.papers() {
            for (target, weight) in self.links(paper) {
                dense[[paper, target]] = weight;
            }
        }
        dense
    }
}

/// Each paper keeps the links to its `neighbours` most similar papers; a link holds if either end lists it, with
/// the weight it has in the similarity graph. Links of no similarity are left out, as they change nothing.
pub fn knn_graph(adjacency: ArrayView2<f32>, neighbours: usize) -> SparseGraph {
    let papers = adjacency.nrows();
    let chosen: Vec<Vec<usize>> = (0..papers)
        .into_par_iter()
        .map_init(Vec::new, |others, paper| nearest(adjacency.row(paper), paper, neighbours, others))
        .collect();
    let mut rows: Vec<Vec<(usize, f32)>> = vec![Vec::new(); papers];
    for (paper, targets) in chosen.iter().enumerate() {
        for &target in targets {
            let weight = adjacency[[paper, target]];
            if weight > 0.0 {
                rows[paper].push((target, weight));
                rows[target].push((paper, weight));
            }
        }
    }
    for row in &mut rows {
        // A link listed by both ends is there twice. The larger copy is kept (the copies differ only if the matrix is
        // off symmetric by rounding), so that the graph is exactly symmetric, as the eigensolver needs.
        row.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
        row.dedup_by_key(|link| link.0);
    }
    SparseGraph::from_rows(rows)
}

/// A set of a run's papers, most central to the set first: by the sum of the similarities of their links to the
/// set's other papers (`links` as the run's graph has them: node, node, similarity; `nodes` the graph's ids). A
/// paper the graph does not have, or with no link in the set, counts nothing; ties go by id, so the order is the
/// same every time. What the features that ask a model of a sample first (extraction, stances) take their sample
/// from: its first papers are the ones most like the rest.
pub fn central_first(papers: &[String], nodes: &[String], links: &[(usize, usize, f64)]) -> Vec<String> {
    let index_of: std::collections::HashMap<&str, usize> =
        nodes.iter().enumerate().map(|(index, id)| (id.as_str(), index)).collect();
    let mut in_set = vec![false; nodes.len()];
    for paper in papers {
        if let Some(&index) = index_of.get(paper.as_str()) {
            in_set[index] = true;
        }
    }
    let mut weight = vec![0.0f64; nodes.len()];
    for &(a, b, similarity) in links {
        if a != b && in_set.get(a) == Some(&true) && in_set.get(b) == Some(&true) && similarity.is_finite() {
            weight[a] += similarity;
            weight[b] += similarity;
        }
    }
    let weight_of = |paper: &String| index_of.get(paper.as_str()).map_or(0.0, |&index| weight[index]);
    let mut ordered: Vec<String> = papers.to_vec();
    ordered.sort_unstable();
    ordered.dedup();
    ordered.sort_by(|a, b| weight_of(b).total_cmp(&weight_of(a)).then_with(|| a.cmp(b)));
    ordered
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ndarray::array;

    #[test]
    fn the_most_central_papers_of_a_set_come_first_ties_by_id() {
        let nodes: Vec<String> = ["a", "b", "c", "d", "e"].iter().map(|id| (*id).to_owned()).collect();
        // d is linked to the most; e is outside the set, so its link to a does not count; a link of no finite
        // similarity counts nothing
        let links = [(0, 3, 0.9), (1, 3, 0.8), (2, 3, 0.7), (0, 1, 0.5), (0, 4, 5.0), (2, 1, f64::NAN)];
        let set: Vec<String> = ["c", "b", "a", "d", "zz", "a"].iter().map(|id| (*id).to_owned()).collect();
        let ordered = central_first(&set, &nodes, &links);
        assert_eq!(ordered, ["d", "a", "b", "c", "zz"], "each paper once; one the graph lacks counts nothing");
        assert_eq!(central_first(&set, &nodes, &links), ordered, "the same every time");
        assert_eq!(central_first(&set, &nodes, &[]), ["a", "b", "c", "d", "zz"], "no links: by id");
    }

    #[test]
    fn links_stop_at_the_gap_in_similarity() {
        // papers 0, 1, 2 are close to each other and far from the rest; so are 3, 4 and 5, 6; 7 leans toward 6
        let mut adjacency = Array2::<f32>::from_elem((8, 8), 0.1);
        for (a, b, similarity) in [(0, 1, 0.9), (0, 2, 0.9), (1, 2, 0.9), (3, 4, 0.9), (5, 6, 0.9), (6, 7, 0.5)] {
            adjacency[[a, b]] = similarity;
            adjacency[[b, a]] = similarity;
        }
        adjacency.diag_mut().fill(0.0);
        let links: std::collections::BTreeSet<(usize, usize)> =
            natural_neighbours(adjacency.view(), 5).iter().map(|link| link.papers).collect();
        // every paper keeps the neighbours before the largest drop in its own ranking, and nothing after it
        assert_eq!(links, [(0, 1), (0, 2), (1, 2), (3, 4), (5, 6), (6, 7)].into_iter().collect());
    }

    #[test]
    fn each_pair_is_linked_once_with_its_similarity_rounded() {
        let adjacency = array![[0.0, 0.81234, 0.0], [0.81234, 0.0, 0.0], [0.0, 0.0, 0.0]];
        let links = natural_neighbours(adjacency.view(), 8);
        assert_eq!(links, vec![Link { papers: (0, 1), similarity: 0.812 }]);
    }

    #[test]
    fn in_a_group_where_all_are_equally_close_each_paper_keeps_its_first_match() {
        // no drop is larger than another, so the first wins; and of equal matches the lower index comes first
        let mut adjacency = Array2::<f32>::from_elem((6, 6), 0.9);
        adjacency.diag_mut().fill(0.0);
        let links: Vec<(usize, usize)> =
            natural_neighbours(adjacency.view(), 3).iter().map(|link| link.papers).collect();
        assert_eq!(links, vec![(0, 1), (0, 2), (0, 3), (0, 4), (0, 5)]);
    }

    #[test]
    fn tiny_graphs_have_no_links_or_the_only_one() {
        assert!(natural_neighbours(Array2::<f32>::zeros((0, 0)).view(), 8).is_empty());
        assert!(natural_neighbours(Array2::<f32>::zeros((1, 1)).view(), 8).is_empty());
        let two = array![[0.0, 0.5], [0.5, 0.0]];
        assert_eq!(natural_neighbours(two.view(), 8), vec![Link { papers: (0, 1), similarity: 0.5 }]);
    }

    #[test]
    fn the_nearest_papers_come_most_similar_first_without_the_paper_itself() {
        let row = array![0.3, 0.0, 0.9, 0.3, 0.5];
        // the room to select in holds whatever the last paper left there: it changes nothing
        let mut others = vec![4, 4, 4, 9, 9, 9, 9];
        assert_eq!(nearest(row.view(), 1, 3, &mut others), vec![2, 4, 0]);
        assert_eq!(nearest(row.view(), 2, 10, &mut others), vec![4, 0, 3, 1]);
        assert!(nearest(row.view(), 2, 0, &mut others).is_empty());
    }

    #[test]
    fn the_largest_drop_is_found_and_the_first_of_equal_drops_wins() {
        assert_eq!(before_the_largest_drop(&[0.9, 0.9, 0.2, 0.1]), 2);
        assert_eq!(before_the_largest_drop(&[1.0, 0.5, 0.0]), 1);
        assert_eq!(before_the_largest_drop(&[0.4]), 1);
        assert_eq!(before_the_largest_drop(&[]), 0);
    }

    #[test]
    fn links_and_the_nearest_neighbour_graph_are_the_same_on_one_thread_and_on_many() {
        let mut random = crate::analysis::random::Random::new(5);
        let points = Array2::from_shape_simple_fn((400, 12), || random.normal() as f32);
        let adjacency = crate::analysis::graph::adjacency_matrix(points.view(), true).unwrap();
        let work = || {
            let graph = knn_graph(adjacency.view(), KNN);
            let block = Array2::from_shape_fn((400, 3), |(row, column)| (row * 3 + column) as f64);
            (natural_neighbours(adjacency.view(), 8), graph.product(block.view()), graph)
        };
        let (one, many) = crate::threads::on_one_and_many(work);
        assert_eq!(one, many);
    }

    #[test]
    fn a_nearest_neighbour_link_holds_if_either_end_lists_it() {
        // 0 and 1 are each other's closest; 2 is closest to 1, but 1 does not list 2 in its single neighbour
        let adjacency = array![[0.0, 0.9, 0.1], [0.9, 0.0, 0.5], [0.1, 0.5, 0.0]];
        let graph = knn_graph(adjacency.view(), 1);
        assert_eq!(graph.to_dense(), array![[0.0, 0.9, 0.0], [0.9, 0.0, 0.5], [0.0, 0.5, 0.0]]);
        assert_eq!(graph.degrees(), vec![0.9_f32 as f64, f64::from(0.9_f32) + f64::from(0.5_f32), 0.5_f32 as f64]);
        let block = array![[1.0], [2.0], [3.0]];
        let product = graph.product(block.view());
        let expected = graph.to_dense().mapv(f64::from).dot(&block);
        assert!((&product - &expected).iter().all(|value| value.abs() < 1e-12));
    }
}
