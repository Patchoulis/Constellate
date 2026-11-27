//! The tree of communities turned into what is stored and shown: one record per community and sub-community (the
//! `communities.json` the interface reads, `Community` in the contracts), and the readable report.
//!
//! A record is the group's mean embedding turned into words (its characteristic terms, the papers nearest to it,
//! and a name and description once [`super::seams::describe`] has written them) plus similarity figures.
//! Everything relative is relative to the siblings: the terms of a sub-community are what sets it apart inside its
//! parent, and its closest communities are among its siblings.
//!
//! Field for field what the Python wrote (`rp/report.py`). Counted maps (`mix`, `acceptance`) keep the Python's
//! order: by count, then in the order first met going through the members.

use ndarray::{Array1, Array2, ArrayView1, ArrayView2, Axis};
use rayon::prelude::*;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::vocabulary::Vocabulary;
use crate::analysis::graph::cluster::Community;
use crate::library::Paper;
use crate::selection::Selection;
use crate::sources::acceptance::Acceptance;

/// Terms listed per community.
pub const KEYWORDS: usize = 12;
/// Papers nearest a community's mean embedding that are listed with it.
pub const NEAREST: usize = 10;
/// Siblings listed as closest.
pub const CLOSEST: usize = 4;
/// Tags listed in a community's mix.
pub const MIX: usize = 6;

/// Names with how many, in a chosen order, written as a JSON object in that order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counted(pub Vec<(String, u64)>);

impl Counted {
    /// Count `items` in the order they come: the first met first.
    fn of(items: impl IntoIterator<Item = String>) -> Self {
        let mut counted: Vec<(String, u64)> = Vec::new();
        for item in items {
            match counted.iter_mut().find(|(name, _)| *name == item) {
                Some((_, count)) => *count += 1,
                None => counted.push((item, 1)),
            }
        }
        Self(counted)
    }

    /// The `most` most common, by count, ties in the order first met (Python's `Counter.most_common`).
    fn most_common(mut self, most: usize) -> Self {
        self.0.sort_by_key(|(_, count)| std::cmp::Reverse(*count)); // stable
        self.0.truncate(most);
        self
    }
}

impl Serialize for Counted {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, count) in &self.0 {
            map.serialize_entry(name, count)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Counted {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // read back in any order (serde_json's maps are sorted): only the counts matter once read
        let map = std::collections::BTreeMap::<String, u64>::deserialize(deserializer)?;
        Ok(Self(map.into_iter().collect()))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Closest {
    pub id: usize,
    pub cosine: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Nearest {
    pub id: String,
    pub title: String,
    pub cosine: f64,
    pub in_community: bool,
}

/// One community as stored (`Community` in the contracts, without the citation counts, which runs made here do
/// not have yet).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Its place among its siblings.
    pub id: usize,
    pub tree_path: String,
    pub size: usize,
    pub name: Option<String>,
    pub description: Option<String>,
    pub keywords: Vec<String>,
    pub mix: Counted,
    pub mean_inner_similarity: f64,
    pub similarity_to_mean: f64,
    pub closest_communities: Vec<Closest>,
    pub nearest_to_centroid: Vec<Nearest>,
    pub ai_flagged: usize,
    pub acceptance: Counted,
    pub members: Vec<usize>,
    pub children: Vec<Record>,
}

impl Record {
    /// Every record of the tree under (and with) this one, depth first.
    pub fn each_mut(&mut self, visit: &mut impl FnMut(&mut Record)) {
        visit(self);
        for child in &mut self.children {
            child.each_mut(visit);
        }
    }
}

/// The papers of a run and what describing a group of them needs.
pub struct Corpus<'a> {
    pub selection: &'a Selection,
    pub papers: &'a [Paper],
    pub embeddings: ArrayView2<'a, f32>,
    pub adjacency: ArrayView2<'a, f32>,
    pub vocabulary: &'a Vocabulary,
    pub flagged: &'a [bool],
    pub tiers: &'a [Acceptance],
}

/// `rows` scaled to length 1 (the Python's `unit`: a tiny number is added to every length).
fn unit_rows(mut rows: Array2<f32>) -> Array2<f32> {
    for mut row in rows.axis_iter_mut(Axis(0)) {
        let length = row.dot(&row).sqrt() + 1e-12;
        row /= length;
    }
    rows
}

fn unit(vector: ArrayView1<f32>) -> Array1<f32> {
    let length = vector.dot(&vector).sqrt() + 1e-12;
    vector.mapv(|value| value / length)
}

/// The mean of the rows of `embeddings` listed in `group`, added up in 64 bits.
pub fn centroid(embeddings: ArrayView2<f32>, group: &[usize]) -> Array1<f32> {
    let mut sum = Array1::<f64>::zeros(embeddings.ncols());
    for &paper in group {
        sum.zip_mut_with(&embeddings.row(paper), |total, value| *total += f64::from(*value));
    }
    let count = group.len().max(1) as f64;
    sum.mapv(|total| (total / count) as f32)
}

/// The mean of every row.
pub fn overall_mean(embeddings: ArrayView2<f32>) -> Array1<f32> {
    let everyone: Vec<usize> = (0..embeddings.nrows()).collect();
    centroid(embeddings, &everyone)
}

/// The `count` papers closest to `centre`, over the whole selection, with their cosine similarity: most similar
/// first, ties to the lower index.
pub fn nearest_papers(embeddings: ArrayView2<f32>, centre: ArrayView1<f32>, count: usize) -> Vec<(usize, f32)> {
    let direction = unit(centre);
    let scores: Vec<f32> = embeddings.outer_iter().map(|row| row.dot(&direction)).collect();
    let mut order: Vec<usize> = (0..scores.len()).collect();
    let closer = |a: &usize, b: &usize| scores[*b].total_cmp(&scores[*a]).then(a.cmp(b));
    let count = count.min(order.len());
    if count > 0 && count < order.len() {
        order.select_nth_unstable_by(count - 1, closer);
        order.truncate(count);
    }
    order.sort_unstable_by(closer);
    order.into_iter().map(|paper| (paper, scores[paper])).collect()
}

/// One record per community of `level` (siblings), with their sub-communities under `children`; `parent` is the
/// community they split, if any.
pub fn community_tree(corpus: &Corpus, level: &[Community], parent: Option<&Community>) -> Vec<Record> {
    let embeddings = corpus.embeddings;
    let groups: Vec<&[usize]> = level.iter().map(|community| community.papers.as_slice()).collect();
    let centres: Vec<Array1<f32>> = groups.par_iter().map(|group| centroid(embeddings, group)).collect();
    // How alike two groups are: cosine similarity of their mean embeddings, after removing the overall mean
    // embedding (the raw mean embeddings of any two groups of papers of one field are all close to 1).
    let mean = overall_mean(embeddings);
    let mut relative = Array2::<f32>::zeros((centres.len(), embeddings.ncols()));
    for (mut row, centre) in relative.axis_iter_mut(Axis(0)).zip(&centres) {
        row.assign(&(centre - &mean));
    }
    let relative = unit_rows(relative);
    let between = relative.dot(&relative.t());
    let within = parent.map(|parent| parent.papers.as_slice());
    (0..level.len())
        .into_par_iter()
        .map(|index| {
            let community = &level[index];
            let group = groups[index];
            let centre = &centres[index];
            let mut others: Vec<usize> = (0..level.len()).filter(|other| *other != index).collect();
            others.sort_by(|a, b| between[[index, *b]].total_cmp(&between[[index, *a]]).then(a.cmp(b)));
            others.truncate(CLOSEST);
            let inside: std::collections::HashSet<usize> = group.iter().copied().collect();
            let direction = unit(centre.view());
            let to_mean: f64 = group.iter().map(|&paper| f64::from(embeddings.row(paper).dot(&direction))).sum::<f64>()
                / group.len().max(1) as f64;
            let inner: f64 = group
                .iter()
                .map(|&i| {
                    let row = corpus.adjacency.row(i);
                    group.iter().map(|&j| f64::from(row[j])).sum::<f64>()
                })
                .sum::<f64>()
                / (group.len() * group.len()).max(1) as f64;
            let tags = group.iter().flat_map(|&paper| {
                let paper = &corpus.papers[paper];
                corpus.selection.tags(&paper.venues, &paper.primary_category)
            });
            let tiers = group.iter().filter_map(|&paper| corpus.tiers[paper].name().map(str::to_owned));
            Record {
                id: index,
                tree_path: community.path.to_string(),
                size: group.len(),
                name: None,
                description: None,
                keywords: corpus.vocabulary.keywords(group, within, KEYWORDS),
                mix: Counted::of(tags).most_common(MIX),
                mean_inner_similarity: inner,
                similarity_to_mean: to_mean,
                closest_communities: others
                    .into_iter()
                    .map(|other| Closest { id: other, cosine: f64::from(between[[index, other]]) })
                    .collect(),
                nearest_to_centroid: nearest_papers(embeddings, centre.view(), NEAREST)
                    .into_iter()
                    .map(|(paper, cosine)| Nearest {
                        id: corpus.papers[paper].id.clone(),
                        title: corpus.papers[paper].title.clone(),
                        cosine: f64::from(cosine),
                        in_community: inside.contains(&paper),
                    })
                    .collect(),
                ai_flagged: group.iter().filter(|&&paper| corpus.flagged[paper]).count(),
                acceptance: Counted::of(tiers),
                members: group.to_vec(),
                children: match community.sub_communities() {
                    [] => Vec::new(),
                    halves => community_tree(corpus, halves, Some(community)),
                },
            }
        })
        .collect()
}

/// What `communities.json` holds (`RunResult` in the contracts, and the Python's `splits`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunResult {
    pub name: String,
    pub papers: usize,
    pub model: String,
    pub centered: bool,
    pub clustering: String,
    pub modularity: f64,
    pub ai_flagged: usize,
    /// The accepted top-level cuts of the bisection method; empty for the others.
    pub splits: Vec<Split>,
    pub communities: Vec<Record>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Split {
    pub parent: String,
    pub sizes: [usize; 2],
    pub modularity: f64,
}

fn title(record: &Record) -> String {
    record.name.clone().unwrap_or_else(|| record.keywords.iter().take(4).cloned().collect::<Vec<_>>().join(", "))
}

fn outline(children: &[Record], depth: usize, lines: &mut Vec<String>) {
    for child in children {
        lines.push(format!("{}- {} ({})", "  ".repeat(depth), title(child), child.size));
        outline(&child.children, depth + 1, lines);
    }
}

fn section(record: &Record, url: &dyn Fn(&str) -> String, lines: &mut Vec<String>) {
    let closest: Vec<String> =
        record.closest_communities.iter().map(|other| format!("#{} ({:.2})", other.id, other.cosine)).collect();
    lines.push(format!("## {}. {}  ({} papers)", record.id, title(record), record.size));
    lines.push(String::new());
    if let Some(description) = record.description.as_deref().filter(|text| !text.is_empty()) {
        lines.push(description.to_owned());
        lines.push(String::new());
    }
    lines.push(format!("*Terms:* {}  ", record.keywords.join(", ")));
    let mix: Vec<String> = record.mix.0.iter().map(|(tag, count)| format!("{tag} {count}")).collect();
    lines.push(format!("*Mix:* {}  ", mix.join(", ")));
    lines.push(format!(
        "*Similarity:* members to the mean embedding {:.3}; mean pairwise weight inside {:.3}; closest communities {}",
        record.similarity_to_mean,
        record.mean_inner_similarity,
        closest.join(", ")
    ));
    lines.push(String::new());
    lines.push("Nearest to the mean embedding:".to_owned());
    lines.push(String::new());
    for paper in record.nearest_to_centroid.iter().take(6) {
        lines.push(format!("- [{}]({}) ({:.3})", paper.title, url(&paper.id), paper.cosine));
    }
    lines.push(String::new());
    if !record.children.is_empty() {
        lines.push("Sub-communities:".to_owned());
        lines.push(String::new());
        outline(&record.children, 0, lines);
        lines.push(String::new());
    }
}

/// The readable report (`report.md`); `url` gives a paper's link by its id.
pub fn markdown(result: &RunResult, url: &dyn Fn(&str) -> String) -> String {
    let centred = if result.centered { " of mean-centred embeddings" } else { "" };
    let head = format!(
        "{} papers, embedded with `{}`; adjacency = max(0, cosine similarity){centred}; clustering method `{}` gave {} \
         communities (modularity {:.3}). {} papers are flagged by the AI-text detector.",
        result.papers,
        result.model,
        result.clustering,
        result.communities.len(),
        result.modularity,
        result.ai_flagged
    );
    let mut lines = vec![format!("# Communities: {}", result.name), String::new(), head, String::new()];
    for record in &result.communities {
        section(record, url, &mut lines);
    }
    lines.join("\n")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn counted_names_keep_the_order_first_met_among_equal_counts_and_are_written_in_it() {
        let counted = Counted::of(["b", "a", "c", "a", "c", "d"].map(str::to_owned)).most_common(3);
        assert_eq!(counted.0, [("a".to_owned(), 2), ("c".to_owned(), 2), ("b".to_owned(), 1)]);
        assert_eq!(serde_json::to_string(&counted).unwrap(), r#"{"a":2,"c":2,"b":1}"#);
    }

    #[test]
    fn the_nearest_papers_are_the_most_similar_first_over_the_whole_selection() {
        let embeddings = ndarray::array![[1.0, 0.0], [0.0, 1.0], [0.7, 0.7], [1.0, 0.1]];
        let nearest = nearest_papers(embeddings.view(), ndarray::array![1.0, 0.0].view(), 2);
        assert_eq!(nearest.iter().map(|(paper, _)| *paper).collect::<Vec<_>>(), [0, 3]);
        assert_eq!(nearest_papers(embeddings.view(), ndarray::array![1.0, 0.0].view(), 10).len(), 4);
    }
}
