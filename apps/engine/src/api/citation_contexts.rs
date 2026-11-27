//! Where papers of the library cite one another, sentence by sentence, and what kind of citation each is.
//!
//! ```text
//! GET    /api/papers/{id}/citation-contexts[?run=R]     the sentences where this paper cites papers of the
//!                                                       library, and where they (of run R) cite it
//!                                                       ([`PaperCitationContexts`])
//! POST   /api/runs/{run}/citation-contexts              read the citing sentences of every paper of the run: a
//!                                                       job, free (no model)
//! GET    /api/runs/{run}/citation-kinds[?community=c]   what the kinds of the community's citations come to
//!                                                       ([`KindsSummary`])
//! GET    /api/runs/{run}/citation-kinds/estimate?community=c&scope=sample|rest
//!                                                       what reading those kinds would cost; nothing is sent
//! POST   /api/runs/{run}/citation-kinds                 {community, scope, budget_usd}: read them, a job
//! DELETE /api/runs/{run}/citation-kinds[?community=c]   forget the kinds read for the papers the community cites
//! ```
//!
//! The first part needs no key and costs nothing: the sentences are the paper's own, each sent as evidence made
//! from its place ([`Evidence::at`]). The kinds are the model's reading, asked for only on request, after the
//! estimate was shown, within the budget the request names; each is sent beside its sentence, and only as
//! justified when its words were found there ([`crate::analysis::citation_kinds`]).

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::routing::{MethodRouter, get, post};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::reviews::papers_of;
use super::routes::{ApiError, Shared, on_the_side, one_name};
use crate::analysis::citation_contexts::{Context, Grouping, PaperContexts, Section};
use crate::analysis::citation_kinds::{
    self, Case, CitationKind, CitedRecord, Classified, Judgement, Plan, Ranked, SAMPLE, Said, Tally, judge,
    kept_readings, most_cited, ranked, tallies,
};
use crate::analysis::citations::Linker;
use crate::evidence::Evidence;
use crate::jobs::Progress;
use crate::library::citation_contexts::{ContextsReading, Entries};
use crate::library::{Feature, Library, PaperReading, Subject};
use crate::model::batch::{Asking, BatchEstimate, IN_FLIGHT, estimate_batch};
use crate::model::language::{Budget, Counting, Https, Limit};
use crate::settings;
use crate::text::passages::Passages;
use crate::text::references::Citing;
use crate::usage::Usd;

/// Papers citing one paper whose sentences are sent: the rest are counted.
pub const MOST_CITING: usize = 100;
/// The most a request may let a job spend, in US dollars.
pub const MOST_BUDGET_USD: f64 = 100.0;
/// A tree path is short ("c7.0.1"); anything longer is not one.
const LONGEST_TREE_PATH: usize = 200;
/// The most papers one job reads the citing sentences of.
pub const MOST_PAPERS: usize = 50_000;

/// The routes of this module, for [`super::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![
        ("/api/papers/{id}/citation-contexts", get(paper_contexts)),
        ("/api/runs/{run}/citation-contexts", post(read_run)),
        ("/api/runs/{run}/citation-kinds", get(summary).post(classify_run).delete(redo)),
        ("/api/runs/{run}/citation-kinds/estimate", get(estimate)),
    ]
}

/// The model the person chose in the settings.
fn chosen_model(engine: &Shared) -> Result<String, ApiError> {
    Ok(settings::load(&engine.settings_file(), &engine.choices)?.describe_model)
}

// ------------------------------------------------------------------------------------------------ one paper

/// What came of reading the paper asked about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperState {
    Read,
    /// No PDF is kept for it: only who cites it can be shown.
    NoPdf,
    /// Its PDF cannot be read.
    Unreadable,
}

/// A citing sentence as the interface shows it: `CitingSentenceShown` in the contracts.
#[derive(Serialize)]
pub struct Shown {
    /// The citing sentence, at its place in the citing paper.
    pub evidence: Evidence,
    pub before: Option<String>,
    pub after: Option<String>,
    pub section: Section,
    pub grouping: Grouping,
    pub written: String,
    /// The model's reading of it, beside it, or that there is none.
    pub judgement: Judgement,
}

/// One paper cited by (or citing) the paper asked about, with the sentences: `CitationContextsOf` in the contracts.
#[derive(Serialize)]
pub struct Between {
    /// The other paper: the cited one in `cites`, the citing one in `cited_by`.
    pub paper: String,
    pub title: String,
    pub tally: Tally,
    pub sentences: Vec<Shown>,
}

/// How many of a run's papers have had their citing sentences read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Coverage {
    pub papers: usize,
    pub read: usize,
    pub without_pdf: usize,
}

/// `PaperCitationContexts` in the contracts.
#[derive(Serialize)]
pub struct PaperCitationContexts {
    pub paper: String,
    pub state: PaperState,
    /// Entries of its reference list; 0 when none was found (or it was not read).
    pub references_listed: usize,
    /// Papers of the library it cites, in the order of its reference list.
    pub cites: Vec<Between>,
    /// Papers whose citing sentences were read that cite it, the most sentences first, at most [`MOST_CITING`].
    pub cited_by: Vec<Between>,
    pub cited_by_total: usize,
    /// Of the run asked about.
    pub coverage: Option<Coverage>,
    /// Sentences whose place could no longer be had from their paper's text (a PDF replaced since): not shown.
    pub left_out: usize,
    /// The model whose readings are shown.
    pub model: String,
}

#[derive(Deserialize)]
struct InRun {
    run: Option<String>,
}

/// The sentences of `contexts` (all `citing` citing `cited`) as shown, with their kinds; those whose place is no
/// longer the same sentence in `passages` are counted in `left_out`.
fn shown(
    library: &Library,
    model: &str,
    citing: &str,
    cited: &str,
    passages: &Passages,
    contexts: &[&Context],
    left_out: &mut usize,
) -> Result<(Tally, Vec<Shown>), ApiError> {
    let cases: Vec<Case> = contexts
        .iter()
        .map(|context| Case { citing: citing.to_owned(), cited: cited.to_owned(), context: (*context).clone() })
        .collect();
    let readings = kept_readings(library, model, &cases)?;
    let mut tally = Tally { papers: 1, ..Tally::default() };
    let mut sentences = Vec::new();
    for (case, reading) in cases.into_iter().zip(readings) {
        let context = case.context;
        let Some(evidence) =
            Evidence::at(citing, passages, &context.address).filter(|evidence| evidence.quote() == context.sentence)
        else {
            *left_out += 1;
            continue;
        };
        let verdict = judge(&context, reading.as_ref());
        tally.add(&verdict);
        sentences.push(Shown {
            judgement: Judgement::of(&verdict, citing, passages),
            evidence,
            before: context.before.map(|neighbour| neighbour.sentence),
            after: context.after.map(|neighbour| neighbour.sentence),
            section: context.section,
            grouping: context.grouping,
            written: context.written,
        });
    }
    Ok((tally, sentences))
}

/// The titles of `ids`, by id.
fn titles(library: &Library, ids: &[&str]) -> Result<HashMap<String, String>, ApiError> {
    let records = library.papers(ids)?;
    Ok(ids.iter().zip(records).filter_map(|(id, paper)| Some(((*id).to_owned(), paper?.title))).collect())
}

/// How far the citing sentences of `ids` have been read.
fn coverage(library: &Library, ids: &[String]) -> Result<Coverage, ApiError> {
    let wanted: Vec<&str> = ids.iter().map(String::as_str).collect();
    let read = library.contexts_kept_among(&wanted)?.len();
    let without_pdf = ids.iter().filter(|id| !library.pdf_file(id).is_file()).count();
    Ok(Coverage { papers: ids.len(), read, without_pdf })
}

fn contexts_of_paper(engine: &Shared, paper: &str, run: Option<&str>) -> Result<PaperCitationContexts, ApiError> {
    let library = engine.library()?;
    library.paper(paper)?.ok_or(ApiError::NotFound)?;
    let model = chosen_model(engine)?;
    let run_ids = run.map(|run| papers_of(engine, run, None)).transpose()?;
    let mut left_out = 0;

    // what it cites: read now if it was not (one PDF), kept for next time
    let own = library.paper_texts(&[paper])?.pop();
    let (state, references_listed, cites) = match own {
        Some(PaperReading::Read(passages)) => {
            let contexts = match library.kept_contexts(&[paper], Entries::InLibrary)?.remove(paper) {
                Some(contexts) => contexts,
                None => {
                    let papers = library.link_papers()?;
                    let linker = Linker::new(&papers);
                    match library.citation_contexts(&[paper], &linker, &|| false, &|_| {})?.pop() {
                        Some((_, ContextsReading::Read(contexts))) => contexts,
                        _ => PaperContexts { citing: Citing::ByNumber, listed: 0, entries: vec![] },
                    }
                }
            };
            let linked = contexts.in_library();
            let ids: Vec<&str> = linked.iter().map(|(cited, _)| cited.as_str()).collect();
            let titles = titles(&library, &ids)?;
            let mut cites = Vec::new();
            for (cited, contexts) in &linked {
                let (tally, sentences) = shown(&library, &model, paper, cited, &passages, contexts, &mut left_out)?;
                if !sentences.is_empty() {
                    let title = titles.get(cited).cloned().unwrap_or_else(|| cited.clone());
                    cites.push(Between { paper: cited.clone(), title, tally, sentences });
                }
            }
            (PaperState::Read, contexts.listed, cites)
        }
        Some(PaperReading::Unreadable { .. }) => (PaperState::Unreadable, 0, Vec::new()),
        Some(PaperReading::NoPdf { .. }) | Some(PaperReading::NotInLibrary) | None => {
            (PaperState::NoPdf, 0, Vec::new())
        }
    };

    // who cites it, among the papers whose sentences were read (of the run, when one was named)
    let in_run: Option<HashSet<&str>> = run_ids.as_ref().map(|ids| ids.iter().map(String::as_str).collect());
    let citing: Vec<String> = library
        .citing_papers(paper)?
        .into_iter()
        .map(|(citer, _)| citer)
        .filter(|citer| citer != paper && in_run.as_ref().is_none_or(|run| run.contains(citer.as_str())))
        .collect();
    let cited_by_total = citing.len();
    // the most citing sentences first: only those papers' passages are loaded
    let citing = &citing[..citing.len().min(MOST_CITING)];
    let wanted: Vec<&str> = citing.iter().map(String::as_str).collect();
    // their passages first, so that contexts of a PDF replaced since no longer count
    let readings = library.paper_texts(&wanted)?;
    let kept = library.kept_contexts(&wanted, Entries::InLibrary)?;
    let titles = titles(&library, &wanted)?;
    let mut cited_by = Vec::new();
    for (citer, reading) in wanted.iter().zip(&readings) {
        let (Some(passages), Some(contexts)) = (reading.passages(), kept.get(*citer)) else { continue };
        let linked = contexts.in_library();
        let Some((_, contexts)) = linked.iter().find(|(cited, _)| cited == paper) else { continue };
        let (tally, sentences) = shown(&library, &model, citer, paper, passages, contexts, &mut left_out)?;
        if !sentences.is_empty() {
            let title = titles.get(*citer).cloned().unwrap_or_else(|| (*citer).to_owned());
            cited_by.push(Between { paper: (*citer).to_owned(), title, tally, sentences });
        }
    }
    cited_by.sort_by(|a, b| b.sentences.len().cmp(&a.sentences.len()).then(a.paper.cmp(&b.paper)));

    Ok(PaperCitationContexts {
        paper: paper.to_owned(),
        state,
        references_listed,
        cites,
        cited_by,
        cited_by_total,
        coverage: run_ids.as_ref().map(|ids| coverage(&library, ids)).transpose()?,
        left_out,
        model,
    })
}

async fn paper_contexts(
    State(engine): State<Shared>,
    Path(paper): Path<String>,
    query: Result<Query<InRun>, QueryRejection>,
) -> Result<Json<PaperCitationContexts>, ApiError> {
    let Query(InRun { run }) = query?;
    let answer = on_the_side(move || contexts_of_paper(&engine, &paper, run.as_deref())).await?;
    Ok(Json(answer))
}

// ------------------------------------------------------------------------------------------------ a run's papers

/// Start reading the citing sentences of every paper of the run.
async fn read_run(State(engine): State<Shared>, Path(run): Path<String>) -> Result<Json<Value>, ApiError> {
    let shared = engine.clone();
    let started = on_the_side(move || {
        let ids = papers_of(&shared, &run, None)?;
        if ids.len() > MOST_PAPERS {
            return Err(ApiError::Invalid(format!("a run of more than {MOST_PAPERS} papers is read in parts")));
        }
        let library = shared.library()?;
        let name = run.clone();
        let id = shared.job.start_tracked("reading citing sentences", move |tracker| {
            let fail = |error: crate::library::LibraryError| error.to_string();
            let papers = library.link_papers().map_err(fail)?;
            let linker = Linker::new(&papers);
            let wanted: Vec<&str> = ids.iter().map(String::as_str).collect();
            let of = u64::try_from(wanted.len()).unwrap_or(u64::MAX);
            let told = |done: usize| {
                tracker.progress(Progress { done: u64::try_from(done).unwrap_or(of), of, usd: Usd::ZERO });
            };
            told(0);
            let read = library.citation_contexts(&wanted, &linker, &|| tracker.cancelled(), &told).map_err(fail)?;
            let count =
                |wanted: fn(&ContextsReading) -> bool| read.iter().filter(|(_, reading)| wanted(reading)).count();
            tracker.log(&format!(
                "Citing sentences read for {} of {} papers: {} have no PDF kept, {} have a PDF that cannot be read{}.",
                count(|reading| matches!(reading, ContextsReading::Read(_))),
                wanted.len(),
                count(|reading| matches!(reading, ContextsReading::NoPdf)),
                count(|reading| matches!(reading, ContextsReading::Unreadable(_))),
                if read.len() < wanted.len() { "; stopped before the rest" } else { "" }
            ));
            Ok(name)
        })?;
        Ok(id)
    })
    .await?;
    Ok(Json(json!({ "ok": true, "id": started })))
}

#[derive(Deserialize)]
struct InCommunity {
    community: Option<String>,
}

/// The tree path asked for, checked to be one.
fn tree_path(community: Option<&str>) -> Result<Option<&str>, ApiError> {
    match community {
        Some(path) if path.is_empty() || path.len() > LONGEST_TREE_PATH => Err(ApiError::NotFound),
        other => Ok(other),
    }
}

/// The citations the papers of a run (or one community of it) make of papers of the library, with the cited
/// papers' records.
struct Citations {
    papers: usize,
    papers_read: usize,
    without_pdf: usize,
    cases: Vec<Case>,
    records: HashMap<String, CitedRecord>,
}

fn citations_of(engine: &Shared, library: &Library, run: &str, community: Option<&str>) -> Result<Citations, ApiError> {
    let ids = papers_of(engine, run, tree_path(community)?)?;
    let wanted: Vec<&str> = ids.iter().map(String::as_str).collect();
    let kept = library.kept_contexts(&wanted, Entries::InLibrary)?;
    let mut cases = Vec::new();
    for citing in &wanted {
        let Some(contexts) = kept.get(*citing) else { continue };
        for (cited, contexts) in contexts.in_library() {
            cases.extend(contexts.into_iter().map(|context| Case {
                citing: (*citing).to_owned(),
                cited: cited.clone(),
                context: context.clone(),
            }));
        }
    }
    let mut cited: Vec<&str> = cases.iter().map(|case| case.cited.as_str()).collect();
    cited.sort_unstable();
    cited.dedup();
    let records = cited
        .iter()
        .zip(library.papers(&cited)?)
        .filter_map(|(id, paper)| {
            let paper = paper?;
            Some(((*id).to_owned(), CitedRecord { title: paper.title, abstract_text: paper.r#abstract }))
        })
        .collect();
    let without_pdf = ids.iter().filter(|id| !library.pdf_file(id).is_file()).count();
    Ok(Citations { papers: ids.len(), papers_read: kept.len(), without_pdf, cases, records })
}

/// What the kinds of a community's citations come to: `CitationKindsSummary` in the contracts. Every figure is a
/// count of citing sentences, out of `tally.sentences`.
#[derive(Serialize)]
pub struct KindsSummary {
    pub papers: usize,
    /// Of its papers, whose citing sentences were read.
    pub papers_read: usize,
    pub without_pdf: usize,
    /// Papers of the library its papers cite.
    pub cited_papers: usize,
    /// All its citations of papers of the library taken together.
    pub tally: Tally,
    pub baseline: Vec<Ranked>,
    pub contrasts: Vec<Ranked>,
    /// Their data or benchmark used.
    pub data: Vec<Ranked>,
    /// Their method used, or extended.
    pub used: Vec<Ranked>,
    pub supports: Vec<Ranked>,
    /// The papers the sample run reads first.
    pub sample: Vec<String>,
    pub titles: BTreeMap<String, String>,
    pub model: String,
}

fn summary_of(engine: &Shared, run: &str, community: Option<&str>) -> Result<KindsSummary, ApiError> {
    let library = engine.library()?;
    let model = chosen_model(engine)?;
    let found = citations_of(engine, &library, run, community)?;
    let readings = kept_readings(&library, &model, &found.cases)?;
    let tallies = tallies(&found.cases, &readings);
    let mut tally = Tally::default();
    for (case, reading) in found.cases.iter().zip(&readings) {
        tally.add(&judge(&case.context, reading.as_ref()));
    }
    tally.papers = found.cases.iter().map(|case| case.citing.as_str()).collect::<HashSet<_>>().len();
    let lists = [
        ranked(&tallies, &[CitationKind::Baseline]),
        ranked(&tallies, &[CitationKind::Contrasts]),
        ranked(&tallies, &[CitationKind::UsesData]),
        ranked(&tallies, &[CitationKind::Uses, CitationKind::Extends]),
        ranked(&tallies, &[CitationKind::Supports]),
    ];
    let sample = most_cited(&found.cases, SAMPLE);
    let named: BTreeMap<String, String> = lists
        .iter()
        .flatten()
        .map(|ranked| ranked.paper.clone())
        .chain(sample.iter().cloned())
        .filter_map(|id| Some((id.clone(), found.records.get(&id)?.title.clone())))
        .collect();
    let [baseline, contrasts, data, used, supports] = lists;
    Ok(KindsSummary {
        papers: found.papers,
        papers_read: found.papers_read,
        without_pdf: found.without_pdf,
        cited_papers: tallies.len(),
        tally,
        baseline,
        contrasts,
        data,
        used,
        supports,
        sample,
        titles: named,
        model,
    })
}

async fn summary(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    query: Result<Query<InCommunity>, QueryRejection>,
) -> Result<Json<KindsSummary>, ApiError> {
    let Query(InCommunity { community }) = query?;
    Ok(Json(on_the_side(move || summary_of(&engine, &run, community.as_deref())).await?))
}

/// Which citations a run of the model reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Those of the [`SAMPLE`] papers the community cites most.
    Sample,
    /// All those not read yet.
    Rest,
}

/// The plan for `scope`'s citations of a community.
fn plan_of(
    engine: &Shared,
    library: &Library,
    run: &str,
    community: Option<&str>,
    scope: Scope,
) -> Result<Plan, ApiError> {
    let model = chosen_model(engine)?;
    let found = citations_of(engine, library, run, community)?;
    let cases = match scope {
        Scope::Sample => {
            let sample: HashSet<String> = most_cited(&found.cases, SAMPLE).into_iter().collect();
            found.cases.into_iter().filter(|case| sample.contains(&case.cited)).collect()
        }
        Scope::Rest => found.cases,
    };
    Ok(Plan::new(&model, cases, &found.records, |key| {
        Ok(library.kept::<Classified>(Feature::CitationKinds, key)?.is_some())
    })?)
}

/// `CitationKindsEstimate` in the contracts: what reading a scope's kinds would cost, before anything is sent.
#[derive(Serialize)]
pub struct KindsEstimate {
    pub scope: Scope,
    pub model: String,
    /// Citing sentences in the scope.
    pub sentences: usize,
    /// Of those, read already: they cost nothing.
    pub kept: usize,
    pub to_ask: usize,
    pub estimate: BatchEstimate,
    /// Whether the model can be asked at all (a key is set); never the key.
    pub can_ask: bool,
}

#[derive(Deserialize)]
struct EstimateQuery {
    community: Option<String>,
    scope: Scope,
}

async fn estimate(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    query: Result<Query<EstimateQuery>, QueryRejection>,
) -> Result<Json<KindsEstimate>, ApiError> {
    let Query(EstimateQuery { community, scope }) = query?;
    let answer = on_the_side(move || {
        let library = engine.library()?;
        let plan = plan_of(&engine, &library, &run, community.as_deref(), scope)?;
        let items = plan.items();
        let estimate = estimate_batch::<Said>(&library, &items, Counting::ASSUMED)?;
        Ok(KindsEstimate {
            scope,
            sentences: plan.cases.len(),
            kept: plan.kept,
            to_ask: plan.to_ask(),
            model: plan.model.clone(),
            estimate,
            can_ask: engine.language.is_ok(),
        })
    })
    .await?;
    Ok(Json(answer))
}

/// `ClassifyRequest` in the contracts.
#[derive(Deserialize)]
struct ClassifyRequest {
    community: Option<String>,
    scope: Scope,
    /// The most the job may spend, in US dollars.
    budget_usd: f64,
}

fn budget_of(usd: f64) -> Result<Budget, ApiError> {
    if !(usd > 0.0 && usd <= MOST_BUDGET_USD) {
        return Err(ApiError::Invalid(format!(
            "the budget must be more than 0 and at most {MOST_BUDGET_USD} US dollars"
        )));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let picodollars = (usd * 1e12).round() as u64;
    Ok(Budget::new(Limit::Dollars(Usd::from_picodollars(picodollars))))
}

async fn classify_run(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    body: Result<Json<ClassifyRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(request) = body?;
    let budget = budget_of(request.budget_usd)?;
    one_name(&run)?;
    let endpoint =
        engine.language.clone().map_err(|why| ApiError::Invalid(format!("the model cannot be asked: {why}")))?;
    let started = on_the_side(move || {
        let library = engine.library()?;
        let community = tree_path(request.community.as_deref())?.map(str::to_owned);
        papers_of(&engine, &run, community.as_deref())?; // a run or community that is not there is not found now
        let shared = engine.clone();
        let id = engine.job.start_tracked("reading citation kinds", move |tracker| {
            let plan = plan_of(&shared, &library, &run, community.as_deref(), request.scope).map_err(|error| error.to_string())?;
            tracker.log(&format!(
                "Reading the kinds of {} citing sentences with {} ({} read before), in {} calls.",
                plan.to_ask(),
                plan.model,
                plan.kept,
                plan.calls.len()
            ));
            let transport = Https::default();
            let asking = Asking { library: &library, transport: &transport, endpoint, recorder: &*library, in_flight: IN_FLIGHT };
            let done = citation_kinds::classify(asking, &plan, &budget, tracker).map_err(|error| error.to_string())?;
            tracker.log(&format!(
                "Read {} citing sentences; {} were missing from the answers, {} calls had no usable answer, {} were not started.",
                done.read, done.unanswered, done.failed, done.not_started
            ));
            Ok(run)
        })?;
        Ok(id)
    })
    .await?;
    Ok(Json(json!({ "ok": true, "id": started })))
}

async fn redo(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    query: Result<Query<InCommunity>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(InCommunity { community }) = query?;
    let forgotten = on_the_side(move || {
        let library = engine.library()?;
        let found = citations_of(&engine, &library, &run, community.as_deref())?;
        let mut cited: Vec<&str> = found.cases.iter().map(|case| case.cited.as_str()).collect();
        cited.sort_unstable();
        cited.dedup();
        let subjects: Vec<Subject> = cited.into_iter().map(|id| Subject::Paper(id.to_owned())).collect();
        Ok(library.forget(Feature::CitationKinds, &subjects)?)
    })
    .await?;
    Ok(Json(json!({ "ok": true, "forgotten": forgotten })))
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
    use crate::analysis::citation_kinds::{PROMPT, sentence_key};
    use crate::library::papers::tests::paper;
    use crate::library::passages::fixtures::pdf_with;
    use crate::library::{Known, Library, Slot};
    use crate::settings::Choices;

    use super::*;

    const AT: &str = "127.0.0.1:8765";
    const MODEL: &str = "claude-opus-5-5";

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

    fn seed_pdf() -> Vec<u8> {
        pdf_with(&[
            "1 Introduction",
            "",
            "Merging is cheap. Task arithmetic adds the updates of each task [1]. We study when it fails.",
            "",
            "2 Experiments",
            "",
            "We compare against the merging of [2] as a baseline. It is weaker than [3].",
            "",
            "References",
            "",
            "[1] A. Lovelace. Editing models with task arithmetic. In ICLR, 2023.",
            "",
            "[2] A. Turing. Resolving interference when merging models. In NeurIPS, 2023.",
            "",
            "[3] G. Hopper. A work that is not in the library. In ICML, 2022.",
        ])
    }

    /// A library of three papers, "seed" citing the other two, and a run "r1" of all three with two communities.
    fn data() -> tempfile::TempDir {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(&data.path().join("library")).unwrap();
        let mut papers = vec![paper("seed"), paper("arith"), paper("ties")];
        papers[1].title = "Editing models with task arithmetic".to_owned();
        papers[1].published = "2023-05-01T00:00:00Z".to_owned();
        papers[2].title = "Resolving interference when merging models".to_owned();
        papers[2].published = "2023-06-01T00:00:00Z".to_owned();
        library.add_papers(&papers, None, Known::Keep).unwrap();
        library.keep_pdf("seed", &seed_pdf()).unwrap();
        let run = data.path().join("runs").join("r1");
        std::fs::create_dir_all(&run).unwrap();
        let nodes = json!({"nodes": [{"id": "seed"}, {"id": "arith"}, {"id": "ties"}], "edges": []});
        std::fs::write(run.join("graph.json"), nodes.to_string()).unwrap();
        let tree = json!({"communities": [
            {"tree_path": "c0", "members": [0], "children": []},
            {"tree_path": "c1", "members": [1, 2], "children": []}]});
        std::fs::write(run.join("communities.json"), tree.to_string()).unwrap();
        data
    }

    fn serve(data: &tempfile::TempDir, key: bool) -> (axum::Router, Shared) {
        let listen = Listen::from_environment(&Environment::from_pairs([
            ("CONSTELLATE_TOKEN", STRONG),
            ("CONSTELLATE_ORIGIN", "http://127.0.0.1:3000"),
        ]))
        .unwrap();
        let mut engine = Engine::new(data.path().to_path_buf(), choices());
        if key {
            engine.language =
                Environment::from_pairs([("ANTHROPIC_API_KEY", "test-key-not-real-0123456789")]).language_endpoint();
        }
        let provider = std::sync::Arc::new(super::super::login::oidc::tests::Fake::new(&[]));
        let (router, engine, _) = super::super::application(
            &listen,
            AT.parse().unwrap(),
            engine,
            super::super::routes::routes(),
            provider,
            || Ok(super::super::login::password::Hashing::light()),
        )
        .unwrap();
        (router, engine)
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

    async fn finished(router: &axum::Router) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let (_, status) = ask(router, "GET", "/api/status", None).await;
            if status["state"] == "done" || status["state"] == "failed" {
                return status;
            }
            assert!(std::time::Instant::now() < deadline, "the job never finished");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn a_papers_citing_sentences_are_served_as_evidence_with_no_key_at_all() {
        let data = data();
        let (router, _) = serve(&data, false);
        let (status, told) = ask(&router, "GET", "/api/papers/seed/citation-contexts?run=r1", None).await;
        assert_eq!(status, StatusCode::OK, "{told}");
        assert_eq!((told["state"].clone(), told["references_listed"].clone()), (json!("read"), json!(3)));
        let cites = told["cites"].as_array().unwrap();
        assert_eq!(
            cites.iter().map(|cited| cited["paper"].clone()).collect::<Vec<_>>(),
            [json!("arith"), json!("ties")]
        );
        let sentence = &cites[1]["sentences"][0];
        assert_eq!(sentence["evidence"]["paper"], "seed");
        assert_eq!(sentence["evidence"]["quote"], "We compare against the merging of [2] as a baseline.");
        assert_eq!(sentence["evidence"]["established"]["kind"], "verified");
        assert_eq!(sentence["after"], "It is weaker than [3].");
        assert_eq!(sentence["section"], json!({"kind": "experiments"}));
        assert_eq!(sentence["judgement"], json!({"state": "not_classified"}));
        assert_eq!(cites[1]["tally"]["not_classified"], 1);
        assert_eq!(told["coverage"], json!({"papers": 3, "read": 1, "without_pdf": 2}));

        // the cited paper has no PDF: it is cited, and says so
        let (_, ties) = ask(&router, "GET", "/api/papers/ties/citation-contexts?run=r1", None).await;
        assert_eq!(ties["state"], "no_pdf");
        assert_eq!(ties["cited_by"][0]["paper"], "seed");
        assert_eq!(
            ties["cited_by"][0]["sentences"][0]["evidence"]["quote"],
            "We compare against the merging of [2] as a baseline."
        );
        assert_eq!(ties["cited_by_total"], 1);

        assert_eq!(ask(&router, "GET", "/api/papers/nowhere/citation-contexts", None).await.0, StatusCode::NOT_FOUND);
        assert_eq!(
            ask(&router, "GET", "/api/papers/seed/citation-contexts?run=none", None).await.0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn a_kind_is_shown_beside_its_sentence_justified_only_by_words_found_there() {
        let data = data();
        let (router, engine) = serve(&data, false);
        ask(&router, "GET", "/api/papers/seed/citation-contexts", None).await;
        // readings kept as a job would have kept them: one justified, one with a quote that is not in the paper
        let library = engine.library().unwrap();
        let contexts = library.kept_contexts(&["seed"], Entries::InLibrary).unwrap().remove("seed").unwrap();
        let linked = contexts.in_library();
        let keep = |cited: &str, kind, quote: &str| {
            let context = linked.iter().find(|(paper, _)| paper == cited).unwrap().1[0];
            let slot = Slot {
                feature: Feature::CitationKinds,
                key: sentence_key(MODEL, "seed", cited, &context.address),
                subject: Subject::Paper(cited.to_owned()),
            };
            library.keep(&slot, MODEL, PROMPT, &Classified { kind, quote: quote.to_owned() }).unwrap();
        };
        keep("ties", CitationKind::Baseline, "compare against the merging");
        keep("arith", CitationKind::Contrasts, "Task arithmetic fails on every benchmark");
        let (_, told) = ask(&router, "GET", "/api/papers/seed/citation-contexts", None).await;
        let ties = &told["cites"][1]["sentences"][0]["judgement"];
        assert_eq!((ties["state"].clone(), ties["kind"].clone()), (json!("justified"), json!("baseline")));
        assert_eq!(ties["because"]["quote"], "compare against the merging");
        assert_eq!(ties["because"]["paper"], "seed");
        let arith = &told["cites"][0]["sentences"][0]["judgement"];
        assert_eq!(*arith, json!({"state": "not_justified", "claimed": "contrasts"}));
        assert_eq!(told["cites"][0]["tally"]["not_justified"], 1);

        // what the community's citations come to: counts, each out of its sentences
        let (status, summary) = ask(&router, "GET", "/api/runs/r1/citation-kinds?community=c0", None).await;
        assert_eq!(status, StatusCode::OK, "{summary}");
        assert_eq!((summary["papers"].clone(), summary["papers_read"].clone()), (json!(1), json!(1)));
        assert_eq!(summary["tally"]["sentences"], 2);
        assert_eq!(summary["baseline"][0]["paper"], "ties");
        assert_eq!(summary["baseline"][0]["count"], 1);
        assert_eq!(summary["titles"]["ties"], "Resolving interference when merging models");
        assert!(summary["contrasts"].as_array().unwrap().is_empty(), "a reading not justified ranks nothing");

        // the redo forgets them
        let (_, forgot) = ask(&router, "DELETE", "/api/runs/r1/citation-kinds?community=c0", None).await;
        assert_eq!(forgot["forgotten"], 2);
    }

    #[tokio::test]
    async fn the_estimate_sends_nothing_and_asking_needs_a_key_and_a_budget() {
        let data = data();
        let (router, _) = serve(&data, false);
        ask(&router, "GET", "/api/papers/seed/citation-contexts", None).await;
        let (status, estimate) = ask(&router, "GET", "/api/runs/r1/citation-kinds/estimate?scope=sample", None).await;
        assert_eq!(status, StatusCode::OK, "{estimate}");
        assert_eq!((estimate["sentences"].clone(), estimate["to_ask"].clone()), (json!(2), json!(2)));
        assert_eq!(estimate["estimate"]["by_model"][0]["calls"], 2, "one call per cited paper");
        assert_eq!(estimate["can_ask"], false);
        assert!(estimate["estimate"]["usd"].as_f64().unwrap() > 0.0);
        let body = json!({"scope": "sample", "budget_usd": 1.0}).to_string();
        let (status, told) = ask(&router, "POST", "/api/runs/r1/citation-kinds", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{told}");
        assert!(told["error"].as_str().unwrap().contains("cannot be asked"));

        let (router, _) = serve(&data, true);
        for budget in [0.0, -1.0, 1e9] {
            let body = json!({"scope": "rest", "budget_usd": budget}).to_string();
            assert_eq!(
                ask(&router, "POST", "/api/runs/r1/citation-kinds", Some(body)).await.0,
                StatusCode::BAD_REQUEST
            );
        }
        let large = json!({"scope": "rest", "budget_usd": 1.0, "community": "x".repeat(5 * 1024 * 1024)}).to_string();
        assert_eq!(
            ask(&router, "POST", "/api/runs/r1/citation-kinds", Some(large)).await.0,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let missing = json!({"scope": "rest", "budget_usd": 1.0, "community": "c9"}).to_string();
        assert_eq!(ask(&router, "POST", "/api/runs/r1/citation-kinds", Some(missing)).await.0, StatusCode::NOT_FOUND);
        assert_eq!(
            ask(&router, "GET", "/api/runs/r1/citation-kinds/estimate?scope=most", None).await.0,
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn a_runs_citing_sentences_are_read_as_a_job_and_a_second_job_is_refused_meanwhile() {
        let data = data();
        let (router, engine) = serve(&data, false);
        let held = engine
            .job
            .start_tracked("something else", |tracker| {
                while !tracker.cancelled() {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Ok("stopped".to_owned())
            })
            .unwrap();
        assert_eq!(ask(&router, "POST", "/api/runs/r1/citation-contexts", None).await.0, StatusCode::CONFLICT);
        engine.job.cancel(held).unwrap();
        finished(&router).await;
        let (status, started) = ask(&router, "POST", "/api/runs/r1/citation-contexts", None).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let status = finished(&router).await;
        assert_eq!(status["state"], "done", "{status}");
        assert_eq!(status["result"], "r1");
        assert!(
            status["log"].to_string().contains("Citing sentences read for 1 of 3 papers: 2 have no PDF kept"),
            "{status}"
        );
        assert_eq!(ask(&router, "POST", "/api/runs/none/citation-contexts", None).await.0, StatusCode::NOT_FOUND);
        let (_, summary) = ask(&router, "GET", "/api/runs/r1/citation-kinds", None).await;
        assert_eq!((summary["papers"].clone(), summary["cited_papers"].clone()), (json!(3), json!(2)));
        assert_eq!(summary["sample"], json!(["arith", "ties"]));
    }
}
