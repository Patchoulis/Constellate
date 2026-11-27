//! Which clustering method gives better communities on a run's papers? A tool for developers, not part of the app.
//!
//! ```text
//! cargo run --release -p constellate --example compare -- neurips2025_icml2026
//! cargo run --release -p constellate --example compare -- neurips2025_icml2026 --k 12 24 48 --model Qwen/Qwen3-Embedding-0.6B
//! cargo run --release -p constellate --example compare -- neurips2025_icml2026 --k 24 --subsamples 10
//! ```
//!
//! The data folder is `CONSTELLATE_DATA` (or the engine's default). Every method clusters the same papers into the
//! same number of communities and is scored on what a reader of the result cares about. No single score is trusted
//! on its own; each covers another's blind spot (the scores are in `pipeline/agreement.rs`):
//!
//! ```text
//!   agreement with the authors   Each conference paper carries the topic its authors filed it under ("Deep
//!     (AMI fine / coarse)        Learning->Large Language Models"): an outside reference nobody tuned against.
//!                                Adjusted mutual information against the fine topic and the coarse one, per
//!                                conference (each has its own list of topics), averaged by paper count.
//!   coherence of the wording     For each community, do its ten most characteristic terms occur together in
//!     (NPMI)                     abstracts? It favours neither k-means nor the graph methods, and needs no labels.
//!   internal quality             Silhouette (cosine, on up to 4,000 papers) and modularity of the similarity
//!                                graph. Kept for reference: each favours the family that optimises something like it.
//!   stability                    Seeds: clustered again from other random starts. Subsamples: 80% of the papers
//!                                clustered afresh; agreement (ARI) on the papers two subsamples share.
//!   shape                        Smallest and largest community.
//!   the run as made              The run's own top-level communities (the Python's, for a run it made), scored the
//!                                same way, and each method's agreement (ARI) with them: what remaking it changes.
//! ```
//!
//! It writes `clustering_comparison.md` (or `clustering_comparison_<model>.md` for another model) beside the run,
//! as the Python's `rp/compare.py` did, and prints it. The scores are the Python's definitions; the clusterings
//! are the engine's (its k-means keeps the best of several starts), and the random samples (subsamples, the
//! silhouette's papers) are drawn with the engine's generator, so the numbers are those of this engine's runs.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::process::ExitCode;

use ndarray::{Array2, Axis};
use serde_json::Value;

use constellate::analysis::graph::cluster::{self, Limits, Method, Papers};
use constellate::analysis::graph::random::Random;
use constellate::analysis::graph::{adjacency_matrix, make_unit, modularity};
use constellate::jobs::{Cancel, Progress, Tracker};
use constellate::library::core::{place, store};
use constellate::library::{Library, Paper};
use constellate::pipeline::agreement::{
    adjusted_mutual_information, adjusted_rand_index, coherence, numbered, silhouette_cosine,
};
use constellate::pipeline::embedding::{self, Embedding, Onnx};
use constellate::pipeline::files;
use constellate::pipeline::model_files::Network;
use constellate::pipeline::report;
use constellate::pipeline::steps::Board;
use constellate::pipeline::vocabulary::Vocabulary;

const SEEDS: u64 = 5;
/// Subsamples of 80% clustered afresh, unless `--subsamples` says how many.
const SUBSAMPLES: usize = 3;
const TOP_TERMS: usize = 10;
const SILHOUETTE_SAMPLE: usize = 4_000;
/// A conference's topics are scored only when this many of its papers have one.
const LEAST_WITH_TOPIC: usize = 100;

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len().max(1) as f64
}

/// `count` distinct indexes of `0..total`, in order.
fn sample(total: usize, count: usize, random: &mut Random) -> Vec<usize> {
    let mut all: Vec<usize> = (0..total).collect();
    for index in 0..count.min(total) {
        let other = index + random.below(total - index);
        all.swap(index, other);
    }
    all.truncate(count.min(total));
    all.sort_unstable();
    all
}

struct Setup {
    center: bool,
    limits: Limits,
}

/// Every method's labellings, the similarity graph and the centred unit vectors they were made from.
type Labelled = (Vec<(Method, Vec<Vec<usize>>)>, Array2<f32>, Array2<f32>);

/// One labelling per seed for every method (one only for the bisection, which has no random start).
fn labelings(embeddings: &Array2<f32>, k: usize, setup: &Setup, seeds: u64) -> Result<Labelled, String> {
    let mut vectors = embeddings.clone();
    vectors -= &report::overall_mean(embeddings.view());
    make_unit(&mut vectors);
    let adjacency = adjacency_matrix(embeddings.view(), setup.center).map_err(|error| error.to_string())?;
    let papers = Papers { adjacency: adjacency.view(), vectors: vectors.view() };
    let mut found = Vec::new();
    for method in Method::ALL {
        let seeds = if method == Method::Fiedler { 1 } else { seeds };
        let labels = (0..seeds)
            .map(|seed| cluster::labels(method, papers, k, setup.limits, seed).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        found.push((method, labels));
    }
    Ok((found, adjacency, vectors))
}

/// AMI against the authors' topics, per conference, averaged by paper count; NaN when no conference has enough.
fn against_topics(labels: &[usize], papers: &[Paper], coarse: bool) -> f64 {
    let venues: BTreeSet<&String> = papers.iter().flat_map(|paper| paper.venues.iter()).collect();
    let (mut scores, mut weights) = (Vec::new(), Vec::new());
    for venue in venues {
        let rows: Vec<usize> = (0..papers.len())
            .filter(|&i| papers[i].venues.contains(venue) && !papers[i].primary_category.is_empty())
            .collect();
        if rows.len() < LEAST_WITH_TOPIC {
            continue;
        }
        let topics: Vec<String> = rows
            .iter()
            .map(|&i| {
                let topic = &papers[i].primary_category;
                if coarse { topic.split("->").next().unwrap_or(topic).to_owned() } else { topic.clone() }
            })
            .collect();
        let found: Vec<usize> = rows.iter().map(|&i| labels[i]).collect();
        scores.push(adjusted_mutual_information(&numbered(&topics), &numbered(&found)));
        weights.push(rows.len() as f64);
    }
    if scores.is_empty() {
        return f64::NAN;
    }
    scores.iter().zip(&weights).map(|(score, weight)| score * weight).sum::<f64>() / weights.iter().sum::<f64>()
}

/// Mean ARI over every pair of labellings; `shared[i]`, when given, are the papers of labelling `i`.
fn mean_pairwise_ari(labelings: &[Vec<usize>], shared: Option<&[Vec<usize>]>) -> f64 {
    let mut scores = Vec::new();
    for a in 0..labelings.len() {
        for b in a + 1..labelings.len() {
            match shared {
                None => scores.push(adjusted_rand_index(&labelings[a], &labelings[b])),
                Some(shared) => {
                    let place: HashMap<usize, usize> =
                        shared[b].iter().enumerate().map(|(at, &paper)| (paper, at)).collect();
                    let (mut first, mut second) = (Vec::new(), Vec::new());
                    for (at, paper) in shared[a].iter().enumerate() {
                        if let Some(&other) = place.get(paper) {
                            first.push(labelings[a][at]);
                            second.push(labelings[b][other]);
                        }
                    }
                    scores.push(adjusted_rand_index(&first, &second));
                }
            }
        }
    }
    if scores.is_empty() { 1.0 } else { mean(&scores) }
}

/// The scores of one labelling.
fn score(
    labels: &[usize],
    papers: &[Paper],
    vocabulary: &Vocabulary,
    vectors: &Array2<f32>,
    adjacency: &Array2<f32>,
    silhouette_rows: &[usize],
) -> Vec<f64> {
    let groups = labels.iter().max().map_or(0, |most| most + 1);
    let members: Vec<Vec<usize>> =
        (0..groups).map(|group| (0..labels.len()).filter(|&i| labels[i] == group).collect()).collect();
    let members: Vec<Vec<usize>> = members.into_iter().filter(|group| !group.is_empty()).collect();
    let sampled = vectors.select(Axis(0), silhouette_rows);
    let sampled_labels: Vec<usize> = silhouette_rows.iter().map(|&i| labels[i]).collect();
    vec![
        against_topics(labels, papers, false),
        against_topics(labels, papers, true),
        coherence(labels, vocabulary, TOP_TERMS),
        silhouette_cosine(sampled.view(), &sampled_labels),
        modularity(adjacency.view(), &members),
        members.iter().map(Vec::len).min().unwrap_or(0) as f64,
        members.iter().map(Vec::len).max().unwrap_or(0) as f64,
    ]
}

/// One row of scores per method at `k` communities, in the Python's columns.
fn rows(
    papers: &[Paper],
    embeddings: &Array2<f32>,
    vocabulary: &Vocabulary,
    k: usize,
    setup: &Setup,
    subsamples: usize,
) -> Result<Vec<Vec<(String, f64)>>, String> {
    eprintln!("k = {k}: {SEEDS} random starts ...");
    let (runs, adjacency, vectors) = labelings(embeddings, k, setup, SEEDS)?;
    eprintln!("k = {k}: {subsamples} subsamples of 80% ...");
    let mut random = Random::new(0);
    let kept: Vec<Vec<usize>> =
        (0..subsamples).map(|_| sample(papers.len(), papers.len() * 4 / 5, &mut random)).collect();
    let subsampled = kept
        .iter()
        .map(|rows| labelings(&embeddings.select(Axis(0), rows), k, setup, 1).map(|(found, _, _)| found))
        .collect::<Result<Vec<_>, _>>()?;
    let silhouette_rows = sample(papers.len(), SILHOUETTE_SAMPLE, &mut Random::new(1));
    let mut table = Vec::new();
    for (index, (method, labelings)) in runs.iter().enumerate() {
        let scores: Vec<Vec<f64>> = labelings
            .iter()
            .map(|labels| score(labels, papers, vocabulary, &vectors, &adjacency, &silhouette_rows))
            .collect();
        let average = |column: usize| mean(&scores.iter().map(|one| one[column]).collect::<Vec<_>>());
        let fine: Vec<f64> = scores.iter().map(|one| one[0]).collect();
        let spread = (fine.iter().map(|x| (x - mean(&fine)).powi(2)).sum::<f64>() / fine.len().max(1) as f64).sqrt();
        let subsample_labels: Vec<Vec<usize>> = subsampled.iter().map(|found| found[index].1[0].clone()).collect();
        table.push(vec![
            ("k".to_owned(), k as f64),
            ("method".to_owned(), index as f64),
            ("AMI fine".to_owned(), average(0)),
            ("spread".to_owned(), spread),
            ("AMI coarse".to_owned(), average(1)),
            ("coherence".to_owned(), average(2)),
            ("silhouette".to_owned(), average(3)),
            ("modularity".to_owned(), average(4)),
            ("seed stability".to_owned(), mean_pairwise_ari(labelings, None)),
            ("subsample stability".to_owned(), mean_pairwise_ari(&subsample_labels, Some(&kept))),
            ("smallest".to_owned(), average(5).round()),
            ("largest".to_owned(), average(6).round()),
        ]);
        let _ = method;
    }
    Ok(table)
}

/// The run as it was made (by the Python, or by this engine), scored like the methods, and how far each method's
/// first random start at the run's k agrees with it (ARI): what remaking the run would change. Empty when the
/// run's communities do not cover its papers as the library has them.
fn as_made(
    result: &Value,
    papers: &[Paper],
    embeddings: &Array2<f32>,
    vocabulary: &Vocabulary,
    setup: &Setup,
) -> Result<Vec<String>, String> {
    let communities = result["communities"].as_array().cloned().unwrap_or_default();
    let mut labels = vec![usize::MAX; papers.len()];
    for (index, community) in communities.iter().enumerate() {
        for member in community["members"].as_array().into_iter().flatten() {
            match member.as_u64().map(|member| member as usize) {
                Some(member) if member < labels.len() => labels[member] = index,
                _ => return Ok(Vec::new()),
            }
        }
    }
    if communities.is_empty() || labels.contains(&usize::MAX) {
        return Ok(Vec::new());
    }
    let k = communities.len();
    eprintln!("the run as made: k = {k} ...");
    let (found, adjacency, vectors) = labelings(embeddings, k, setup, 1)?;
    let silhouette_rows = sample(papers.len(), SILHOUETTE_SAMPLE, &mut Random::new(1));
    let scores = score(&labels, papers, vocabulary, &vectors, &adjacency, &silhouette_rows);
    let clustering = result["clustering"].as_str().unwrap_or("?");
    let mut lines = vec![
        format!("The run as it was made ({clustering}, {k} communities, before sub-communities):"),
        String::new(),
        "| AMI fine | AMI coarse | coherence | silhouette | modularity | smallest | largest |".to_owned(),
        "|---|---|---|---|---|---|---|".to_owned(),
        format!(
            "| {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.0} | {:.0} |",
            scores[0], scores[1], scores[2], scores[3], scores[4], scores[5], scores[6]
        ),
        String::new(),
        format!("Agreement (ARI) of each method's first random start at k = {k} with the run as made:"),
        String::new(),
    ];
    for (method, labelings) in &found {
        lines.push(format!("- {}: {:.3}", method.id(), adjusted_rand_index(&labels, &labelings[0])));
    }
    lines.push(String::new());
    Ok(lines)
}

/// Mean rank of each method over every k, on the scores fair to all methods (1 = best).
fn ranking(table: &[Vec<(String, f64)>]) -> Vec<(f64, Method)> {
    let fair = ["AMI fine", "AMI coarse", "coherence", "subsample stability"];
    let value = |row: &Vec<(String, f64)>, name: &str| {
        row.iter().find(|(column, _)| column == name).map_or(f64::NAN, |(_, v)| *v)
    };
    let mut ranks: Vec<Vec<f64>> = vec![Vec::new(); Method::ALL.len()];
    let ks: BTreeSet<u64> = table.iter().map(|row| value(row, "k") as u64).collect();
    for k in ks {
        let at_k: Vec<&Vec<(String, f64)>> = table.iter().filter(|row| value(row, "k") as u64 == k).collect();
        for name in fair {
            if at_k.iter().any(|row| value(row, name).is_nan()) {
                continue;
            }
            let mut ordered = at_k.clone();
            ordered.sort_by(|a, b| value(b, name).total_cmp(&value(a, name)));
            for (place, row) in ordered.iter().enumerate() {
                ranks[value(row, "method") as usize].push((place + 1) as f64);
            }
        }
    }
    let mut ranked: Vec<(f64, Method)> =
        ranks.iter().zip(Method::ALL).map(|(places, method)| (mean(places), method)).collect();
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.id().cmp(b.1.id())));
    ranked
}

fn compare(name: &str, ks: Vec<usize>, model: Option<String>, subsamples: usize) -> Result<String, String> {
    let data = place::data_folder(std::env::var("CONSTELLATE_DATA").ok()).map_err(|error| error.to_string())?;
    let folder = files::folder_of(&data, name).map_err(|error| error.to_string())?;
    let stored: Value =
        store::read_json(&folder.join(files::RUN)).map_err(|error| error.to_string())?.unwrap_or_default();
    let settings = &stored["settings"];
    let run_model = settings["embedding_model"].as_str().unwrap_or("Qwen/Qwen3-Embedding-0.6B").to_owned();
    let model = model.unwrap_or_else(|| run_model.clone());
    let setup = Setup {
        center: settings["center_embeddings"].as_bool().unwrap_or(true),
        limits: Limits {
            min_community_size: settings["min_community_size"].as_u64().unwrap_or(15) as usize,
            min_split_modularity: settings["min_split_modularity"].as_f64().unwrap_or(0.05),
            sub_levels: 0,
        },
    };
    let result: Value =
        store::read_json(&folder.join(files::COMMUNITIES)).map_err(|error| error.to_string())?.unwrap_or_default();
    let ks = if ks.is_empty() { vec![result["communities"].as_array().map_or(0, Vec::len)] } else { ks };
    let library = Library::open(&data.join("library")).map_err(|error| error.to_string())?;
    // a data folder the Python made: its files-based library moved in first, as the engine does at its first open
    library.import_once(&data.join("library")).map_err(|error| error.to_string())?;
    let ids = files::paper_ids(&folder).map_err(|error| error.to_string())?;
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    let papers: Vec<Paper> = library.papers(&ids).map_err(|error| error.to_string())?.into_iter().flatten().collect();
    // papers the model has not embedded are embedded first, and kept
    let onnx = Onnx::new(std::sync::Arc::new(Network::default()));
    let log = |line: &str| eprintln!("{line}");
    let progress = |_: Progress| {};
    let cancel = Cancel::new();
    let tracker = Tracker::new(&log, &progress, &cancel);
    let board = Board::default();
    let todo = embedding::missing(&library, &model, &papers, false).map_err(|error| error.to_string())?;
    embedding::prepare(&onnx as &dyn Embedding, &model, &data, todo.len(), &tracker, &board)
        .map_err(|error| error.to_string())?;
    let (embeddings, _) = embedding::embed(&onnx, &library, &model, &papers, &todo, &tracker, &board)
        .map_err(|error| error.to_string())?;
    let texts: Vec<String> = papers.iter().map(|paper| format!("{} {}", paper.title, paper.r#abstract)).collect();
    let vocabulary = Vocabulary::new(&texts);
    let with_topic = papers.iter().filter(|paper| !paper.primary_category.is_empty()).count();
    eprintln!("{} papers, {with_topic} with a topic, embeddings: {model}", papers.len());
    let mut table = Vec::new();
    for k in ks {
        table.extend(rows(&papers, &embeddings, &vocabulary, k, &setup, subsamples)?);
    }
    let made = as_made(&result, &papers, &embeddings, &vocabulary, &setup)?;
    let columns: Vec<String> = table[0].iter().map(|(column, _)| column.clone()).collect();
    let cell = |column: &str, value: f64| match column {
        "method" => Method::ALL[value as usize].id().to_owned(),
        "k" | "smallest" | "largest" => format!("{value:.0}"),
        _ if value.is_nan() => "nan".to_owned(),
        _ => format!("{value:.3}"),
    };
    let mut lines = vec![
        format!("# Clustering comparison: {name}"),
        String::new(),
        format!(
            "{} papers, embeddings from `{model}`. Scores are means over {SEEDS} random starts; \"spread\" is the standard \
             deviation of AMI fine between starts. Stability is the mean adjusted Rand index between those starts, and \
             between {subsamples} subsamples of 80% of the papers (on the papers two subsamples share). What each score \
             means and what it cannot tell is in apps/engine/examples/compare.rs.",
            papers.len()
        ),
        String::new(),
        format!("| {} |", columns.join(" | ")),
        format!("|{}", "---|".repeat(columns.len())),
    ];
    for row in &table {
        let cells: Vec<String> = row.iter().map(|(column, value)| cell(column, *value)).collect();
        lines.push(format!("| {} |", cells.join(" | ")));
    }
    lines.push(String::new());
    lines.push(
        "Mean rank over every k on the scores that are fair to all methods (AMI fine, AMI coarse, coherence, subsample \
         stability); 1 = best:"
            .to_owned(),
    );
    lines.push(String::new());
    for (rank, method) in ranking(&table) {
        lines.push(format!("- {}: {rank:.2}", method.id()));
    }
    lines.push(String::new());
    lines.extend(made);
    let text = lines.join("\n");
    let suffix =
        if model == run_model { String::new() } else { format!("_{}", model.rsplit('/').next().unwrap_or(&model)) };
    let out: PathBuf = folder.join(format!("clustering_comparison{suffix}.md"));
    store::replace(&out, text.as_bytes()).map_err(|error| error.to_string())?;
    Ok(text)
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some(name) = arguments.first().cloned() else {
        eprintln!("usage: compare <run> [--k 12 24 48] [--model <embedding model>] [--subsamples 3]");
        return ExitCode::from(2);
    };
    let (mut ks, mut model, mut reading_ks, mut subsamples) = (Vec::new(), None, false, SUBSAMPLES);
    let mut rest = arguments.iter().skip(1);
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--k" => reading_ks = true,
            "--model" => {
                reading_ks = false;
                model = rest.next().cloned();
            }
            "--subsamples" => {
                reading_ks = false;
                match rest.next().and_then(|count| count.parse::<usize>().ok()).filter(|count| *count >= 2) {
                    Some(count) => subsamples = count,
                    None => {
                        eprintln!("--subsamples takes a whole number of at least 2");
                        return ExitCode::from(2);
                    }
                }
            }
            number if reading_ks => match number.parse::<usize>() {
                Ok(k) => ks.push(k),
                Err(_) => {
                    eprintln!("--k takes whole numbers, not {number}");
                    return ExitCode::from(2);
                }
            },
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::from(2);
            }
        }
    }
    match compare(&name, ks, model, subsamples) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("compare: {error}");
            ExitCode::FAILURE
        }
    }
}
