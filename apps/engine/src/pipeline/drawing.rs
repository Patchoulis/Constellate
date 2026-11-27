//! What the graph view needs (`graph.json`, `GraphData` in the contracts): every paper with its community, a rough
//! position, its AI-text flag and how it was accepted; and each paper's few closest papers as edges.
//!
//! The position is the paper's place along the two leading principal components of the mean-centred embeddings,
//! scaled into [-1, 1]. The interface's layout (`apps/web/lib/layout.ts`) starts its packing of the communities
//! from where these put them; it computes every position it draws itself.
//!
//! Field for field what the Python wrote (`rp/graph.py`, `drawing`), with two differences that change no
//! picture: the components' signs are chosen so that each axis's largest entry is positive (LAPACK, under
//! NumPy, left them to chance), and for more than [`SAMPLE`] papers the components are found from a sample drawn
//! with the engine's own seeded generator (NumPy's generator drew another sample of the same size).

use ndarray::{Array2, ArrayView2, Axis};
use rayon::prelude::*;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use crate::analysis::eigen::{self, EigenError};
use crate::analysis::links::natural_neighbours;
use crate::analysis::random::Random;
use crate::library::Paper;
use crate::sources::acceptance::{Acceptance, Accepted, Term};

/// The most papers the principal components are found from.
pub const SAMPLE: usize = 3_000;

/// One paper as the graph view draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    pub title: String,
    pub community: usize,
    pub x: f64,
    pub y: f64,
    pub ai: bool,
    pub acceptance: Acceptance,
    /// Only for orals and spotlights (a poster's term is left out to keep the graph small), as the Python did.
    pub acceptance_term: Option<Term>,
}

impl Serialize for Node {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let fields = 7 + usize::from(self.acceptance_term.is_some());
        let mut node = serializer.serialize_struct("GraphNode", fields)?;
        node.serialize_field("id", &self.id)?;
        node.serialize_field("title", &self.title)?;
        node.serialize_field("community", &self.community)?;
        node.serialize_field("x", &self.x)?;
        node.serialize_field("y", &self.y)?;
        node.serialize_field("ai", &self.ai)?;
        node.serialize_field("acceptance", &self.acceptance)?;
        if let Some(term) = &self.acceptance_term {
            node.serialize_field("acceptance_term", term)?;
        }
        node.end()
    }
}

/// `graph.json`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Drawing {
    pub nodes: Vec<Node>,
    /// [paper, paper, similarity], each pair once.
    pub edges: Vec<(usize, usize, f32)>,
}

/// Rounded to `places` decimals, as Python's `round` does for the numbers here.
fn rounded(value: f64, places: i32) -> f64 {
    let scale = 10_f64.powi(places);
    (value * scale).round() / scale
}

/// The rows the principal components are found from: all of them up to [`SAMPLE`], else a seeded sample.
fn sample(papers: usize) -> Vec<usize> {
    if papers <= SAMPLE {
        return (0..papers).collect();
    }
    let mut random = Random::new(0);
    let mut all: Vec<usize> = (0..papers).collect();
    for index in 0..SAMPLE {
        let other = index + random.below(papers - index);
        all.swap(index, other);
    }
    all.truncate(SAMPLE);
    all.sort_unstable();
    all
}

/// Each paper's place on the two leading principal components of the centred embeddings, scaled so that the
/// largest is 1 in size, rounded to four decimals.
pub fn positions(embeddings: ArrayView2<f32>) -> Result<Vec<(f64, f64)>, EigenError> {
    let (papers, width) = embeddings.dim();
    if papers == 0 || width == 0 {
        return Ok(vec![(0.0, 0.0); papers]);
    }
    let mean = super::report::overall_mean(embeddings);
    let mut centred = embeddings.to_owned();
    centred -= &mean;
    let rows: Array2<f64> = centred.select(Axis(0), &sample(papers)).mapv(f64::from);
    // the right singular vectors of the sample are the eigenvectors of its Gram matrix X^T X, found from its
    // products alone: X^T (X v)
    let wanted = 2.min(width);
    let axes = eigen::leading(width, wanted, |block| rows.t().dot(&rows.dot(&block)))?;
    let projected: Vec<(f64, f64)> = centred
        .outer_iter()
        .into_par_iter()
        .map(|row| {
            let along = |axis: usize| {
                if axis >= wanted {
                    return 0.0;
                }
                row.iter().zip(axes.vectors.column(axis)).map(|(value, weight)| f64::from(*value) * weight).sum()
            };
            (along(0), along(1))
        })
        .collect();
    let largest = projected.iter().fold(0.0_f64, |most, (x, y)| most.max(x.abs()).max(y.abs())) + 1e-12;
    Ok(projected.into_iter().map(|(x, y)| (rounded(x / largest, 4), rounded(y / largest, 4))).collect())
}

/// The drawing of a run: `community[i]` is paper i's top-level community, `accepted[i]` how it was accepted.
pub fn drawing(
    papers: &[Paper],
    embeddings: ArrayView2<f32>,
    adjacency: ArrayView2<f32>,
    community: &[usize],
    flagged: &[bool],
    accepted: &[Accepted],
    most_links: usize,
) -> Result<Drawing, EigenError> {
    let places = positions(embeddings)?;
    let nodes = papers
        .iter()
        .enumerate()
        .map(|(index, paper)| {
            let shown = &accepted[index];
            let tagged = matches!(shown.tier, Acceptance::Oral | Acceptance::Spotlight);
            Node {
                id: paper.id.clone(),
                title: paper.title.clone(),
                community: community[index],
                x: places[index].0,
                y: places[index].1,
                ai: flagged[index],
                acceptance: shown.tier,
                acceptance_term: if tagged { shown.term.clone() } else { None },
            }
        })
        .collect();
    let edges = natural_neighbours(adjacency, most_links)
        .into_iter()
        .map(|link| (link.papers.0, link.papers.1, link.similarity))
        .collect();
    Ok(Drawing { nodes, edges })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn positions_spread_two_groups_apart_along_the_first_component_within_one() {
        let embeddings = ndarray::array![[5.0, 0.1, 0.0], [5.1, 0.0, 0.2], [-5.0, 0.0, 0.1], [-5.2, 0.2, 0.0]];
        let places = positions(embeddings.view()).unwrap();
        assert!(places.iter().all(|(x, y)| x.abs() <= 1.0 && y.abs() <= 1.0));
        assert!(places[0].0.signum() == places[1].0.signum() && places[0].0.signum() != places[2].0.signum());
        assert!(places.iter().any(|(x, _)| (x.abs() - 1.0).abs() < 1e-9));
    }

    #[test]
    fn a_large_run_is_sampled_the_same_way_every_time() {
        let first = sample(10_000);
        assert_eq!(first.len(), SAMPLE);
        assert_eq!(first, sample(10_000));
        assert!(first.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(sample(12), (0..12).collect::<Vec<_>>());
    }

    #[test]
    fn a_poster_has_no_term_in_the_graph_and_an_arxiv_paper_no_tier() {
        let node = |acceptance, term: Option<Term>| Node {
            id: "p".into(),
            title: "t".into(),
            community: 0,
            x: 0.0,
            y: 0.0,
            ai: false,
            acceptance,
            acceptance_term: term,
        };
        let written = serde_json::to_value(node(Acceptance::NotListed, None)).unwrap();
        assert_eq!(written["acceptance"], serde_json::Value::Null);
        assert!(written.get("acceptance_term").is_none());
        let term = Term { conference: "NeurIPS 2025".into(), called: "Oral".into(), means: "m".into(), known: true };
        let written = serde_json::to_value(node(Acceptance::Oral, Some(term))).unwrap();
        assert_eq!(written["acceptance_term"]["called"], "Oral");
    }
}
