//! Where a community's papers agree and disagree on a question ([`crate::features::stances`]), over HTTP.
//!
//! ```text
//! GET  /api/runs/{run}/communities/{community}/stances                    the questions kept for it, terms its papers
//!                                                                         keep using, the model's proposals (Listing)
//! POST /api/runs/{run}/communities/{community}/stances/estimate           {question, include_unrelated}: the plan
//!                                                                         and its cost; nothing is sent (Plan)
//! POST /api/runs/{run}/communities/{community}/stances/ask                {question, include_unrelated, part}: the job
//! GET  /api/runs/{run}/communities/{community}/stances/{id}               what is known so far (StanceResult)
//! POST /api/runs/{run}/communities/{community}/stances/{id}/redo          ask that question again from scratch
//! POST /api/runs/{run}/communities/{community}/stances/proposals/estimate the cost of proposing questions
//! POST /api/runs/{run}/communities/{community}/stances/proposals          the job that proposes them
//! ```
//!
//! and the same for a set of marked papers, which travels in the body: `POST /api/runs/{run}/marked/stances/list`,
//! `.../marked/stances/estimate`, `.../marked/stances/ask` (each with `papers`), `GET .../marked/stances/{id}`,
//! `POST .../marked/stances/{id}/redo`.
//!
//! Nothing is sent to the model but by `ask` and `proposals`, which start a tracked job (its progress and stop
//! button are those of `/api/status`), capped by a budget of the most the estimate said it could cost. What is
//! answered is kept per paper, model and question, so a question asked again, or about a sub-community of papers
//! already asked, costs nothing. The questions asked of a community are kept with it (in the kept answers, under
//! the community), so they can be opened again without paying.
//!
//! A finished job says it was this feature's, about the community (or the run, for marked papers): the interface
//! opens no run for it, and the stances view shows what was answered.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::routing::{MethodRouter, get, post};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::api::routes::{ApiError, Shared, on_the_side, one_name};
use crate::evidence::Papers;
use crate::features::stances::{
    self, Placed, Prepared, Proposals, Read, Record, Recurring, Relevance, SAMPLE, Said, Shown,
};
use crate::jobs::{Finished, Tracker};
use crate::library::core::store::read_json;
use crate::library::{AnswerKey, Feature, Library, Paper, Slot, Subject};
use crate::model::batch::{self, Asking, BatchEstimate, Failure, IN_FLIGHT, Item, Outcome, Stopped};
use crate::model::chosen::{Accepted, Chosen, Terms, budget};
use crate::model::language::{Budget, Provider};

/// The most marked papers one question may be about.
pub const MOST_MARKED: usize = 2_000;
/// Terms suggested without a model.
const SUGGESTIONS: usize = 8;
/// The version of how the questions of a community are kept.
const LIST: u32 = 1;

/// The routes of this module, for [`crate::api::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![
        ("/api/runs/{run}/communities/{community}/stances", get(community_listing)),
        ("/api/runs/{run}/communities/{community}/stances/estimate", post(community_estimate)),
        ("/api/runs/{run}/communities/{community}/stances/ask", post(community_ask)),
        ("/api/runs/{run}/communities/{community}/stances/proposals/estimate", post(proposals_estimate)),
        ("/api/runs/{run}/communities/{community}/stances/proposals", post(proposals_ask)),
        ("/api/runs/{run}/communities/{community}/stances/{id}", get(community_result)),
        ("/api/runs/{run}/communities/{community}/stances/{id}/redo", post(community_redo)),
        ("/api/runs/{run}/marked/stances/list", post(marked_listing)),
        ("/api/runs/{run}/marked/stances/estimate", post(marked_estimate)),
        ("/api/runs/{run}/marked/stances/ask", post(marked_ask)),
        ("/api/runs/{run}/marked/stances/{id}", get(marked_result)),
        ("/api/runs/{run}/marked/stances/{id}/redo", post(marked_redo)),
    ]
}

/// The papers a question is about: a community of the run (by its tree path), or papers the user marked.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Scope {
    Community(String),
    Marked(Vec<String>),
}

impl Scope {
    /// What the questions of this scope are kept under, with the run.
    fn subject(&self, run: &str) -> Subject {
        match self {
            Scope::Community(tree_path) => Subject::Community { run: run.to_owned(), community: tree_path.clone() },
            Scope::Marked(_) => Subject::Run(run.to_owned()),
        }
    }

    fn label(&self) -> &str {
        match self {
            Scope::Community(tree_path) => tree_path,
            Scope::Marked(_) => "marked papers",
        }
    }
}

/// The run's graph as the pipeline writes it: only what this needs.
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
    acceptance: Option<String>,
}

#[derive(Deserialize)]
struct Tree {
    communities: Vec<Branch>,
}

#[derive(Deserialize)]
struct Branch {
    tree_path: String,
    members: Vec<usize>,
    #[serde(default)]
    children: Vec<Branch>,
}

fn find<'a>(branches: &'a [Branch], tree_path: &str) -> Option<&'a Branch> {
    branches
        .iter()
        .find_map(|branch| if branch.tree_path == tree_path { Some(branch) } else { find(&branch.children, tree_path) })
}

/// A scope's papers, read from the run: most central first, with their records.
struct InScope {
    scope: Scope,
    /// Every paper of the run, by graph index.
    ids: Vec<String>,
    acceptance: Vec<Option<String>>,
    /// The scope's papers as graph indices, most central first, and their records (None when not in the library).
    order: Vec<usize>,
    records: Vec<Option<Paper>>,
}

impl InScope {
    fn records(&self) -> Vec<Record<'_>> {
        self.records
            .iter()
            .map(|record| match record {
                Some(paper) => Record { title: &paper.title, abstract_text: &paper.r#abstract },
                None => Record { title: "", abstract_text: "" },
            })
            .collect()
    }

    fn id(&self, position: usize) -> &str {
        &self.ids[self.order[position]]
    }
}

fn read_run(engine: &Shared, library: &Library, run: &str, scope: Scope) -> Result<InScope, ApiError> {
    let folder = engine.data.join("runs").join(one_name(run)?);
    let graph: Graph = read_json(&folder.join("graph.json"))?.ok_or(ApiError::NotFound)?;
    let ids: Vec<String> = graph.nodes.iter().map(|node| node.id.clone()).collect();
    let members: Vec<usize> = match &scope {
        Scope::Community(tree_path) => {
            let tree: Tree = read_json(&folder.join("communities.json"))?.ok_or(ApiError::NotFound)?;
            let branch = find(&tree.communities, tree_path).ok_or(ApiError::NotFound)?;
            branch.members.iter().copied().filter(|member| *member < ids.len()).collect()
        }
        Scope::Marked(papers) => {
            let at: HashMap<&str, usize> = ids.iter().enumerate().map(|(index, id)| (id.as_str(), index)).collect();
            papers
                .iter()
                .map(|paper| {
                    at.get(paper.as_str())
                        .copied()
                        .ok_or_else(|| ApiError::Invalid("a marked paper is not one of this run's papers".to_owned()))
                })
                .collect::<Result<_, _>>()?
        }
    };
    let order = stances::central_first(&members, &graph.edges, &ids);
    let wanted: Vec<&str> = order.iter().map(|index| ids[*index].as_str()).collect();
    let records = library.papers(&wanted)?;
    Ok(InScope {
        scope,
        acceptance: graph.nodes.into_iter().map(|node| node.acceptance).collect(),
        ids,
        order,
        records,
    })
}

/// A question as it was asked of a scope, kept so that it can be listed and opened again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asked {
    pub id: String,
    pub question: String,
    /// Raised by a redo: the answers of earlier rounds are not looked at again.
    pub round: u32,
    pub include_unrelated: bool,
    /// For marked papers: which ones. Empty for a community.
    #[serde(default)]
    pub papers: Vec<String>,
    pub asked_at: String,
}

/// Everything kept about a scope's questions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Questions {
    questions: Vec<Asked>,
    #[serde(default)]
    proposal_round: u32,
    /// The ids of questions listed whose jobs have kept no answer yet: those that may make room for a new one
    /// ([`make_room`]). A question kept before this was written down is taken as answered, and never removed.
    #[serde(default)]
    unanswered: Vec<String>,
}

/// The most questions kept for one scope. Past it, the oldest questions with no answer kept are taken off the list
/// (the job's log says so); when every one has answers, a new question is refused rather than one with answers
/// dropped.
pub const MOST_QUESTIONS: usize = 200;

/// Take the oldest questions with no answer kept off `kept`'s list until it holds at most [`MOST_QUESTIONS`],
/// leaving the newest (just asked) where it is: how many went, or `None` when no room could be made.
fn make_room(kept: &mut Questions) -> Option<usize> {
    let mut removed = 0;
    while kept.questions.len() > MOST_QUESTIONS {
        // newest first: the last one without an answer is the oldest, and the first is the one just asked
        let place = kept.questions.iter().skip(1).rposition(|asked| kept.unanswered.contains(&asked.id))? + 1;
        let gone = kept.questions.remove(place);
        kept.unanswered.retain(|id| *id != gone.id);
        removed += 1;
    }
    Some(removed)
}

/// One change to a scope's questions at a time: each is a read, a change and a write.
static CHANGING: Mutex<()> = Mutex::new(());

/// One question started at a time, from reading its round to its job's start (or its refusal): so that the key its
/// budget was worked out under is the one it is asked under, and a refused start can take back its listing.
static STARTING: Mutex<()> = Mutex::new(());

fn list_slot(run: &str, scope: &Scope) -> Slot {
    let question = format!("questions\n{run}\n{}", scope.label());
    Slot {
        feature: Feature::Stances,
        key: AnswerKey::of_feature(Feature::Stances, LIST, &[], &question),
        subject: scope.subject(run),
    }
}

fn questions(library: &Library, run: &str, scope: &Scope) -> Result<Questions, ApiError> {
    let slot = list_slot(run, scope);
    Ok(library.kept::<Questions>(Feature::Stances, &slot.key)?.map(|kept| kept.answer).unwrap_or_default())
}

fn change_questions<T>(
    library: &Library,
    run: &str,
    scope: &Scope,
    change: impl FnOnce(&mut Questions) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    let _one_at_a_time = CHANGING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut kept = questions(library, run, scope)?;
    let changed = change(&mut kept)?;
    library.keep(&list_slot(run, scope), "", LIST, &kept)?;
    Ok(changed)
}

fn now() -> String {
    DateTime::<Utc>::from(SystemTime::now()).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A question's id: the same question about the same papers is one question.
fn question_id(question: &str, papers: &[String]) -> String {
    let mut sorted: Vec<&str> = papers.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let hash = Sha256::digest(format!("{question}\n{}", sorted.join("\n")).as_bytes());
    hash.iter().take(8).map(|byte| format!("{byte:02x}")).collect()
}

/// Which of a scope's papers would be asked: the pre-filter's verdict and the order.
struct Plan {
    relevance: Relevance,
    /// Positions in the scope's order of the papers to ask, most central first.
    candidates: Vec<usize>,
    include_unrelated: bool,
    /// The scope's papers the library has.
    present: usize,
}

impl Plan {
    /// Only the papers the library has are read and counted: one it lacks can be neither asked nor skipped.
    fn of(papers: &InScope, question: &str, include_unrelated: bool) -> Plan {
        let present: Vec<usize> =
            (0..papers.order.len()).filter(|position| papers.records[*position].is_some()).collect();
        let records = papers.records();
        let kept: Vec<Record<'_>> = present.iter().map(|position| records[*position]).collect();
        let found = stances::relevance(question, &kept);
        let mut related = vec![false; papers.order.len()];
        for (position, is) in present.iter().zip(&found.related) {
            related[*position] = *is;
        }
        let candidates = present.iter().copied().filter(|position| include_unrelated || related[*position]).collect();
        Plan {
            relevance: Relevance { terms: found.terms, related },
            candidates,
            include_unrelated,
            present: present.len(),
        }
    }

    fn related(&self) -> usize {
        self.relevance.related.iter().filter(|related| **related).count()
    }

    fn part(&self, part: Part) -> &[usize] {
        match part {
            Part::Sample => &self.candidates[..self.candidates.len().min(SAMPLE)],
            Part::All => &self.candidates,
        }
    }
}

/// Which of the papers to ask: the most central first, then the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Part {
    Sample,
    All,
}

/// What the interface sends to estimate or ask a question.
#[derive(Deserialize)]
struct QuestionBody {
    question: String,
    #[serde(default)]
    include_unrelated: bool,
    #[serde(default)]
    papers: Vec<String>,
}

#[derive(Deserialize)]
struct AskBody {
    #[serde(flatten)]
    question: QuestionBody,
    part: Part,
    #[serde(flatten)]
    approved: Approved,
}

/// What the user was shown and clicked: the model (of its provider) and the most it could cost, in dollars, or in
/// tokens for a model with no price. A job that would now cost more, or ask another model, is refused rather than
/// run on terms the user did not see.
const NOTHING_ACCEPTED: &str = "say what was accepted of the estimate shown (accepted_usd or accepted_tokens)";

#[derive(Clone, Debug, Deserialize)]
struct Approved {
    #[serde(default)]
    provider: Option<Provider>,
    model: String,
    /// The estimate's "at most", in US dollars; null when the model's price is not known.
    #[serde(alias = "approved_usd", alias = "budget_usd")]
    accepted_usd: Option<f64>,
    /// For a model with no price: the estimate's "at most", in tokens.
    #[serde(default, alias = "budget_tokens")]
    accepted_tokens: Option<u64>,
}

impl Approved {
    fn check_model(&self, chosen: &Chosen) -> Result<(), ApiError> {
        chosen.as_shown(self.provider, &self.model).map_err(|why| ApiError::Invalid(why.to_string()))
    }

    /// The budget of a batch whose estimate is `estimate`, if it is within what was approved: the worst case shown.
    fn budget(&self, estimate: &BatchEstimate) -> Result<Budget, String> {
        let accepted = Accepted::of(self.accepted_usd, self.accepted_tokens).map_err(|why| why.to_string())?;
        // naming nothing is not accepting: for a model with no price (a price shown is refused unless sent back) it
        // would run on a worst case never agreed to
        if accepted == Accepted::Shown && estimate.cost.calls > 0 && estimate.cost.most_usd.is_none() {
            return Err(NOTHING_ACCEPTED.to_owned());
        }
        budget(&estimate.cost, accepted, Terms::AsShown).map_err(|why| why.to_string())
    }
}

#[derive(Deserialize)]
struct Marked {
    papers: Vec<String>,
}

fn marked(papers: Vec<String>) -> Result<Scope, ApiError> {
    if papers.is_empty() {
        return Err(ApiError::Invalid("mark at least one paper first".to_owned()));
    }
    if papers.len() > MOST_MARKED {
        return Err(ApiError::Invalid(format!("a question can be about at most {MOST_MARKED} marked papers")));
    }
    let mut papers = papers;
    papers.sort_unstable();
    papers.dedup();
    Ok(Scope::Marked(papers))
}

fn checked(question: &str) -> Result<String, ApiError> {
    stances::checked_question(question).map_err(|why| ApiError::Invalid(why.to_string()))
}

/// The papers about which the pre-filter decided, said so the user can judge it.
#[derive(Serialize)]
pub struct Filtered {
    /// The scope's papers the library has: `related` and `unrelated` add up to it.
    pub papers: usize,
    /// The scope's papers the library does not have: neither asked nor skipped, with `papers` the scope's total.
    pub not_in_library: usize,
    /// Papers sharing a distinctive word with the question.
    pub related: usize,
    /// Papers sharing none: skipped unless `include_unrelated`.
    pub unrelated: usize,
    pub include_unrelated: bool,
    /// The words looked for (folded: "model" for "models").
    pub terms: Vec<String>,
}

fn filtered(plan: &Plan) -> Filtered {
    Filtered {
        papers: plan.present,
        not_in_library: plan.relevance.related.len() - plan.present,
        related: plan.related(),
        unrelated: plan.present - plan.related(),
        include_unrelated: plan.include_unrelated,
        terms: plan.relevance.terms.clone(),
    }
}

/// One part's cost, before anything is sent.
#[derive(Serialize)]
pub struct PartEstimate {
    pub papers: usize,
    /// Of them, read from their PDF and from their abstract only.
    pub with_pdf: usize,
    pub abstract_only: usize,
    pub estimate: BatchEstimate,
}

/// What asking a question would cost: `StancePlan` in the contracts. An estimate, counted from characters.
#[derive(Serialize)]
pub struct StancePlan {
    pub id: String,
    pub question: String,
    pub provider: Provider,
    pub model: String,
    /// Whether the model can be asked at all (its provider's key is set); never the key.
    pub api_key_set: bool,
    pub filter: Filtered,
    /// The most central papers, asked first.
    pub sample: PartEstimate,
    /// All the papers to ask (those of the sample already answered are counted as kept).
    pub all: PartEstimate,
}

/// The texts of the papers at `positions`, prepared as they are sent, in that order.
fn prepared(library: &Library, papers: &InScope, positions: &[usize]) -> Result<Vec<Prepared>, ApiError> {
    let ids: Vec<&str> = positions.iter().map(|position| papers.id(*position)).collect();
    let readings = library.paper_texts(&ids)?;
    Ok(ids.iter().zip(&readings).filter_map(|(id, reading)| stances::prepare(id, reading)).collect())
}

fn part_estimate(
    library: &Library,
    prepared: &[Prepared],
    question: &str,
    key: &str,
    chosen: &Chosen,
) -> Result<PartEstimate, ApiError> {
    let schema = stances::schema();
    let prompt = stances::question_prompt(question);
    let items = stances::items(prepared, &prompt, key, chosen, &schema);
    Ok(PartEstimate {
        papers: prepared.len(),
        with_pdf: prepared.iter().filter(|paper| paper.read == Read::Pdf).count(),
        abstract_only: prepared.iter().filter(|paper| paper.read == Read::Abstract).count(),
        estimate: batch::estimate_batch::<Said>(library, &items, chosen)?,
    })
}

/// The round a question is asked in now (0 for one never asked).
fn round_of(library: &Library, run: &str, scope: &Scope, id: &str) -> Result<u32, ApiError> {
    Ok(questions(library, run, scope)?.questions.iter().find(|asked| asked.id == id).map_or(0, |asked| asked.round))
}

fn scope_papers(scope: &Scope) -> &[String] {
    match scope {
        Scope::Community(_) => &[],
        Scope::Marked(papers) => papers,
    }
}

fn estimate(engine: &Shared, run: &str, scope: Scope, asked: QuestionBody) -> Result<StancePlan, ApiError> {
    let question = checked(&asked.question)?;
    let library = engine.library()?;
    let papers = read_run(engine, &library, run, scope)?;
    let plan = Plan::of(&papers, &question, asked.include_unrelated);
    let id = question_id(&question, scope_papers(&papers.scope));
    let chosen = engine.reading()?;
    let key = stances::key_question(&question, round_of(&library, run, &papers.scope, &id)?);
    let all = prepared(&library, &papers, plan.part(Part::All))?;
    let sample = &all[..all.len().min(SAMPLE)];
    Ok(StancePlan {
        sample: part_estimate(&library, sample, &question, &key, &chosen)?,
        all: part_estimate(&library, &all, &question, &key, &chosen)?,
        filter: filtered(&plan),
        id,
        question,
        provider: chosen.provider(),
        model: chosen.model().to_owned(),
        api_key_set: chosen.can_ask(),
    })
}

/// The budget `approved` gives `items`, worked out before any job is started: a budget refused (the estimate grew
/// past what was accepted, or nothing was accepted) is the request's 400, never a job that fails.
fn budget_for<T: serde::de::DeserializeOwned>(
    library: &Library,
    chosen: &Chosen,
    items: &[Item<'_>],
    approved: &Approved,
) -> Result<Budget, ApiError> {
    let estimate = batch::estimate_batch::<T>(library, items, chosen)?;
    approved.budget(&estimate).map_err(ApiError::Invalid)
}

/// What came of asking a batch.
struct Came {
    /// Items answered now or before.
    answered: usize,
    /// Why the job fails, when it does.
    failing: Option<String>,
}

/// Ask `items`, kept answers skipped, within `budget`. The job fails, with why, when the model could not be asked
/// (a refused key, billing, the budget) or answered none of what was asked: the interface shows a failed job's
/// error, not a finished one's log.
fn ask_items<T>(
    engine: &Shared,
    library: &Library,
    chosen: &Chosen,
    items: &[Item<'_>],
    budget: &Budget,
    tracker: &Tracker<'_>,
) -> Result<Came, String>
where
    T: serde::de::DeserializeOwned + Serialize + Send + Clone,
{
    let endpoint = chosen.endpoint().map_err(|why| why.to_string())?;
    let asking = Asking { library, transport: &*engine.transport, endpoint, recorder: library, in_flight: IN_FLIGHT };
    let outcomes = batch::ask_all::<T>(asking, items, budget, tracker).map_err(|error| error.to_string())?;
    let count = |wanted: fn(&Outcome<T>) -> bool| outcomes.iter().filter(|outcome| wanted(outcome)).count();
    let stopped = outcomes.iter().find_map(|outcome| match outcome {
        Outcome::NotStarted(Stopped::Cancelled) => None,
        Outcome::NotStarted(why) => Some(why.to_string()),
        Outcome::Failed(Failure::Unavailable(why)) => Some(why.to_string()),
        _ => None,
    });
    let answered = count(|outcome| matches!(outcome, Outcome::Answered(_) | Outcome::Kept(_)));
    let failed = count(|outcome| matches!(outcome, Outcome::Failed(_)));
    let failing = stopped
        .clone()
        .or_else(|| (answered == 0 && failed > 0).then(|| "the model gave no usable answer for any paper".to_owned()));
    let line = format!(
        "{} answered now, {} kept from before, {} without a usable answer, {} not asked{}",
        count(|outcome| matches!(outcome, Outcome::Answered(_))),
        count(|outcome| matches!(outcome, Outcome::Kept(_))),
        count(|outcome| matches!(outcome, Outcome::Failed(_))),
        count(|outcome| matches!(outcome, Outcome::NotStarted(_))),
        stopped.map(|why| format!(" ({why})")).unwrap_or_default(),
    );
    tracker.log(&line);
    // what was answered is kept all the same
    let failing = failing.map(|why| format!("{why} ({line})"));
    Ok(Came { answered, failing })
}

/// `{job}`: the number of the job started, for `/api/status` and its stop button; and the question's id.
#[derive(Serialize)]
pub struct Started {
    pub job: u64,
    pub id: String,
}

fn ask(
    engine: &Shared,
    run: &str,
    scope: Scope,
    asked: QuestionBody,
    part: Part,
    approved: Approved,
) -> Result<Started, ApiError> {
    let question = checked(&asked.question)?;
    let chosen = engine.reading()?;
    chosen.endpoint().map_err(|why| ApiError::Invalid(format!("the model cannot be asked: {why}")))?;
    let library = engine.library()?;
    let papers = read_run(engine, &library, run, scope)?;
    let plan = Plan::of(&papers, &question, asked.include_unrelated);
    let id = question_id(&question, scope_papers(&papers.scope));
    approved.check_model(&chosen)?;
    let ids: Vec<String> = plan.part(part).iter().map(|position| papers.id(*position).to_owned()).collect();
    let wanted: Vec<&str> = ids.iter().map(String::as_str).collect();
    let readings = library.paper_texts(&wanted)?;
    let prepared: Vec<Prepared> =
        wanted.iter().zip(&readings).filter_map(|(id, reading)| stances::prepare(id, reading)).collect();
    let (schema, prompt) = (stances::schema(), stances::question_prompt(&question));
    let _one = STARTING.lock().unwrap_or_else(PoisonError::into_inner);
    let round = round_of(&library, run, &papers.scope, &id)?;
    let key = stances::key_question(&question, round);
    // the budget before anything is listed or started: refused, it is this request's answer
    let budget =
        budget_for::<Said>(&library, &chosen, &stances::items(&prepared, &prompt, &key, &chosen, &schema), &approved)?;
    let (new, removed) = change_questions(&library, run, &papers.scope, |kept| {
        let include_unrelated = asked.include_unrelated;
        if let Some(known) = kept.questions.iter_mut().find(|known| known.id == id) {
            known.include_unrelated = include_unrelated;
            return Ok((false, 0));
        }
        kept.questions.insert(
            0,
            Asked {
                id: id.clone(),
                question: question.clone(),
                round,
                include_unrelated,
                papers: scope_papers(&papers.scope).to_vec(),
                asked_at: now(),
            },
        );
        kept.unanswered.push(id.clone());
        let removed = make_room(kept).ok_or_else(|| {
            ApiError::Invalid(format!(
                "at most {MOST_QUESTIONS} questions are kept for these papers, and each has answers kept: ask one of \
                 them again instead"
            ))
        })?;
        Ok((true, removed))
    })?;
    let worker = Shared::clone(engine);
    let subject = papers.scope.subject(run);
    let (scope, owned_run, question_id) = (papers.scope.clone(), run.to_owned(), id.clone());
    let about = crate::jobs::About { feature: Feature::Stances, subject: subject.clone(), item: Some(id.clone()) };
    let started = engine.job.start_about("asking where papers stand on a question", about, move |tracker| {
        if removed > 0 {
            tracker.log(&format!(
                "{removed} older questions with no answer kept were taken off the list, to keep at most \
                 {MOST_QUESTIONS}"
            ));
        }
        let library = worker.library().map_err(|error| error.to_string())?;
        tracker.log(&format!("Reading {} papers for: {question}", ids.len()));
        let items = stances::items(&prepared, &prompt, &key, &chosen, &schema);
        let came = ask_items::<Said>(&worker, &library, &chosen, &items, &budget, tracker)?;
        if came.answered > 0 {
            change_questions(&library, &owned_run, &scope, |kept| {
                kept.unanswered.retain(|known| *known != question_id);
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        }
        match came.failing {
            Some(why) => Err(why),
            None => Ok(Finished::Feature { feature: Feature::Stances, subject }),
        }
    });
    match started {
        Ok(job) => Ok(Started { job, id }),
        Err(busy) => {
            // nothing was asked: a question listed only now is taken off again
            if new {
                change_questions(&library, run, &papers.scope, |kept| {
                    kept.questions.retain(|known| known.id != id);
                    kept.unanswered.retain(|known| *known != id);
                    Ok(())
                })?;
            }
            Err(busy.into())
        }
    }
}

/// One paper as it is placed: `StanceRow` in the contracts.
#[derive(Serialize)]
pub struct Row {
    pub paper: String,
    pub title: String,
    /// The conference's tier: oral, spotlight, poster; null for a paper of no conference.
    pub tier: Option<String>,
    pub year: Option<u32>,
    pub read: Read,
    pub placed: Placed,
    /// Among the most central papers, asked first.
    pub in_sample: bool,
}

/// What is known so far about a question: `StanceResult` in the contracts. Every placement is the model's reading
/// of one paper, shown only with quotes found in that paper.
#[derive(Serialize)]
pub struct StanceResult {
    pub id: String,
    pub question: String,
    pub asked_at: String,
    pub provider: Provider,
    pub model: String,
    pub filter: Filtered,
    /// Papers that would be asked (related ones, or all when unrelated ones are included).
    pub to_ask: usize,
    /// Of them, those with an answer kept.
    pub answered: usize,
    /// Of them, those asked as the sample.
    pub sample: usize,
    /// Papers placed, most central first.
    pub rows: Vec<Row>,
}

fn result(engine: &Shared, run: &str, scope: &Scope, id: &str) -> Result<StanceResult, ApiError> {
    let library = engine.library()?;
    let asked = questions(&library, run, scope)?
        .questions
        .into_iter()
        .find(|asked| asked.id == id)
        .ok_or(ApiError::NotFound)?;
    let scope = match scope {
        Scope::Community(_) => scope.clone(),
        Scope::Marked(_) => Scope::Marked(asked.papers.clone()),
    };
    let papers = read_run(engine, &library, run, scope)?;
    let plan = Plan::of(&papers, &asked.question, asked.include_unrelated);
    let chosen = engine.reading()?;
    let key = stances::key_question(&asked.question, asked.round);
    // an answer kept for either reading first (cheap), then the papers' texts only for those; the answer shown is
    // the one of what the paper reads as now
    let kept = |position: usize, read: Read| -> Result<Option<Said>, ApiError> {
        let key = stances::answer_key(&chosen, papers.id(position), read, &key);
        Ok(library.kept::<Said>(Feature::Stances, &key)?.map(|kept| kept.answer))
    };
    let mut answered: Vec<(usize, Option<Said>, Option<Said>)> = Vec::new();
    for position in plan.part(Part::All) {
        let (pdf, from_abstract) = (kept(*position, Read::Pdf)?, kept(*position, Read::Abstract)?);
        if pdf.is_some() || from_abstract.is_some() {
            answered.push((*position, pdf, from_abstract));
        }
    }
    let ids: Vec<&str> = answered.iter().map(|(position, ..)| papers.id(*position)).collect();
    let readings = library.paper_texts(&ids)?;
    let sample: Vec<usize> = plan.part(Part::Sample).to_vec();
    let rows: Vec<Row> = answered
        .iter()
        .zip(readings)
        .filter_map(|((position, pdf, from_abstract), reading)| {
            let (read, said) = match reading.passages() {
                Some(_) => (Read::Pdf, pdf.as_ref()?),
                None => (Read::Abstract, from_abstract.as_ref()?),
            };
            let text = reading.into_text()?;
            let record = papers.records[*position].as_ref()?;
            let paper = papers.id(*position);
            Some(Row {
                paper: paper.to_owned(),
                title: record.title.clone(),
                tier: papers.acceptance[papers.order[*position]].clone(),
                year: record.published.get(..4).and_then(|year| year.parse().ok()),
                read,
                placed: stances::place(paper, said, &text),
                in_sample: sample.contains(position),
            })
        })
        .collect();
    Ok(StanceResult {
        id: asked.id,
        question: asked.question,
        asked_at: asked.asked_at,
        provider: chosen.provider(),
        model: chosen.model().to_owned(),
        filter: filtered(&plan),
        to_ask: plan.candidates.len(),
        answered: rows.len(),
        sample: sample.len(),
        rows,
    })
}

fn redo(engine: &Shared, run: &str, scope: &Scope, id: &str) -> Result<Asked, ApiError> {
    let library = engine.library()?;
    // not while a question is being started: its budget was worked out under the round it read
    let _one = STARTING.lock().unwrap_or_else(PoisonError::into_inner);
    change_questions(&library, run, scope, |kept| {
        let asked = kept.questions.iter_mut().find(|asked| asked.id == id).ok_or(ApiError::NotFound)?;
        asked.round += 1;
        Ok(asked.clone())
    })
}

/// The model's proposals as shown: those whose quotes were found in two papers' abstracts.
#[derive(Serialize)]
pub struct ShownProposals {
    #[serde(flatten)]
    pub shown: Shown,
    pub model: String,
    pub kept_at: String,
}

/// A scope's questions and what may be asked of it: `StanceListing` in the contracts.
#[derive(Serialize)]
pub struct Listing {
    pub papers: usize,
    /// Newest first.
    pub questions: Vec<Asked>,
    /// Words its papers keep using, counted without a model.
    pub suggestions: Vec<Recurring>,
    /// The model's proposals, when they were asked for (communities only).
    pub proposals: Option<ShownProposals>,
}

/// The papers proposals are asked from: the most central, by id, with their records.
fn proposal_papers(papers: &InScope) -> Vec<(&str, Record<'_>)> {
    let records = papers.records();
    (0..papers.order.len())
        .filter(|position| papers.records[*position].is_some())
        .take(stances::PROPOSE_PAPERS)
        .map(|position| (papers.id(position), records[position]))
        .collect()
}

fn proposal_slot_items<'a>(
    papers: &[(&str, Record<'_>)],
    shared: &'a str,
    prompt: &'a str,
    round: u32,
    chosen: &'a Chosen,
    schema: &'a serde_json::Value,
    subject: Subject,
) -> Vec<Item<'a>> {
    let ids: Vec<&str> = papers.iter().map(|(id, _)| *id).collect();
    stances::proposal_items(&ids, shared, prompt, round, chosen, schema, subject)
}

fn listing(engine: &Shared, run: &str, scope: Scope) -> Result<Listing, ApiError> {
    let library = engine.library()?;
    let papers = read_run(engine, &library, run, scope)?;
    let kept = questions(&library, run, &papers.scope)?;
    let questions = match &papers.scope {
        Scope::Community(_) => kept.questions.clone(),
        Scope::Marked(marked) => kept.questions.iter().filter(|asked| &asked.papers == marked).cloned().collect(),
    };
    // the run's papers, for what is distinctive of these
    let all: Vec<&str> = papers.ids.iter().map(String::as_str).collect();
    let everyone: Vec<Paper> = library.papers(&all)?.into_iter().flatten().collect();
    let run_records: Vec<Record<'_>> =
        everyone.iter().map(|paper| Record { title: &paper.title, abstract_text: &paper.r#abstract }).collect();
    let suggestions = stances::recurring_terms(&papers.records(), &run_records, SUGGESTIONS);
    let proposals = match &papers.scope {
        Scope::Marked(_) => None,
        Scope::Community(_) => {
            let sent = proposal_papers(&papers);
            let chosen = engine.reading()?;
            let shared = stances::proposal_text(&sent);
            let prompt = stances::proposal_prompt();
            let schema = stances::proposal_schema();
            let subject = papers.scope.subject(run);
            let items = proposal_slot_items(&sent, &shared, &prompt, kept.proposal_round, &chosen, &schema, subject);
            library.kept::<Proposals>(Feature::Stances, &items[0].slot.key)?.map(|found| {
                let texts: Papers =
                    sent.iter().map(|(id, record)| ((*id).to_owned(), stances::abstract_text(record))).collect();
                ShownProposals {
                    shown: stances::proposals(found.answer, &texts),
                    model: found.model,
                    kept_at: found.kept_at,
                }
            })
        }
    };
    Ok(Listing { papers: papers.order.len(), questions, suggestions, proposals })
}

/// What proposing questions would cost: `ProposalPlan` in the contracts. An estimate.
#[derive(Serialize)]
pub struct ProposalPlan {
    pub papers: usize,
    pub provider: Provider,
    pub model: String,
    pub estimate: BatchEstimate,
    /// Whether the model can be asked at all (its provider's key is set); never the key.
    pub api_key_set: bool,
}

/// Proposing: `fresh` to propose anew, past the proposals kept (which stay shown until the new ones are asked for).
#[derive(Deserialize)]
struct ProposalBody {
    #[serde(default)]
    fresh: bool,
}

#[derive(Deserialize)]
struct ProposeBody {
    #[serde(default)]
    fresh: bool,
    #[serde(flatten)]
    approved: Approved,
}

fn proposal_round(library: &Library, run: &str, scope: &Scope, fresh: bool) -> Result<u32, ApiError> {
    Ok(questions(library, run, scope)?.proposal_round + u32::from(fresh))
}

fn proposal_plan(engine: &Shared, run: &str, community: &str, fresh: bool) -> Result<ProposalPlan, ApiError> {
    let library = engine.library()?;
    let scope = Scope::Community(community.to_owned());
    let papers = read_run(engine, &library, run, scope)?;
    let round = proposal_round(&library, run, &papers.scope, fresh)?;
    let sent = proposal_papers(&papers);
    let chosen = engine.reading()?;
    let shared = stances::proposal_text(&sent);
    let prompt = stances::proposal_prompt();
    let schema = stances::proposal_schema();
    let items = proposal_slot_items(&sent, &shared, &prompt, round, &chosen, &schema, papers.scope.subject(run));
    let estimate = batch::estimate_batch::<Proposals>(&library, &items, &chosen)?;
    Ok(ProposalPlan {
        papers: sent.len(),
        provider: chosen.provider(),
        model: chosen.model().to_owned(),
        estimate,
        api_key_set: chosen.can_ask(),
    })
}

fn propose(engine: &Shared, run: &str, community: &str, body: ProposeBody) -> Result<Started, ApiError> {
    let chosen = engine.reading()?;
    chosen.endpoint().map_err(|why| ApiError::Invalid(format!("the model cannot be asked: {why}")))?;
    let library = engine.library()?;
    let scope = Scope::Community(community.to_owned());
    let papers = read_run(engine, &library, run, scope)?;
    let round = proposal_round(&library, run, &papers.scope, body.fresh)?;
    let approved = body.approved;
    let sent: Vec<(String, String, String)> = proposal_papers(&papers)
        .into_iter()
        .map(|(id, record)| (id.to_owned(), record.title.to_owned(), record.abstract_text.to_owned()))
        .collect();
    let subject = papers.scope.subject(run);
    let scope = papers.scope.clone();
    let run = run.to_owned();
    let fresh = body.fresh;
    approved.check_model(&chosen)?;
    let budget = {
        let records: Vec<(&str, Record<'_>)> =
            sent.iter().map(|(id, title, abstract_text)| (id.as_str(), Record { title, abstract_text })).collect();
        let shared = stances::proposal_text(&records);
        let (prompt, schema) = (stances::proposal_prompt(), stances::proposal_schema());
        let items = proposal_slot_items(&records, &shared, &prompt, round, &chosen, &schema, subject.clone());
        // the budget before the job: refused, it is this request's answer
        budget_for::<Proposals>(&library, &chosen, &items, &approved)?
    };
    let worker = Shared::clone(engine);
    let about = crate::jobs::About { feature: Feature::Stances, subject: subject.clone(), item: None };
    let job = engine.job.start_about("proposing questions for a community", about, move |tracker| {
        let library = worker.library().map_err(|error| error.to_string())?;
        let records: Vec<(&str, Record<'_>)> =
            sent.iter().map(|(id, title, abstract_text)| (id.as_str(), Record { title, abstract_text })).collect();
        let shared = stances::proposal_text(&records);
        let prompt = stances::proposal_prompt();
        let schema = stances::proposal_schema();
        let items = proposal_slot_items(&records, &shared, &prompt, round, &chosen, &schema, subject.clone());
        let came = ask_items::<Proposals>(&worker, &library, &chosen, &items, &budget, tracker)?;
        if let Some(why) = came.failing {
            return Err(why);
        }
        // proposing anew: the new round's proposals are shown once they are kept, and the old ones until then (a
        // job refused, stopped or failed leaves them shown)
        let new_kept =
            library.kept::<Proposals>(Feature::Stances, &items[0].slot.key).map_err(|error| error.to_string())?;
        if fresh && new_kept.is_some() {
            change_questions(&library, &run, &scope, |kept| {
                kept.proposal_round = kept.proposal_round.max(round);
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        }
        Ok(Finished::Feature { feature: Feature::Stances, subject })
    })?;
    Ok(Started { job, id: "proposals".to_owned() })
}

async fn community_listing(
    State(engine): State<Shared>,
    Path((run, community)): Path<(String, String)>,
) -> Result<Json<Listing>, ApiError> {
    Ok(Json(on_the_side(move || listing(&engine, &run, Scope::Community(community))).await?))
}

async fn community_estimate(
    State(engine): State<Shared>,
    Path((run, community)): Path<(String, String)>,
    body: Result<Json<QuestionBody>, JsonRejection>,
) -> Result<Json<StancePlan>, ApiError> {
    let Json(asked) = body?;
    Ok(Json(on_the_side(move || estimate(&engine, &run, Scope::Community(community), asked)).await?))
}

async fn community_ask(
    State(engine): State<Shared>,
    Path((run, community)): Path<(String, String)>,
    body: Result<Json<AskBody>, JsonRejection>,
) -> Result<Json<Started>, ApiError> {
    let Json(AskBody { question, part, approved }) = body?;
    Ok(Json(on_the_side(move || ask(&engine, &run, Scope::Community(community), question, part, approved)).await?))
}

async fn community_result(
    State(engine): State<Shared>,
    Path((run, community, id)): Path<(String, String, String)>,
) -> Result<Json<StanceResult>, ApiError> {
    Ok(Json(on_the_side(move || result(&engine, &run, &Scope::Community(community), &id)).await?))
}

async fn community_redo(
    State(engine): State<Shared>,
    Path((run, community, id)): Path<(String, String, String)>,
) -> Result<Json<Asked>, ApiError> {
    Ok(Json(on_the_side(move || redo(&engine, &run, &Scope::Community(community), &id)).await?))
}

async fn proposals_estimate(
    State(engine): State<Shared>,
    Path((run, community)): Path<(String, String)>,
    body: Result<Json<ProposalBody>, JsonRejection>,
) -> Result<Json<ProposalPlan>, ApiError> {
    let Json(ProposalBody { fresh }) = body?;
    Ok(Json(on_the_side(move || proposal_plan(&engine, &run, &community, fresh)).await?))
}

async fn proposals_ask(
    State(engine): State<Shared>,
    Path((run, community)): Path<(String, String)>,
    body: Result<Json<ProposeBody>, JsonRejection>,
) -> Result<Json<Started>, ApiError> {
    let Json(body) = body?;
    Ok(Json(on_the_side(move || propose(&engine, &run, &community, body)).await?))
}

async fn marked_listing(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    body: Result<Json<Marked>, JsonRejection>,
) -> Result<Json<Listing>, ApiError> {
    let Json(Marked { papers }) = body?;
    Ok(Json(on_the_side(move || listing(&engine, &run, marked(papers)?)).await?))
}

async fn marked_estimate(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    body: Result<Json<QuestionBody>, JsonRejection>,
) -> Result<Json<StancePlan>, ApiError> {
    let Json(mut asked) = body?;
    let papers = std::mem::take(&mut asked.papers);
    Ok(Json(on_the_side(move || estimate(&engine, &run, marked(papers)?, asked)).await?))
}

async fn marked_ask(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    body: Result<Json<AskBody>, JsonRejection>,
) -> Result<Json<Started>, ApiError> {
    let Json(AskBody { mut question, part, approved }) = body?;
    let papers = std::mem::take(&mut question.papers);
    Ok(Json(on_the_side(move || ask(&engine, &run, marked(papers)?, question, part, approved)).await?))
}

async fn marked_result(
    State(engine): State<Shared>,
    Path((run, id)): Path<(String, String)>,
) -> Result<Json<StanceResult>, ApiError> {
    Ok(Json(on_the_side(move || result(&engine, &run, &Scope::Marked(Vec::new()), &id)).await?))
}

async fn marked_redo(
    State(engine): State<Shared>,
    Path((run, id)): Path<(String, String)>,
) -> Result<Json<Asked>, ApiError> {
    Ok(Json(on_the_side(move || redo(&engine, &run, &Scope::Marked(Vec::new()), &id)).await?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(super) mod tests {
    use std::sync::Condvar;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::api::guard::config::tests::STRONG;
    use crate::api::guard::config::{Environment, Listen};
    use crate::api::routes::Engine;
    use crate::api::tests::served;
    use crate::library::Known;
    use crate::library::papers::tests::paper;
    use crate::model::language::fake::{message, ok};
    use crate::model::language::{ApiKey, Reply, Transport, TransportError};
    use crate::settings::Choices;

    const AT: &str = "127.0.0.1:8765";
    const QUESTION: &str = "Does pre-training on synthetic data help downstream accuracy?";

    /// Whether the tests' transport answers (open) or holds every request (closed), and how many it was sent.
    static GATE: Mutex<bool> = Mutex::new(true);
    static OPENED: Condvar = Condvar::new();
    static POSTS: AtomicUsize = AtomicUsize::new(0);

    fn gate(open: bool) {
        *GATE.lock().unwrap() = open;
        OPENED.notify_all();
    }

    /// The model as these routes' jobs meet it in tests: every paper is said to support the question, with a
    /// quote that is only in the first paper's abstract. Never the network.
    pub struct Answering;

    impl Transport for Answering {
        fn post(
            &self,
            _: &str,
            _: &ApiKey,
            _: &[(&str, &str)],
            _: &[u8],
            _: Duration,
        ) -> Result<Reply, TransportError> {
            let mut open = GATE.lock().unwrap();
            while !*open {
                open = OPENED.wait(open).unwrap();
            }
            drop(open);
            POSTS.fetch_add(1, Ordering::SeqCst);
            let said = json!({"stance": "supports", "condition": "",
                              "quotes": ["pre-training on synthetic data improves downstream accuracy"]});
            let usage = json!({"input_tokens": 300, "output_tokens": 80});
            Ok(ok(&message("claude-opus-5-5", &said.to_string(), "end_turn", usage)))
        }

        fn wait(&self, _: Duration) {}
    }

    fn choices() -> Choices {
        let one = |name: &str| vec![name.to_owned()];
        Choices {
            embedding_models: one("Qwen/Qwen3-Embedding-0.6B"),
            clustering: one("spectral"),
            acceptance: one("all"),
            flags: one("llm_declared"),
            describe_models: one("claude-opus-5-5"),
        }
    }

    /// A run of five papers: c0 (p1, p2, p3, and p5, which the library does not have) with a sub-community c0.0
    /// (p1, p2), and c1 (p4).
    fn engine(key: bool) -> (axum::Router, tempfile::TempDir) {
        let data = tempfile::tempdir().unwrap();
        crate::api::other_providers::reads_with_naming(data.path());
        {
            let library = crate::library::Library::open(&data.path().join("library")).unwrap();
            let abstracts = [
                "We find that pre-training on synthetic data improves downstream accuracy on all tasks.",
                "Synthetic data for pre-training gave no gain in downstream accuracy over real data.",
                "A graph neural network for molecules.",
                "Robust image classifiers.",
            ];
            let papers: Vec<_> = abstracts
                .iter()
                .enumerate()
                .map(|(index, text)| {
                    let mut paper = paper(&format!("p{}", index + 1));
                    paper.r#abstract = (*text).to_owned();
                    paper
                })
                .collect();
            library.add_papers(&papers, None, Known::Keep).unwrap();
        }
        let run = data.path().join("runs").join("r1");
        std::fs::create_dir_all(&run).unwrap();
        let graph = json!({"nodes": [
            {"id": "p1", "acceptance": "oral"}, {"id": "p2", "acceptance": "poster"},
            {"id": "p3", "acceptance": null}, {"id": "p4"}, {"id": "p5"}],
            "edges": [[0, 1, 0.9], [1, 2, 0.5], [0, 2, 0.1]]});
        std::fs::write(run.join("graph.json"), graph.to_string()).unwrap();
        let tree = json!({"communities": [
            {"tree_path": "c0", "members": [0, 1, 2, 4], "children": [{"tree_path": "c0.0", "members": [0, 1]}]},
            {"tree_path": "c1", "members": [3]}]});
        std::fs::write(run.join("communities.json"), tree.to_string()).unwrap();
        let listen = Listen::from_environment(&Environment::from_pairs([
            ("CONSTELLATE_TOKEN", STRONG),
            ("CONSTELLATE_ORIGIN", "http://127.0.0.1:3000"),
        ]))
        .unwrap();
        let mut engine = Engine::new(data.path().to_path_buf(), choices());
        engine.transport = std::sync::Arc::new(Answering);
        if key {
            engine.set_claude_key("test-key-not-real", None);
        }
        let (router, _) = served(&listen, AT, engine);
        (router, data)
    }

    async fn call(router: &axum::Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let body = body.map_or(Body::empty(), |body| Body::from(body.to_string()));
        call_raw(router, method, path, body).await
    }

    async fn call_raw(router: &axum::Router, method: &str, path: &str, body: Body) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, AT)
            .header(header::AUTHORIZATION, format!("Bearer {STRONG}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body)
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// The job's state once it is no longer running.
    async fn finished(router: &axum::Router) -> Value {
        for _ in 0..500 {
            let (_, status) = call(router, "GET", "/api/status", None).await;
            if status["state"] != "running" {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the job did not finish");
    }

    const C0: &str = "/api/runs/r1/communities/c0/stances";

    #[tokio::test]
    async fn another_provider_is_priced_asked_in_its_own_shape_kept_and_asked_again_when_the_provider_changes() {
        use crate::api::other_providers::{
            ACCEPTED, Chosen as Asked, MODEL, Scripted, accepting, call as send, choose_reading as choose, finished,
            priced_for, reading_router as router, sent_to,
        };

        let (_, data) = engine(false);
        let transport = Scripted::new(|_| {
            json!({"stance": "supports", "condition": "", "quotes": ["pre-training on synthetic data improves downstream accuracy"]})
                .to_string()
        });
        let router = router(data.path(), Asked::OpenAi, &transport);
        let mut sent = 0;
        for chosen in [Asked::OpenAi, Asked::ThisComputer] {
            choose(data.path(), chosen);
            let (_, plan) = send(&router, "POST", &format!("{C0}/estimate"), Some(json!({"question": QUESTION}))).await;
            assert_eq!(
                (plan["provider"].clone(), plan["model"].clone()),
                (json!(chosen.provider().id()), json!(MODEL)),
                "{plan}"
            );
            assert_eq!(plan["api_key_set"], true);
            let cost = plan["sample"]["estimate"].clone();
            assert_eq!(cost["to_ask"], 2);
            priced_for(&cost, chosen);
            assert_eq!(transport.sent().len(), sent, "an estimate sends nothing");

            let mut asked = accepting(&cost, ACCEPTED);
            asked["question"] = json!(QUESTION);
            asked["part"] = json!("sample");
            // naming nothing accepted is refused, also for a model with no price
            let mut bare = asked.clone();
            bare.as_object_mut().unwrap().retain(|field, _| !field.starts_with("accepted"));
            let (status, refused) = send(&router, "POST", &format!("{C0}/ask"), Some(bare)).await;
            if status == StatusCode::OK {
                let done = finished(&router, &refused["job"]).await;
                assert_eq!(done["state"], "failed", "{chosen:?}: {done}");
            }
            assert_eq!(transport.sent().len(), sent, "{chosen:?}: nothing accepted, nothing sent");
            let (status, started) = send(&router, "POST", &format!("{C0}/ask"), Some(asked.clone())).await;
            assert_eq!(status, StatusCode::OK, "{started}");
            let done = finished(&router, &started["job"]).await;
            assert_eq!(done["state"], "done", "{done}");
            let made = json!({"kind": "feature", "feature": "stances", "subject": {"community": {"run": "r1", "community": "c0"}}});
            assert_eq!((done["outcome"].clone(), done["result"].clone()), (made, Value::Null), "no run to open");
            let all = transport.sent();
            assert_eq!(all.len() - sent, 2, "one call per paper");
            sent_to(&all[sent..], chosen);
            sent = all.len();

            let id = started["id"].as_str().unwrap();
            let (_, result) = send(&router, "GET", &format!("{C0}/{id}"), None).await;
            assert_eq!(
                (result["provider"].clone(), result["answered"].clone()),
                (json!(chosen.provider().id()), json!(2)),
                "{result}"
            );
            let p1 = result["rows"].as_array().unwrap().iter().find(|row| row["paper"] == "p1").unwrap().clone();
            assert_eq!(p1["placed"]["group"], "supports");
            assert_eq!(p1["placed"]["evidence"][0]["established"]["kind"], "abstract");

            let (_, again) =
                send(&router, "POST", &format!("{C0}/estimate"), Some(json!({"question": QUESTION}))).await;
            let kept = again["sample"]["estimate"].clone();
            assert_eq!(kept["to_ask"], 0, "kept: nothing left to ask");
            let mut asked = accepting(&kept, ACCEPTED);
            asked["question"] = json!(QUESTION);
            asked["part"] = json!("sample");
            let (_, started) = send(&router, "POST", &format!("{C0}/ask"), Some(asked)).await;
            assert_eq!(finished(&router, &started["job"]).await["state"], "done");
            assert_eq!(transport.sent().len(), sent, "kept: not asked again");
        }
    }

    #[tokio::test]
    async fn a_question_is_estimated_asked_in_turn_kept_redone_and_refused_while_busy() {
        let (router, _data) = engine(true);
        let posts = || POSTS.load(Ordering::SeqCst);
        let before = posts();
        let (status, plan) =
            call(&router, "POST", &format!("{C0}/estimate"), Some(json!({"question": QUESTION}))).await;
        assert_eq!(status, StatusCode::OK, "{plan}");
        crate::api::contract::assert_fits("StancePlan", &plan, &[]);
        assert_eq!(
            (plan["filter"]["papers"].clone(), plan["filter"]["related"].clone(), plan["filter"]["unrelated"].clone()),
            (json!(3), json!(2), json!(1)),
            "the paper on molecules shares no word with the question"
        );
        assert_eq!(plan["filter"]["not_in_library"], 1, "p5 is neither asked nor skipped, but counted");
        assert_eq!((plan["sample"]["papers"].clone(), plan["sample"]["abstract_only"].clone()), (json!(2), json!(2)));
        assert!(plan["all"]["estimate"]["usd"].as_f64().unwrap() > 0.0);
        assert_eq!(posts(), before, "the estimate sends nothing");
        let id = plan["id"].as_str().unwrap().to_owned();
        let sample_most = plan["sample"]["estimate"]["most_usd"].clone();
        let all_most = plan["all"]["estimate"]["most_usd"].clone();

        // a click on terms the user was not shown starts nothing, or stops before sending anything
        let other =
            json!({"question": QUESTION, "part": "sample", "model": "another-model", "approved_usd": sample_most});
        assert_eq!(call(&router, "POST", &format!("{C0}/ask"), Some(other)).await.0, StatusCode::BAD_REQUEST);
        let cheap =
            json!({"question": QUESTION, "part": "sample", "model": "claude-opus-5-5", "approved_usd": 0.000_001});
        let (status, refused) = call(&router, "POST", &format!("{C0}/ask"), Some(cheap)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "a budget refused is the request's answer, not a failed job");
        assert!(refused["error"].as_str().unwrap().contains("changed since it was shown"), "{refused}");
        assert_eq!(posts(), before, "nothing was sent past what was approved");
        let (_, listing) = call(&router, "GET", C0, None).await;
        assert!(listing["questions"].as_array().unwrap().is_empty(), "a refused question is not listed: {listing}");

        let ask =
            json!({"question": QUESTION, "part": "sample", "model": "claude-opus-5-5", "approved_usd": sample_most});
        let (status, started) = call(&router, "POST", &format!("{C0}/ask"), Some(ask.clone())).await;
        assert_eq!((status, started["id"].as_str()), (StatusCode::OK, Some(id.as_str())));
        crate::api::contract::assert_fits("StanceStarted", &started, &[]);
        let done = finished(&router).await;
        let made = json!({"kind": "feature", "feature": "stances", "subject": {"community": {"run": "r1", "community": "c0"}}});
        assert_eq!(
            (done["state"].clone(), done["outcome"].clone(), done["result"].clone()),
            (json!("done"), made, Value::Null),
            "{done}"
        );
        assert_eq!(posts() - before, 2);

        let (status, result) = call(&router, "GET", &format!("{C0}/{id}"), None).await;
        assert_eq!(status, StatusCode::OK);
        crate::api::contract::assert_fits("StanceResult", &result, &[]);
        assert_eq!((result["answered"].clone(), result["to_ask"].clone()), (json!(2), json!(2)));
        let rows = result["rows"].as_array().unwrap();
        assert_eq!(
            (rows[0]["paper"].clone(), rows[1]["paper"].clone()),
            (json!("p2"), json!("p1")),
            "most central first"
        );
        assert_eq!((rows[1]["tier"].clone(), rows[1]["year"].clone()), (json!("oral"), json!(2026)));
        assert_eq!(rows[1]["placed"]["group"], "supports");
        assert_eq!(rows[1]["read"], "abstract");
        assert_eq!(rows[1]["placed"]["evidence"][0]["established"]["kind"], "abstract");
        assert_eq!(
            rows[0]["placed"],
            json!({"group": "not_verified", "quotes_not_found": 1}),
            "the quote is not in p2: it is not counted as a yes"
        );

        // asked again, and about the sub-community: what is kept is not paid for twice
        let all = json!({"question": QUESTION, "part": "all", "model": "claude-opus-5-5", "approved_usd": all_most});
        call(&router, "POST", &format!("{C0}/ask"), Some(all)).await;
        assert_eq!(finished(&router).await["state"], "done");
        let sub = "/api/runs/r1/communities/c0.0/stances";
        call(&router, "POST", &format!("{sub}/ask"), Some(ask.clone())).await;
        finished(&router).await;
        assert_eq!(posts() - before, 2);

        let (_, listing) = call(&router, "GET", C0, None).await;
        crate::api::contract::assert_fits("StanceListing", &listing, &[]);
        assert_eq!(listing["questions"][0]["question"], QUESTION);
        assert_eq!(listing["proposals"], Value::Null);

        let (status, redone) = call(&router, "POST", &format!("{C0}/{id}/redo"), None).await;
        crate::api::contract::assert_fits("StanceAsked", &redone, &[]);
        assert_eq!((status, redone["round"].clone()), (StatusCode::OK, json!(1)));
        let (_, result) = call(&router, "GET", &format!("{C0}/{id}"), None).await;
        assert_eq!(result["answered"], 0, "a redo asks again");

        gate(false);
        let (status, _) = call(&router, "POST", &format!("{C0}/ask"), Some(ask.clone())).await;
        assert_eq!(status, StatusCode::OK);
        let (status, busy) = call(&router, "POST", &format!("{C0}/ask"), Some(ask)).await;
        // another question, refused while busy: not listed as if it had been asked
        let other = json!({"question": "Does synthetic data hurt accuracy?", "part": "sample",
            "model": "claude-opus-5-5", "approved_usd": 100.0});
        let (refused, _) = call(&router, "POST", &format!("{C0}/ask"), Some(other)).await;
        let (_, listing) = call(&router, "GET", C0, None).await;
        gate(true);
        assert_eq!(status, StatusCode::CONFLICT, "{busy}");
        assert_eq!(refused, StatusCode::CONFLICT);
        assert_eq!(listing["questions"].as_array().unwrap().len(), 1, "only the question asked: {listing}");
        finished(&router).await;
        assert_eq!(posts() - before, 4);
    }

    #[test]
    fn past_the_most_questions_the_oldest_without_answers_make_room_and_answered_ones_never_do() {
        let asked = |id: usize| Asked {
            id: format!("q{id}"),
            question: format!("question {id}"),
            round: 0,
            include_unrelated: false,
            papers: Vec::new(),
            asked_at: String::new(),
        };
        // newest first, as kept: q0 is the oldest
        let mut kept = Questions {
            questions: (0..=MOST_QUESTIONS).rev().map(asked).collect(),
            proposal_round: 0,
            unanswered: vec!["q3".to_owned(), "q7".to_owned(), format!("q{MOST_QUESTIONS}")],
        };
        assert_eq!(make_room(&mut kept), Some(1));
        assert_eq!(kept.questions.len(), MOST_QUESTIONS);
        assert!(!kept.questions.iter().any(|asked| asked.id == "q3"), "the oldest without an answer went");
        assert!(kept.questions.iter().any(|asked| asked.id == "q0"), "an older one with answers stays");
        assert_eq!(kept.unanswered, ["q7".to_owned(), format!("q{MOST_QUESTIONS}")]);
        // every older one answered: no room is made, and the newest is never the one taken off
        let mut full = Questions {
            questions: (0..=MOST_QUESTIONS).rev().map(asked).collect(),
            proposal_round: 0,
            unanswered: vec![format!("q{MOST_QUESTIONS}")],
        };
        assert_eq!(make_room(&mut full), None);
    }

    #[tokio::test]
    async fn proposing_anew_leaves_the_kept_proposals_shown_when_the_job_keeps_none() {
        let (router, data) = engine(true);
        {
            // proposals kept from before, over c0's papers most central first
            let library = Library::open(&data.path().join("library")).unwrap();
            let chosen = Chosen::claude("claude-opus-5-5");
            let slot = Slot {
                feature: Feature::Stances,
                key: chosen.key(Feature::Stances, stances::PROPOSE_PROMPT, &["p2", "p1", "p3"], "proposals\nround 0"),
                subject: Subject::Community { run: "r1".to_owned(), community: "c0".to_owned() },
            };
            let quote =
                |paper: &str, quote: &str| crate::evidence::Quote { paper: paper.to_owned(), quote: quote.to_owned() };
            let kept = Proposals {
                questions: vec![crate::evidence::Claim {
                    statement: QUESTION.to_owned(),
                    quotes: vec![
                        quote("p1", "pre-training on synthetic data improves downstream accuracy"),
                        quote("p2", "Synthetic data for pre-training gave no gain in downstream accuracy"),
                    ],
                }],
            };
            library.keep(&slot, "claude-opus-5-5", stances::PROPOSE_PROMPT, &kept).unwrap();
        }
        let (status, listing) = call(&router, "GET", C0, None).await;
        assert_eq!(status, StatusCode::OK, "{listing}");
        assert_eq!(listing["proposals"]["questions"][0]["statement"], QUESTION, "{listing}");
        crate::api::contract::assert_fits("StanceListing", &listing, &[]);

        // proposing anew, on terms the estimate no longer matches: refused before any job, and nothing kept
        let fresh = json!({"fresh": true, "model": "claude-opus-5-5", "approved_usd": 0.000_001});
        let (status, refused) = call(&router, "POST", &format!("{C0}/proposals"), Some(fresh)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        let (_, listing) = call(&router, "GET", C0, None).await;
        assert_eq!(listing["proposals"]["questions"][0]["statement"], QUESTION, "still shown: {listing}");
    }

    #[tokio::test]
    async fn what_is_not_there_is_not_found() {
        let (router, _data) = engine(false);
        assert_eq!(call(&router, "GET", C0, None).await.0, StatusCode::OK);
        for path in [
            "/api/runs/r9/communities/c0/stances",
            "/api/runs/r1/communities/c7/stances",
            "/api/runs/r1/communities/c0/stances/0123456789abcdef",
            "/api/runs/..%2Fr1/communities/c0/stances",
            "/api/runs/r1/marked/stances/0123456789abcdef",
        ] {
            assert_eq!(call(&router, "GET", path, None).await.0, StatusCode::NOT_FOUND, "{path}");
        }
        assert_eq!(call(&router, "POST", &format!("{C0}/0123456789abcdef/redo"), None).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_question_or_a_set_of_papers_too_large_or_a_model_that_cannot_be_asked_is_refused() {
        let (router, _data) = engine(false);
        let long = json!({"question": "a".repeat(stances::LONGEST_QUESTION + 1)});
        assert_eq!(call(&router, "POST", &format!("{C0}/estimate"), Some(long)).await.0, StatusCode::BAD_REQUEST);
        let many: Vec<String> = (0..=MOST_MARKED).map(|n| format!("p{n}")).collect();
        let marked = json!({"question": QUESTION, "papers": many});
        assert_eq!(
            call(&router, "POST", "/api/runs/r1/marked/stances/estimate", Some(marked)).await.0,
            StatusCode::BAD_REQUEST
        );
        let huge =
            Body::from(format!("{{\"question\": \"{}\"}}", "a".repeat(crate::api::guard::security::LARGEST_BODY)));
        assert_eq!(call_raw(&router, "POST", &format!("{C0}/estimate"), huge).await.0, StatusCode::PAYLOAD_TOO_LARGE);
        let approved = json!({"question": QUESTION, "part": "sample", "model": "claude-opus-5-5", "approved_usd": 1.0});
        let (status, refused) = call(&router, "POST", &format!("{C0}/ask"), Some(approved)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(refused["error"].as_str().unwrap().starts_with("the model cannot be asked"));
        assert_eq!(call(&router, "GET", "/api/status", None).await.1["state"], "idle", "no job was started");
        let propose = json!({"model": "claude-opus-5-5", "approved_usd": 1.0});
        let (status, _) = call(&router, "POST", &format!("{C0}/proposals"), Some(propose)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let unapproved = json!({"question": QUESTION, "part": "sample"});
        assert_eq!(call(&router, "POST", &format!("{C0}/ask"), Some(unapproved)).await.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn marked_papers_are_a_scope_of_their_own_with_suggestions_and_an_estimate() {
        let (router, _data) = engine(false);
        let (status, listing) =
            call(&router, "POST", "/api/runs/r1/marked/stances/list", Some(json!({"papers": ["p2", "p1"]}))).await;
        assert_eq!((status, listing["papers"].clone()), (StatusCode::OK, json!(2)));
        let body = json!({"question": QUESTION, "papers": ["p1", "p4"], "include_unrelated": true});
        let (status, plan) = call(&router, "POST", "/api/runs/r1/marked/stances/estimate", Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{plan}");
        assert_eq!((plan["filter"]["unrelated"].clone(), plan["all"]["papers"].clone()), (json!(1), json!(2)));
        let stranger = json!({"question": QUESTION, "papers": ["elsewhere"]});
        assert_eq!(
            call(&router, "POST", "/api/runs/r1/marked/stances/estimate", Some(stranger)).await.0,
            StatusCode::BAD_REQUEST
        );
        let (status, proposals) =
            call(&router, "POST", &format!("{C0}/proposals/estimate"), Some(json!({"fresh": false}))).await;
        assert_eq!((status, proposals["papers"].clone()), (StatusCode::OK, json!(3)));
        crate::api::contract::assert_fits("StanceProposalPlan", &proposals, &[]);
        assert_eq!(proposals["estimate"]["to_ask"], 1);
        crate::api::contract::assert_fits("StanceListing", &listing, &[]);
        crate::api::contract::assert_fits("StancePlan", &plan, &[]);
    }
}
