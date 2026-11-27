//! A run made end to end, from a library and canned sources, and compared with what the Python made of the same
//! papers.
//!
//! # The parity fixture
//!
//! `parity/input.json` is 164 made-up papers (titles and abstracts drawn from made-up topic words; no real paper's
//! data) at two conferences, with planted embeddings: 4 topics of 56, 44, 36 and 28 papers (sizes all different,
//! so the order of communities by size is fixed), 2 sub-topics each, plus noise, rounded to 4 decimals so that
//! both sides read the same float32 numbers. `parity/<method>/` holds the `communities.json`, `graph.json` and
//! `report.md` the Python pipeline (`rp/pipeline.py`: filter, similarity, communities, summarise, write) wrote for
//! it, once per clustering method, with 4 communities, communities of at least 6 papers, 2 levels of
//! sub-communities, no descriptions and no detector.
//!
//! What must be equal, and what may differ and why (the Rust is the reference from now on):
//!
//! * **Membership**: every community and sub-community has exactly the same papers.
//! * **Names of the tree** (`tree_path`): the top-level communities are named after the label k-means gave them
//!   (`c1`), and which label a group gets depends on the random start; and which half of a cut is `.0` depends on
//!   the sign of an eigenvector or the label k-means gave it. So names are compared by their shape (same depth, a
//!   child named after its parent), and halves are matched by their papers.
//! * **Numbers**: within [`CLOSE`] (float32 arithmetic summed in another order); positions up to the sign of each
//!   axis, which the SVD under NumPy leaves to chance.
//! * **Terms**: the same, in the same order, except where two terms weigh exactly the same (then the Rust puts
//!   them in alphabetical order, NumPy in no defined one): compared as the same terms with equal weights grouped.
//! * **spectral-knn**: on this fixture the nearest-neighbour graph is in 7 pieces, more than the 4 communities,
//!   so its 4 leading eigenvectors are any 4 of a 7-dimensional space and the grouping of pieces depends on the
//!   solver. The engine's run keeps every piece whole; the Python's cut through two (its one-start k-means stopped
//!   in a worse optimum). See that test.
//!
//! The halves of a cut may come in the other order, with the other names (`.0` for `.1`): in the spectral and
//! k-means runs of this fixture 3 of the 4 cuts do (which half k-means labels 0 is a matter of its start); in the
//! bisection run none does. The top-level order (by size) is the same everywhere: the levels compared count where
//! the order differed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Mutex;

use ndarray::Array2;
use serde_json::Value;

use super::steps::{Board, StepOutcome};
use super::*;
use crate::jobs::{Cancel, Progress, Tracker};
use crate::library::{Known, Library, Paper, SliceOutcome};
use crate::pipeline::steps::embedding::Embedding;
use crate::pipeline::steps::gather::Sources;
use crate::sources::Fetch;
use crate::sources::fake::Fake;

/// How close two numbers must be.
const CLOSE: f64 = 1e-4;

const INPUT: &str = include_str!("parity/input.json");

fn expected(method: &str, file: &str) -> String {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pipeline/parity").join(method);
    std::fs::read_to_string(folder.join(file)).unwrap()
}

/// An embedder that must not be asked: every paper of the fixture is embedded already.
struct Unasked;

impl Embedding for Unasked {
    fn prepare(&self, _: &str, _: &Path, _: &Tracker<'_>, _: &Board) -> Result<StepOutcome, RunError> {
        panic!("the model was readied though every paper is embedded")
    }
    fn encode(&self, _: &str, _: &[&str]) -> Result<Vec<Vec<f32>>, RunError> {
        panic!("a paper was embedded twice")
    }
    fn device(&self) -> String {
        "nothing".into()
    }
}

/// Words of a text made into a vector: each word adds to one of a few directions, so papers about the same words
/// lie together. For the end-to-end test, where the real model is not run.
pub(crate) struct Words {
    pub asked: Mutex<usize>,
}

impl Embedding for Words {
    fn prepare(&self, _: &str, _: &Path, _: &Tracker<'_>, _: &Board) -> Result<StepOutcome, RunError> {
        Ok(StepOutcome::Kept("a stand-in".into()))
    }
    fn encode(&self, _: &str, texts: &[&str]) -> Result<Vec<Vec<f32>>, RunError> {
        *self.asked.lock().unwrap() += texts.len();
        Ok(texts
            .iter()
            .map(|text| {
                let mut vector = [0.0_f32; 16];
                for word in text.split_whitespace() {
                    let hash =
                        word.bytes().fold(7_u32, |hash, byte| hash.wrapping_mul(31).wrapping_add(u32::from(byte)));
                    vector[(hash % 16) as usize] += 1.0;
                }
                let length = vector.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
                vector.iter().map(|x| x / length).collect()
            })
            .collect())
    }
    fn device(&self) -> String {
        "a stand-in".into()
    }
}

fn settings_for(method: &str) -> Settings {
    Settings {
        max_communities: 4,
        min_community_size: 6,
        sub_levels: 2,
        describe: false,
        detect_ai: false,
        clustering: method.to_owned(),
        ..Settings::default()
    }
}

/// A data folder whose library holds the fixture's papers and vectors, its conferences fetched already.
fn library_of_fixture(data: &Path) -> (Library, Selection) {
    let input: Value = serde_json::from_str(INPUT).unwrap();
    let papers: Vec<Paper> = serde_json::from_value(input["papers"].clone()).unwrap();
    let library = Library::open(&data.join("library")).unwrap();
    let venues: BTreeSet<String> = papers.iter().flat_map(|paper| paper.venues.clone()).collect();
    for venue in &venues {
        let listed: Vec<Paper> = papers.iter().filter(|paper| paper.venues.contains(venue)).cloned().collect();
        let key = format!("venue|{venue}|False|official");
        library.add_slice(&key, SliceOutcome::Complete, &listed, Some(venue), Known::Keep).unwrap();
    }
    let rows: Vec<Vec<f32>> = serde_json::from_value(input["embeddings"].clone()).unwrap();
    let width = rows[0].len();
    let vectors = Array2::from_shape_vec((rows.len(), width), rows.concat()).unwrap();
    library.add_embeddings("Qwen/Qwen3-Embedding-0.6B", &papers, vectors.view()).unwrap();
    let selection = Selection::parse(&["NeurIPS 2025".into(), "ICML 2026".into()], None, None, &[], false).unwrap();
    (library, selection)
}

/// The model a test's runs are named with when nothing was accepted: the settings', which cannot be asked.
fn kept_only(settings: &Settings) -> seams::Naming {
    use crate::model::language::{Provider, Unavailable};
    let chosen = crate::model::chosen::Chosen::of(
        Provider::Anthropic,
        &settings.describe_model,
        settings.describe_effort,
        Err(Unavailable::NoKey),
    );
    seams::Naming::KeptOnly(chosen)
}

/// Make the run with `settings`, nothing accepted for naming it; answers its log.
fn make_run(
    data: &Path,
    library: &Library,
    selection: &Selection,
    settings: Settings,
    fetch: &(dyn Fetch + Sync),
    embedding: &dyn Embedding,
    redo: Redo,
) -> Result<(String, Vec<String>), RunError> {
    let naming = kept_only(&settings);
    let transport = crate::model::language::fake::Fake::default();
    make_named(data, library, selection, settings, fetch, embedding, redo, &naming, &transport)
}

/// Make the run with `settings`, naming its communities as `naming` allows through `transport`.
#[allow(clippy::too_many_arguments)]
fn make_named(
    data: &Path,
    library: &Library,
    selection: &Selection,
    settings: Settings,
    fetch: &(dyn Fetch + Sync),
    embedding: &dyn Embedding,
    redo: Redo,
    naming: &seams::Naming,
    transport: &dyn crate::model::language::Transport,
) -> Result<(String, Vec<String>), RunError> {
    let board = Board::default();
    let log = Mutex::new(Vec::new());
    let write = |line: &str| log.lock().unwrap().push(line.to_owned());
    let progress = |_: Progress| {};
    let cancel = Cancel::new();
    let tracker = Tracker::new(&write, &progress, &cancel);
    let context = Context {
        data,
        library,
        settings,
        sources: Sources { fetch, pause: &|_| {}, today: chrono::NaiveDate::from_ymd_opt(2026, 11, 27).unwrap() },
        embedding,
        board: &board,
        transport,
        naming,
    };
    let request = Request { selection: selection.clone(), redo };
    let name = make(&context, &request, &tracker)?;
    assert_eq!(board.now(), None, "the board is cleared once the run ends");
    Ok((name, log.into_inner().unwrap()))
}

fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or_else(|| panic!("{value} is not a number"))
}

fn close(a: &Value, b: &Value, what: &str) {
    assert!((number(a) - number(b)).abs() <= CLOSE, "{what}: {a} and {b}");
}

/// The members of a record, as a set.
fn members(record: &Value) -> BTreeSet<u64> {
    record["members"].as_array().unwrap().iter().map(|member| member.as_u64().unwrap()).collect()
}

/// What the terms of a community are compared with: the vocabulary of the run's papers, the community's papers and
/// those it is set apart from.
struct Terms<'a> {
    vocabulary: &'a vocabulary::Vocabulary,
    group: Vec<usize>,
    within: Option<Vec<usize>>,
}

/// The same terms, place for place, except that terms of exactly equal weight may come in another order (and so a
/// tie at the cut may let in another term of that weight): the weights, place for place, must be the same.
fn same_terms(python: &Value, rust: &Value, terms: &Terms, what: &str) {
    let list = |value: &Value| -> Vec<String> {
        value.as_array().unwrap().iter().map(|term| term.as_str().unwrap().to_owned()).collect()
    };
    let (python, rust) = (list(python), list(rust));
    if python == rust {
        return;
    }
    let weigh = |term: &String| terms.vocabulary.weight(&terms.group, terms.within.as_deref(), term).unwrap();
    let (a, b): (Vec<f64>, Vec<f64>) = (python.iter().map(weigh).collect(), rust.iter().map(weigh).collect());
    assert_eq!(a.len(), b.len(), "{what}: {python:?} and {rust:?}");
    for (place, (a, b)) in a.iter().zip(&b).enumerate() {
        assert!(
            (a - b).abs() <= 1e-12 * a.abs().max(1.0),
            "{what}: term {place} of {python:?} and {rust:?} weighs {a} and {b}"
        );
    }
}

/// Compare one level of the tree (siblings), matching them by their papers. Answers how many records were
/// compared, and fills `halves_swapped` with the cuts whose halves came in the other order.
fn same_level(
    python: &[Value],
    rust: &[Value],
    parent: &str,
    within: Option<Vec<usize>>,
    vocabulary: &vocabulary::Vocabulary,
    swapped: &mut usize,
) -> usize {
    assert_eq!(python.len(), rust.len(), "{parent}: how many communities");
    // which Rust record each Python record is
    let place: Vec<usize> = python
        .iter()
        .map(|record| {
            rust.iter()
                .position(|other| members(other) == members(record))
                .unwrap_or_else(|| panic!("{parent}: no community has the papers of {}", record["tree_path"]))
        })
        .collect();
    if place.iter().enumerate().any(|(index, at)| index != *at) {
        *swapped += 1;
    }
    let mut compared = 0;
    for (python, &at) in python.iter().zip(&place) {
        let rust = &rust[at];
        let path = python["tree_path"].as_str().unwrap();
        let (python_path, rust_path) = (path, rust["tree_path"].as_str().unwrap());
        assert_eq!(python_path.matches('.').count(), rust_path.matches('.').count(), "the depth of {path}");
        assert_eq!(rust["id"].as_u64().unwrap() as usize, at);
        for field in ["size", "ai_flagged", "members", "name", "description"] {
            assert_eq!(python[field], rust[field], "{path}: {field}");
        }
        // counted maps: the same counts (their order is the order first met, the same on both sides)
        for field in ["mix", "acceptance"] {
            assert_eq!(python[field], rust[field], "{path}: {field}");
        }
        close(&python["mean_inner_similarity"], &rust["mean_inner_similarity"], path);
        close(&python["similarity_to_mean"], &rust["similarity_to_mean"], path);
        let group: Vec<usize> = members(rust).into_iter().map(|member| member as usize).collect();
        let terms = Terms { vocabulary, group: group.clone(), within: within.clone() };
        same_terms(&python["keywords"], &rust["keywords"], &terms, path);
        let near = |record: &Value| -> Vec<(String, bool)> {
            record["nearest_to_centroid"]
                .as_array()
                .unwrap()
                .iter()
                .map(|paper| (paper["id"].as_str().unwrap().to_owned(), paper["in_community"].as_bool().unwrap()))
                .collect()
        };
        assert_eq!(near(python), near(rust), "{path}: nearest papers");
        for (a, b) in python["nearest_to_centroid"]
            .as_array()
            .unwrap()
            .iter()
            .zip(rust["nearest_to_centroid"].as_array().unwrap())
        {
            close(&a["cosine"], &b["cosine"], path);
            assert_eq!(a["title"], b["title"]);
        }
        let closest = |record: &Value, renumber: &dyn Fn(u64) -> u64| -> Vec<u64> {
            record["closest_communities"]
                .as_array()
                .unwrap()
                .iter()
                .map(|other| renumber(other["id"].as_u64().unwrap()))
                .collect()
        };
        assert_eq!(closest(python, &|id| place[id as usize] as u64), closest(rust, &|id| id), "{path}: closest");
        for child in rust["children"].as_array().unwrap() {
            assert!(child["tree_path"].as_str().unwrap().starts_with(&format!("{rust_path}.")), "{rust_path}'s child");
        }
        compared += 1 + same_level(
            python["children"].as_array().unwrap(),
            rust["children"].as_array().unwrap(),
            path,
            Some(group),
            vocabulary,
            swapped,
        );
    }
    compared
}

/// The graphs: the same nodes in the same order, the same links.
fn same_graph(python: &Value, rust: &Value) {
    let (a, b) = (python["nodes"].as_array().unwrap(), rust["nodes"].as_array().unwrap());
    assert_eq!(a.len(), b.len());
    // positions up to the sign of each axis
    let sign = |axis: &str| {
        let dot: f64 = a.iter().zip(b).map(|(p, r)| number(&p[axis]) * number(&r[axis])).sum();
        if dot < 0.0 { -1.0 } else { 1.0 }
    };
    let (x, y) = (sign("x"), sign("y"));
    for (python, rust) in a.iter().zip(b) {
        let what = python["id"].as_str().unwrap();
        for field in ["id", "title", "community", "ai", "acceptance", "acceptance_term"] {
            assert_eq!(python.get(field), rust.get(field), "{what}: {field}");
        }
        assert!((number(&python["x"]) - x * number(&rust["x"])).abs() <= 2e-3, "{what}: x");
        assert!((number(&python["y"]) - y * number(&rust["y"])).abs() <= 2e-3, "{what}: y");
        let keys = |node: &Value| node.as_object().unwrap().keys().cloned().collect::<BTreeSet<_>>();
        assert_eq!(keys(python), keys(rust), "{what}: the same fields");
    }
    let links = |graph: &Value| -> BTreeMap<(u64, u64), f64> {
        graph["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|edge| ((edge[0].as_u64().unwrap(), edge[1].as_u64().unwrap()), number(&edge[2])))
            .collect()
    };
    let (python_links, rust_links) = (links(python), links(rust));
    assert_eq!(python_links.keys().collect::<Vec<_>>(), rust_links.keys().collect::<Vec<_>>(), "the same links");
    for (pair, similarity) in &python_links {
        assert!((similarity - rust_links[pair]).abs() <= 1.5e-3, "link {pair:?}");
    }
}

/// Make the fixture's run with `method` and compare it with the Python's. Answers the cuts whose halves came in
/// the other order.
fn parity(method: &str) -> usize {
    let data = tempfile::tempdir().unwrap();
    let (library, selection) = library_of_fixture(data.path());
    let (name, _) =
        make_run(data.path(), &library, &selection, settings_for(method), &Fake::new(), &Unasked, Redo::new()).unwrap();
    assert_eq!(name, "neurips2025_icml2026");
    let folder = data.path().join("runs").join(&name);
    let read =
        |file: &str| -> Value { serde_json::from_str(&std::fs::read_to_string(folder.join(file)).unwrap()).unwrap() };
    let (python, rust) =
        (serde_json::from_str::<Value>(&expected(method, "communities.json")).unwrap(), read("communities.json"));
    for field in ["name", "papers", "model", "centered", "clustering", "ai_flagged"] {
        assert_eq!(python[field], rust[field], "{field}");
    }
    close(&python["modularity"], &rust["modularity"], "modularity");
    let keys = |value: &Value| value.as_object().unwrap().keys().cloned().collect::<BTreeSet<_>>();
    assert_eq!(keys(&python), keys(&rust), "communities.json has the same fields");
    let (a, b) = (python["splits"].as_array().unwrap(), rust["splits"].as_array().unwrap());
    assert_eq!(a.len(), b.len());
    for (a, b) in a.iter().zip(b) {
        assert_eq!(a["sizes"], b["sizes"]);
        close(&a["modularity"], &b["modularity"], "a split's modularity");
    }
    let papers: Vec<Paper> =
        serde_json::from_value(serde_json::from_str::<Value>(INPUT).unwrap()["papers"].clone()).unwrap();
    let texts: Vec<String> = papers.iter().map(|paper| format!("{} {}", paper.title, paper.r#abstract)).collect();
    let vocabulary = vocabulary::Vocabulary::new(&texts);
    let mut swapped = 0;
    let (a, b) = (python["communities"].as_array().unwrap(), rust["communities"].as_array().unwrap());
    let compared = same_level(a, b, "", None, &vocabulary, &mut swapped);
    assert!(compared >= 8, "{compared} communities compared");
    same_graph(&serde_json::from_str(&expected(method, "graph.json")).unwrap(), &read("graph.json"));
    // every record has the contract's fields, at every depth
    let community_fields: BTreeSet<&str> = [
        "id",
        "tree_path",
        "size",
        "name",
        "description",
        "keywords",
        "mix",
        "mean_inner_similarity",
        "similarity_to_mean",
        "closest_communities",
        "nearest_to_centroid",
        "acceptance",
        "ai_flagged",
        "members",
        "children",
    ]
    .into();
    let mut stack: Vec<Value> = rust["communities"].as_array().unwrap().clone();
    while let Some(record) = stack.pop() {
        let found: BTreeSet<String> = record.as_object().unwrap().keys().cloned().collect();
        assert_eq!(found.iter().map(String::as_str).collect::<BTreeSet<_>>(), community_fields);
        stack.extend(record["children"].as_array().unwrap().iter().cloned());
    }
    swapped
}

#[test]
fn a_spectral_run_has_the_communities_graph_and_figures_the_python_made_of_the_same_papers() {
    assert!(parity("spectral") <= 4, "only halves of cuts may come in another order");
}

#[test]
fn a_kmeans_run_has_the_communities_the_python_made_of_the_same_papers() {
    assert!(parity("kmeans") <= 4, "only halves of cuts may come in another order");
}

#[test]
fn a_bisection_run_has_the_communities_and_cuts_the_python_made_of_the_same_papers() {
    assert_eq!(parity("fiedler"), 0, "the halves come in the same order: the eigenvectors' signs agree");
}

#[test]
fn a_spectral_run_on_nearest_neighbours_cuts_along_the_pieces_of_the_graph_as_the_python_did() {
    // On this fixture the nearest-neighbour graph falls apart into 7 pieces (each sub-topic's papers link only
    // among themselves), more than the 4 communities asked for, so its normalised matrix has the eigenvalue 1 seven
    // times and the 4 leading eigenvectors are any 4 of that space: which pieces end up together depends on the
    // solver. What a good answer must do is cut only between pieces, never through one, and the engine's does. The
    // Python's did not: its k-means ran from one random start (scikit-learn, n_init=1) and stopped in a worse local
    // optimum (objective 55.1, where ten starts all reach 50.7), cutting through two pieces. That is the difference,
    // and it is the Python's: the engine's k-means keeps the best of several starts and then improves it.
    let input: Value = serde_json::from_str(INPUT).unwrap();
    let rows: Vec<Vec<f32>> = serde_json::from_value(input["embeddings"].clone()).unwrap();
    let embeddings = Array2::from_shape_vec((rows.len(), rows[0].len()), rows.concat()).unwrap();
    let adjacency = crate::analysis::graph::adjacency_matrix(embeddings.view(), true).unwrap();
    let neighbours = crate::analysis::graph::links::knn_graph(adjacency.view(), crate::analysis::graph::links::KNN);
    // the pieces, by labelling each paper with the smallest paper it is joined to
    let mut piece: Vec<usize> = (0..rows.len()).collect();
    loop {
        let mut changed = false;
        for paper in 0..rows.len() {
            for (other, _) in neighbours.links(paper) {
                let least = piece[paper].min(piece[other]);
                if piece[paper] != least || piece[other] != least {
                    (piece[paper], piece[other], changed) = (least, least, true);
                }
            }
        }
        if !changed {
            break;
        }
    }
    let pieces: BTreeSet<usize> = piece.iter().copied().collect();
    assert!(pieces.len() > 4, "the fixture's nearest-neighbour graph is in {} pieces", pieces.len());
    let whole_pieces = |communities: &Value| {
        communities.as_array().unwrap().iter().all(|community| {
            let inside = members(community);
            inside.iter().all(|&paper| {
                (0..rows.len())
                    .filter(|&other| piece[other] == piece[paper as usize])
                    .all(|other| inside.contains(&(other as u64)))
            })
        })
    };
    let python: Value = serde_json::from_str(&expected("spectral-knn", "communities.json")).unwrap();
    assert!(!whole_pieces(&python["communities"]), "the Python's run cut through pieces of the graph");
    let data = tempfile::tempdir().unwrap();
    let (library, selection) = library_of_fixture(data.path());
    let (name, _) =
        make_run(data.path(), &library, &selection, settings_for("spectral-knn"), &Fake::new(), &Unasked, Redo::new())
            .unwrap();
    let rust: Value = serde_json::from_str(
        &std::fs::read_to_string(data.path().join("runs").join(name).join(files::COMMUNITIES)).unwrap(),
    )
    .unwrap();
    assert!(whole_pieces(&rust["communities"]), "a community cuts through a piece of the graph");
    let sizes: Vec<u64> =
        rust["communities"].as_array().unwrap().iter().map(|community| community["size"].as_u64().unwrap()).collect();
    assert_eq!(sizes.iter().sum::<u64>(), 164);
}

#[test]
fn the_report_reads_as_the_pythons_where_the_tree_came_in_the_same_order() {
    let data = tempfile::tempdir().unwrap();
    let (library, selection) = library_of_fixture(data.path());
    let (name, _) =
        make_run(data.path(), &library, &selection, settings_for("spectral"), &Fake::new(), &Unasked, Redo::new())
            .unwrap();
    let rust = std::fs::read_to_string(data.path().join("runs").join(name).join(files::REPORT)).unwrap();
    let python = expected("spectral", "report.md");
    let (a, b): (Vec<&str>, Vec<&str>) = (python.lines().collect(), rust.lines().collect());
    assert_eq!(a.len(), b.len());
    // lines with figures may differ in a last digit; the others are the same (terms aside, see the fixture)
    let same = a.iter().zip(&b).filter(|(a, b)| a == b).count();
    assert!(same * 10 >= a.len() * 8, "{same} of {} lines the same", a.len());
    assert_eq!(a[0], b[0]);
}

#[test]
fn a_run_is_the_same_byte_for_byte_on_one_thread_and_on_many() {
    let data = tempfile::tempdir().unwrap();
    let (library, selection) = library_of_fixture(data.path());
    let files_of = || {
        let (name, _) =
            make_run(data.path(), &library, &selection, settings_for("spectral"), &Fake::new(), &Unasked, Redo::new())
                .unwrap();
        let folder = data.path().join("runs").join(name);
        [files::COMMUNITIES, files::GRAPH, files::REPORT, files::CENTROIDS, files::RUN]
            .map(|file| std::fs::read(folder.join(file)).unwrap())
    };
    let one = crate::threads::pool(1).install(files_of);
    let many = crate::threads::pool(16).install(files_of);
    assert!(one == many, "the files differ between one thread and sixteen");
}

/// The name of the run [`python_data_folder`] holds.
pub(crate) const PYTHON_RUN: &str = "neurips2025";

/// A data folder as the Python left it: its files-based library (`library/papers.jsonl`, `download_done.json`,
/// `embeddings/<model>/vectors.npy` and `index.json`), `settings.json`, and one run made by it, renamed, in
/// `runs/`. 36 made-up conference papers on three made-up topics, each with its vector.
pub(crate) fn python_data_folder(data: &Path) {
    let library = data.join("library");
    let qwen = library.join("embeddings").join("Qwen_Qwen3-Embedding-0.6B");
    std::fs::create_dir_all(&qwen).unwrap();
    let mut lines = Vec::new();
    let mut index = serde_json::Map::new();
    let mut vectors = Array2::<f32>::zeros((36, 4));
    for paper in 0..36 {
        let topic = paper % 3;
        let record = serde_json::json!({
            "id": format!("neurips-2025-{}", paper + 1),
            "title": format!("Made-up title {paper}"),
            "abstract": format!("Made-up words of topic{topic} and more topic{topic} words."),
            "authors": ["A. Author"],
            "published": "2025-12-01T00:00:00Z",
            "updated": "2025-12-01T00:00:00Z",
            "comment": if paper % 9 == 0 { "Accept (oral)" } else { "Accept (poster)" },
            "primary_category": "Topic",
            "categories": ["Topic"],
            "url": format!("https://example.org/{paper}"),
            "pdf_url": null,
            "venues": ["NeurIPS 2025"],
        });
        let kept: Paper = serde_json::from_value(record.clone()).unwrap();
        index.insert(kept.id.clone(), serde_json::json!([paper, kept.text_hash()]));
        lines.push(record.to_string());
        vectors[[paper, topic]] = 1.0;
        vectors[[paper, 3]] = 0.01 * paper as f32;
    }
    std::fs::write(library.join("papers.jsonl"), lines.join("\n") + "\n").unwrap();
    std::fs::write(library.join("download_done.json"), r#"["venue|NeurIPS 2025|False|official"]"#).unwrap();
    std::fs::write(qwen.join("vectors.npy"), files::npy(vectors.view())).unwrap();
    std::fs::write(qwen.join("index.json"), Value::Object(index).to_string()).unwrap();
    std::fs::write(data.join("settings.json"), r#"{"max_communities": 3, "min_community_size": 3, "describe": false}"#)
        .unwrap();
    // the run the Python made of them, its files written the way Python's json.dumps writes them
    let run = data.join("runs").join(PYTHON_RUN);
    std::fs::create_dir_all(&run).unwrap();
    std::fs::write(
        run.join(files::RUN),
        r#"{"selection": {"venues": ["NeurIPS 2025"], "start": null, "end": null, "categories": [], "include_workshops": false}, "settings": {"clustering": "spectral"}, "paper_ids": ["neurips-2025-1"]}"#,
    )
    .unwrap();
    std::fs::write(
        run.join(files::COMMUNITIES),
        r#"{"name": "neurips2025", "papers": 36, "model": "Qwen/Qwen3-Embedding-0.6B", "centered": true, "clustering": "spectral", "modularity": 0.4123, "ai_flagged": 0, "splits": [], "communities": [{"id": 0, "tree_path": "c2", "size": 36, "name": null, "description": null, "keywords": ["topic0"], "mix": {"NeurIPS 2025": 36}, "mean_inner_similarity": 0.1, "similarity_to_mean": 0.5, "closest_communities": [], "nearest_to_centroid": [], "ai_flagged": 0, "acceptance": {"poster": 32, "oral": 4}, "members": [0], "children": []}]}"#,
    )
    .unwrap();
    std::fs::write(
        run.join(files::GRAPH),
        r#"{"nodes": [{"id": "neurips-2025-1", "title": "Made-up title 0", "community": 0, "x": 0.5, "y": -0.25, "ai": false, "acceptance": "oral", "acceptance_term": {"conference": "NeurIPS 2025", "called": "Oral", "means": "1.6% of the papers in its list", "known": true}}], "edges": []}"#,
    )
    .unwrap();
    std::fs::write(run.join(files::TITLE), "My conference run").unwrap();
    std::fs::write(run.join("adjacency.npy"), files::npy(Array2::<f32>::zeros((1, 1)).view())).unwrap();
}

#[test]
fn an_old_library_is_moved_in_and_its_run_made_again_fetches_and_embeds_nothing() {
    let data = tempfile::tempdir().unwrap();
    python_data_folder(data.path());
    let library = Library::open(&data.path().join("library")).unwrap();
    let imported = library.import_once(&data.path().join("library")).unwrap().unwrap();
    assert_eq!((imported.papers, imported.embeddings["Qwen/Qwen3-Embedding-0.6B"]), (36, 36));
    let folder = files::folder_of(data.path(), PYTHON_RUN).unwrap();
    let selection = files::stored_selection(&folder).unwrap();
    let settings = Settings { max_communities: 3, min_community_size: 3, ..settings_for("spectral") };
    let (name, log) =
        make_run(data.path(), &library, &selection, settings, &Fake::new(), &Unasked, Redo::new()).unwrap();
    assert_eq!(name, PYTHON_RUN);
    assert!(log.iter().any(|line| line.contains("1 of 1 already downloaded")), "{log:?}");
    let result: Value =
        serde_json::from_str(&std::fs::read_to_string(folder.join(files::COMMUNITIES)).unwrap()).unwrap();
    let sizes: Vec<u64> =
        result["communities"].as_array().unwrap().iter().map(|c| c["size"].as_u64().unwrap()).collect();
    assert_eq!(sizes, [12, 12, 12]);
    assert_eq!(files::title(&folder, PYTHON_RUN), "My conference run", "made again, it keeps its title");
}

/// A conference's list as its site serves it, of made-up papers on three made-up topics.
fn listing(papers: usize) -> String {
    let topics = [
        "lattice quark gluon hadron meson boson plasma collider",
        "protein folding enzyme ligand binding molecule residue docking",
        "policy reward agent rollout planning exploration bandit regret",
    ];
    let results: Vec<Value> = (0..papers)
        .map(|index| {
            let words = topics[index % 3];
            serde_json::json!({
                "id": index + 1,
                "name": format!("Paper {index} on {}", words.split(' ').nth(index % 8).unwrap_or("x")),
                "eventtype": "Poster",
                "abstract": format!("{words} {words} number{}", index % 5),
                "decision": if index % 10 == 0 { "Accept (oral)" } else { "Accept (poster)" },
            })
        })
        .collect();
    serde_json::json!({ "results": results }).to_string()
}

fn conference_fetch(papers: usize) -> Fake {
    use crate::sources::conferences::{Part, list_url};
    use crate::sources::fake::{ok, status};
    Fake::new()
        .with(&list_url("neurips.cc", "neurips", "2025", Part::OralsPosters), ok("application/json", &listing(papers)))
        .with(&list_url("neurips.cc", "neurips", "2025", Part::Abstracts), status(404))
}

#[test]
fn a_run_gathers_embeds_and_writes_and_a_second_run_fetches_and_embeds_nothing_again() {
    let data = tempfile::tempdir().unwrap();
    let library = Library::open(&data.path().join("library")).unwrap();
    let selection = Selection::parse(&["NeurIPS 2025".into()], None, None, &[], false).unwrap();
    let fetch = conference_fetch(90);
    let words = Words { asked: Mutex::new(0) };
    let settings = Settings { max_communities: 3, min_community_size: 5, ..settings_for("spectral") };
    let (name, log) =
        make_run(data.path(), &library, &selection, settings.clone(), &fetch, &words, Redo::new()).unwrap();
    assert_eq!(name, "neurips2025");
    assert_eq!(*words.asked.lock().unwrap(), 90);
    for step in Step::ALL {
        assert!(log.iter().any(|line| line.ends_with(step.title())), "the log names {step:?}");
    }
    assert!(log.iter().any(|line| line.contains("skipped: switched off")), "{log:?}");
    let asked = fetch.asked().len();
    let folder = data.path().join("runs").join(&name);
    let first = std::fs::read(folder.join(files::COMMUNITIES)).unwrap();
    let result: Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(result["papers"], 90);
    let graph: Value = serde_json::from_str(&std::fs::read_to_string(folder.join(files::GRAPH)).unwrap()).unwrap();
    assert_eq!(graph["nodes"][0]["acceptance"], "oral");
    assert_eq!(graph["nodes"][0]["acceptance_term"]["called"], "Oral");
    assert!(graph["nodes"][1].get("acceptance_term").is_none(), "a poster's term is left out");
    // again: the list is complete in the library, every paper embedded
    let (_, again) =
        make_run(data.path(), &library, &selection, settings.clone(), &fetch, &words, Redo::new()).unwrap();
    assert_eq!(fetch.asked().len(), asked, "nothing fetched again");
    assert_eq!(*words.asked.lock().unwrap(), 90, "nothing embedded again");
    assert!(again.iter().any(|line| line.contains("1 of 1 already downloaded")));
    assert_eq!(std::fs::read(folder.join(files::COMMUNITIES)).unwrap(), first, "the same run again");
    // redoing the embeddings embeds every paper again, and only that
    let redo: Redo = [RedoStep::Embeddings].into();
    make_run(data.path(), &library, &selection, settings, &fetch, &words, redo).unwrap();
    assert_eq!((*words.asked.lock().unwrap(), fetch.asked().len()), (180, asked));
}

#[test]
fn a_community_described_before_gets_its_description_back_and_nothing_is_asked() {
    let data = tempfile::tempdir().unwrap();
    let library = Library::open(&data.path().join("library")).unwrap();
    let selection = Selection::parse(&["NeurIPS 2025".into()], None, None, &[], false).unwrap();
    let fetch = conference_fetch(90);
    let words = Words { asked: Mutex::new(0) };
    let settings = Settings { max_communities: 3, min_community_size: 5, describe: true, ..settings_for("spectral") };
    let (name, _) = make_run(data.path(), &library, &selection, settings.clone(), &fetch, &words, Redo::new()).unwrap();
    let file = data.path().join("runs").join(&name).join(files::COMMUNITIES);
    let result: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(result["communities"][0]["name"], Value::Null);
    // as the Python's describer kept it: by the model and the community's papers
    let papers = library.select(&selection).unwrap();
    let members: Vec<usize> = serde_json::from_value(result["communities"][0]["members"].clone()).unwrap();
    let key = crate::library::community_key(&settings.describe_model, members.iter().map(|&i| papers[i].id.as_str()));
    let kept = crate::library::Description { name: "Made-up physics".into(), description: "About it.".into() };
    library.keep_description(&key, &kept).unwrap();
    let (_, log) = make_run(data.path(), &library, &selection, settings.clone(), &fetch, &words, Redo::new()).unwrap();
    let result: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        (result["communities"][0]["name"].clone(), result["communities"][0]["description"].clone()),
        (Value::from("Made-up physics"), Value::from("About it."))
    );
    assert!(log.iter().any(|line| line.contains("kept from before: 1 of")), "{log:?}");
    assert!(log.iter().any(|line| line.contains("No cost was accepted for naming")), "{log:?}");
    // redone without anything accepted: nothing is asked, and the kept description is not used
    let redo: Redo = [RedoStep::Descriptions].into();
    let (_, log) = make_run(data.path(), &library, &selection, settings, &fetch, &words, redo).unwrap();
    let result: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(result["communities"][0]["name"], Value::Null);
    assert!(log.iter().any(|line| line.contains("skipped: no cost was accepted")), "{log:?}");
}

/// The names of a run's communities (and sub-communities), depth first: null for one not named.
fn names_of(data: &Path, name: &str) -> Vec<Value> {
    fn walk(communities: &Value, into: &mut Vec<Value>) {
        for community in communities.as_array().unwrap() {
            into.push(community["name"].clone());
            walk(&community["children"], into);
        }
    }
    let file = data.join("runs").join(name).join(files::COMMUNITIES);
    let result: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let mut names = Vec::new();
    walk(&result["communities"], &mut names);
    names
}

fn naming_settings() -> Settings {
    Settings { max_communities: 3, min_community_size: 5, describe: true, ..settings_for("spectral") }
}

/// Naming within what was accepted of the estimate before the run, as `POST /api/runs` builds it.
fn accepted(settings: &Settings) -> (seams::Naming, f64) {
    use crate::model::chosen::Accepted;
    let chosen = crate::features::writing::describe::tests::chosen("claude-opus-5-5");
    let shown = crate::features::writing::describe::estimate_before_run(&chosen, settings);
    let most = shown.cost.most_usd.unwrap().dollars();
    let budget =
        crate::features::writing::describe::budget_before_run(&chosen, settings, Accepted::Dollars(most)).unwrap();
    (seams::Naming::Within { chosen, budget }, most)
}

#[test]
fn a_run_named_within_what_was_accepted_carries_its_names_and_costs_the_estimate_at_most() {
    let data = tempfile::tempdir().unwrap();
    let library = Library::open(&data.path().join("library")).unwrap();
    let selection = Selection::parse(&["NeurIPS 2025".into()], None, None, &[], false).unwrap();
    let (fetch, words) = (conference_fetch(90), Words { asked: Mutex::new(0) });
    let transport = crate::features::writing::describe::tests::scripted();
    let (naming, most) = accepted(&naming_settings());
    let (name, log) = make_named(
        data.path(),
        &library,
        &selection,
        naming_settings(),
        &fetch,
        &words,
        Redo::new(),
        &naming,
        &*transport,
    )
    .unwrap();
    let names = names_of(data.path(), &name);
    assert!(names.iter().all(|name| name.as_str().is_some_and(|name| name.starts_with("Topic "))), "{names:?}");
    let report = std::fs::read_to_string(data.path().join("runs").join(&name).join(files::REPORT)).unwrap();
    assert!(report.contains(names[0].as_str().unwrap()), "the report carries the names:\n{report}");
    let seams::Naming::Within { budget, .. } = &naming else { unreachable!() };
    let spent = budget.spent().1.dollars();
    assert!(spent > 0.0 && spent <= most, "spent {spent} of the {most} accepted");
    assert!(log.iter().any(|line| line.contains("named now")), "{log:?}");
    // the same run again: every name kept, nothing sent
    let sent = transport.sent().len();
    let (again, _) = accepted(&naming_settings());
    make_named(data.path(), &library, &selection, naming_settings(), &fetch, &words, Redo::new(), &again, &*transport)
        .unwrap();
    assert_eq!(transport.sent().len(), sent, "a second identical run sends nothing");
    assert_eq!(names_of(data.path(), &name), names);
    // made again with nothing accepted: the kept names, and still nothing sent
    make_named(
        data.path(),
        &library,
        &selection,
        naming_settings(),
        &fetch,
        &words,
        Redo::new(),
        &kept_only(&naming_settings()),
        &*transport,
    )
    .unwrap();
    assert_eq!((transport.sent().len(), names_of(data.path(), &name)), (sent, names));
}

#[test]
fn a_run_with_nothing_accepted_is_made_without_names_and_sends_nothing() {
    let data = tempfile::tempdir().unwrap();
    let library = Library::open(&data.path().join("library")).unwrap();
    let selection = Selection::parse(&["NeurIPS 2025".into()], None, None, &[], false).unwrap();
    let (fetch, words) = (conference_fetch(90), Words { asked: Mutex::new(0) });
    let transport = crate::features::writing::describe::tests::scripted();
    // a model that could be asked (its key set): nothing is sent all the same, since nothing was accepted
    let naming = seams::Naming::KeptOnly(crate::features::writing::describe::tests::chosen("claude-opus-5-5"));
    let (name, log) = make_named(
        data.path(),
        &library,
        &selection,
        naming_settings(),
        &fetch,
        &words,
        Redo::new(),
        &naming,
        &*transport,
    )
    .unwrap();
    assert!(names_of(data.path(), &name).iter().all(Value::is_null));
    assert_eq!(transport.sent().len(), 0);
    assert!(log.iter().any(|line| line.contains("skipped: no cost was accepted")), "{log:?}");
}

#[test]
fn a_budget_reached_part_way_stops_the_naming_and_the_run_is_made_with_the_names_it_got() {
    use crate::model::language::{Budget, Limit};
    let data = tempfile::tempdir().unwrap();
    let library = Library::open(&data.path().join("library")).unwrap();
    let selection = Selection::parse(&["NeurIPS 2025".into()], None, None, &[], false).unwrap();
    let (fetch, words) = (conference_fetch(90), Words { asked: Mutex::new(0) });
    let transport = crate::features::writing::describe::tests::scripted();
    let chosen = crate::features::writing::describe::tests::chosen("claude-opus-5-5");
    // the run made once without names, so that the worst case of each of its calls is known from its own prompts
    let (name, _) =
        make_run(data.path(), &library, &selection, naming_settings(), &fetch, &words, Redo::new()).unwrap();
    let folder = data.path().join("runs").join(&name);
    let tree = crate::features::writing::read_tree(&folder).unwrap().unwrap();
    let groups = crate::features::writing::describe::Group::from_stored(&tree.communities);
    let ids = files::paper_ids(&folder).unwrap();
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    let papers: Vec<Paper> = library.papers(&ids).unwrap().into_iter().map(Option::unwrap).collect();
    let embeddings = library.vectors(&naming_settings().embedding_model, &papers).unwrap();
    let made = crate::features::writing::describe::Run {
        name: &name,
        folder: None,
        papers: &papers,
        embeddings: Some(embeddings.view()),
        communities: &groups,
        redo: false,
    };
    let estimate = crate::features::writing::describe::estimate_run(&library, &made, &chosen).unwrap();
    let worst = estimate.cost.most_tokens / u64::try_from(estimate.to_ask).unwrap();
    assert!(estimate.to_ask >= 2);
    // room for the worst case of one call and a little more, not for a second once the first has taken what it
    // took (the scripted provider bills 2,300 tokens a call): what is held, then what is spent
    let budget = Budget::new(Limit::Tokens(worst + 1_000));
    let naming = seams::Naming::Within { chosen, budget };
    let (name, log) = make_named(
        data.path(),
        &library,
        &selection,
        naming_settings(),
        &fetch,
        &words,
        Redo::new(),
        &naming,
        &*transport,
    )
    .unwrap();
    let names = names_of(data.path(), &name);
    let named = names.iter().filter(|name| !name.is_null()).count();
    assert!(named >= 1 && named < names.len(), "{names:?}");
    assert!(log.iter().any(|line| line.contains("stopped (")), "{log:?}");
    assert!(log.iter().any(|line| line.contains("modularity")), "the run is written: {log:?}");
}

#[test]
fn a_run_scores_its_abstracts_with_the_trained_detector_once_and_reuses_the_scores_when_made_again() {
    let data = tempfile::tempdir().unwrap();
    let library = Library::open(&data.path().join("library")).unwrap();
    crate::features::detector::tests::build_in(&library);
    let selection = Selection::parse(&["NeurIPS 2025".into()], None, None, &[], false).unwrap();
    let (fetch, words) = (conference_fetch(90), Words { asked: Mutex::new(0) });
    let settings = Settings { detect_ai: true, ..naming_settings() };
    let (_, log) = make_run(data.path(), &library, &selection, settings.clone(), &fetch, &words, Redo::new()).unwrap();
    assert!(log.iter().any(|line| line.contains("done: 90 scored now, 0 kept from before")), "{log:?}");
    let (_, again) = make_run(data.path(), &library, &selection, settings, &fetch, &words, Redo::new()).unwrap();
    assert!(again.iter().any(|line| line.contains("kept from before: every score")), "{again:?}");
    // untrained, or switched off: skipped, each with its own reason
    let fresh = tempfile::tempdir().unwrap();
    let untrained = Library::open(&fresh.path().join("library")).unwrap();
    let settings = Settings { detect_ai: true, ..naming_settings() };
    let (_, log) = make_run(fresh.path(), &untrained, &selection, settings, &fetch, &words, Redo::new()).unwrap();
    assert!(log.iter().any(|line| line.contains("skipped: the AI-text detector is not trained")), "{log:?}");
}

#[test]
fn too_few_papers_are_refused_with_how_many_are_needed() {
    let data = tempfile::tempdir().unwrap();
    let library = Library::open(&data.path().join("library")).unwrap();
    let selection = Selection::parse(&["NeurIPS 2025".into()], None, None, &[], false).unwrap();
    let words = Words { asked: Mutex::new(0) };
    let error = make_run(
        data.path(),
        &library,
        &selection,
        settings_for("spectral"),
        &conference_fetch(10),
        &words,
        Redo::new(),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "only 10 papers: at least 24 are needed to look for communities");
    assert_eq!(*words.asked.lock().unwrap(), 0, "nothing embedded for a run that cannot be made");
}

/// The Python's `test_filtering_down_to_too_few_papers_is_refused_with_a_reason`: enough papers are gathered, but
/// the settings leave too few of them.
#[test]
fn filtering_down_to_too_few_papers_is_refused_with_how_many_are_needed() {
    let data = tempfile::tempdir().unwrap();
    let library = Library::open(&data.path().join("library")).unwrap();
    let selection = Selection::parse(&["NeurIPS 2025".into()], None, None, &[], false).unwrap();
    let words = Words { asked: Mutex::new(0) };
    let settings = Settings { acceptance_filter: "oral".to_owned(), ..settings_for("spectral") };
    let error =
        make_run(data.path(), &library, &selection, settings, &conference_fetch(90), &words, Redo::new()).unwrap_err();
    // one paper in ten of the listing is an oral
    assert_eq!(error.to_string(), "only 9 papers: at least 24 are needed to look for communities");
    assert!(!data.path().join("runs").join("neurips2025").exists(), "no run is written");
}

#[test]
fn a_conference_that_cannot_be_fetched_now_fails_the_run_and_keeps_nothing_as_done() {
    use crate::sources::conferences::{Part, list_url};
    use crate::sources::fake::status;
    let data = tempfile::tempdir().unwrap();
    let library = Library::open(&data.path().join("library")).unwrap();
    let selection = Selection::parse(&["NeurIPS 2025".into()], None, None, &[], false).unwrap();
    let down = Fake::new().with(&list_url("neurips.cc", "neurips", "2025", Part::OralsPosters), status(503));
    let words = Words { asked: Mutex::new(0) };
    let error =
        make_run(data.path(), &library, &selection, settings_for("spectral"), &down, &words, Redo::new()).unwrap_err();
    assert!(error.to_string().contains("NeurIPS 2025"), "{error}");
    assert!(library.needs_fetching("venue|NeurIPS 2025|False|official").unwrap());
}

#[test]
fn a_cancelled_run_stops_between_batches_keeps_what_it_embedded_and_goes_on_from_there() {
    struct Cancelling<'a> {
        words: Words,
        cancel: &'a Cancel,
        after: usize,
    }
    impl Embedding for Cancelling<'_> {
        fn prepare(
            &self,
            model: &str,
            data: &Path,
            tracker: &Tracker<'_>,
            board: &Board,
        ) -> Result<StepOutcome, RunError> {
            self.words.prepare(model, data, tracker, board)
        }
        fn encode(&self, model: &str, texts: &[&str]) -> Result<Vec<Vec<f32>>, RunError> {
            let vectors = self.words.encode(model, texts);
            if *self.words.asked.lock().unwrap() >= self.after {
                self.cancel.cancel();
            }
            vectors
        }
        fn device(&self) -> String {
            "a stand-in".into()
        }
    }
    let data = tempfile::tempdir().unwrap();
    let library = Library::open(&data.path().join("library")).unwrap();
    let selection = Selection::parse(&["NeurIPS 2025".into()], None, None, &[], false).unwrap();
    let fetch = conference_fetch(900);
    let cancel = Cancel::new();
    // cancelled after the first chunk of 400 is embedded (and kept): within the second chunk
    let embedding = Cancelling { words: Words { asked: Mutex::new(0) }, cancel: &cancel, after: 416 };
    let board = Board::default();
    let progress = |_: Progress| {};
    let tracker = Tracker::new(&|_| {}, &progress, &cancel);
    let context = Context {
        data: data.path(),
        library: &library,
        settings: settings_for("spectral"),
        sources: Sources {
            fetch: &fetch,
            pause: &|_| {},
            today: chrono::NaiveDate::from_ymd_opt(2026, 11, 27).unwrap(),
        },
        embedding: &embedding,
        board: &board,
        transport: &crate::model::language::fake::Fake::default(),
        naming: &kept_only(&settings_for("spectral")),
    };
    let request = Request { selection: selection.clone(), redo: Redo::new() };
    let error = make(&context, &request, &tracker).unwrap_err();
    assert!(matches!(error, RunError::Cancelled(Step::Embed)), "{error}");
    let papers = library.select(&selection).unwrap();
    let kept = library.embedded("Qwen/Qwen3-Embedding-0.6B", &papers).unwrap().iter().filter(|done| **done).count();
    assert_eq!(kept, 400, "the first chunk is kept; the one under way is not half-kept");
    // started again: only the rest is embedded
    let words = Words { asked: Mutex::new(0) };
    make_run(data.path(), &library, &selection, settings_for("spectral"), &fetch, &words, Redo::new()).unwrap();
    assert_eq!(*words.asked.lock().unwrap(), 500);
}

/// The step lines of a run's log: "[n/N] title" and the "took" after it.
fn timings(log: &[String]) -> String {
    log.iter().filter(|line| line.starts_with('[') || line.contains("took ")).cloned().collect::<Vec<_>>().join("\n")
}

/// How long a conference-sized run takes, without the embedding model: 12,000 made-up papers (abstracts of
/// made-up words, 40 topics), 1,024 numbers each, clustered at random around 40 centres, already embedded.
/// `cargo test --release -p constellate --lib -- --ignored --nocapture twelve_thousand`.
#[test]
#[ignore = "a measurement, not a check: 12,000 papers"]
fn twelve_thousand_made_up_papers_are_made_into_a_run() {
    let (papers, width, topics) = (12_000, 1_024, 40);
    let data = tempfile::tempdir().unwrap();
    let library = Library::open(&data.path().join("library")).unwrap();
    let mut random = crate::analysis::graph::random::Random::new(3);
    let centres = Array2::from_shape_simple_fn((topics, width), || random.normal() as f32);
    let words: Vec<String> = (0..4_000).map(|word| format!("word{word:04}x")).collect();
    let mut made = Vec::with_capacity(papers);
    let mut vectors = Array2::<f32>::zeros((papers, width));
    for paper in 0..papers {
        let topic = paper % topics;
        let mut text = Vec::new();
        for _ in 0..150 {
            let word = if random.uniform() < 0.5 { topic * 100 + random.below(100) } else { random.below(4_000) };
            text.push(words[word].clone());
        }
        made.push(Paper {
            id: format!("neurips-2025-{}", paper + 1),
            title: text[..8].join(" "),
            r#abstract: text[8..].join(" "),
            authors: Vec::new(),
            published: "2025-12-01T00:00:00Z".into(),
            updated: "2025-12-01T00:00:00Z".into(),
            comment: if paper % 50 == 0 { "Accept (oral)".into() } else { "Accept (poster)".into() },
            primary_category: format!("Topic {topic}"),
            categories: Vec::new(),
            venues: Vec::new(),
            url: None,
            pdf_url: None,
        });
        for column in 0..width {
            vectors[[paper, column]] = centres[[topic, column]] + 1.5 * random.normal() as f32;
        }
    }
    library
        .add_slice(
            "venue|NeurIPS 2025|False|official",
            SliceOutcome::Complete,
            &made,
            Some("NeurIPS 2025"),
            Known::Keep,
        )
        .unwrap();
    library.add_embeddings("Qwen/Qwen3-Embedding-0.6B", &made, vectors.view()).unwrap();
    let selection = Selection::parse(&["NeurIPS 2025".into()], None, None, &[], false).unwrap();
    let started = std::time::Instant::now();
    let (_, log) =
        make_run(data.path(), &library, &selection, Settings::default(), &Fake::new(), &Unasked, Redo::new()).unwrap();
    println!(
        "{}\nin all {:.1}s on {} threads",
        timings(&log),
        started.elapsed().as_secs_f64(),
        rayon::current_num_threads()
    );
}

/// A real run made again from a copy of a data folder the Python left (its library is moved in first, its
/// vectors are the model's): `CONSTELLATE_MEASURE=<copy of a data folder> CONSTELLATE_MEASURE_RUN=<run name>
/// cargo test --release -p constellate --lib -- --ignored --nocapture a_real_run`. Nothing is fetched (a slice
/// that would be fails the run) and nothing embedded.
#[test]
#[ignore = "a measurement on a copy of a real data folder"]
fn a_real_run_is_made_again_from_a_python_data_folder() {
    let data = std::path::PathBuf::from(std::env::var("CONSTELLATE_MEASURE").unwrap());
    let name = std::env::var("CONSTELLATE_MEASURE_RUN").unwrap();
    let started = std::time::Instant::now();
    let library = Library::open(&data.join("library")).unwrap();
    let moved = library.import_old(&data.join("library"));
    println!("moved in: {moved:?} in {:.1}s", started.elapsed().as_secs_f64());
    assert!(moved.problems().is_empty(), "{:?}", moved.problems());
    let again = std::time::Instant::now();
    assert_eq!(library.import_old(&data.join("library")), crate::library::core::import::Moved::default());
    println!("at the next start: nothing to move, in {:.3}s", again.elapsed().as_secs_f64());
    let folder = files::folder_of(&data, &name).unwrap();
    let selection = files::stored_selection(&folder).unwrap();
    let settings =
        crate::settings::load(&data.join("settings.json"), &crate::settings::Choices::default()).unwrap_or_default();
    let settings = Settings { describe: false, detect_ai: false, ..settings };
    let started = std::time::Instant::now();
    let (_, log) = make_run(&data, &library, &selection, settings, &Fake::new(), &Unasked, Redo::new()).unwrap();
    println!(
        "{}\nin all {:.1}s on {} threads",
        timings(&log),
        started.elapsed().as_secs_f64(),
        rayon::current_num_threads()
    );
}
