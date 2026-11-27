//! The extraction table of a community, a sub-community or a hand-picked set of a run's papers
//! ([`crate::analysis::extraction`]).
//!
//! ```text
//! GET  /api/runs/{run}/extraction/columns    the run's columns ([`Columns`]): kept, or the defaults
//! PUT  /api/runs/{run}/extraction/columns    {columns}: keep these (checked first)
//! POST /api/runs/{run}/extraction/table      {scope}: the table as kept, every quote checked again ([`Table`]);
//!                                            nothing is asked
//! POST /api/runs/{run}/extraction/estimate   {scope, part}: what asking would cost ([`Costing`]); nothing is sent
//! POST /api/runs/{run}/extraction/run        {scope, part, provider, model, budget_usd | budget_tokens}: ask, as
//!                                            one job, within that budget (in tokens for a model with no price)
//! POST /api/runs/{run}/extraction/forget     {papers}: forget these papers' cells, to ask them again (a redo)
//! ```
//!
//! A scope is `{"community": "<tree path>"}` or `{"papers": ["<id>", ...]}` (the marked papers, which must be
//! the run's). Its papers are ordered most central first ([`central_first`]); `part` is `sample` (the first
//! [`SAMPLE`]) or `rest` (the others). The sample's estimate is reckoned from its papers' own texts; the rest's
//! from the sample's texts, applied to every paper of the rest that has a PDF (and from its own abstract for one
//! that has none): an estimate either way, and said to be one. The budget of a run is what the user agreed to
//! after seeing the estimate, for the model named with it; no call is started that could pass it, and a run whose
//! estimate has grown past it since (papers added, a PDF kept, columns changed) is refused, to be looked at again.

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
    self, Cell, Column, LeftOut, MOST_PAPERS, Plan, Read, SAMPLE, SaidCell, check_columns, checked_rows,
    default_columns, extrapolate, kept_cells, prompt_text,
};
use crate::analysis::links::central_first;
use crate::evidence::PaperText;
use crate::jobs::{Finished, Tracker};
use crate::library::store::read_json;
use crate::library::{Feature, Library, PaperReading, Subject};
use crate::model::batch::{Asking, IN_FLIGHT};
use crate::model::chosen::{Accepted, Refused, Terms, budget};
use crate::model::language::{Estimate, Provider};

/// The most a run of the table may be given to spend, in US dollars: a bound on what is accepted, far above
/// what a table costs.
pub const MOST_BUDGET_USD: f64 = 500.0;
/// The same for a model with no price, in tokens: a table of a few hundred whole papers.
pub const MOST_BUDGET_TOKENS: u64 = 500_000_000;
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

impl Scope {
    /// What a finished table job is about, for the interface to tell its own: the community, or the run (for
    /// papers marked by hand).
    fn subject(&self, run: &str) -> Subject {
        match self {
            Scope::Community(tree_path) => Subject::Community { run: run.to_owned(), community: tree_path.clone() },
            Scope::Papers(_) => Subject::Run(run.to_owned()),
        }
    }
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
    /// What the user agreed to spend at most: in US dollars, or in tokens for a model with no price.
    #[serde(alias = "accepted_usd")]
    budget_usd: Option<f64>,
    #[serde(alias = "accepted_tokens")]
    budget_tokens: Option<u64>,
    /// The model whose estimate the user saw, and its provider.
    model: Option<String>,
    provider: Option<Provider>,
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
    let ordered = central_first(&wanted, &ids, &graph.edges);
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
    /// The model whose answers these are, and its provider.
    pub provider: Provider,
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
    let chosen = engine.chosen()?;
    let columns = columns_of(&library, run)?;
    let ids = borrowed(&ordered);
    let records = library.papers(&ids)?;
    // what is kept from either reading: which one is shown is known only once the paper is read
    let of = |read: Read| ids.iter().map(|id| (*id, read)).collect::<Vec<_>>();
    let from_pdf = kept_cells(&library, &chosen, &columns, &of(Read::Pdf))?;
    let from_abstract = kept_cells(&library, &chosen, &columns, &of(Read::Abstract))?;
    // only the papers with something kept are read: their quotes are checked against their text
    let answered: Vec<bool> = from_pdf
        .iter()
        .zip(&from_abstract)
        .map(|(pdf, abstract_only)| pdf.iter().chain(abstract_only).any(Option::is_some))
        .collect();
    let asked: Vec<&str> = ids.iter().zip(&answered).filter(|(_, answered)| **answered).map(|(id, _)| *id).collect();
    let mut readings = library.paper_texts(&asked)?.into_iter();
    let mut texts: Vec<Option<PaperText>> = Vec::with_capacity(ids.len());
    let mut kinds: Vec<Text> = Vec::with_capacity(ids.len());
    let mut kept: Vec<Vec<Option<SaidCell>>> = Vec::with_capacity(ids.len());
    let pairs = from_pdf.into_iter().zip(from_abstract).zip(answered);
    for ((id, record), ((pdf, abstract_only), answered)) in ids.iter().zip(&records).zip(pairs) {
        let reading = if answered { readings.next() } else { None };
        // the cells of the paper as it reads now: an abstract's answers are not the PDF's
        kept.push(match reading.as_ref().and_then(Read::of) {
            Some(Read::Pdf) => pdf,
            Some(Read::Abstract) => abstract_only,
            None => vec![None; columns.len()],
        });
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
    Ok(Table {
        columns,
        provider: chosen.provider(),
        model: chosen.model().to_owned(),
        rows,
        sample: ordered.len().min(SAMPLE),
        left_out,
    })
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
    /// What asking them would cost, at the chosen provider's prices (no dollars for a model with no price).
    #[serde(flatten)]
    pub cost: Estimate,
    /// Whether the model can be asked at all (its provider's key is set); never the key.
    pub api_key_set: bool,
    pub basis: Basis,
}

fn length(text: &str) -> u64 {
    u64::try_from(text.chars().count()).unwrap_or(u64::MAX)
}

fn costing(engine: &Shared, run: &str, scope: &Scope, part: Part) -> Result<Costing, ApiError> {
    let (_, ordered) = papers_of(engine, run, scope)?;
    let library = engine.library()?;
    let chosen = engine.chosen()?;
    let columns = columns_of(&library, run)?;
    let sample = borrowed(part_of(&ordered, Part::Sample));
    let readings = library.paper_texts(&sample)?;
    let papers = part_of(&ordered, part).len();
    if part == Part::Sample {
        let pairs: Vec<(&str, &PaperReading)> = sample.iter().copied().zip(&readings).collect();
        let estimate = Plan::new(&library, &chosen, &columns, &pairs)?.estimate(&library)?;
        return Ok(Costing {
            part,
            papers,
            to_ask: estimate.to_ask,
            cost: estimate.cost,
            api_key_set: chosen.can_ask(),
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
    // a paper with a PDF is taken to be read from it (one that cannot be read is then reckoned at more than its
    // abstract costs: never less)
    let with_pdf: Vec<bool> = rest.iter().map(|id| library.pdf_file(id).is_file()).collect();
    let read: Vec<(&str, Read)> =
        rest.iter().zip(&with_pdf).map(|(id, pdf)| (*id, if *pdf { Read::Pdf } else { Read::Abstract })).collect();
    let kept = kept_cells(&library, &chosen, &columns, &read)?;
    let records = library.papers(&rest)?;
    let mut characters = Vec::new();
    for ((pdf, cells), record) in with_pdf.iter().zip(&kept).zip(records) {
        let Some(record) = record else { continue };
        if cells.iter().all(Option::is_some) {
            continue;
        }
        characters.push(if *pdf {
            typical
        } else {
            let reading = PaperReading::NoPdf { title: record.title, abstract_text: record.r#abstract };
            prompt_text(&reading).map_or(0, |text| length(&text))
        });
    }
    Ok(Costing {
        part,
        papers,
        to_ask: characters.len(),
        cost: extrapolate(&chosen, &columns, &characters),
        api_key_set: chosen.can_ask(),
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

/// A budget refused, told to whoever sent it.
fn refused(why: Refused) -> ApiError {
    ApiError::Invalid(why.to_string())
}

/// A hundredth of a cent: the rounding of a figure as it travelled to the interface and back.
const SLACK_USD: f64 = 1e-4;

/// Is what asking would cost now (`cost`, its worst case) within what the user accepted after seeing an
/// estimate? Not when it has grown since: papers added to the community, a PDF kept since (read in full now, not
/// from its abstract), columns added or changed. The budget alone would stop such a run part way, on a table the
/// user agreed to pay for in full; they see the new estimate first instead.
fn within_accepted(cost: &Estimate, accepted: Accepted) -> Result<(), Refused> {
    let grown = match (cost.most_usd, accepted) {
        _ if cost.calls == 0 => false,
        (Some(most), Accepted::Dollars(usd)) => most.dollars() > usd + SLACK_USD,
        (None, Accepted::Tokens(tokens)) => cost.most_tokens > tokens,
        // in the other unit: refused by the budget itself
        _ => false,
    };
    if grown { Err(Refused::Changed) } else { Ok(()) }
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
    let Json(started) = body?;
    // a budget is needed: the user sets it, up to the bound, after seeing the estimate
    let accepted = match Accepted::of(started.budget_usd, started.budget_tokens).map_err(refused)? {
        Accepted::Shown => return Err(ApiError::Invalid("give the budget the run may spend".to_owned())),
        accepted => accepted,
    };
    let Started { scope, part, model, provider, .. } = started;
    let id = on_the_side(move || {
        let chosen = engine.chosen()?;
        let endpoint = chosen.endpoint().map_err(|why| ApiError::Invalid(why.to_string()))?;
        // the estimate accepted was for one model: without its name, nothing says it was this one's
        let model = model.ok_or_else(|| ApiError::Invalid("say which model's estimate was shown".to_owned()))?;
        chosen.as_shown(provider, &model).map_err(|why| ApiError::Invalid(why.to_string()))?;
        // in dollars for a model with a price, in tokens for one without, as the estimate was shown
        let shown = costing(&engine, &run, &scope, part)?;
        let terms = Terms::AtMost { usd: MOST_BUDGET_USD, tokens: MOST_BUDGET_TOKENS };
        let budget = budget(&shown.cost, accepted, terms).map_err(refused)?;
        within_accepted(&shown.cost, accepted).map_err(refused)?;
        let (_, ordered) = papers_of(&engine, &run, &scope)?;
        let papers = part_of(&ordered, part).to_vec();
        let library = engine.library()?;
        let columns = columns_of(&library, &run)?;
        let transport = engine.transport.clone();
        let subject = scope.subject(&run);
        let about = crate::jobs::About { feature: Feature::Extraction, subject: subject.clone(), item: None };
        let work = move |tracker: &Tracker<'_>| {
            let ids = borrowed(&papers);
            let readings = library.paper_texts(&ids).map_err(|error| error.to_string())?;
            let pairs: Vec<(&str, &PaperReading)> = ids.iter().copied().zip(&readings).collect();
            let plan = Plan::new(&library, &chosen, &columns, &pairs).map_err(|error| error.to_string())?;
            tracker.log(&format!("Asking {chosen} about {} of {} papers", plan.papers(), papers.len()));
            let asking = Asking {
                library: &library,
                transport: &*transport,
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
            Ok(Finished::Feature { feature: Feature::Extraction, subject })
        };
        Ok(engine.job.start_about("the extraction table", about, work)?)
    })
    .await?;
    Ok(Json(Begun { ok: true, id }))
}

/// How many kept answers were forgotten: the papers' cells and the whole answers they came from, of every model
/// and provider and in every run's table (a paper's cells are its own, not a run's), as the interface warns.
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
    use crate::analysis::extraction::{Read, SaidCell, cell_key, default_columns};
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

    fn served_at(data: &tempfile::TempDir, key: bool) -> (axum::Router, crate::jobs::Job<crate::jobs::Finished>) {
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

    /// Keep the model's answer for one default column of one paper with a PDF, as a run would have.
    fn keep(data: &tempfile::TempDir, paper: &str, column: &str, said: SaidCell) {
        let library = Library::open(&data.path().join("library")).unwrap();
        let column = default_columns().into_iter().find(|one| one.id == column).unwrap();
        let slot = Slot {
            feature: Feature::Extraction,
            key: cell_key(&crate::model::chosen::Chosen::claude(MODEL), paper, Read::Pdf, &column),
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

    #[test]
    fn a_start_takes_the_accepted_amount_under_either_name() {
        let named = |body: Value| serde_json::from_value::<super::Started>(body).unwrap();
        let old = named(json!({"scope": {"community": "c0"}, "part": "sample", "budget_usd": 1.5}));
        let new = named(json!({"scope": {"community": "c0"}, "part": "sample", "accepted_usd": 1.5}));
        assert_eq!((old.budget_usd, new.budget_usd), (Some(1.5), Some(1.5)));
        let tokens = named(json!({"scope": {"community": "c0"}, "part": "sample", "accepted_tokens": 9}));
        assert_eq!(tokens.budget_tokens, Some(9));
    }

    #[tokio::test]
    async fn another_provider_is_priced_asked_in_its_own_shape_kept_and_asked_again_when_the_provider_changes() {
        use super::super::other_providers::{
            BUDGET, Chosen as Asked, MODEL, Scripted, accepting, call, choose, finished, priced_for, router, sent_to,
        };

        let data = data();
        let cell = json!({"column": "task", "status": "stated", "text": "Image classification", "yes_no": "",
            "number": "", "unit": "", "context": "", "items": [],
            "quotes": ["We study image classification with sparse mixtures of experts in every layer"]});
        let answer = json!({"cells": [cell]}).to_string();
        let transport = Scripted::new(move |_| answer.clone());
        let router = router(data.path(), Asked::OpenAi, &transport);
        let scope = json!({"community": "c0.0"});
        let mut sent = 0;
        for chosen in [Asked::OpenAi, Asked::ThisComputer] {
            choose(data.path(), chosen);
            let estimate = json!({"scope": scope, "part": "sample"});
            let (_, cost) = call(&router, "POST", "/api/runs/r1/extraction/estimate", Some(estimate.clone())).await;
            assert_eq!((cost["to_ask"].clone(), cost["api_key_set"].clone()), (json!(3), json!(true)), "{cost}");
            priced_for(&cost, chosen);
            assert_eq!(transport.sent().len(), sent, "an estimate sends nothing");

            let mut started = accepting(&cost, BUDGET);
            started["scope"] = scope.clone();
            started["part"] = json!("sample");
            let (status, begun) = call(&router, "POST", "/api/runs/r1/extraction/run", Some(started)).await;
            assert_eq!(status, StatusCode::OK, "{begun}");
            let done = finished(&router, &begun["id"]).await;
            assert_eq!(done["state"], "done", "{done}");
            let made = json!({"kind": "feature", "feature": "extraction", "subject": {"community": {"run": "r1", "community": "c0.0"}}});
            assert_eq!((done["outcome"].clone(), done["result"].clone()), (made, Value::Null), "no run to open");
            let asked = transport.sent();
            assert_eq!(asked.len() - sent, 3, "one call per paper");
            sent_to(&asked[sent..], chosen);
            sent = asked.len();

            let (_, table) =
                call(&router, "POST", "/api/runs/r1/extraction/table", Some(json!({"scope": scope}))).await;
            assert_eq!(
                (table["provider"].clone(), table["model"].clone()),
                (json!(chosen.provider().id()), json!(MODEL))
            );
            let p01 = table["rows"].as_array().unwrap().iter().find(|row| row["paper"] == "p01").unwrap().clone();
            assert_eq!(p01["cells"][0]["kind"], "stated", "{p01}");
            assert_eq!(p01["cells"][0]["evidence"][0]["established"]["kind"], "verified");
            let p02 = table["rows"].as_array().unwrap().iter().find(|row| row["paper"] == "p02").unwrap().clone();
            assert_eq!(p02["cells"][0]["kind"], "not_verified", "a quote not in the paper is no value");

            // kept: nothing left to ask, and a run asks nothing
            let (_, again) = call(&router, "POST", "/api/runs/r1/extraction/estimate", Some(estimate)).await;
            assert_eq!(again["to_ask"], 0, "{again}");
            let enough = match chosen {
                Asked::OpenAi => {
                    json!({"scope": scope, "part": "sample", "budget_usd": 1.0, "provider": "openai", "model": MODEL})
                }
                Asked::ThisComputer => {
                    json!({"scope": scope, "part": "sample", "budget_tokens": 100_000, "provider": "compatible", "model": MODEL})
                }
            };
            let (_, begun) = call(&router, "POST", "/api/runs/r1/extraction/run", Some(enough)).await;
            assert_eq!(finished(&router, &begun["id"]).await["state"], "done");
            assert_eq!(transport.sent().len(), sent, "kept: not asked again");
        }
    }

    #[tokio::test]
    async fn every_answer_of_the_routes_fits_its_type_in_the_contracts() {
        use crate::api::contract::assert_fits;

        let data = data();
        let (router, _) = served_at(&data, false);
        let (_, columns) = ask(&router, "GET", "/api/runs/r1/extraction/columns", None).await;
        assert_fits("ExtractionColumns", &columns, &[]);
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
        let (_, table) = post(&router, "/api/runs/r1/extraction/table", json!({"scope": {"community": "c0"}})).await;
        assert_eq!(table["rows"][0]["cells"].as_array().map(Vec::len), Some(8), "{table}");
        assert_fits("ExtractionTable", &table, &[]);
        for part in ["sample", "rest"] {
            let (_, cost) =
                post(&router, "/api/runs/r1/extraction/estimate", json!({"scope": {"community": "c0"}, "part": part}))
                    .await;
            assert_fits("ExtractionEstimate", &cost, &[]);
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
        let started = |budget: f64| json!({"scope": {"community": "c0.0"}, "part": "sample", "budget_usd": budget, "provider": "anthropic", "model": MODEL});
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
                Ok(crate::jobs::Finished::Nothing)
            })
            .unwrap();
        let (status, told) = post(&router, "/api/runs/r1/extraction/run", started(1.0)).await;
        assert_eq!(
            (status, told["error"].clone()),
            (StatusCode::CONFLICT, json!("something else is already in progress"))
        );
        let mut elsewhere = started(1.0);
        elsewhere["scope"] = json!({"community": "c9"});
        assert_eq!(post(&router, "/api/runs/r1/extraction/run", elsewhere).await.0, StatusCode::NOT_FOUND);
        job.cancel(id).unwrap();
    }

    #[tokio::test]
    async fn a_run_is_refused_without_the_model_shown_or_when_its_estimate_grew_past_what_was_accepted() {
        use super::super::other_providers::{BUDGET, Chosen as Asked, Scripted, accepting, router};

        let data = data();
        // never the network: had anything been started, it would have been asked of this
        let transport = Scripted::new(|_| json!({"cells": []}).to_string());
        let router = router(data.path(), Asked::OpenAi, &transport);
        let scope = json!({"community": "c0.0"});
        let estimate = || post(&router, "/api/runs/r1/extraction/estimate", json!({"scope": scope, "part": "sample"}));
        let (_, shown) = estimate().await;
        let mut accepted = accepting(&shown, BUDGET);
        accepted["scope"] = scope.clone();
        accepted["part"] = json!("sample");
        let refused = |told: &Value| told["error"].as_str().unwrap().contains("changed since it was shown");

        let mut nameless = accepted.clone();
        nameless.as_object_mut().unwrap().remove("model");
        let (status, told) = post(&router, "/api/runs/r1/extraction/run", nameless).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(told["error"].as_str().unwrap().contains("which model"), "{told}");

        // a column added since: more would be asked than was accepted
        let mut more = default_columns();
        more.push(crate::analysis::extraction::Column {
            id: "ood".to_owned(),
            label: "OOD".to_owned(),
            question: "Does the paper evaluate on out-of-distribution data?".to_owned(),
            kind: crate::analysis::extraction::AnswerKind::YesNo,
        });
        let put = json!({"columns": more}).to_string();
        assert_eq!(ask(&router, "PUT", "/api/runs/r1/extraction/columns", Some(put)).await.0, StatusCode::OK);
        let (status, told) = post(&router, "/api/runs/r1/extraction/run", accepted.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{told}");
        assert!(refused(&told), "{told}");

        // the new estimate seen, then p03's PDF kept: it would be read in full, not from its abstract
        let (_, seen) = estimate().await;
        assert!(seen["most_usd"].as_f64() > shown["most_usd"].as_f64());
        let library = Library::open(&data.path().join("library")).unwrap();
        let long = "We report every experiment of this study in full detail in the sections that follow here.";
        library.keep_pdf("p03", &pdf_with(&[long; 80])).unwrap();
        let mut again = accepted.clone();
        again["budget_usd"] = seen["most_usd"].clone();
        let (status, told) = post(&router, "/api/runs/r1/extraction/run", again).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{told}");
        assert!(refused(&told), "{told}");
        assert!(transport.sent().is_empty(), "nothing was asked");
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
