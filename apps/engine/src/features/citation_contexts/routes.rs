//! Where papers of the library cite one another, sentence by sentence, and what kind of citation each is.
//!
//! ```text
//! GET    /api/papers/{id}/citation-contexts[?run=R[&community=c]]
//!                                                       the sentences where this paper cites papers of the
//!                                                       library, and where they (of run R, of its community c)
//!                                                       cite it ([`PaperCitationContexts`])
//! POST   /api/runs/{run}/citation-contexts              read the citing sentences of every paper of the run: a
//!                                                       job, free (no model)
//! GET    /api/runs/{run}/citation-kinds[?community=c]   what the kinds of the community's citations come to
//!                                                       ([`KindsSummary`])
//! GET    /api/runs/{run}/citation-kinds/estimate?community=c&scope=sample|rest
//!                                                       what reading those kinds would cost; nothing is sent
//! POST   /api/runs/{run}/citation-kinds                 {community, scope, provider, model, to_ask, budget_usd |
//!                                                       budget_tokens}: read them, a job
//! DELETE /api/runs/{run}/citation-kinds[?community=c]   forget the chosen model's readings of the community's citing sentences: how many
//! ```
//!
//! The first part needs no key and costs nothing: the sentences are the paper's own, each sent as evidence made
//! from its place ([`Evidence::at`]). The kinds are the model's reading, asked for only on request, after the
//! estimate was shown, within the budget the request names; each is sent beside its sentence, and only as
//! justified when its words were found there ([`crate::features::citation_contexts::kinds`]).

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::routing::{MethodRouter, get, post};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::analysis::citations::Linker;
use crate::api::papers::reviews::papers_of;
use crate::api::routes::{ApiError, Shared, on_the_side, one_name};
use crate::evidence::Evidence;
use crate::features::citation_contexts::kinds::{
    self, Case, CitationKind, CitedRecord, Judgement, Plan, Ranked, SAMPLE, Said, Tally, Verdict, judge, kept_reading,
    kept_readings, most_cited, ranked, tallies, verdicts,
};
use crate::features::citation_contexts::store::{ContextsReading, Entries};
use crate::features::citation_contexts::{Context, Grouping, Neighbour, PaperContexts, Section};
use crate::jobs::{Finished, Progress};
use crate::library::{AnswerKey, Feature, Library, PaperReading, Subject};
use crate::model::batch::{Asking, BatchEstimate, IN_FLIGHT, estimate_batch};
use crate::model::chosen::{Accepted, Chosen, Refused, Terms, budget};
use crate::model::language::Provider;
use crate::text::passages::Passages;
use crate::text::references::Citing;
use crate::usage::Usd;

/// Papers citing one paper whose sentences are sent: the rest are counted.
pub const MOST_CITING: usize = 100;
/// The most a request may let a job spend, in US dollars.
pub const MOST_BUDGET_USD: f64 = 100.0;
/// The same for a model with no price, in tokens.
pub const MOST_BUDGET_TOKENS: u64 = 100_000_000;
/// A tree path is short ("c7.0.1"); anything longer is not one.
const LONGEST_TREE_PATH: usize = 200;
/// The most papers one job reads the citing sentences of.
pub const MOST_PAPERS: usize = 50_000;

/// The routes of this module, for [`crate::api::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![
        ("/api/papers/{id}/citation-contexts", get(paper_contexts)),
        ("/api/runs/{run}/citation-contexts", post(read_run)),
        ("/api/runs/{run}/citation-kinds", get(summary).post(classify_run).delete(redo)),
        ("/api/runs/{run}/citation-kinds/estimate", get(estimate)),
    ]
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
    /// The sentences on either side, also made from their place in the citing paper (none when it was not found).
    pub before: Option<Evidence>,
    pub after: Option<Evidence>,
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
    /// With a PDF that could not be read: their citing sentences cannot be read either.
    pub unreadable: usize,
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
    /// With a run: only the papers of this community of it are looked at as citing papers.
    community: Option<String>,
}

/// The sentences of `contexts` (all `citing` citing `cited`) as shown, with their kinds; those whose place is no
/// longer the same sentence in `passages` are counted in `left_out`.
fn shown(
    library: &Library,
    chosen: &Chosen,
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
    let readings = kept_readings(library, chosen, &cases)?;
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
        let judgement = Judgement::of(&judge(&context, reading.as_ref()), citing, passages);
        // counted as shown: words no longer had from the paper's pages make it not justified in the count too
        tally.add_shown(&judgement);
        let around = |neighbour: Option<Neighbour>| {
            let neighbour = neighbour?;
            Evidence::at(citing, passages, &neighbour.address).filter(|evidence| evidence.quote() == neighbour.sentence)
        };
        sentences.push(Shown {
            judgement,
            evidence,
            before: around(context.before),
            after: around(context.after),
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
    let unreadable = library.unreadable_among(&wanted)?.len();
    Ok(Coverage { papers: ids.len(), read, without_pdf, unreadable })
}

fn contexts_of_paper(
    engine: &Shared,
    paper: &str,
    run: Option<&str>,
    community: Option<&str>,
) -> Result<PaperCitationContexts, ApiError> {
    let library = engine.library()?;
    library.paper(paper)?.ok_or(ApiError::NotFound)?;
    let chosen = engine.chosen()?;
    let model = chosen.model().to_owned();
    let run_ids = run.map(|run| papers_of(engine, run, None)).transpose()?;
    let citing_ids = match (run, tree_path(community)?) {
        (Some(run), Some(community)) => Some(papers_of(engine, run, Some(community))?),
        (_, None) => run_ids.clone(),
        (None, Some(_)) => return Err(ApiError::Invalid("a community is one of a run: name the run".to_owned())),
    };
    let mut left_out = 0;

    // what it cites
    let own = library.paper_texts(&[paper])?.pop();
    let (state, references_listed, cites) = match own {
        Some(PaperReading::Read(passages)) => {
            // read now if it was not (one PDF), or linked again: a paper added to the library since is found
            let papers = library.link_papers()?;
            let linker = Linker::new(&papers);
            let contexts = match library.citation_contexts(&[paper], &linker, &|| false, &|_| {})?.pop() {
                Some((_, ContextsReading::Read(contexts))) => contexts,
                _ => PaperContexts { citing: Citing::ByNumber, listed: 0, entries: vec![] },
            };
            let linked = contexts.in_library();
            let ids: Vec<&str> = linked.iter().map(|(cited, _)| cited.as_str()).collect();
            let titles = titles(&library, &ids)?;
            let mut cites = Vec::new();
            for (cited, contexts) in &linked {
                let (tally, sentences) = shown(&library, &chosen, paper, cited, &passages, contexts, &mut left_out)?;
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
    let in_run: Option<HashSet<&str>> = citing_ids.as_ref().map(|ids| ids.iter().map(String::as_str).collect());
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
        let (tally, sentences) = shown(&library, &chosen, citer, paper, passages, contexts, &mut left_out)?;
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
    let Query(InRun { run, community }) = query?;
    let answer = on_the_side(move || contexts_of_paper(&engine, &paper, run.as_deref(), community.as_deref())).await?;
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
            Ok(Finished::Nothing)
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
    unreadable: usize,
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
    let unreadable = library.unreadable_among(&wanted)?.len();
    Ok(Citations { papers: ids.len(), papers_read: kept.len(), without_pdf, unreadable, cases, records })
}

/// What the kinds of a community's citations come to: `CitationKindsSummary` in the contracts. Every figure is a
/// count of citing sentences, out of `tally.sentences`.
#[derive(Serialize)]
pub struct KindsSummary {
    pub papers: usize,
    /// Of its papers, whose citing sentences were read.
    pub papers_read: usize,
    pub without_pdf: usize,
    /// Of its papers, those with a PDF that could not be read.
    pub unreadable: usize,
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

/// Citing papers whose passages are held at once while a community's justified readings are checked.
const PAGES_AT_ONCE: usize = 200;

/// `verdicts` of `cases`, each justified one checked against its citing paper's pages as the paper's own view
/// checks it ([`Judgement::of`]): one whose sentence, or whose words, are no longer at their place there counts as
/// not justified. Only the papers with a justified reading have their passages loaded (those the model was paid
/// to read, not every paper of the run), a round at a time, from the request's own thread.
fn on_the_pages(library: &Library, cases: &[Case], mut verdicts: Vec<Verdict>) -> Result<Vec<Verdict>, ApiError> {
    let mut citing: Vec<&str> = cases
        .iter()
        .zip(&verdicts)
        .filter(|(_, verdict)| matches!(verdict, Verdict::Justified { .. }))
        .map(|(case, _)| case.citing.as_str())
        .collect();
    citing.sort_unstable();
    citing.dedup();
    for papers in citing.chunks(PAGES_AT_ONCE) {
        let readings: HashMap<&str, PaperReading> = papers.iter().copied().zip(library.paper_texts(papers)?).collect();
        for (case, verdict) in cases.iter().zip(verdicts.iter_mut()) {
            let Verdict::Justified { kind, .. } = verdict else { continue };
            let claimed = *kind;
            let Some(reading) = readings.get(case.citing.as_str()) else { continue };
            let on_its_page = reading.passages().is_some_and(|passages| {
                let sentence = Evidence::at(&case.citing, passages, &case.context.address);
                sentence.is_some_and(|sentence| sentence.quote() == case.context.sentence)
                    && matches!(Judgement::of(&*verdict, &case.citing, passages), Judgement::Justified { .. })
            });
            if !on_its_page {
                *verdict = Verdict::NotJustified { claimed };
            }
        }
    }
    Ok(verdicts)
}

fn summary_of(engine: &Shared, run: &str, community: Option<&str>) -> Result<KindsSummary, ApiError> {
    let library = engine.library()?;
    let chosen = engine.chosen()?;
    let model = chosen.model().to_owned();
    let found = citations_of(engine, &library, run, community)?;
    let readings = kept_readings(&library, &chosen, &found.cases)?;
    let verdicts = on_the_pages(&library, &found.cases, verdicts(&found.cases, &readings))?;
    let tallies = tallies(&found.cases, &verdicts);
    let mut tally = Tally::default();
    for verdict in &verdicts {
        tally.add(verdict);
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
        unreadable: found.unreadable,
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
    let chosen = engine.chosen()?;
    let found = citations_of(engine, library, run, community)?;
    let cases = match scope {
        Scope::Sample => {
            let sample: HashSet<String> = most_cited(&found.cases, SAMPLE).into_iter().collect();
            found.cases.into_iter().filter(|case| sample.contains(&case.cited)).collect()
        }
        Scope::Rest => found.cases,
    };
    Ok(Plan::new(&chosen, cases, &found.records, |key, case| Ok(kept_reading(library, key, case)?.is_some()))?)
}

/// `CitationKindsEstimate` in the contracts: what reading a scope's kinds would cost, before anything is sent.
#[derive(Serialize)]
pub struct KindsEstimate {
    pub scope: Scope,
    pub provider: Provider,
    pub model: String,
    /// Citing sentences in the scope.
    pub sentences: usize,
    /// Of those, read already: they cost nothing.
    pub kept: usize,
    pub to_ask: usize,
    pub estimate: BatchEstimate,
    /// Whether the model can be asked at all (its provider's key is set); never the key.
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
        let estimate = estimate_batch::<Said>(&library, &items, &plan.chosen)?;
        Ok(KindsEstimate {
            scope,
            sentences: plan.kept + plan.to_ask(),
            kept: plan.kept,
            to_ask: plan.to_ask(),
            provider: plan.chosen.provider(),
            model: plan.chosen.model().to_owned(),
            estimate,
            can_ask: plan.chosen.can_ask(),
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
    /// The most the job may spend: in US dollars, or in tokens for a model with no price.
    #[serde(alias = "accepted_usd")]
    budget_usd: Option<f64>,
    #[serde(alias = "accepted_tokens")]
    budget_tokens: Option<u64>,
    /// The model (of its provider) and the number of sentences the estimate the person saw was for: asking
    /// another model, or more sentences, is refused (estimate again).
    provider: Option<Provider>,
    model: String,
    to_ask: usize,
}

fn refused(why: Refused) -> ApiError {
    ApiError::Invalid(why.to_string())
}

async fn classify_run(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    body: Result<Json<ClassifyRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(request) = body?;
    let accepted = match Accepted::of(request.budget_usd, request.budget_tokens).map_err(refused)? {
        Accepted::Shown => return Err(ApiError::Invalid("give the budget the job may spend".to_owned())),
        accepted => accepted,
    };
    one_name(&run)?;
    let started = on_the_side(move || {
        let chosen = engine.chosen()?;
        let endpoint =
            chosen.endpoint().map_err(|why| ApiError::Invalid(format!("the model cannot be asked: {why}")))?;
        let library = engine.library()?;
        let community = tree_path(request.community.as_deref())?.map(str::to_owned);
        papers_of(&engine, &run, community.as_deref())?; // a run or community that is not there is not found now
        chosen.as_shown(request.provider, &request.model).map_err(|why| ApiError::Invalid(why.to_string()))?;
        // in dollars for a model with a price, in tokens for one without, as the estimate was shown
        let plan = plan_of(&engine, &library, &run, community.as_deref(), request.scope)?;
        // papers read, added or relinked since the estimate was shown: more to ask than was accepted is refused
        // here, before a job is started, as it is again in the job
        if plan.to_ask() > request.to_ask {
            return Err(refused(Refused::Changed));
        }
        let cost = estimate_batch::<Said>(&library, &plan.items(), &plan.chosen)?.cost;
        let terms = Terms::AtMost { usd: MOST_BUDGET_USD, tokens: MOST_BUDGET_TOKENS };
        let budget = budget(&cost, accepted, terms).map_err(refused)?;
        let shared = engine.clone();
        let transport = engine.transport.clone();
        let subject = match &community {
            Some(community) => Subject::Community { run: run.clone(), community: community.clone() },
            None => Subject::Run(run.clone()),
        };
        let about = crate::jobs::About { feature: Feature::CitationKinds, subject: subject.clone(), item: None };
        let id = engine.job.start_about("reading citation kinds", about, move |tracker| {
            let plan = plan_of(&shared, &library, &run, community.as_deref(), request.scope)
                .map_err(|error| error.to_string())?;
            if plan.chosen.as_shown(request.provider, &request.model).is_err() || plan.to_ask() > request.to_ask {
                return Err("What would be asked changed since the estimate was shown: estimate again.".to_owned());
            }
            tracker.log(&format!(
                "Reading the kinds of {} citing sentences with {} ({} read before), in {} calls.",
                plan.to_ask(),
                plan.chosen,
                plan.kept,
                plan.calls.len()
            ));
            let asking = Asking {
                library: &library,
                transport: &*transport,
                endpoint,
                recorder: &*library,
                in_flight: IN_FLIGHT,
            };
            let done = kinds::classify(asking, &plan, &budget, tracker).map_err(|error| error.to_string())?;
            tracker.log(&format!(
                "Read {} citing sentences; {} were missing from the answers, {} calls had no usable answer, {} calls \
                 were not started (the budget, a stop). {} citing sentences were not read: they stay not classified.",
                done.read, done.unanswered, done.failed, done.not_started, done.not_asked
            ));
            Ok(Finished::Feature { feature: Feature::CitationKinds, subject })
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
        let chosen = engine.chosen()?;
        let found = citations_of(&engine, &library, &run, community.as_deref())?;
        // the chosen model's readings of the scope's own citing sentences, and the calls the next run would make
        // for them (whose kept answers it would otherwise read them from again): not another model's, nor those
        // of sentences of other communities that cite the same papers
        let plan = Plan::new(&chosen, found.cases, &found.records, |_, _| Ok(false))?;
        let calls: Vec<AnswerKey> = plan.calls.iter().map(|call| call.key.clone()).collect();
        let forgotten = library.forget_keys(Feature::CitationKinds, &plan.keys)?;
        library.forget_keys(Feature::CitationKinds, &calls)?;
        Ok(forgotten)
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

    use crate::api::guard::config::tests::STRONG;
    use crate::api::guard::config::{Environment, Listen};
    use crate::api::routes::Engine;
    use crate::features::citation_contexts::kinds::{Classified, PROMPT, sentence_key};
    use crate::library::papers::passages::fixtures::pdf_with;
    use crate::library::papers::tests::paper;
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
            engine.set_claude_key("test-key-not-real-0123456789", None);
        }
        let provider = std::sync::Arc::new(crate::api::guard::login::oidc::tests::Fake::new(&[]));
        let (router, engine, _) = crate::api::application(
            &listen,
            AT.parse().unwrap(),
            engine,
            crate::api::routes::routes(),
            provider,
            || Ok(crate::api::guard::login::password::Hashing::light()),
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

    #[test]
    fn a_start_takes_the_accepted_amount_under_either_name() {
        let named = |body: Value| serde_json::from_value::<super::ClassifyRequest>(body).unwrap();
        let old = named(json!({"scope": "sample", "model": "m", "to_ask": 1, "budget_usd": 1.5}));
        let new = named(json!({"scope": "sample", "model": "m", "to_ask": 1, "accepted_usd": 1.5}));
        assert_eq!((old.budget_usd, new.budget_usd), (Some(1.5), Some(1.5)));
        assert_eq!(
            named(json!({"scope": "rest", "model": "m", "to_ask": 1, "accepted_tokens": 9})).budget_tokens,
            Some(9)
        );
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
        assert_eq!(sentence["after"]["quote"], "It is weaker than [3].");
        assert_eq!(sentence["after"]["established"]["kind"], "verified");
        assert_eq!(sentence["section"], json!({"kind": "experiments"}));
        assert_eq!(sentence["judgement"], json!({"state": "not_classified"}));
        assert_eq!(cites[1]["tally"]["not_classified"], 1);
        assert_eq!(told["coverage"], json!({"papers": 3, "read": 1, "without_pdf": 2, "unreadable": 0}));

        // the cited paper has no PDF: it is cited, and says so
        let (_, ties) = ask(&router, "GET", "/api/papers/ties/citation-contexts?run=r1", None).await;
        assert_eq!(ties["state"], "no_pdf");
        assert_eq!(ties["cited_by"][0]["paper"], "seed");
        assert_eq!(
            ties["cited_by"][0]["sentences"][0]["evidence"]["quote"],
            "We compare against the merging of [2] as a baseline."
        );
        assert_eq!(ties["cited_by_total"], 1);
        // of one community: its papers only are looked at as citing papers
        let (_, other) = ask(&router, "GET", "/api/papers/ties/citation-contexts?run=r1&community=c1", None).await;
        assert_eq!(other["cited_by_total"], 0);
        let (_, own) = ask(&router, "GET", "/api/papers/ties/citation-contexts?run=r1&community=c0", None).await;
        assert_eq!(own["cited_by_total"], 1);
        assert_eq!(
            ask(&router, "GET", "/api/papers/ties/citation-contexts?community=c0", None).await.0,
            StatusCode::BAD_REQUEST
        );

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
                key: sentence_key(&crate::model::chosen::Chosen::claude(MODEL), "seed", cited, &context.address),
                subject: Subject::Paper(cited.to_owned()),
            };
            let reading = Classified { kind, quote: quote.to_owned(), sentence: Some(context.sentence.clone()) };
            library.keep(&slot, MODEL, PROMPT, &reading).unwrap();
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

        // another model's reading of the same sentence: not the one shown, and not forgotten with it
        let context = linked.iter().find(|(paper, _)| paper == "ties").unwrap().1[0];
        let other =
            sentence_key(&crate::model::chosen::Chosen::claude("claude-sonnet-5-5"), "seed", "ties", &context.address);
        let slot =
            Slot { feature: Feature::CitationKinds, key: other.clone(), subject: Subject::Paper("ties".to_owned()) };
        let reading = Classified { kind: CitationKind::Baseline, quote: "x".to_owned(), sentence: None };
        library.keep(&slot, "claude-sonnet-5-5", PROMPT, &reading).unwrap();
        // the redo forgets the chosen model's two readings, and counts those
        let (_, forgot) = ask(&router, "DELETE", "/api/runs/r1/citation-kinds?community=c0", None).await;
        assert_eq!(forgot["forgotten"], 2);
        assert!(library.kept::<Classified>(Feature::CitationKinds, &other).unwrap().is_some(), "another model's stays");
    }

    #[tokio::test]
    async fn another_provider_is_priced_asked_in_its_own_shape_kept_and_asked_again_when_the_provider_changes() {
        use crate::api::other_providers::{
            BUDGET, Chosen as Asked, MODEL, Scripted, accepting, call, choose, finished, priced_for, router, sent_to,
        };

        let data = data();
        // the baseline is justified by words of its sentence; the other reading is a mention
        let transport = Scripted::new(|body: &str| {
            let said = if body.contains("Resolving interference") {
                json!([{"id": "1", "kind": "baseline", "quote": "We compare against the merging of"}])
            } else {
                json!([{"id": "1", "kind": "mention", "quote": ""}])
            };
            json!({"citations": said}).to_string()
        });
        let router = router(data.path(), Asked::OpenAi, &transport);
        call(&router, "GET", "/api/papers/seed/citation-contexts", None).await;
        let mut sent = 0;
        for chosen in [Asked::OpenAi, Asked::ThisComputer] {
            choose(data.path(), chosen);
            let (_, estimate) = call(&router, "GET", "/api/runs/r1/citation-kinds/estimate?scope=sample", None).await;
            assert_eq!(
                (estimate["to_ask"].clone(), estimate["can_ask"].clone()),
                (json!(2), json!(true)),
                "{estimate}"
            );
            assert_eq!(estimate["provider"], chosen.provider().id());
            priced_for(&estimate["estimate"], chosen);
            assert_eq!(transport.sent().len(), sent, "an estimate sends nothing");

            let mut asked = accepting(&estimate["estimate"], BUDGET);
            asked["scope"] = json!("sample");
            asked["to_ask"] = estimate["to_ask"].clone();
            let (status, begun) = call(&router, "POST", "/api/runs/r1/citation-kinds", Some(asked.clone())).await;
            assert_eq!(status, StatusCode::OK, "{begun}");
            let done = finished(&router, &begun["id"]).await;
            assert_eq!(done["state"], "done", "{done}");
            let made = json!({"kind": "feature", "feature": "citation_kinds", "subject": {"run": "r1"}});
            assert_eq!((done["outcome"].clone(), done["result"].clone()), (made, Value::Null), "no run to open");
            let all = transport.sent();
            assert_eq!(all.len() - sent, 2, "one call per cited paper");
            sent_to(&all[sent..], chosen);
            sent = all.len();

            let (_, summary) = call(&router, "GET", "/api/runs/r1/citation-kinds", None).await;
            assert_eq!(summary["model"], MODEL);
            assert_eq!(summary["baseline"][0]["paper"], "ties", "{summary}");
            let (_, again) = call(&router, "GET", "/api/runs/r1/citation-kinds/estimate?scope=sample", None).await;
            assert_eq!(again["to_ask"], 0, "kept: nothing left to ask");
            asked["to_ask"] = json!(0);
            let (_, begun) = call(&router, "POST", "/api/runs/r1/citation-kinds", Some(asked)).await;
            assert_eq!(finished(&router, &begun["id"]).await["state"], "done");
            assert_eq!(transport.sent().len(), sent, "kept: not asked again");
        }
    }

    #[tokio::test]
    async fn the_estimate_sends_nothing_and_asking_needs_a_key_and_a_budget() {
        let data = data();
        let (router, _) = serve(&data, false);
        ask(&router, "GET", "/api/papers/seed/citation-contexts", None).await;
        let (status, estimate) = ask(&router, "GET", "/api/runs/r1/citation-kinds/estimate?scope=sample", None).await;
        assert_eq!(status, StatusCode::OK, "{estimate}");
        assert_eq!((estimate["sentences"].clone(), estimate["to_ask"].clone()), (json!(2), json!(2)));
        assert_eq!(estimate["estimate"]["calls"], 2, "one call per cited paper");
        assert_eq!(estimate["can_ask"], false);
        assert!(estimate["estimate"]["usd"].as_f64().unwrap() > 0.0);
        let body = json!({"scope": "sample", "budget_usd": 1.0, "model": MODEL, "to_ask": 2}).to_string();
        let (status, told) = ask(&router, "POST", "/api/runs/r1/citation-kinds", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{told}");
        assert!(told["error"].as_str().unwrap().contains("cannot be asked"));

        let (router, _) = serve(&data, true);
        for budget in [0.0, -1.0, 1e9] {
            let body = json!({"scope": "rest", "budget_usd": budget, "model": MODEL, "to_ask": 2}).to_string();
            assert_eq!(
                ask(&router, "POST", "/api/runs/r1/citation-kinds", Some(body)).await.0,
                StatusCode::BAD_REQUEST
            );
        }
        let large = json!({"scope": "rest", "budget_usd": 1.0, "model": MODEL, "to_ask": 2, "community": "x".repeat(5 * 1024 * 1024)}).to_string();
        assert_eq!(
            ask(&router, "POST", "/api/runs/r1/citation-kinds", Some(large)).await.0,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let missing =
            json!({"scope": "rest", "budget_usd": 1.0, "model": MODEL, "to_ask": 2, "community": "c9"}).to_string();
        assert_eq!(ask(&router, "POST", "/api/runs/r1/citation-kinds", Some(missing)).await.0, StatusCode::NOT_FOUND);
        assert_eq!(
            ask(&router, "GET", "/api/runs/r1/citation-kinds/estimate?scope=most", None).await.0,
            StatusCode::BAD_REQUEST
        );
        let other = json!({"scope": "rest", "budget_usd": 1.0, "model": "another-model", "to_ask": 2}).to_string();
        let (status, told) = ask(&router, "POST", "/api/runs/r1/citation-kinds", Some(other)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{told}");
        assert!(told["error"].as_str().unwrap().contains("estimate before asking"));
    }

    #[tokio::test]
    async fn a_start_asking_more_sentences_than_the_estimate_shown_is_refused_before_any_job() {
        let data = data();
        let (router, _) = serve(&data, true);
        ask(&router, "GET", "/api/papers/seed/citation-contexts", None).await;
        let (_, estimate) = ask(&router, "GET", "/api/runs/r1/citation-kinds/estimate?scope=rest", None).await;
        assert_eq!(estimate["to_ask"], 2, "{estimate}");
        // shown when one sentence was to be asked; two are now (a PDF read since, a paper added)
        let body = json!({"scope": "rest", "budget_usd": 1.0, "model": MODEL, "to_ask": 1}).to_string();
        let (status, told) = ask(&router, "POST", "/api/runs/r1/citation-kinds", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{told}");
        assert!(told["error"].as_str().unwrap().contains("changed since it was shown"), "{told}");
        let (_, status) = ask(&router, "GET", "/api/status", None).await;
        assert!(!["running", "done", "failed"].contains(&status["state"].as_str().unwrap_or("")), "no job: {status}");
    }

    #[tokio::test]
    async fn a_communitys_figures_count_a_kind_justified_only_where_the_citing_papers_pages_say_its_words() {
        let data = data();
        let (router, engine) = serve(&data, false);
        ask(&router, "GET", "/api/papers/seed/citation-contexts", None).await;
        let library = engine.library().unwrap();
        let contexts = library.kept_contexts(&["seed"], Entries::InLibrary).unwrap().remove("seed").unwrap();
        let linked = contexts.in_library();
        let context = linked.iter().find(|(paper, _)| paper == "ties").unwrap().1[0].clone();
        let case = |sentence: &str| Case {
            citing: "seed".to_owned(),
            cited: "ties".to_owned(),
            context: Context { sentence: sentence.to_owned(), ..context.clone() },
        };
        let reading =
            Classified { kind: CitationKind::Baseline, quote: "compare against the merging".into(), sentence: None };
        // as the page has it: justified; a sentence kept that the page no longer says at its place: not
        let cases = [case(&context.sentence), case("We compare against the merging of [9] as a weak baseline.")];
        let judged = verdicts(&cases, &[Some(reading.clone()), Some(reading)]);
        assert!(judged.iter().all(|verdict| matches!(verdict, Verdict::Justified { .. })), "{judged:?}");
        let checked = on_the_pages(&library, &cases, judged).unwrap();
        assert!(matches!(checked[0], Verdict::Justified { kind: CitationKind::Baseline, .. }), "{checked:?}");
        assert_eq!(checked[1], Verdict::NotJustified { claimed: CitationKind::Baseline });
    }

    #[tokio::test]
    async fn every_answer_of_these_routes_fits_its_type_in_the_contracts() {
        use crate::api::contract::assert_fits;
        let data = data();
        let (router, engine) = serve(&data, false);
        ask(&router, "GET", "/api/papers/seed/citation-contexts", None).await;
        let library = engine.library().unwrap();
        let contexts = library.kept_contexts(&["seed"], Entries::InLibrary).unwrap().remove("seed").unwrap();
        let ties = contexts.in_library().into_iter().find(|(paper, _)| paper == "ties").unwrap().1[0].clone();
        let slot = Slot {
            feature: Feature::CitationKinds,
            key: sentence_key(&crate::model::chosen::Chosen::claude(MODEL), "seed", "ties", &ties.address),
            subject: Subject::Paper("ties".to_owned()),
        };
        let reading =
            Classified { kind: CitationKind::Baseline, quote: "compare against the merging".into(), sentence: None };
        library.keep(&slot, MODEL, PROMPT, &reading).unwrap();

        let (status, paper) = ask(&router, "GET", "/api/papers/seed/citation-contexts?run=r1", None).await;
        assert_eq!(status, StatusCode::OK, "{paper}");
        assert_eq!(paper["cites"][1]["sentences"][0]["judgement"]["state"], "justified", "{paper}");
        assert_fits("PaperCitationContexts", &paper, &["kinds"]);
        let (_, cited) = ask(&router, "GET", "/api/papers/ties/citation-contexts?run=r1", None).await;
        assert_eq!(cited["cited_by"][0]["paper"], "seed", "{cited}");
        assert_fits("PaperCitationContexts", &cited, &["kinds"]);
        let (status, summary) = ask(&router, "GET", "/api/runs/r1/citation-kinds", None).await;
        assert_eq!(status, StatusCode::OK, "{summary}");
        assert_eq!(summary["baseline"][0]["paper"], "ties", "{summary}");
        assert_fits("CitationKindsSummary", &summary, &["kinds", "titles"]);
        let (status, estimate) = ask(&router, "GET", "/api/runs/r1/citation-kinds/estimate?scope=sample", None).await;
        assert_eq!(status, StatusCode::OK, "{estimate}");
        assert_fits("CitationKindsEstimate", &estimate, &[]);
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
                Ok(crate::jobs::Finished::Nothing)
            })
            .unwrap();
        assert_eq!(ask(&router, "POST", "/api/runs/r1/citation-contexts", None).await.0, StatusCode::CONFLICT);
        engine.job.cancel(held).unwrap();
        finished(&router).await;
        let (status, started) = ask(&router, "POST", "/api/runs/r1/citation-contexts", None).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let status = finished(&router).await;
        assert_eq!(status["state"], "done", "{status}");
        assert_eq!(
            (status["outcome"].clone(), status["result"].clone()),
            (json!({"kind": "nothing"}), Value::Null),
            "no run to open"
        );
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
