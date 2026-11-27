//! Communities: a choice of clustering methods behind one interface, and the tree of sub-communities.
//!
//! Every method answers the same two questions on the same similarity graph: cut all papers into k communities,
//! and cut one community in two (which is how sub-communities grow). See [`Method`] for what each one does.
//!
//! Sub-communities come from the same method applied again inside a community. A cut is kept only if both halves
//! have at least `min_community_size` papers and the cut's modularity *inside the group* reaches
//! `min_split_modularity`. (Measured against the whole graph instead, every cut below the first few is rejected:
//! the resolution limit of modularity.)

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use ndarray::{Array2, ArrayView2, Axis, CowArray, Ix2};
use rayon::prelude::*;

use super::eigen::EigenError;
use super::graph::{make_unit, modularity};
use super::kmeans::kmeans;
use super::links::{KNN, knn_graph};
use super::spectral::{Graph, fiedler_side, leading_eigenvectors};

/// The seed every random choice of a clustering starts from, so that a run can be repeated exactly.
const SEED: u64 = 0;

/// How communities are cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    /// Classic spectral clustering (Ng, Jordan and Weiss, 2001): k-means on the rows, scaled to length one, of the
    /// top-k eigenvectors of the normalised graph. The default: on papers it agreed with authors' own topics about
    /// as well as k-means while being far more repeatable.
    Spectral,
    /// The same on the graph of each paper's [`KNN`] nearest neighbours. Slightly closer to authors' topics, less
    /// even sizes.
    SpectralKnn,
    /// k-means on the mean-centred, unit-length embeddings. Fast; its results move most between runs.
    KMeans,
    /// Recursive bisection by the sign of the Fiedler vector, largest community first. Deterministic, and a
    /// hierarchy by construction, but each cut is final, so early mistakes stay.
    Fiedler,
}

impl Method {
    pub const ALL: [Self; 4] = [Self::Spectral, Self::SpectralKnn, Self::KMeans, Self::Fiedler];

    /// Every method's id, for messages: "spectral, spectral-knn, ...".
    fn ids() -> String {
        Self::ALL.map(Self::id).join(", ")
    }

    /// The name the settings and the interface know it by.
    pub fn id(self) -> &'static str {
        match self {
            Self::Spectral => "spectral",
            Self::SpectralKnn => "spectral-knn",
            Self::KMeans => "kmeans",
            Self::Fiedler => "fiedler",
        }
    }

    /// The name as shown.
    pub fn label(self) -> &'static str {
        match self {
            Self::Spectral => "Spectral Clustering",
            Self::SpectralKnn => "Spectral on Nearest Neighbours",
            Self::KMeans => "K-Means",
            Self::Fiedler => "Recursive Fiedler Bisection",
        }
    }

    /// A few words about it, shown beside the name.
    pub fn note(self) -> &'static str {
        match self {
            Self::Spectral => "most repeatable; the default",
            Self::SpectralKnn => "closest to authors' topics, uneven sizes",
            Self::KMeans => "fast; changes most between runs",
            Self::Fiedler => "deterministic hierarchy; least accurate",
        }
    }

    /// Whether it cuts into k at once, or by cutting in two again and again.
    fn shape(self) -> Shape {
        match self {
            Self::Spectral => Shape::AtOnce(AtOnce::Spectral),
            Self::SpectralKnn => Shape::AtOnce(AtOnce::SpectralKnn),
            Self::KMeans => Shape::AtOnce(AtOnce::KMeans),
            Self::Fiedler => Shape::Bisection,
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(self.id())
    }
}

impl FromStr for Method {
    type Err = ClusterError;

    fn from_str(id: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|method| method.id() == id).ok_or_else(|| ClusterError::UnknownMethod(id.to_owned()))
    }
}

/// How a method makes k communities.
enum Shape {
    AtOnce(AtOnce),
    Bisection,
}

/// The methods that cut into k communities at once.
enum AtOnce {
    Spectral,
    SpectralKnn,
    KMeans,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ClusterError {
    #[error("unknown clustering method {0:?}; the methods are {ids}", ids = Method::ids())]
    UnknownMethod(String),
    #[error("the similarity graph is {rows} x {columns} and there are {vectors} embeddings; they must all agree")]
    Shape { rows: usize, columns: usize, vectors: usize },
    #[error("a community holds paper {paper}, but the graph has only {papers} papers")]
    NotInGraph { paper: usize, papers: usize },
    #[error("{communities} communities cannot be made of {papers} papers")]
    TooFewPapers { papers: usize, communities: usize },
    #[error("the vector of paper {paper} holds a number that is not finite")]
    NotFinite { paper: usize },
    #[error("spectral clustering failed: {0}")]
    Eigen(#[from] EigenError),
}

/// The papers as clustering sees them.
#[derive(Clone, Copy, Debug)]
pub struct Papers<'a> {
    /// The similarity graph (`graph::adjacency_matrix`).
    pub adjacency: ArrayView2<'a, f32>,
    /// The mean-centred, unit-length embeddings the graph was made from, one row per paper.
    pub vectors: ArrayView2<'a, f32>,
}

impl Papers<'_> {
    /// A square graph with one embedding per paper; anything else is refused before it can index out of bounds.
    fn check(&self) -> Result<(), ClusterError> {
        let (rows, columns) = self.adjacency.dim();
        let vectors = self.vectors.nrows();
        if rows == columns && vectors == rows { Ok(()) } else { Err(ClusterError::Shape { rows, columns, vectors }) }
    }
}

/// The settings that decide which cuts become sub-communities.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// Both halves of a cut need at least this many papers.
    pub min_community_size: usize,
    /// A cut is kept only if its modularity inside the group is at least this.
    pub min_split_modularity: f64,
    /// How many times a community is cut again into sub-communities.
    pub sub_levels: usize,
}

impl Default for Limits {
    /// The settings' defaults.
    fn default() -> Self {
        Self { min_community_size: 15, min_split_modularity: 0.05, sub_levels: 2 }
    }
}

/// Where a tree of communities starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Root {
    /// A community of a method that cuts into k at once, by its label: `c7`.
    Community(usize),
    /// The whole graph, which the Fiedler method cuts in two again and again: `r`.
    Bisection,
}

/// One of the two halves of a cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Half {
    First,
    Second,
}

/// A community's name, unique in a run: its root, then `.0` or `.1` for each cut below it, as in `c7.0.1`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Path {
    pub root: Root,
    pub halves: Vec<Half>,
}

impl Path {
    pub fn new(root: Root) -> Self {
        Self { root, halves: Vec::new() }
    }

    /// The path of one half of this community.
    pub fn child(&self, half: Half) -> Self {
        let mut halves = self.halves.clone();
        halves.push(half);
        Self { root: self.root, halves }
    }
}

impl fmt::Display for Path {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self.root {
            Root::Community(label) => write!(formatter, "c{label}")?,
            Root::Bisection => formatter.write_str("r")?,
        }
        // ".0" / ".1", not "0" / "1": the first half of "c1" must not be named like community "c10"
        for half in &self.halves {
            formatter.write_str(match half {
                Half::First => ".0",
                Half::Second => ".1",
            })?;
        }
        Ok(())
    }
}

/// A community: its place in the tree, its papers (indexes into the run's papers), and what it was cut into.
#[derive(Clone, Debug, PartialEq)]
pub struct Community {
    pub path: Path,
    pub papers: Vec<usize>,
    pub parts: Parts,
}

/// What a community holds below it.
#[derive(Clone, Debug, PartialEq)]
pub enum Parts {
    /// It was not cut: no cut was good enough, or the tree is deep enough.
    Whole,
    /// It was cut in two, with this modularity inside the community.
    Halves { modularity: f64, halves: Box<[Community; 2]> },
}

impl Community {
    fn whole(path: Path, papers: Vec<usize>) -> Self {
        Self { path, papers, parts: Parts::Whole }
    }

    /// Its sub-communities: none, or two.
    pub fn sub_communities(&self) -> &[Community] {
        match &self.parts {
            Parts::Whole => &[],
            Parts::Halves { halves, .. } => halves.as_slice(),
        }
    }

    /// How many communities are in its tree below it, at every level.
    pub fn descendants(&self) -> usize {
        self.sub_communities().iter().map(|half| 1 + half.descendants()).sum()
    }
}

/// One accepted top-level cut of the Fiedler method: which community, the sizes of its halves, its modularity.
#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    pub parent: Path,
    pub sizes: [usize; 2],
    pub modularity: f64,
}

/// The communities of a run, largest first, each with its tree of sub-communities; and, for the Fiedler method,
/// the top-level cuts in the order they were made.
#[derive(Clone, Debug, PartialEq)]
pub struct Partition {
    pub communities: Vec<Community>,
    pub cuts: Vec<Cut>,
}

impl Partition {
    /// A community index per paper (its place in `communities`), for `papers` papers.
    pub fn labels(&self, papers: usize) -> Vec<usize> {
        let mut labels = vec![0; papers];
        for (index, community) in self.communities.iter().enumerate() {
            for &paper in &community.papers {
                labels[paper] = index;
            }
        }
        labels
    }
}

/// The communities of a run: `count` of them (for the Fiedler method at most `count`: it stops when no community
/// can be cut acceptably), largest first, each subdivided `limits.sub_levels` deep.
pub fn communities(method: Method, papers: Papers, count: usize, limits: Limits) -> Result<Partition, ClusterError> {
    let size = papers.adjacency.nrows();
    papers.check()?;
    // one NaN would make its group's centre NaN in k-means, and every distance to it 0: every paper would join it
    if let Some(paper) = papers.vectors.rows().into_iter().position(|row| row.iter().any(|value| !value.is_finite())) {
        return Err(ClusterError::NotFinite { paper });
    }
    if count == 0 || count > size {
        return Err(ClusterError::TooFewPapers { papers: size, communities: count });
    }
    let (mut tops, cuts) = match method.shape() {
        Shape::Bisection => by_bisection(method, papers, count, limits)?,
        Shape::AtOnce(at_once) => {
            let labels = partition(at_once, papers, count, SEED)?;
            let mut members: Vec<Vec<usize>> = vec![Vec::new(); count];
            for (paper, &label) in labels.iter().enumerate() {
                members[label].push(paper);
            }
            let tops = members
                .into_iter()
                .enumerate()
                .filter(|(_, members)| !members.is_empty())
                .map(|(label, members)| Community::whole(Path::new(Root::Community(label)), members))
                .collect();
            (tops, Vec::new())
        }
    };
    // a stable sort: communities of equal size keep their order
    tops.sort_by_key(|community| std::cmp::Reverse(community.papers.len()));
    // Every community is subdivided on its own, side by side: what a community is cut into depends on its own
    // papers, the graph and the fixed seed, all only read. The answers are collected in the communities' order,
    // and the first error in that order is the one told, so neither depends on which finished first.
    let subdivided: Vec<Result<Community, ClusterError>> =
        tops.into_par_iter().map(|community| subdivide(method, papers, community, limits.sub_levels, limits)).collect();
    let communities = subdivided.into_iter().collect::<Result<_, _>>()?;
    Ok(Partition { communities, cuts })
}

/// A community label per paper, for the methods that cut into `count` at once.
fn partition(method: AtOnce, papers: Papers, count: usize, seed: u64) -> Result<Vec<usize>, ClusterError> {
    Ok(match method {
        AtOnce::Spectral => spectral_labels(Graph::Dense(papers.adjacency), count, seed)?,
        AtOnce::SpectralKnn => {
            let neighbours = knn_graph(papers.adjacency, KNN);
            spectral_labels(Graph::Sparse(&neighbours), count, seed)?
        }
        AtOnce::KMeans => kmeans(papers.vectors, count, seed).labels,
    })
}

/// Normalised spectral clustering: k-means on the rows of the top `count` eigenvectors, each row scaled to length
/// one, so that a paper is placed by the direction of its row and not by how strongly it is linked.
fn spectral_labels(graph: Graph, count: usize, seed: u64) -> Result<Vec<usize>, EigenError> {
    let spectrum = leading_eigenvectors(graph, count)?;
    let mut rows = spectrum.vectors.mapv(|value| value as f32);
    make_unit(&mut rows);
    Ok(kmeans(rows.view(), count, seed).labels)
}

/// One group cut in two: `true` for the papers of the first half, or `None` when the method cannot cut it.
/// `papers` is the group's own graph and vectors.
pub fn bisect(method: Method, papers: Papers, seed: u64) -> Result<Option<Vec<bool>>, ClusterError> {
    papers.check()?;
    if papers.adjacency.nrows() < 4 {
        return Ok(None);
    }
    let side: Vec<bool> = match method.shape() {
        Shape::Bisection => return Ok(fiedler_side(papers.adjacency)?),
        Shape::AtOnce(at_once) => partition(at_once, papers, 2, seed)?.iter().map(|&label| label == 0).collect(),
    };
    let one_sided = side.iter().all(|&first| first) || !side.iter().any(|&first| first);
    Ok(if one_sided { None } else { Some(side) })
}

/// The rows and columns of `matrix` listed in `members`. When that is every row in order, the matrix itself is
/// used: the Fiedler method's first cut is of the whole graph, and a copy of it would double the memory a run needs.
fn restricted<'a>(matrix: ArrayView2<'a, f32>, members: &[usize]) -> CowArray<'a, f32, Ix2> {
    let everything =
        members.len() == matrix.nrows() && members.iter().enumerate().all(|(index, &paper)| index == paper);
    if everything {
        return CowArray::from(matrix);
    }
    // row by row on every core: each row of the copy is written by one thread, from the graph nobody writes
    let mut copy = Array2::<f32>::zeros((members.len(), members.len()));
    copy.axis_iter_mut(Axis(0)).into_par_iter().zip(members.par_iter()).for_each(|(mut row, &paper)| {
        let source = matrix.row(paper);
        for (entry, &other) in row.iter_mut().zip(members) {
            *entry = source[other];
        }
    });
    CowArray::from(copy)
}

/// The rows of `vectors` listed in `members`, without a copy when that is every row in order.
fn rows_of<'a>(vectors: ArrayView2<'a, f32>, members: &[usize]) -> CowArray<'a, f32, Ix2> {
    let everything =
        members.len() == vectors.nrows() && members.iter().enumerate().all(|(index, &paper)| index == paper);
    if everything { CowArray::from(vectors) } else { CowArray::from(vectors.select(Axis(0), members)) }
}

/// The two halves of `community` and the cut's modularity inside it, if it cuts acceptably: both halves at least
/// `min_community_size` papers, modularity at least `min_split_modularity`.
pub fn split(
    method: Method,
    papers: Papers,
    community: &Community,
    limits: Limits,
) -> Result<Option<(f64, [Community; 2])>, ClusterError> {
    papers.check()?;
    let members = &community.papers;
    if let Some(&paper) = members.iter().find(|&&paper| paper >= papers.adjacency.nrows()) {
        return Err(ClusterError::NotInGraph { paper, papers: papers.adjacency.nrows() });
    }
    if members.len() < 2 * limits.min_community_size {
        return Ok(None);
    }
    let sub = restricted(papers.adjacency, members);
    let vectors = rows_of(papers.vectors, members);
    let Some(side) = bisect(method, Papers { adjacency: sub.view(), vectors: vectors.view() }, SEED)? else {
        return Ok(None);
    };
    let (first, second): (Vec<usize>, Vec<usize>) = (0..members.len()).partition(|&index| side[index]);
    if first.len().min(second.len()) < limits.min_community_size {
        return Ok(None);
    }
    let quality = modularity(sub.view(), &[first.clone(), second.clone()]);
    if quality < limits.min_split_modularity {
        return Ok(None);
    }
    let global = |local: Vec<usize>| local.into_iter().map(|index| members[index]).collect();
    Ok(Some((
        quality,
        [
            Community::whole(community.path.child(Half::First), global(first)),
            Community::whole(community.path.child(Half::Second), global(second)),
        ],
    )))
}

/// `community` with its sub-communities, `levels` deep. A community that does not cut acceptably simply has none.
pub fn subdivide(
    method: Method,
    papers: Papers,
    community: Community,
    levels: usize,
    limits: Limits,
) -> Result<Community, ClusterError> {
    if levels == 0 {
        return Ok(community);
    }
    let Some((modularity, [first, second])) = split(method, papers, &community, limits)? else {
        return Ok(community);
    };
    // the two halves side by side, for the same reasons as the communities ([`communities`]); the first half's
    // error first
    let (first, second) = rayon::join(
        || subdivide(method, papers, first, levels - 1, limits),
        || subdivide(method, papers, second, levels - 1, limits),
    );
    let halves = [first?, second?];
    Ok(Community { parts: Parts::Halves { modularity, halves: Box::new(halves) }, ..community })
}

/// Communities by cutting the largest group still open in two until there are `count` (the Fiedler method). A
/// group that does not cut acceptably is closed. Returns the groups and the cuts made.
fn by_bisection(
    method: Method,
    papers: Papers,
    count: usize,
    limits: Limits,
) -> Result<(Vec<Community>, Vec<Cut>), ClusterError> {
    let mut groups = vec![Community::whole(Path::new(Root::Bisection), (0..papers.adjacency.nrows()).collect())];
    let mut closed: HashSet<Path> = HashSet::new();
    let mut cuts = Vec::new();
    while groups.len() < count {
        // the largest open group; of equal ones, the first
        let Some(index) = (0..groups.len())
            .filter(|&index| !closed.contains(&groups[index].path))
            .reduce(|best, index| if groups[index].papers.len() > groups[best].papers.len() { index } else { best })
        else {
            break;
        };
        match split(method, papers, &groups[index], limits)? {
            None => {
                closed.insert(groups[index].path.clone());
            }
            Some((modularity, halves)) => {
                let parent = groups.remove(index);
                cuts.push(Cut {
                    parent: parent.path,
                    sizes: [halves[0].papers.len(), halves[1].papers.len()],
                    modularity,
                });
                groups.extend(halves);
            }
        }
    }
    Ok((groups, cuts))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::analysis::graph::adjacency_matrix;
    use crate::analysis::kmeans::tests::{blobs, same_grouping};

    /// The planted groups' graph and their mean-centred unit vectors.
    fn blob_papers() -> (Array2<f32>, Array2<f32>, Vec<usize>) {
        let (points, truth) = blobs();
        let adjacency = adjacency_matrix(points.view(), true).unwrap();
        let mut vectors = &points - &points.mean_axis(Axis(0)).unwrap();
        make_unit(&mut vectors);
        (adjacency, vectors, truth)
    }

    fn loose(sub_levels: usize) -> Limits {
        Limits { min_community_size: 5, min_split_modularity: 0.0, sub_levels }
    }

    #[test]
    fn every_method_finds_planted_groups() {
        let (adjacency, vectors, truth) = blob_papers();
        let papers = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
        for method in Method::ALL {
            let partition = communities(method, papers, 3, loose(0)).unwrap();
            assert!(same_grouping(&partition.labels(120), &truth), "{method}");
            let sizes: Vec<usize> = partition.communities.iter().map(|community| community.papers.len()).collect();
            assert_eq!(sizes, vec![40, 40, 40], "{method}");
        }
    }

    #[test]
    fn communities_come_largest_first() {
        let (adjacency, vectors, _) = blob_papers();
        let papers = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
        for method in Method::ALL {
            let partition = communities(method, papers, 5, loose(0)).unwrap();
            let sizes: Vec<usize> = partition.communities.iter().map(|community| community.papers.len()).collect();
            assert!(sizes.windows(2).all(|pair| pair[0] >= pair[1]), "{method}: {sizes:?}");
            assert_eq!(sizes.iter().sum::<usize>(), 120);
        }
    }

    #[test]
    fn sub_communities_partition_their_parent() {
        let (adjacency, vectors, _) = blob_papers();
        let papers = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
        let partition = communities(Method::Spectral, papers, 2, loose(1)).unwrap();
        let cut: Vec<&Community> =
            partition.communities.iter().filter(|community| !community.sub_communities().is_empty()).collect();
        assert!(!cut.is_empty(), "one of two communities holds two planted groups and should be cut");
        for community in cut {
            let mut together: Vec<usize> =
                community.sub_communities().iter().flat_map(|half| half.papers.iter().copied()).collect();
            together.sort_unstable();
            let mut parent = community.papers.clone();
            parent.sort_unstable();
            assert_eq!(together, parent);
            assert_eq!(community.descendants(), 2, "one level deep");
        }
    }

    #[test]
    fn sub_communities_are_named_by_their_place_in_the_tree() {
        let (adjacency, vectors, _) = blob_papers();
        let papers = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
        let partition = communities(Method::Spectral, papers, 2, loose(1)).unwrap();
        for community in &partition.communities {
            for (half, sub) in community.sub_communities().iter().enumerate() {
                assert_eq!(sub.path.to_string(), format!("{}.{half}", community.path));
            }
        }
        let path = Path::new(Root::Community(1)).child(Half::First).child(Half::Second);
        assert_eq!(path.to_string(), "c1.0.1");
        assert_ne!(Path::new(Root::Community(1)).child(Half::First).to_string(), "c10");
        assert_eq!(Path::new(Root::Bisection).child(Half::Second).to_string(), "r.1");
    }

    #[test]
    fn the_fiedler_method_records_its_cuts_and_names_by_the_bisection_tree() {
        let (adjacency, vectors, _) = blob_papers();
        let papers = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
        let partition = communities(Method::Fiedler, papers, 3, loose(0)).unwrap();
        assert_eq!(partition.cuts.len(), 2);
        assert_eq!(partition.cuts[0].parent.to_string(), "r");
        assert_eq!(partition.cuts[0].sizes.iter().sum::<usize>(), 120);
        assert!(partition.communities.iter().all(|community| community.path.to_string().starts_with("r.")));
    }

    #[test]
    fn the_fiedler_method_still_cuts_a_graph_with_a_paper_linked_to_nobody() {
        let (adjacency, vectors, truth) = blob_papers();
        // a 121st paper with no similarity to any other
        let mut lonely = Array2::<f32>::zeros((121, 121));
        lonely.slice_mut(ndarray::s![..120, ..120]).assign(&adjacency);
        let mut with_it = Array2::<f32>::zeros((121, vectors.ncols()));
        with_it.slice_mut(ndarray::s![..120, ..]).assign(&vectors);
        let papers = Papers { adjacency: lonely.view(), vectors: with_it.view() };
        let partition = communities(Method::Fiedler, papers, 3, loose(0)).unwrap();
        assert_eq!(partition.communities.len(), 3, "{:?}", partition.cuts);
        assert!(same_grouping(&partition.labels(121)[..120], &truth));
    }

    #[test]
    fn a_group_too_small_to_split_is_left_whole() {
        let (adjacency, vectors, _) = blob_papers();
        let papers = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
        let community = Community::whole(Path::new(Root::Community(0)), (0..30).collect());
        let limits = Limits { min_community_size: 20, min_split_modularity: 0.0, sub_levels: 1 };
        assert_eq!(split(Method::Spectral, papers, &community, limits).unwrap(), None);
    }

    #[test]
    fn a_cut_below_the_modularity_asked_for_is_refused() {
        let (adjacency, vectors, _) = blob_papers();
        let papers = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
        // one planted group: any cut of it is weak
        let community = Community::whole(Path::new(Root::Community(0)), (0..40).collect());
        let strict = Limits { min_community_size: 5, min_split_modularity: 0.9, sub_levels: 1 };
        assert_eq!(split(Method::Spectral, papers, &community, strict).unwrap(), None);
        // two planted groups cut well
        let two = Community::whole(Path::new(Root::Community(0)), (0..80).collect());
        let (quality, halves) = split(Method::Spectral, papers, &two, loose(1)).unwrap().unwrap();
        assert!(quality > 0.3, "{quality}");
        assert_eq!(halves[0].papers.len() + halves[1].papers.len(), 80);
    }

    #[test]
    fn sub_communities_stop_at_the_levels_asked_for() {
        let (adjacency, vectors, _) = blob_papers();
        let papers = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
        let depth = |community: &Community| -> usize {
            fn deepest(community: &Community) -> usize {
                community.sub_communities().iter().map(|half| 1 + deepest(half)).max().unwrap_or(0)
            }
            deepest(community)
        };
        for levels in 0..3 {
            let partition = communities(Method::KMeans, papers, 1, loose(levels)).unwrap();
            assert!(partition.communities.iter().all(|community| depth(community) <= levels), "{levels}");
        }
    }

    #[test]
    fn an_unknown_method_is_refused_and_every_method_is_known_by_its_id() {
        assert_eq!("magic".parse::<Method>(), Err(ClusterError::UnknownMethod("magic".to_owned())));
        for method in Method::ALL {
            assert_eq!(method.id().parse::<Method>(), Ok(method));
            assert!(!method.label().is_empty() && !method.note().is_empty());
        }
        assert_eq!(Method::SpectralKnn.to_string(), "spectral-knn");
    }

    #[test]
    fn more_communities_than_papers_are_refused() {
        let (adjacency, vectors, _) = blob_papers();
        let papers = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
        let error = communities(Method::Spectral, papers, 121, loose(0)).unwrap_err();
        assert_eq!(error, ClusterError::TooFewPapers { papers: 120, communities: 121 });
    }

    #[test]
    fn a_graph_and_embeddings_that_do_not_agree_are_refused() {
        let (adjacency, vectors, _) = blob_papers();
        let short = vectors.slice(ndarray::s![..100, ..]);
        let papers = Papers { adjacency: adjacency.view(), vectors: short };
        let error = communities(Method::KMeans, papers, 3, loose(0)).unwrap_err();
        assert_eq!(error, ClusterError::Shape { rows: 120, columns: 120, vectors: 100 });
        let whole = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
        let stray = Community::whole(Path::new(Root::Community(0)), vec![0, 500]);
        assert_eq!(
            split(Method::KMeans, whole, &stray, loose(1)),
            Err(ClusterError::NotInGraph { paper: 500, papers: 120 })
        );
        let message = "magic".parse::<Method>().unwrap_err().to_string();
        assert!(message.ends_with("the methods are spectral, spectral-knn, kmeans, fiedler"), "{message}");
    }

    #[test]
    fn a_vector_that_is_not_finite_is_refused_rather_than_drawing_every_paper_into_one_community() {
        let (adjacency, mut vectors, _) = blob_papers();
        vectors[[17, 3]] = f32::NAN;
        let papers = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
        for method in Method::ALL {
            assert_eq!(
                communities(method, papers, 3, loose(0)),
                Err(ClusterError::NotFinite { paper: 17 }),
                "{method}"
            );
        }
    }

    #[test]
    fn a_whole_graph_is_not_copied_to_be_cut() {
        let matrix = Array2::<f32>::eye(3);
        assert!(restricted(matrix.view(), &[0, 1, 2]).is_view());
        assert!(!restricted(matrix.view(), &[0, 2]).is_view());
        assert_eq!(restricted(matrix.view(), &[2, 0]).into_owned(), Array2::<f32>::eye(2));
    }

    #[test]
    fn every_method_cuts_the_same_communities_and_sub_communities_on_one_thread_and_on_many() {
        // twelve loose groups of 25: enough communities to be cut side by side, each large enough to be cut again
        let mut random = crate::analysis::random::Random::new(21);
        let centres = Array2::from_shape_simple_fn((12, 20), || 3.0 * random.normal());
        let points = Array2::from_shape_fn((300, 20), |(point, axis)| {
            (centres[[point % 12, axis]] + 1.5 * random.normal()) as f32
        });
        let adjacency = adjacency_matrix(points.view(), true).unwrap();
        let mut vectors = &points - &points.mean_axis(Axis(0)).unwrap();
        make_unit(&mut vectors);
        let papers = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
        let limits = Limits { min_community_size: 4, min_split_modularity: 0.0, sub_levels: 3 };
        for method in Method::ALL {
            let (one, many) = crate::threads::on_one_and_many(|| communities(method, papers, 6, limits).unwrap());
            assert_eq!(one, many, "{method}");
            assert!(one.communities.iter().any(|community| community.descendants() > 0), "{method}: nothing was cut");
        }
    }

    /// Two groups of 120 papers, each of two sub-groups of 60: the sub-groups' centres closer to their group's
    /// than the groups are to each other, as topics within a field are.
    fn nested_papers() -> (Array2<f32>, Array2<f32>) {
        let mut random = crate::analysis::random::Random::new(17);
        let groups = Array2::from_shape_simple_fn((2, 64), || 1.0 * random.normal());
        let subs = Array2::from_shape_simple_fn((4, 64), || 0.6 * random.normal());
        let points = Array2::from_shape_fn((240, 64), |(point, axis)| {
            (groups[[point / 120, axis]] + subs[[point / 60, axis]] + 0.35 * random.normal()) as f32
        });
        let adjacency = adjacency_matrix(points.view(), true).unwrap();
        let mut vectors = &points - &points.mean_axis(Axis(0)).unwrap();
        make_unit(&mut vectors);
        (adjacency, vectors)
    }

    #[test]
    fn nested_groups_become_communities_with_sub_communities_under_the_default_limits() {
        let (adjacency, vectors) = nested_papers();
        let papers = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
        let truth: Vec<usize> = (0..240).map(|paper| paper / 120).collect();
        let sub_truth: Vec<usize> = (0..240).map(|paper| paper / 60).collect();
        for method in Method::ALL {
            let partition = communities(method, papers, 2, Limits::default()).unwrap();
            assert_eq!(partition.communities.len(), 2, "{method}");
            assert!(same_grouping(&partition.labels(240), &truth), "{method}");
            let mut leaves = vec![0; 240];
            for (community, top) in partition.communities.iter().enumerate() {
                let subs = top.sub_communities();
                assert_eq!(subs.iter().map(|sub| sub.papers.len()).collect::<Vec<_>>(), [60, 60], "{method}");
                assert!(matches!(top.parts, Parts::Halves { modularity, .. } if modularity >= 0.05), "{method}");
                for (half, sub) in subs.iter().enumerate() {
                    for &paper in &sub.papers {
                        leaves[paper] = 2 * community + half;
                    }
                }
            }
            assert!(same_grouping(&leaves, &sub_truth), "{method}");
        }
    }

    /// How long a conference-sized run takes on this computer: `cargo test --release -- --ignored --nocapture`.
    ///
    /// The embeddings are noise, with no groups in them, so every cut of every community has a modularity of about
    /// 0.01 to 0.03 (measured on 3,000 of them: 0.028 for the spectral and Fiedler cuts, 0.008 on nearest
    /// neighbours), below the default [`Limits::min_split_modularity`] of 0.05: the methods that cut into k at
    /// once give 24 communities and no sub-communities, and the Fiedler method, which keeps a cut only if it is
    /// good enough, refuses its first and gives 1. That is the data, not the methods: on planted groups within
    /// groups every method finds both levels (`nested_groups_become_communities_with_sub_communities_...`).
    #[test]
    #[ignore = "a measurement, not a check: 12,487 papers of 1,024 numbers each"]
    fn a_conference_sized_run_is_clustered_in_seconds() {
        use crate::analysis::links::natural_neighbours;
        use std::time::Instant;
        let (count, width) = (12_487, 1_024);
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let embeddings = Array2::from_shape_fn((count, width), |_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 2_000) as f32 / 1_000.0 - 1.0
        });
        let adjacency = adjacency_matrix(embeddings.view(), true).unwrap();
        let mut vectors = &embeddings - &embeddings.mean_axis(Axis(0)).unwrap();
        make_unit(&mut vectors);
        let papers = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
        for wanted in [2, 24, 50] {
            let started = Instant::now();
            let spectrum = leading_eigenvectors(Graph::Dense(adjacency.view()), wanted).unwrap();
            println!("{wanted} leading eigenvectors: {:.2}s", started.elapsed().as_secs_f32());
            assert_eq!(spectrum.vectors.ncols(), wanted);
        }
        let started = Instant::now();
        let links = natural_neighbours(adjacency.view(), 8);
        println!("natural neighbours: {:.2}s ({} links)", started.elapsed().as_secs_f32(), links.len());
        for method in Method::ALL {
            let started = Instant::now();
            let partition = communities(method, papers, 24, Limits::default()).unwrap();
            let subs: usize = partition.communities.iter().map(Community::descendants).sum();
            println!(
                "{method}: {} communities, {subs} sub-communities in {:.2}s",
                partition.communities.len(),
                started.elapsed().as_secs_f32()
            );
        }
    }
}
