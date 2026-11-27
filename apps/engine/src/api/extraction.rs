//! The extraction table of a community, a sub-community or a hand-picked set of a run's papers
//! ([`crate::analysis::extraction`]).
//!
//! ```text
//! GET  /api/runs/{run}/extraction/columns    the run's columns ([`Columns`]): kept, or the defaults
//! PUT  /api/runs/{run}/extraction/columns    {columns}: keep these (checked first)
//! POST /api/runs/{run}/extraction/table      {scope}: the table as kept, every quote checked again ([`Table`]);
//!                                            nothing is asked
//! POST /api/runs/{run}/extraction/estimate   {scope, part}: what asking would cost ([`Costing`]); nothing is sent
//! POST /api/runs/{run}/extraction/run        {scope, part, budget_usd}: ask, as one job, within that budget
//! POST /api/runs/{run}/extraction/forget     {papers}: forget these papers' cells, to ask them again (a redo)
//! ```
//!
//! A scope is `{"community": "<tree path>"}` or `{"papers": ["<id>", ...]}` (the marked papers, which must be
//! the run's). Its papers are ordered most central first ([`by_centrality`]); `part` is `sample` (the first
//! [`SAMPLE`]) or `rest` (the others). The sample's estimate is reckoned from its papers' own texts; the rest's
//! from the sample's texts, applied to every paper of the rest that has a PDF (and from its own abstract for one
//! that has none): an estimate either way, and said to be one. The budget of a run is what the user agreed to
//! after seeing the estimate; no call is started that could pass it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::routing::{MethodRouter, get, post};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use super::routes::{ApiError, Shared, on_the_side, one_name};
use crate::analysis::extraction::{
    self, Cell, Column, LeftOut, MOST_PAPERS, Plan, SAMPLE, by_centrality, check_columns, checked_rows,
    default_columns, extrapolate, kept_cells, prompt_text,
};
use crate::evidence::PaperText;
use crate::jobs::Tracker;
use crate::library::store::read_json;
use crate::library::{Feature, Library, PaperReading, Subject};
use crate::model::batch::{Asking, IN_FLIGHT};
use crate::model::language::{Budget, Counting, Https, Limit};
use crate::settings;
use crate::usage::{Tokens, Usd};

/// The most a run of the table may be given to spend, in US dollars: a bound on what is accepted, far above
/// what a table costs.
pub const MOST_BUDGET_USD: f64 = 500.0;
/// How long a paper's text is taken to be when no paper of the sample has been read yet, in characters: a
/// conference paper of about 25 pages.
pub const TYPICAL_CHARACTERS: u64 = 90_000;
/// The longest paper id accepted in a scope.
const LONGEST_ID: usize = 200;

/// The routes of this module, for [`super::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![
        ("/api/runs/{run}/extraction/columns", get(columns).put(save_columns)),
        ("/api/runs/{run}/extraction/table", post(read_table)),
        ("/api/runs/{run}/extraction/estimate", post(estimate)),
        ("/api/runs/{run}/extraction/run", post(start)),
        ("/api/runs/{run}/extraction/forget", post(forget)),
    ]
}

/// Which papers a table is about.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Scope {
    /// A community or sub-community, by its tree path.
    Community(String),
    /// Papers of the run, chosen by hand (the marked ones).
    Papers(Vec<String>),
}

/// Which of a scope's papers are asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    Sample,
    Rest,
}

#[derive(Deserialize)]
struct Asked {
    scope: Scope,
}

#[derive(Deserialize)]
struct Costed {
    scope: Scope,
    part: Part,
}

#[derive(Deserialize)]
struct Started {
    scope: Scope,
    part: Part,
    /// What the user agreed to spend at most, in US dollars.
    budget_usd: f64,
}

#[derive(Deserialize)]
struct Forgetting {
    papers: Vec<String>,
}

#[derive(Deserialize)]
struct NewColumns {
    columns: Vec<Column>,
}

/// The run's graph, as much of it as the table needs.
#[derive(Deserialize)]
struct Graph {
    nodes: Vec<Node>,
    #[serde(default)]
    edges: Vec<(usize, usize, f64)>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    #[serde(default)]
    title: String,
    /// Passed on as the run has it (the contracts' `Acceptance`, `AcceptanceTerm`).
    #[serde(default)]
    acceptance: Option<JsonValue>,
    #[serde(default)]
    acceptance_term: Option<JsonValue>,
}

#[derive(Deserialize)]
struct Communities {
    communities: Vec<Community>,
}

#[derive(Deserialize)]
struct Community {
    tree_path: String,
    members: Vec<usize>,
    #[serde(default)]
    children: Vec<Community>,
}

fn find<'a>(communities: &'a [Community], tree_path: &str) -> Option<&'a Community> {
    communities.iter().find_map(|community| {
        if community.tree_path == tree_path { Some(community) } else { find(&community.children, tree_path) }
    })
}

fn run_folder(engine: &Shared, run: &str) -> Result<PathBuf, ApiError> {
    Ok(engine.data.join("runs").join(one_name(run)?))
}

fn graph_of(engine: &Shared, run: &str) -> Result<Graph, ApiError> {
    read_json(&run_folder(engine, run)?.join("graph.json"))?.ok_or(ApiError::NotFound)
}

/// A scope's papers, most central first, with the run's graph.
fn papers_of(engine: &Shared, run: &str, scope: &Scope) -> Result<(Graph, Vec<String>), ApiError> {
    let graph = graph_of(engine, run)?;
    let wanted: Vec<String> = match scope {
        Scope::Community(tree_path) => {
            let file = run_folder(engine, run)?.join("communities.json");
            let tree: Communities = read_json(&file)?.ok_or(ApiError::NotFound)?;
            let community = find(&tree.communities, tree_path).ok_or(ApiError::NotFound)?;
            community.members.iter().filter_map(|member| graph.nodes.get(*member)).map(|node| node.id.clone()).collect()
        }
        Scope::Papers(papers) => {
            if papers.len() > MOST_PAPERS || papers.iter().any(|paper| paper.len() > LONGEST_ID) {
                return Err(ApiError::Invalid(format!(
                    "a table has at most {MOST_PAPERS} papers, each id at most {LONGEST_ID} characters"
                )));
            }
            let known: HashSet<&str> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
            if let Some(stranger) = papers.iter().find(|paper| !known.contains(paper.as_str())) {
                return Err(ApiError::Invalid(format!("{stranger} is not a paper of this run")));
            }
            papers.clone()
        }
    };
    if wanted.len() > MOST_PAPERS {
        return Err(ApiError::Invalid(format!("a table has at most {MOST_PAPERS} papers: choose a sub-community")));
    }
    let ids: Vec<String> = graph.nodes.iter().map(|node| node.id.clone()).collect();
    let ordered = by_centrality(&wanted, &ids, &graph.edges);
    Ok((graph, ordered))
}

/// The papers of `part` of an ordered scope.
fn part_of(ordered: &[String], part: Part) -> &[String] {
    let split = ordered.len().min(SAMPLE);
    match part {
        Part::Sample => &ordered[..split],
        Part::Rest => &ordered[split..],
    }
}

/// The model the person chose in the settings (the one that writes descriptions and overviews).
fn model_of(engine: &Shared) -> Result<String, ApiError> {
    Ok(settings::load(&engine.settings_file(), &engine.choices)?.describe_model)
}

/// The run's columns: those kept, or the defaults.
fn columns_of(library: &Library, run: &str) -> Result<Vec<Column>, ApiError> {
    Ok(library
        .extraction_columns::<Vec<Column>>(run)?
        .filter(|kept| check_columns(kept).is_ok())
        .unwrap_or_else(default_columns))
}

fn borrowed(ids: &[String]) -> Vec<&str> {
    ids.iter().map(String::as_str).collect()
}

/// A run's columns, and the defaults (to add one back).
#[derive(Serialize)]
pub struct Columns {
    pub columns: Vec<Column>,
    pub defaults: Vec<Column>,
}

async fn columns(State(engine): State<Shared>, Path(run): Path<String>) -> Result<Json<Columns>, ApiError> {
    let columns = on_the_side(move || {
        graph_of(&engine, &run)?;
        Ok(Columns { columns: columns_of(&*engine.library()?, &run)?, defaults: default_columns() })
    })
    .await?;
    Ok(Json(columns))
}

async fn save_columns(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    body: Result<Json<NewColumns>, JsonRejection>,
) -> Result<Json<Columns>, ApiError> {
    let Json(NewColumns { columns }) = body?;
    check_columns(&columns).map_err(|why| ApiError::Invalid(why.to_string()))?;
    let saved = on_the_side(move || {
        graph_of(&engine, &run)?;
        let library = engine.library()?;
        library.keep_extraction_columns(&run, &columns)?;
        Ok(Columns { columns, defaults: default_columns() })
    })
    .await?;
    Ok(Json(saved))
}

/// How a row's paper was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Text {
    /// Its PDF is kept (and was read, if any of its cells was asked).
    Pdf,
    /// No PDF is kept: it is asked from its title and abstract.
    NoPdf,
    /// Its PDF could not be read: it is asked from its title and abstract.
    Unreadable,
    /// Not in the library: never asked.
    NotInLibrary,
}

/// One paper of the table.
#[derive(Serialize)]
pub struct Row {
    pub paper: String,
    pub title: String,
    pub year: Option<u16>,
    pub acceptance: Option<JsonValue>,
    pub acceptance_term: Option<JsonValue>,
    pub text: Text,
    /// One of the sample's papers.
    pub sample: bool,
    /// In the columns' order.
    pub cells: Vec<Cell>,
}

/// The table as kept, every quote checked again.
#[derive(Serialize)]
pub struct Table {
    pub columns: Vec<Column>,
    /// The model whose answers these are.
    pub model: String,
    /// Most central first: the sample's papers come first.
    pub rows: Vec<Row>,
    /// How many papers the sample has.
    pub sample: usize,
    /// Cells not shown as values, by why.
    pub left_out: LeftOut,
}

/// The year a paper is from: its record's date, or a conference id's year ("iclr-2024-12").
fn year_of(published: &str, id: &str) -> Option<u16> {
    let from = |text: &str| text.get(..4).and_then(|year| year.parse::<u16>().ok()).filter(|year| *year > 1900);
    from(published).or_else(|| id.split('-').nth(1).and_then(from))
}

fn table_of(engine: &Shared, run: &str, scope: &Scope) -> Result<Table, ApiError> {
    let (graph, ordered) = papers_of(engine, run, scope)?;
    let library = engine.library()?;
    let model = model_of(engine)?;
    let columns = columns_of(&library, run)?;
    let ids = borrowed(&ordered);
    let records = library.papers(&ids)?;
    let kept = kept_cells(&library, &model, &columns, &ids)?;
    // only the papers with something kept are read: their quotes are checked against their text
    let answered = |cells: &[Option<_>]| cells.iter().any(Option::is_some);
    let asked: Vec<&str> = ids.iter().zip(&kept).filter(|(_, cells)| answered(cells)).map(|(id, _)| *id).collect();
    let mut readings = library.paper_texts(&asked)?.into_iter();
    let mut texts: Vec<Option<PaperText>> = Vec::with_capacity(ids.len());
    let mut kinds: Vec<Text> = Vec::with_capacity(ids.len());
    for ((id, cells), record) in ids.iter().zip(&kept).zip(&records) {
        let reading = if answered(cells) { readings.next() } else { None };
        let kind = match (&reading, record) {
            (_, None) | (Some(PaperReading::NotInLibrary), _) => Text::NotInLibrary,
            (Some(PaperReading::Read(_)), _) => Text::Pdf,
            (Some(PaperReading::NoPdf { .. }), _) => Text::NoPdf,
            (Some(PaperReading::Unreadable { .. }), _) => Text::Unreadable,
            (None, Some(_)) if library.pdf_file(id).is_file() => Text::Pdf,
            (None, Some(_)) => Text::NoPdf,
        };
        kinds.push(kind);
        texts.push(reading.and_then(PaperReading::into_text));
    }
    let cells = checked_rows(&ids, &columns, kept, texts);
    let left_out = LeftOut::of(&cells);
    let by_id: HashMap<&str, &Node> = graph.nodes.iter().map(|node| (node.id.as_str(), node)).collect();
    let rows = ordered
        .iter()
        .zip(cells)
        .zip(kinds)
        .zip(records)
        .enumerate()
        .map(|(index, (((paper, cells), text), record))| {
            let node = by_id.get(paper.as_str());
            let title = record.as_ref().map(|record| record.title.clone());
            Row {
                paper: paper.clone(),
                title: title.or_else(|| node.map(|node| node.title.clone())).unwrap_or_default(),
                year: record.as_ref().and_then(|record| year_of(&record.published, paper)),
                acceptance: node.and_then(|node| node.acceptance.clone()),
                acceptance_term: node.and_then(|node| node.acceptance_term.clone()),
                text,
                sample: index < SAMPLE,
                cells,
            }
        })
        .collect();
    Ok(Table { columns, model, rows, sample: ordered.len().min(SAMPLE), left_out })
}

async fn read_table(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    body: Result<Json<Asked>, JsonRejection>,
) -> Result<Json<Table>, ApiError> {
    let Json(Asked { scope }) = body?;
    Ok(Json(on_the_side(move || table_of(&engine, &run, &scope)).await?))
}

/// What an estimate was reckoned from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Basis {
    /// The papers' own texts.
    Texts,
    /// The mean length of the sample's papers that were read, for every paper of the rest with a PDF (and its
    /// own abstract for each without).
    SampleTexts { papers: usize, characters: u64 },
    /// No paper of the sample has been read: a typical paper's length ([`TYPICAL_CHARACTERS`]).
    Typical { characters: u64 },
}

/// What asking part of a table would cost, before anything is sent. An estimate, always.
#[derive(Serialize)]
pub struct Costing {
    pub part: Part,
    /// Papers in the part.
    pub papers: usize,
    /// Of those, the papers with a cell to ask; the others are kept (or not in the library).
    pub to_ask: usize,
    pub model: String,
    /// Likely, and at most; `None` when the model's price is not known.
    pub usd: Option<Usd>,
    pub most_usd: Option<Usd>,
    pub tokens: Tokens,
    pub counting: Counting,
    pub basis: Basis,
}

fn length(text: &str) -> u64 {
    u64::try_from(text.chars().count()).unwrap_or(u64::MAX)
}

fn costing(engine: &Shared, run: &str, scope: &Scope, part: Part) -> Result<Costing, ApiError> {
    let (_, ordered) = papers_of(engine, run, scope)?;
    let library = engine.library()?;
    let model = model_of(engine)?;
    let columns = columns_of(&library, run)?;
    let sample = borrowed(part_of(&ordered, Part::Sample));
    let readings = library.paper_texts(&sample)?;
    let papers = part_of(&ordered, part).len();
    if part == Part::Sample {
        let pairs: Vec<(&str, &PaperReading)> = sample.iter().copied().zip(&readings).collect();
        let estimate = Plan::new(&library, &model, &columns, &pairs)?.estimate(&library)?;
        let tokens = estimate.by_model.iter().fold(Tokens::default(), |sum, one| sum + one.tokens);
        let counting = estimate.by_model.first().map_or(Counting::ASSUMED, |one| one.counting);
        return Ok(Costing {
            part,
            papers,
            to_ask: estimate.to_ask,
            model,
            usd: estimate.usd,
            most_usd: estimate.most_usd,
            tokens,
            counting,
            basis: Basis::Texts,
        });
    }
    // the rest: from the sample's texts, read (and kept) by now, so that the rest's PDFs need not be read here
    let lengths: Vec<u64> = readings
        .iter()
        .filter(|reading| matches!(reading, PaperReading::Read(_)))
        .filter_map(prompt_text)
        .map(|text| length(&text))
        .collect();
    let read = lengths.len();
    let (typical, basis) = match u64::try_from(read) {
        Ok(read_papers) if read_papers > 0 => {
            let mean = lengths.iter().sum::<u64>() / read_papers;
            (mean, Basis::SampleTexts { papers: read, characters: mean })
        }
        _ => (TYPICAL_CHARACTERS, Basis::Typical { characters: TYPICAL_CHARACTERS }),
    };
    let rest = borrowed(part_of(&ordered, Part::Rest));
    let kept = kept_cells(&library, &model, &columns, &rest)?;
    let records = library.papers(&rest)?;
    let mut characters = Vec::new();
    for ((id, cells), record) in rest.iter().zip(&kept).zip(records) {
        let Some(record) = record else { continue };
        if cells.iter().all(Option::is_some) {
            continue;
        }
        characters.push(if library.pdf_file(id).is_file() {
            typical
        } else {
            let reading = PaperReading::NoPdf { title: record.title, abstract_text: record.r#abstract };
            prompt_text(&reading).map_or(0, |text| length(&text))
        });
    }
    let estimate = extrapolate(&model, &columns, &characters);
    Ok(Costing {
        part,
        papers,
        to_ask: characters.len(),
        model,
        usd: estimate.usd,
        most_usd: estimate.most_usd,
        tokens: estimate.tokens,
        counting: estimate.counting,
        basis,
    })
}

async fn estimate(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    body: Result<Json<Costed>, JsonRejection>,
) -> Result<Json<Costing>, ApiError> {
    let Json(Costed { scope, part }) = body?;
    Ok(Json(on_the_side(move || costing(&engine, &run, &scope, part)).await?))
}

/// Dollars as a budget holds them.
fn dollars(usd: f64) -> Usd {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let picodollars = (usd * 1e12).round() as u64;
    Usd::from_picodollars(picodollars)
}

/// The job's number, for following it in `/api/status` and stopping it.
#[derive(Serialize)]
pub struct Begun {
    pub ok: bool,
    pub id: u64,
}

async fn start(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    body: Result<Json<Started>, JsonRejection>,
) -> Result<Json<Begun>, ApiError> {
    let Json(Started { scope, part, budget_usd }) = body?;
    if !(budget_usd.is_finite() && budget_usd > 0.0 && budget_usd <= MOST_BUDGET_USD) {
        return Err(ApiError::Invalid(format!("the budget is more than 0 and at most {MOST_BUDGET_USD} US dollars")));
    }
    let id = on_the_side(move || {
        let endpoint = engine.endpoint().map_err(|why| ApiError::Invalid(why.to_string()))?;
        let (_, ordered) = papers_of(&engine, &run, &scope)?;
        let papers = part_of(&ordered, part).to_vec();
        let model = model_of(&engine)?;
        let library = engine.library()?;
        let columns = columns_of(&library, &run)?;
        let budget = Budget::new(Limit::Dollars(dollars(budget_usd)));
        let work = move |tracker: &Tracker<'_>| {
            let ids = borrowed(&papers);
            let readings = library.paper_texts(&ids).map_err(|error| error.to_string())?;
            let pairs: Vec<(&str, &PaperReading)> = ids.iter().copied().zip(&readings).collect();
            let plan = Plan::new(&library, &model, &columns, &pairs).map_err(|error| error.to_string())?;
            tracker.log(&format!("Asking the model about {} of {} papers", plan.papers(), papers.len()));
            let transport = Https::default();
            let asking = Asking {
                library: &library,
                transport: &transport,
                endpoint,
                recorder: &*library,
                in_flight: IN_FLIGHT,
            };
            let ran = extraction::run(asking, &plan, &budget, tracker).map_err(|error| error.to_string())?;
            tracker.log(&format!(
                "{} answered, {} kept from before, {} with no usable answer, {} not asked",
                ran.answered, ran.kept, ran.failed, ran.not_started
            ));
            // stopped early (the budget reached, a stop asked for, the API unusable): told as the job's failure,
            // so that it is never shown as a finished table; what was answered is kept either way
            if let Some(why) = ran.stopped {
                return Err(format!("{why}: {} of {} papers were not asked", ran.not_started, ran.papers));
            }
            // a job's result names the run it was about, as a run's job does
            Ok(run)
        };
        Ok(engine.job.start_tracked("the extraction table", work)?)
    })
    .await?;
    Ok(Json(Begun { ok: true, id }))
}

/// How many cells were forgotten.
#[derive(Serialize)]
pub struct Forgotten {
    pub forgotten: usize,
}

async fn forget(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    body: Result<Json<Forgetting>, JsonRejection>,
) -> Result<Json<Forgotten>, ApiError> {
    let Json(Forgetting { papers }) = body?;
    let forgotten = on_the_side(move || {
        // the run's own papers only, as for a scope
        papers_of(&engine, &run, &Scope::Papers(papers.clone()))?;
        let subjects: Vec<Subject> = papers.into_iter().map(Subject::Paper).collect();
        Ok(engine.library()?.forget(Feature::Extraction, &subjects)?)
    })
    .await?;
    Ok(Json(Forgotten { forgotten }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::super::config::tests::STRONG;
    use super::super::config::{Environment, Listen};
    use super::super::routes::Engine;
    use super::super::tests::served;
    use crate::analysis::extraction::{SaidCell, cell_key, default_columns};
    use crate::library::papers::tests::paper;
    use crate::library::passages::fixtures::pdf_with;
    use crate::library::{Feature, Known, Library, Slot, Subject};
    use crate::settings::Choices;

    const AT: &str = "127.0.0.1:8765";
    const MODEL: &str = "claude-opus-5-5";
    const P01: [&str; 3] = [
        "Introduction",
        "We study image classification with sparse mixtures of experts in every layer.",
        "All models were trained on 8 A100 GPUs for three days in total.",
    ];

    fn choices() -> Choices {
        let one = |name: &str| vec![name.to_owned()];
        Choices {
            embedding_models: one("Qwen/Qwen3-Embedding-0.6B"),
            clustering: one("spectral"),
            acceptance: one("all"),
            flags: one("llm_declared"),
            describe_models: one(MODEL),
        }
    }

    fn ids() -> Vec<String> {
        (1..=12).map(|n| format!("p{n:02}")).collect()
    }

    /// A data folder with twelve papers in the library (two with PDFs) and a run "r1" of them and one more the
    /// library does not have: community c0 has all thirteen, c0.0 the first three, c1 the last.
    fn data() -> tempfile::TempDir {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(&data.path().join("library")).unwrap();
        let papers: Vec<_> = ids().iter().map(|id| paper(id)).collect();
        library.add_papers(&papers, None, Known::Keep).unwrap();
        library.keep_pdf("p01", &pdf_with(&P01)).unwrap();
        library
            .keep_pdf("p02", &pdf_with(&["Introduction", "We address translation with a model of modest size."]))
            .unwrap();
        let run = data.path().join("runs").join("r1");
        std::fs::create_dir_all(&run).unwrap();
        let mut nodes: Vec<Value> =
            ids().iter().map(|id| json!({"id": id, "title": id, "acceptance": "poster"})).collect();
        nodes[0]["acceptance"] = json!("oral");
        nodes.push(json!({"id": "x99", "title": "Not kept", "acceptance": null}));
        // p03 is the most central of c0.0, then p01
        let graph = json!({"nodes": nodes, "edges": [[2, 0, 0.9], [2, 1, 0.8], [0, 1, 0.3]]});
        std::fs::write(run.join("graph.json"), graph.to_string()).unwrap();
        let tree = json!({"communities": [
            {"tree_path": "c0", "members": (0..13).collect::<Vec<_>>(), "children": [
                {"tree_path": "c0.0", "members": [0, 1, 2], "children": []}]},
            {"tree_path": "c1", "members": [12], "children": []}]});
        std::fs::write(run.join("communities.json"), tree.to_string()).unwrap();
        data
    }

    fn served_at(data: &tempfile::TempDir, key: bool) -> (axum::Router, crate::jobs::Job<String>) {
        let listen = Listen::from_environment(&Environment::from_pairs([
            ("CONSTELLATE_TOKEN", STRONG),
            ("CONSTELLATE_ORIGIN", "http://127.0.0.1:3000"),
        ]))
        .unwrap();
        let mut engine = Engine::new(data.path().to_path_buf(), choices());
        if key {
            engine.language =
                Environment::from_pairs([("ANTHROPIC_API_KEY", "test-key-not-real-0123")]).language_endpoint();
        }
        let job = engine.job.clone();
        let (router, _) = served(&listen, AT, engine);
        // the job is shared: what a test starts on it is what the routes see
        (router, job)
    }

    async fn ask(router: &axum::Router, method: &str, path: &str, body: Option<String>) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, AT)
            .header(header::AUTHORIZATION, format!("Bearer {STRONG}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or(Body::empty(), Body::from))
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn post(router: &axum::Router, path: &str, body: Value) -> (StatusCode, Value) {
        ask(router, "POST", path, Some(body.to_string())).await
    }

    /// Keep the model's answer for one default column of one paper, as a run would have.
    fn keep(data: &tempfile::TempDir, paper: &str, column: &str, said: SaidCell) {
        let library = Library::open(&data.path().join("library")).unwrap();
        let column = default_columns().into_iter().find(|one| one.id == column).unwrap();
        let slot = Slot {
            feature: Feature::Extraction,
            key: cell_key(MODEL, paper, &column),
            subject: Subject::Paper(paper.to_owned()),
        };
        library.keep(&slot, MODEL, super::extraction::PROMPT, &said).unwrap();
    }

    fn stated(column: &str, text: &str, quote: &str) -> SaidCell {
        SaidCell {
            column: column.to_owned(),
            status: "stated".to_owned(),
            text: text.to_owned(),
            quotes: vec![quote.to_owned()],
            ..SaidCell::default()
        }
    }

    #[tokio::test]
    async fn columns_are_the_defaults_until_saved_and_then_survive_a_restart() {
        let data = data();
        let (router, _) = served_at(&data, false);
        let (status, told) = ask(&router, "GET", "/api/runs/r1/extraction/columns", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(told["columns"].as_array().unwrap().len(), 8);
        assert_eq!(told["columns"], told["defaults"]);
        let mine = json!({"columns": [
            {"id": "ood", "label": "OOD", "question": "Does the paper evaluate on out-of-distribution data?", "kind": "yes_no"},
            told["columns"][1].clone()]});
        let (status, saved) = ask(&router, "PUT", "/api/runs/r1/extraction/columns", Some(mine.to_string())).await;
        assert_eq!((status, saved["columns"].clone()), (StatusCode::OK, mine["columns"].clone()));
        let (again, _) = served_at(&data, false);
        assert_eq!(ask(&again, "GET", "/api/runs/r1/extraction/columns", None).await.1["columns"], mine["columns"]);
        let empty = json!({"columns": []}).to_string();
        assert_eq!(
            ask(&router, "PUT", "/api/runs/r1/extraction/columns", Some(empty)).await.0,
            StatusCode::BAD_REQUEST
        );
        let wrong_kind = json!({"columns": [{"id": "a", "label": "A", "question": "Q?", "kind": "essay"}]}).to_string();
        assert_eq!(
            ask(&router, "PUT", "/api/runs/r1/extraction/columns", Some(wrong_kind)).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(ask(&router, "GET", "/api/runs/none/extraction/columns", None).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_table_starts_unasked_and_shows_kept_cells_only_with_their_evidence() {
        let data = data();
        let (router, _) = served_at(&data, false);
        let scope = json!({"scope": {"community": "c0.0"}});
        let (status, table) = post(&router, "/api/runs/r1/extraction/table", scope.clone()).await;
        assert_eq!(status, StatusCode::OK);
        let rows = table["rows"].as_array().unwrap();
        let order: Vec<&str> = rows.iter().map(|row| row["paper"].as_str().unwrap()).collect();
        assert_eq!(order, ["p03", "p01", "p02"], "most central first");
        assert_eq!(table["sample"], 3);
        assert_eq!((rows[0]["text"].clone(), rows[1]["text"].clone()), (json!("no_pdf"), json!("pdf")));
        assert_eq!((rows[1]["acceptance"].clone(), rows[1]["year"].clone()), (json!("oral"), json!(2026)));
        assert!(rows.iter().all(|row| row["cells"].as_array().unwrap().iter().all(|cell| cell["kind"] == "not_asked")));

        keep(
            &data,
            "p01",
            "task",
            stated("task", "Image classification", "We study image classification with sparse mixtures"),
        );
        keep(
            &data,
            "p01",
            "compute",
            stated("compute", "512 TPUs", "trained on 512 TPU v4 chips for two weeks in all"),
        );
        let (_, table) = post(&router, "/api/runs/r1/extraction/table", scope).await;
        let cells = &table["rows"][1]["cells"];
        assert_eq!(cells[0]["kind"], "stated");
        assert_eq!(cells[0]["value"], json!({"kind": "text", "text": "Image classification"}));
        assert_eq!(cells[0]["evidence"][0]["paper"], "p01");
        assert_eq!(cells[0]["evidence"][0]["established"]["address"]["page"], 1);
        assert_eq!(cells[5], json!({"kind": "not_verified", "why": "quotes_not_found"}), "the fabricated quote");
        assert_eq!(table["left_out"], json!({"quotes_not_found": 1, "value_not_in_quote": 0, "malformed": 0}));
        assert!(!table.to_string().contains("512 TPUs"), "a value without found evidence is never sent");
        let marked = json!({"scope": {"papers": ["p02", "p01"]}});
        let (_, picked) = post(&router, "/api/runs/r1/extraction/table", marked).await;
        assert_eq!(picked["rows"][0]["paper"], "p01");
    }

    #[tokio::test]
    async fn estimates_send_nothing_and_the_rest_is_reckoned_from_the_sample() {
        let data = data();
        let (router, _) = served_at(&data, false);
        let sample = json!({"scope": {"community": "c0"}, "part": "sample"});
        let (status, cost) = post(&router, "/api/runs/r1/extraction/estimate", sample).await;
        assert_eq!(status, StatusCode::OK, "{cost}");
        assert_eq!((cost["papers"].clone(), cost["to_ask"].clone()), (json!(10), json!(10)));
        assert_eq!((cost["model"].clone(), cost["basis"].clone()), (json!(MODEL), json!({"kind": "texts"})));
        assert!(cost["usd"].as_f64().unwrap() > 0.0 && cost["usd"].as_f64() <= cost["most_usd"].as_f64());
        assert_eq!(cost["counting"]["kind"], "assumed");
        let rest = json!({"scope": {"community": "c0"}, "part": "rest"});
        let (_, cost) = post(&router, "/api/runs/r1/extraction/estimate", rest).await;
        // p11, p12 and x99: the last is not in the library, so never asked
        assert_eq!((cost["papers"].clone(), cost["to_ask"].clone()), (json!(3), json!(2)));
        assert_eq!(cost["basis"]["kind"], "sample_texts");
        assert_eq!(cost["basis"]["papers"], 2, "p01 and p02 were read");
        let library = Library::open(&data.path().join("library")).unwrap();
        assert_eq!(library.spending().unwrap().total.calls, 0, "nothing was sent");
    }

    #[tokio::test]
    async fn a_run_needs_a_key_a_budget_and_no_other_job_running() {
        let data = data();
        let started = |budget: f64| json!({"scope": {"community": "c0.0"}, "part": "sample", "budget_usd": budget});
        let (no_key, _) = served_at(&data, false);
        let (status, told) = post(&no_key, "/api/runs/r1/extraction/run", started(1.0)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(told["error"].as_str().unwrap().contains("No API key"));

        let (router, job) = served_at(&data, true);
        for budget in [0.0, -1.0, 1e9] {
            assert_eq!(post(&router, "/api/runs/r1/extraction/run", started(budget)).await.0, StatusCode::BAD_REQUEST);
        }
        // another job is running: refused, and nothing is asked
        let id = job
            .start_tracked("something else", |tracker| {
                while !tracker.cancelled() {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Ok(String::new())
            })
            .unwrap();
        let (status, told) = post(&router, "/api/runs/r1/extraction/run", started(1.0)).await;
        assert_eq!(
            (status, told["error"].clone()),
            (StatusCode::CONFLICT, json!("something else is already in progress"))
        );
        let elsewhere = json!({"scope": {"community": "c9"}, "part": "sample", "budget_usd": 1.0});
        assert_eq!(post(&router, "/api/runs/r1/extraction/run", elsewhere).await.0, StatusCode::NOT_FOUND);
        job.cancel(id).unwrap();
    }

    #[tokio::test]
    async fn too_many_papers_a_stranger_or_a_body_too_large_is_refused_and_a_redo_forgets() {
        let data = data();
        let (router, _) = served_at(&data, false);
        let many: Vec<String> = (0..=super::MOST_PAPERS).map(|n| format!("p{n}")).collect();
        let (status, _) = post(&router, "/api/runs/r1/extraction/table", json!({"scope": {"papers": many}})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let stranger = json!({"scope": {"papers": ["p01", "elsewhere"]}});
        assert_eq!(post(&router, "/api/runs/r1/extraction/table", stranger).await.0, StatusCode::BAD_REQUEST);
        let huge = json!({"scope": {"papers": ["x".repeat(5 * 1024 * 1024)]}});
        assert_eq!(post(&router, "/api/runs/r1/extraction/table", huge).await.0, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            post(&router, "/api/runs/none/extraction/table", json!({"scope": {"community": "c0"}})).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            post(&router, "/api/runs/r1/extraction/table", json!({"scope": "c0"})).await.0,
            StatusCode::BAD_REQUEST
        );

        keep(
            &data,
            "p01",
            "task",
            stated("task", "Image classification", "We study image classification with sparse mixtures"),
        );
        let (status, told) = post(&router, "/api/runs/r1/extraction/forget", json!({"papers": ["p01"]})).await;
        assert_eq!((status, told), (StatusCode::OK, json!({"forgotten": 1})));
        let (_, table) = post(&router, "/api/runs/r1/extraction/table", json!({"scope": {"papers": ["p01"]}})).await;
        assert_eq!(table["rows"][0]["cells"][0]["kind"], "not_asked");
        assert_eq!(
            post(&router, "/api/runs/r1/extraction/forget", json!({"papers": ["elsewhere"]})).await.0,
            StatusCode::BAD_REQUEST
        );
    }
}
