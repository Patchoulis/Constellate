//! Questions asked of a run or of one of its communities, answered with cited statements (`features::answers`), and
//! the passages that talk about something, found without a model (`features::answers::retrieval`).
//!
//! ```text
//! GET    /api/runs/{run}/passages/found?q=..&community=..   the passages that match best (`PassagesFound`)
//! GET    /api/runs/{run}/answers/estimate?q=..&community=.. what asking would cost, nothing sent (`AnswerEstimate`)
//! POST   /api/runs/{run}/answers   {id, question, community?, provider, model, accepted_usd | accepted_tokens}:
//!                                   start asking (`AnswerStarted`); `id` is the estimate's
//! GET    /api/runs/{run}/answers                            the questions asked of the run (`AnswerHistory`)
//! GET    /api/runs/{run}/answers/{id}                       one, its kept answer checked again (`AnswerView`)
//! DELETE /api/runs/{run}/answers/{id}                       forget it and its answer
//! POST   /api/runs/{run}/answers/{id}/redo                  forget its answer only: asked again at a new click
//! ```
//!
//! A scope's index is built once on a thread of its own, the first time it is searched, and kept (with the
//! others under a memory budget: [`crate::features::answers::retrieval::Indexes`]); while it is being built (reading PDFs
//! never read before takes a while) a request waits up to [`WAIT`] and is then told how far it has got
//! (`{"state": "indexing", "done", "of"}`), and asks again.
//!
//! Money is spent only by the POST, which the interface sends when the user clicks the button showing the
//! estimate. It carries the most the user was shown (`accepted_usd`, or `accepted_tokens` for a model with no
//! price), and the estimate's `id`: if the passages that would be sent are not those estimated (the question was
//! edited, papers were added, a PDF was read since: the id is made from them), or what the question would cost at
//! most is now more, nothing is asked and the estimate must be asked for again.
//! That worst case is the call's budget: no call is started that could pass it as the budget reckons it (prompt
//! tokens from characters, at 3 a token, which errs high for English prose; text that tokenizes more densely, such
//! as mathematics, can cost somewhat more, which is why the interface says it is an estimate). A request naming
//! another model or provider than the one estimated, or accepting dollars for a model whose price is not known,
//! is refused. A model whose price is not known is capped in tokens instead, by the same worst case
//! ([`crate::model::chosen::budget`]).
//!
//! Retrieval here is lexical: the engine has no embedding model loaded to embed a question with. When it has,
//! the similarity of each paper's embedding to the question goes to `Index::search` as `Ranking::Embedded`.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::routing::{MethodRouter, get, post};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::api::papers::reviews::papers_of;
use crate::api::routes::{ApiError, Shared, on_the_side, one_name};
use crate::evidence::{Cited, PaperText, Papers};
use crate::features::answers::retrieval::{
    Builder, Coverage, Document, Got, Hit, Index, Indexes, LONGEST_QUERY, Ranking, Unit, Unread, Wait,
};
use crate::features::answers::store::Asked;
use crate::features::answers::{
    self, Answer, AnswerCounts, Found, LONGEST_QUESTION, MAX_TOKENS, NotAnswered, Outcome, PROMPT_TOKENS, Prepared,
    Said, SaidOutcome, Scope, Statement,
};
use crate::jobs::{About, Finished, Phase, Tracker};
use crate::library::papers::passages::PDF_FOLDER;
use crate::library::{AnswerKey, Feature, Library, PaperReading};
use crate::model::batch::{Asking, IN_FLIGHT};
use crate::model::chosen::{Accepted, Terms, budget};
use crate::model::language::{Budget, Estimate, Provider};
use crate::usage::Usd;

/// How long a request waits for an index being built before it is told how far the building has got.
pub const WAIT: Duration = Duration::from_secs(15);
/// Papers read through the library at a time while an index is built.
const CHUNK: usize = 200;
/// What the job asking a question is called in `/api/status`.
const JOB: &str = "answering a question";

/// The indexes of this engine, by library, run, scope and members.
static INDEXES: LazyLock<Indexes> = LazyLock::new(Indexes::default);

/// By index name: the modification time of the library's PDF folder when that index's papers were last seen to
/// be as it has them (none when there is no folder). Keeping a PDF (a download, a replacement) changes the
/// folder's time; only then are the papers an index has without a PDF looked at one by one ([`up_to_date`]).
static SEEN: LazyLock<Mutex<HashMap<String, Option<SystemTime>>>> = LazyLock::new(Mutex::default);

fn seen() -> MutexGuard<'static, HashMap<String, Option<SystemTime>>> {
    SEEN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// When a PDF was last kept in (or taken from) the library, as its PDF folder's time says.
fn pdfs_changed_at(library: &Library) -> Option<SystemTime> {
    std::fs::metadata(library.path().with_file_name(PDF_FOLDER)).and_then(|folder| folder.modified()).ok()
}

/// Does `index` (kept under `name`) still have the papers as the library has them? Not once a paper it has by its
/// abstract, because no PDF of it was kept, has a PDF: that paper's full text would be searched by an index built
/// now, whether or not its abstract matches anything.
fn up_to_date(library: &Library, name: &str, index: &Index) -> bool {
    let now = pdfs_changed_at(library);
    if seen().get(name) == Some(&now) {
        return true;
    }
    let fresh = index.without_pdf().all(|paper| !library.pdf_file(paper).exists());
    if fresh {
        let kept = INDEXES.kept();
        let mut seen = seen();
        seen.retain(|scope, _| kept.binary_search(scope).is_ok());
        seen.insert(name.to_owned(), now);
    }
    fresh
}

/// Held while a question is listed and its job started ([`start`]).
static STARTING: Mutex<()> = Mutex::new(());

/// The routes of this module, for [`crate::api::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![
        ("/api/runs/{run}/passages/found", get(passages_found)),
        ("/api/runs/{run}/answers/estimate", get(estimate)),
        ("/api/runs/{run}/answers", get(history).post(start)),
        ("/api/runs/{run}/answers/{id}", get(view).delete(remove)),
        ("/api/runs/{run}/answers/{id}/redo", post(redo)),
    ]
}

#[derive(Deserialize)]
struct Search {
    q: String,
    community: Option<String>,
}

/// `AnswerRequest` in the contracts.
#[derive(Deserialize)]
struct Request {
    /// The id of the estimate the user was shown (`AnswerEstimate.id`). It is the key of the answer, made from the
    /// question, the scope, the model and the passages that would be sent: when any of those changed since (the
    /// question was edited, papers were added, a PDF was read), what would be asked is not what was accepted.
    id: String,
    question: String,
    community: Option<String>,
    /// The model the estimate was for, and its provider: a request for another one is refused.
    #[serde(default)]
    provider: Option<Provider>,
    model: String,
    /// The most it would cost, as the user was shown it; null when the model's price is not known.
    accepted_usd: Option<f64>,
    /// For a model with no price: the most tokens it would take, as the user was shown it.
    #[serde(default)]
    accepted_tokens: Option<u64>,
}

/// A query or a question, trimmed, if it is not empty and not longer than `longest` characters.
fn bounded(text: &str, longest: usize) -> Result<String, ApiError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ApiError::Invalid("the question is empty".to_owned()));
    }
    if text.chars().nth(longest).is_some() {
        return Err(ApiError::Invalid(format!("the question is longer than {longest} characters")));
    }
    Ok(text.to_owned())
}

/// The scope's papers' index, as [`Indexes::get`] finds it, and its name there.
fn index_of(engine: &Shared, library: &Arc<Library>, scope: &Scope) -> Result<(Got, String), ApiError> {
    let ids = papers_of(engine, &scope.run, scope.community.as_deref())?;
    let members: String = Sha256::digest(ids.join("\n").as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
    let name =
        format!("{}\n{}\n{}\n{members}", library.path().display(), scope.run, scope.community.as_deref().unwrap_or(""));
    let (library, building) = (Arc::clone(library), name.clone());
    let got = INDEXES
        .get(&name, Wait::AtMost(WAIT), move |progress| {
            // before anything is read: a PDF kept while the papers are read is then a change seen after
            seen().insert(building, pdfs_changed_at(&library));
            build(&library, &ids, progress)
        })
        .map_err(|why| {
            eprintln!("answers: an index could not be built: {why}");
            ApiError::Files
        })?;
    Ok((got, name))
}

/// Was `hit` indexed from the text its paper has now? A paper whose PDF was read after the index was built was
/// indexed by its abstract, and its text is now its passages: the index is out of date.
fn current(hit: &Hit, texts: &Papers) -> bool {
    matches!(
        (hit.unit, texts.get(&hit.paper)),
        (Unit::Passage { .. }, Some(PaperText::Read(_))) | (Unit::Record, Some(PaperText::Unread { .. }))
    )
}

/// The index of `ids`, read through the library a chunk at a time (PDFs kept but never read are read now).
fn build(
    library: &Library,
    ids: &[String],
    progress: &crate::features::answers::retrieval::Progress,
) -> Result<Index, String> {
    let mut builder = Builder::default();
    for (number, chunk) in ids.chunks(CHUNK).enumerate() {
        progress.set(number * CHUNK, ids.len());
        let wanted: Vec<&str> = chunk.iter().map(String::as_str).collect();
        let readings = library.paper_texts(&wanted).map_err(|error| error.to_string())?;
        let documents: Vec<Document<'_>> = chunk
            .iter()
            .zip(&readings)
            .map(|(paper, reading)| match reading {
                PaperReading::Read(passages) => Document::Read { paper, passages },
                PaperReading::NoPdf { title, abstract_text } => {
                    Document::Record { paper, title, abstract_text, why: Unread::NoPdf }
                }
                PaperReading::Unreadable { title, abstract_text, .. } => {
                    Document::Record { paper, title, abstract_text, why: Unread::Unreadable }
                }
                PaperReading::NotInLibrary => Document::Missing,
            })
            .collect();
        builder.add(&documents);
    }
    progress.set(ids.len(), ids.len());
    Ok(builder.finish())
}

/// The texts of the papers `hits` are from, as the library has them now: what was read is not read again, but a
/// PDF kept since and never read is read now (so this runs on a thread of its own, never a rayon worker).
fn texts_of(library: &Library, hits: &[Hit]) -> Result<Papers, ApiError> {
    let mut ids: Vec<&str> = hits.iter().map(|hit| hit.paper.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    Ok(library.citable(&ids)?)
}

/// What a search came to.
enum Searched {
    Ready { hits: Vec<Hit>, coverage: Coverage, texts: Papers },
    Indexing { done: usize, of: usize },
}

/// The passages that match `query`, with their papers' texts. An index found out of date (a paper it has by its
/// abstract has a PDF now, or a passage found is of a text its paper no longer has) is dropped and built again
/// (the request then waits for it, or is told how far it has got).
fn search(engine: &Shared, library: &Arc<Library>, scope: &Scope, query: &str) -> Result<Searched, ApiError> {
    for _ in 0..2 {
        let (got, name) = index_of(engine, library, scope)?;
        let index = match got {
            Got::Ready(index) => index,
            Got::Building { done, of } => return Ok(Searched::Indexing { done, of }),
        };
        if !up_to_date(library, &name, &index) {
            INDEXES.forget(&name);
            continue;
        }
        let hits = index.search(query, Ranking::Lexical);
        let texts = texts_of(library, &hits)?;
        if hits.iter().all(|hit| current(hit, &texts)) {
            return Ok(Searched::Ready { hits, coverage: index.coverage(), texts });
        }
        INDEXES.forget(&name);
    }
    Err(ApiError::Invalid("the papers' texts are changing: try again in a moment".to_owned()))
}

/// `PassagesFound` in the contracts.
#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum PassagesFound {
    Ready { query: String, coverage: Coverage, passages: Vec<Found> },
    Indexing { done: usize, of: usize },
}

fn scope_of(run: String, community: Option<String>) -> Result<Scope, ApiError> {
    one_name(&run)?;
    let community = community.map(|path| path.trim().to_owned()).filter(|path| !path.is_empty());
    Ok(Scope { run, community })
}

async fn passages_found(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    query: Result<Query<Search>, QueryRejection>,
) -> Result<Json<PassagesFound>, ApiError> {
    let Query(Search { q, community }) = query?;
    let q = bounded(&q, LONGEST_QUERY)?;
    let scope = scope_of(run, community)?;
    let found = on_the_side(move || {
        let library = engine.library()?;
        Ok(match search(&engine, &library, &scope, &q)? {
            Searched::Ready { hits, coverage, texts } => {
                PassagesFound::Ready { passages: answers::found(&hits, &texts, &[]), query: q, coverage }
            }
            Searched::Indexing { done, of } => PassagesFound::Indexing { done, of },
        })
    })
    .await?;
    Ok(Json(found))
}

/// A question prepared for its scope, with the passages found for it and the coverage of the search.
enum Ready {
    Prepared { prepared: Box<Prepared>, coverage: Coverage },
    Indexing { done: usize, of: usize },
}

fn prepared(engine: &Shared, library: &Arc<Library>, scope: &Scope, question: &str) -> Result<Ready, ApiError> {
    let chosen = engine.chosen()?;
    Ok(match search(engine, library, scope, question)? {
        Searched::Ready { hits, coverage, texts } => {
            let prepared = Box::new(answers::prepare(&chosen, scope, question, &hits, &texts));
            Ready::Prepared { prepared, coverage }
        }
        Searched::Indexing { done, of } => Ready::Indexing { done, of },
    })
}

/// `AnswerEstimate` in the contracts: what one question would cost, before anything is sent.
#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum AnswerEstimate {
    Ready {
        /// The question's id once asked (the key its answer is kept under).
        id: String,
        provider: Provider,
        model: String,
        /// Its answer is kept: asking costs nothing.
        kept: bool,
        /// Passages that fit in the prompt, and the papers they are from.
        passages: usize,
        papers: usize,
        coverage: Coverage,
        /// The cap on the passages in the prompt, and on the answer, in tokens.
        prompt_cap_tokens: u64,
        max_answer_tokens: u32,
        /// What asking would cost (`AskingCost`: `counting` says how tokens were reckoned, an estimate either way);
        /// null when kept.
        cost: Option<Box<Estimate>>,
        /// The most it can cost, which is what the POST must accept; null when the price is not known.
        most_usd: Option<Usd>,
        /// The most tokens it can take: what the POST accepts when the price is not known.
        most_tokens: u64,
        /// Whether the model can be asked at all (its provider's key is set): never the key.
        api_key_set: bool,
    },
    Indexing {
        done: usize,
        of: usize,
    },
}

async fn estimate(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    query: Result<Query<Search>, QueryRejection>,
) -> Result<Json<AnswerEstimate>, ApiError> {
    let Query(Search { q, community }) = query?;
    let question = bounded(&q, LONGEST_QUESTION)?;
    let scope = scope_of(run, community)?;
    let told = on_the_side(move || {
        let library = engine.library()?;
        Ok(match prepared(&engine, &library, &scope, &question)? {
            Ready::Prepared { prepared, coverage } => {
                let estimate = prepared.estimate(&library)?;
                let kept = estimate.to_ask == 0;
                AnswerEstimate::Ready {
                    id: prepared.key.as_str().to_owned(),
                    provider: prepared.chosen.provider(),
                    model: prepared.chosen.model().to_owned(),
                    kept,
                    passages: prepared.given.len(),
                    papers: prepared.papers.len(),
                    coverage,
                    prompt_cap_tokens: PROMPT_TOKENS,
                    max_answer_tokens: MAX_TOKENS,
                    most_usd: estimate.cost.most_usd,
                    most_tokens: estimate.cost.most_tokens,
                    api_key_set: prepared.chosen.can_ask(),
                    cost: (!kept).then(|| Box::new(estimate.cost)),
                }
            }
            Ready::Indexing { done, of } => AnswerEstimate::Indexing { done, of },
        })
    })
    .await?;
    Ok(Json(told))
}

/// The first line of the log of the job asking question `id`: how a view tells that its own question is being
/// asked, not another.
fn asking_line(id: &str) -> String {
    format!("Asking the model question {id}.")
}

/// `AnswerStarted` in the contracts: the question's id, and the job asking it (none when its answer was kept).
#[derive(Serialize)]
struct Started {
    id: String,
    job: Option<u64>,
}

/// The budget of a question: its worst case, if that is no more than the user accepted (in dollars, or in tokens
/// for a model with no price).
fn budget_of(prepared: &Prepared, library: &Library, request: &Request) -> Result<Budget, ApiError> {
    let estimate = prepared.estimate(library)?;
    let accepted = Accepted::of(request.accepted_usd, request.accepted_tokens)
        .map_err(|why| ApiError::Invalid(why.to_string()))?;
    // naming nothing is not accepting: for a model with no price (a price shown is refused unless sent back) it
    // would run on a worst case never agreed to
    if accepted == Accepted::Shown && estimate.cost.calls > 0 && estimate.cost.most_usd.is_none() {
        return Err(ApiError::Invalid(
            "say what was accepted of the estimate shown (accepted_usd or accepted_tokens)".to_owned(),
        ));
    }
    budget(&estimate.cost, accepted, Terms::AsShown).map_err(|why| ApiError::Invalid(why.to_string()))
}

/// What the question's job does: ask it, and say in its log what came of it.
fn answer(asking: Asking<'_>, prepared: &Prepared, budget: &Budget, tracker: &Tracker<'_>) -> Result<String, String> {
    match answers::ask(asking, prepared, budget, tracker) {
        Ok(said) if said.outcome == SaidOutcome::NotInPapers => Ok("the papers in scope do not say".to_owned()),
        Ok(said) => Ok(format!("answered with {} statements, checked when shown", said.statements.len())),
        Err(NotAnswered::Library(error)) => {
            eprintln!("answers: {error}");
            Err("the engine could not read or write its files".to_owned())
        }
        Err(why) => Err(why.to_string()),
    }
}

async fn start(
    State(engine): State<Shared>,
    Path(run): Path<String>,
    body: Result<Json<Request>, JsonRejection>,
) -> Result<Json<Started>, ApiError> {
    let Json(request) = body?;
    let question = bounded(&request.question, LONGEST_QUESTION)?;
    let scope = scope_of(run, request.community.clone())?;
    let started = on_the_side(move || {
        let library = engine.library()?;
        let (prepared, _) = match prepared(&engine, &library, &scope, &question)? {
            Ready::Prepared { prepared, coverage } => (*prepared, coverage),
            Ready::Indexing { .. } => {
                return Err(ApiError::Invalid("the papers are still being indexed: try again in a moment".to_owned()));
            }
        };
        prepared.chosen.as_shown(request.provider, &request.model).map_err(|why| ApiError::Invalid(why.to_string()))?;
        if prepared.key.as_str() != request.id {
            return Err(ApiError::Invalid(
                "the passages this question would be answered from changed since the estimate: ask for the estimate \
                 again"
                    .to_owned(),
            ));
        }
        if prepared.given.is_empty() {
            return Err(ApiError::Invalid("no passage of these papers matches the question".to_owned()));
        }
        let asked = Asked {
            run: scope.run.clone(),
            key: prepared.key.as_str().to_owned(),
            community: scope.community.clone(),
            question: question.clone(),
            provider: prepared.chosen.provider(),
            model: prepared.chosen.model().to_owned(),
            passages: prepared.given.clone(),
            asked_at: String::new(),
        };
        let id = asked.key.clone();
        // one start at a time from here to the job's start, so that a second click (another tab) on the same
        // question sees the first one's listing, and a refused start never takes away a question being asked
        let _one = STARTING.lock().unwrap_or_else(PoisonError::into_inner);
        if library.kept::<Said>(Feature::Answers, &prepared.key)?.is_some() {
            library.keep_question(&asked)?;
            return Ok(Started { id, job: None });
        }
        let endpoint = prepared.chosen.endpoint().map_err(|why| ApiError::Invalid(why.to_string()))?;
        let budget = budget_of(&prepared, &library, &request)?;
        // listed before the call is started, so that a call paid for is never missing from the history
        let new = library.question(&scope.run, &id)?.is_none();
        library.keep_question(&asked)?;
        let worker = Arc::clone(&library);
        let line = asking_line(&id);
        let transport = Arc::clone(&engine.transport);
        let subject = prepared.subject.clone();
        let about = About { feature: Feature::Answers, subject: subject.clone(), item: Some(id.clone()) };
        let started = engine.job.start_about(JOB, about, move |tracker| {
            tracker.log(&line);
            let asking =
                Asking { library: &worker, transport: &*transport, endpoint, recorder: &*worker, in_flight: IN_FLIGHT };
            let came = answer(asking, &prepared, &budget, tracker)?;
            tracker.log(&format!("The model {came}."));
            Ok(Finished::Feature { feature: Feature::Answers, subject })
        });
        match started {
            Ok(job) => Ok(Started { id, job: Some(job) }),
            Err(busy) => {
                if new {
                    library.forget_question(&scope.run, &id)?;
                }
                Err(busy.into())
            }
        }
    })
    .await?;
    Ok(Json(started))
}

/// `AnswerSummary` in the contracts: one line of a run's questions.
#[derive(Serialize)]
struct Summary {
    id: String,
    question: String,
    community: Option<String>,
    model: String,
    asked_at: String,
    /// Its answer is kept: reopening it costs nothing.
    answered: bool,
}

/// The scope of a question kept, and the key its answer would be kept under now.
fn kept_answer(library: &Library, asked: &Asked) -> Result<Option<Said>, ApiError> {
    let scope = Scope { run: asked.run.clone(), community: asked.community.clone() };
    let (key, _) = answers::key_of(asked.provider, &asked.model, &scope, &asked.question, &asked.passages);
    // asked with an older prompt: not this answer's key any more, so not answered as far as it goes
    if key.as_str() != asked.key {
        return Ok(None);
    }
    Ok(library.kept::<Said>(Feature::Answers, &key)?.map(|kept| kept.answer))
}

async fn history(State(engine): State<Shared>, Path(run): Path<String>) -> Result<Json<Vec<Summary>>, ApiError> {
    one_name(&run)?;
    let listed = on_the_side(move || {
        let library = engine.library()?;
        library
            .questions(&run)?
            .into_iter()
            .map(|asked| {
                Ok(Summary {
                    answered: kept_answer(&library, &asked)?.is_some(),
                    id: asked.key,
                    question: asked.question,
                    community: asked.community,
                    model: asked.model,
                    asked_at: asked.asked_at,
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()
    })
    .await?;
    Ok(Json(listed))
}

/// Where a question stands, with what each state has: only a kept answer, checked again, has statements and
/// counts.
#[derive(Clone, Debug, PartialEq)]
enum Standing {
    Kept(Answer),
    /// Being asked now.
    Asking,
    /// Not answered: the call failed or was stopped, or the answer was forgotten. Asking again costs again.
    Unanswered,
}

/// `AnswerState` in the contracts: what the interface switches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AnswerState {
    Answered,
    NotInPapers,
    Unverified,
    Asking,
    Unanswered,
    /// Answered with no statement at all.
    Empty,
}

impl Standing {
    /// Where question `key` stands: its kept answer checked again (`kept`), or, with none, whether the job
    /// running now (`running`: what it was said to be about as it started) is the one asking it. What it is about
    /// is set before the job runs a line, so a view taken the moment after the question was started sees it
    /// being asked (a line of its log, written once its work has begun, would not be there yet).
    fn of(kept: Option<Answer>, running: Option<&About>, key: &str) -> Self {
        match (kept, running) {
            (Some(answer), _) => Self::Kept(answer),
            (None, Some(about)) if about.feature == Feature::Answers && about.item.as_deref() == Some(key) => {
                Self::Asking
            }
            (None, _) => Self::Unanswered,
        }
    }

    fn state(&self) -> AnswerState {
        match self {
            Self::Kept(answer) => match answer.outcome() {
                Outcome::Answered => AnswerState::Answered,
                Outcome::NotInPapers => AnswerState::NotInPapers,
                Outcome::Unverified => AnswerState::Unverified,
                Outcome::Empty => AnswerState::Empty,
            },
            Self::Asking => AnswerState::Asking,
            Self::Unanswered => AnswerState::Unanswered,
        }
    }
}

/// `AnswerView` in the contracts.
#[derive(Serialize)]
struct View {
    id: String,
    question: String,
    community: Option<String>,
    model: String,
    asked_at: String,
    state: AnswerState,
    /// Only when answered: each statement with the evidence found for it.
    statements: Vec<Cited<Statement>>,
    counts: AnswerCounts,
    /// The passages the model was given, best first, each with whether the answer cites it.
    passages: Vec<Found>,
    /// The papers the statements shown cite: what can be lit up on the graph.
    cited_papers: Vec<String>,
}

async fn view(State(engine): State<Shared>, Path((run, id)): Path<(String, String)>) -> Result<Json<View>, ApiError> {
    one_name(&run)?;
    let told = on_the_side(move || {
        let library = engine.library()?;
        let asked = library.question(&run, &id)?.ok_or(ApiError::NotFound)?;
        let texts = texts_of(&library, &asked.passages)?;
        let kept = kept_answer(&library, &asked)?
            .map(|said| answers::checked(said, &answers::citable(&texts, &papers_in(&asked.passages))));
        let job = engine.job.snapshot(0);
        let running = job.about.as_ref().filter(|_| job.state == Phase::Running);
        let standing = Standing::of(kept, running, &asked.key);
        let state = standing.state();
        let (statements, counts) = match standing {
            Standing::Kept(answer) => {
                let counts = answer.counts();
                (answer.into_statements(), counts)
            }
            Standing::Asking | Standing::Unanswered => (Vec::new(), AnswerCounts::default()),
        };
        Ok(View {
            passages: answers::found(&asked.passages, &texts, &statements),
            cited_papers: answers::cited_papers(&statements),
            statements,
            counts,
            state,
            id: asked.key,
            question: asked.question,
            community: asked.community,
            model: asked.model,
            asked_at: asked.asked_at,
        })
    })
    .await?;
    Ok(Json(told))
}

fn papers_in(hits: &[Hit]) -> Vec<String> {
    let mut papers: Vec<String> = hits.iter().map(|hit| hit.paper.clone()).collect();
    papers.sort_unstable();
    papers.dedup();
    papers
}

async fn remove(
    State(engine): State<Shared>,
    Path((run, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    one_name(&run)?;
    let gone = on_the_side(move || Ok(engine.library()?.forget_question(&run, &id)?)).await?;
    if !gone {
        return Err(ApiError::NotFound);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn redo(
    State(engine): State<Shared>,
    Path((run, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    one_name(&run)?;
    let forgotten = on_the_side(move || {
        let library = engine.library()?;
        let asked = library.question(&run, &id)?.ok_or(ApiError::NotFound)?;
        // the key the question was kept with, not one made again now: a question asked with an older prompt has
        // its answer under the key of then. That question's answer only: the run's others keep theirs. False
        // when there was no answer to forget.
        Ok(library.forget_key(Feature::Answers, &AnswerKey::from_stored(asked.key))?)
    })
    .await?;
    Ok(Json(serde_json::json!({ "ok": true, "forgotten": forgotten })))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::api::guard::config::tests::STRONG;
    use crate::api::guard::config::{Environment, Listen};
    use crate::api::guard::security::LARGEST_BODY;
    use crate::api::routes::Engine;
    use crate::api::tests::served;
    use crate::library::papers::passages::fixtures::pdf_with;
    use crate::library::papers::tests::paper;
    use crate::library::{Known, Slot, Subject};
    use crate::settings::Choices;

    const AT: &str = "127.0.0.1:8765";
    const QUESTION: &str = "Does merging count shared directions twice?";

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

    /// A data folder with three papers (two with a PDF, one without) and a run of them with two communities.
    fn data() -> tempfile::TempDir {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(&data.path().join("library")).unwrap();
        let mut unread = paper("icml-2026-3");
        unread.r#abstract = "Merging fine-tuned models inflates the spectrum of shared directions.".to_owned();
        library.add_papers(&[paper("2601.00001"), paper("2601.00002"), unread], None, Known::Keep).unwrap();
        let merging = ["Introduction", "We show that merging task vectors counts shared directions twice."];
        library.keep_pdf("2601.00001", &pdf_with(&merging)).unwrap();
        let distilling = ["Method", "Our method distils a large teacher into a small student without labels."];
        library.keep_pdf("2601.00002", &pdf_with(&distilling)).unwrap();
        let run = data.path().join("runs").join("r1");
        std::fs::create_dir_all(&run).unwrap();
        let nodes = json!({"nodes": [{"id": "2601.00001"}, {"id": "2601.00002"}, {"id": "icml-2026-3"}], "edges": []});
        std::fs::write(run.join("graph.json"), nodes.to_string()).unwrap();
        let tree = json!({"communities": [
            {"tree_path": "c0", "members": [0, 2], "children": []},
            {"tree_path": "c1", "members": [1], "children": []}]});
        std::fs::write(run.join("communities.json"), tree.to_string()).unwrap();
        data
    }

    fn engine(data: &tempfile::TempDir) -> Engine {
        Engine::new(data.path().to_path_buf(), choices())
    }

    fn router(engine: Engine) -> axum::Router {
        let pairs = [("CONSTELLATE_TOKEN", STRONG), ("CONSTELLATE_ORIGIN", "http://127.0.0.1:3000")];
        let listen = Listen::from_environment(&Environment::from_pairs(pairs)).unwrap();
        served(&listen, AT, engine).0
    }

    /// A key, and an address where nothing listens: should anything be sent, it reaches no one.
    fn with_key(mut engine: Engine) -> Engine {
        engine.set_claude_key("test-key-not-real-0123456789", Some("http://127.0.0.1:1"));
        engine
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

    fn query(text: &str) -> String {
        url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
    }

    #[tokio::test]
    async fn the_passages_found_come_with_their_evidence_and_the_coverage_of_the_search() {
        let data = data();
        let router = router(engine(&data));
        let path = format!("/api/runs/r1/passages/found?q={}", query("merging shared directions"));
        let (status, found) = ask(&router, "GET", &path, None).await;
        assert_eq!(status, StatusCode::OK, "{found}");
        assert_eq!(found["state"], "ready");
        assert_eq!(
            found["coverage"],
            json!({"papers": 3, "full_text": 2, "capped": 0, "no_pdf": 1, "unreadable": 0, "missing": 0})
        );
        let passages = found["passages"].as_array().unwrap();
        let read = passages.iter().find(|found| found["evidence"]["paper"] == "2601.00001").unwrap();
        assert_eq!(read["evidence"]["established"]["kind"], "verified");
        assert_eq!(read["evidence"]["established"]["address"]["page"], 1);
        assert!(read["evidence"]["quote"].as_str().unwrap().contains("counts shared directions twice"));
        assert_eq!(read["cited"], false);
        assert!(!passages.iter().any(|found| found["evidence"]["paper"] == "2601.00002"));
        // a community's search is over its papers only
        let path = format!("/api/runs/r1/passages/found?q={}&community=c1", query("merging shared directions"));
        let (_, only) = ask(&router, "GET", &path, None).await;
        assert_eq!(only["coverage"]["papers"], 1);
        assert!(only["passages"].as_array().unwrap().is_empty());
        // a paper without a PDF is found by its abstract
        let (_, record) = ask(&router, "GET", "/api/runs/r1/passages/found?q=spectrum", None).await;
        assert_eq!(record["passages"][0]["evidence"]["paper"], "icml-2026-3");
        assert_eq!(record["passages"][0]["evidence"]["established"]["kind"], "abstract");
    }

    #[tokio::test]
    async fn a_pdf_kept_after_the_index_was_built_is_searched_even_when_its_abstract_matched_nothing() {
        let data = data();
        let router = router(engine(&data));
        let path = format!("/api/runs/r1/passages/found?q={}", query("calibrate thermometers"));
        let (_, before) = ask(&router, "GET", &path, None).await;
        assert_eq!((before["state"].clone(), before["coverage"]["no_pdf"].clone()), (json!("ready"), json!(1)));
        assert!(before["passages"].as_array().unwrap().is_empty(), "{before}");

        let library = Library::open(&data.path().join("library")).unwrap();
        let read = ["Method", "We calibrate the thermometers before every merging experiment we report."];
        library.keep_pdf("icml-2026-3", &pdf_with(&read)).unwrap();
        let (_, after) = ask(&router, "GET", &path, None).await;
        assert_eq!(after["state"], "ready", "{after}");
        assert_eq!((after["coverage"]["full_text"].clone(), after["coverage"]["no_pdf"].clone()), (json!(3), json!(0)));
        assert_eq!(after["passages"][0]["evidence"]["paper"], "icml-2026-3", "{after}");
        assert_eq!(after["passages"][0]["evidence"]["established"]["kind"], "verified");
    }

    #[tokio::test]
    async fn a_search_of_nothing_or_too_much_or_of_no_run_is_refused() {
        let data = data();
        let router = router(engine(&data));
        let long = "x".repeat(LONGEST_QUERY + 1);
        for (path, status) in [
            (format!("/api/runs/r1/passages/found?q={long}"), StatusCode::BAD_REQUEST),
            ("/api/runs/r1/passages/found?q=%20".to_owned(), StatusCode::BAD_REQUEST),
            ("/api/runs/r1/passages/found".to_owned(), StatusCode::BAD_REQUEST),
            ("/api/runs/none/passages/found?q=merging".to_owned(), StatusCode::NOT_FOUND),
            ("/api/runs/..%2Fdata/passages/found?q=merging".to_owned(), StatusCode::NOT_FOUND),
            ("/api/runs/r1/passages/found?q=merging&community=c9".to_owned(), StatusCode::NOT_FOUND),
            ("/api/runs/r1/answers/nothing-of-the-kind".to_owned(), StatusCode::NOT_FOUND),
        ] {
            assert_eq!(ask(&router, "GET", &path, None).await.0, status, "{path}");
        }
        let long = json!({"question": "x".repeat(LONGEST_QUESTION + 1), "accepted_usd": 1.0}).to_string();
        assert_eq!(ask(&router, "POST", "/api/runs/r1/answers", Some(long)).await.0, StatusCode::BAD_REQUEST);
        let huge = json!({"question": "x".repeat(LARGEST_BODY), "accepted_usd": 1.0}).to_string();
        assert_eq!(ask(&router, "POST", "/api/runs/r1/answers", Some(huge)).await.0, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(ask(&router, "DELETE", "/api/runs/r1/answers/none", None).await.0, StatusCode::NOT_FOUND);
        assert_eq!(ask(&router, "POST", "/api/runs/r1/answers/none/redo", None).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn another_provider_is_priced_asked_in_its_own_shape_kept_and_asked_again_when_the_provider_changes() {
        use crate::api::other_providers::{
            ACCEPTED, Chosen as Asked, MODEL, Scripted, accepting, call, choose, finished, priced_for, router, sent_to,
        };

        let data = data();
        let transport = Scripted::new(|_| {
            json!({"outcome": "answered", "statements": [{"statement": {"sentence": "Merging counts shared directions twice.", "about": "one_paper"},
                "quotes": [{"paper": "2601.00001", "quote": "merging task vectors counts shared directions twice"}]}]})
            .to_string()
        });
        let router = router(data.path(), Asked::OpenAi, &transport);
        let estimate = format!("/api/runs/r1/answers/estimate?q={}&community=c0", query(QUESTION));
        let mut sent = 0;
        let mut ids = Vec::new();
        for chosen in [Asked::OpenAi, Asked::ThisComputer] {
            choose(data.path(), chosen);
            let (_, told) = call(&router, "GET", &estimate, None).await;
            assert_eq!(
                (told["state"].clone(), told["kept"].clone(), told["api_key_set"].clone()),
                (json!("ready"), json!(false), json!(true)),
                "{told}"
            );
            assert_eq!(
                (told["provider"].clone(), told["model"].clone()),
                (json!(chosen.provider().id()), json!(MODEL))
            );
            priced_for(&told["cost"], chosen);
            assert_eq!(told["most_tokens"], told["cost"]["most_tokens"]);
            assert_eq!(transport.sent().len(), sent, "an estimate sends nothing");

            let mut asked = accepting(&told["cost"], ACCEPTED);
            asked["id"] = told["id"].clone();
            asked["question"] = json!(QUESTION);
            asked["community"] = json!("c0");
            let (status, started) = call(&router, "POST", "/api/runs/r1/answers", Some(asked.clone())).await;
            assert_eq!(status, StatusCode::OK, "{started}");
            let done = finished(&router, &started["job"]).await;
            assert_eq!(done["state"], "done", "{done}");
            let made = json!({"kind": "feature", "feature": "answers", "subject": {"community": {"run": "r1", "community": "c0"}}});
            assert_eq!((done["outcome"].clone(), done["result"].clone()), (made, Value::Null), "no run to open");
            let all = transport.sent();
            sent_to(&all[sent..], chosen);
            sent = all.len();

            let id = started["id"].as_str().unwrap().to_owned();
            let (_, view) = call(&router, "GET", &format!("/api/runs/r1/answers/{id}"), None).await;
            assert_eq!(view["state"], "answered", "{view}");
            assert_eq!(view["statements"][0]["evidence"][0]["established"]["kind"], "verified");
            let (_, again) = call(&router, "POST", "/api/runs/r1/answers", Some(asked)).await;
            assert_eq!((again["id"].as_str(), again["job"].clone()), (Some(id.as_str()), Value::Null), "kept: no job");
            assert_eq!(transport.sent().len(), sent, "kept: not asked again");
            ids.push(id);
        }
        assert_ne!(ids[0], ids[1], "one question asked of two providers is two answers");
        // each is listed, and its own answer found, whichever provider is chosen now
        let (_, listed) = call(&router, "GET", "/api/runs/r1/answers", None).await;
        assert!(listed.as_array().unwrap().iter().all(|asked| asked["answered"] == true), "{listed}");
    }

    #[tokio::test]
    async fn asking_a_model_with_no_price_without_naming_what_was_accepted_is_refused_and_sends_nothing() {
        use crate::api::other_providers::{Chosen as Asked, MODEL, Scripted, call, choose, router};

        let data = data();
        let transport = Scripted::new(|_| json!({"outcome": "not_in_papers", "statements": []}).to_string());
        let router = router(data.path(), Asked::ThisComputer, &transport);
        choose(data.path(), Asked::ThisComputer);
        let estimate = format!("/api/runs/r1/answers/estimate?q={}&community=c0", query(QUESTION));
        let (_, told) = call(&router, "GET", &estimate, None).await;
        assert_eq!((told["state"].clone(), told["cost"]["most_usd"].clone()), (json!("ready"), Value::Null), "{told}");
        let asked = json!({"id": told["id"], "question": QUESTION, "community": "c0", "provider": "compatible", "model": MODEL});
        let (status, said) = call(&router, "POST", "/api/runs/r1/answers", Some(asked)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{said}");
        assert!(transport.sent().is_empty());
    }

    #[tokio::test]
    async fn the_estimate_says_what_one_call_would_cost_and_sends_nothing() {
        let data = data();
        let router = router(engine(&data));
        let path = format!("/api/runs/r1/answers/estimate?q={}", query(QUESTION));
        let (status, told) = ask(&router, "GET", &path, None).await;
        assert_eq!(status, StatusCode::OK, "{told}");
        assert_eq!((told["state"].clone(), told["kept"].clone()), (json!("ready"), json!(false)));
        assert_eq!((told["model"].clone(), told["api_key_set"].clone()), (json!("claude-opus-5-5"), json!(false)));
        assert_eq!(told["cost"]["calls"], 1);
        assert_eq!(told["cost"]["counting"]["kind"], "assumed");
        assert!(told["cost"]["usd"].as_f64().unwrap() > 0.0);
        assert!(told["most_usd"].as_f64().unwrap() >= told["cost"]["usd"].as_f64().unwrap());
        assert_eq!(told["prompt_cap_tokens"], PROMPT_TOKENS);
        assert!(told["passages"].as_u64().unwrap() >= 1);
        assert_eq!(told["id"].as_str().unwrap().len(), 64);
        let library = Library::open(&data.path().join("library")).unwrap();
        assert!(library.questions("r1").unwrap().is_empty(), "an estimate asks nothing and keeps nothing");
    }

    /// The id of the estimate of `question` over the whole run, as the interface reads it before the user clicks.
    async fn estimated(router: &axum::Router, question: &str) -> Value {
        let (status, told) =
            ask(router, "GET", &format!("/api/runs/r1/answers/estimate?q={}", query(question)), None).await;
        assert_eq!((status, told["state"].clone()), (StatusCode::OK, json!("ready")), "{told}");
        told["id"].clone()
    }

    #[tokio::test]
    async fn asking_needs_a_key_a_free_job_and_the_cost_the_user_accepted() {
        let data = data();
        let id = estimated(&router(engine(&data)), QUESTION).await;
        let request = |accepted: Value| {
            json!({"id": id, "question": QUESTION, "model": "claude-opus-5-5", "accepted_usd": accepted}).to_string()
        };
        let (status, told) =
            ask(&router(engine(&data)), "POST", "/api/runs/r1/answers", Some(request(json!(5.0)))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(told["error"].as_str().unwrap().contains("No API key"), "{told}");

        let router = router(with_key(engine(&data)));
        let (status, told) = ask(&router, "POST", "/api/runs/r1/answers", Some(request(json!(0.000_001)))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "less than it can cost was accepted: {told}");
        assert_eq!(
            ask(&router, "POST", "/api/runs/r1/answers", Some(request(Value::Null))).await.0,
            StatusCode::BAD_REQUEST
        );
        let nothing = "quantum chromodynamics";
        let none = json!({"id": estimated(&router, nothing).await, "question": nothing, "model": "claude-opus-5-5", "accepted_usd": 5.0})
            .to_string();
        let (status, told) = ask(&router, "POST", "/api/runs/r1/answers", Some(none)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(told["error"].as_str().unwrap().contains("no passage"), "{told}");
        let other =
            json!({"id": id, "question": QUESTION, "model": "claude-haiku-5-5", "accepted_usd": 5.0}).to_string();
        let (status, told) = ask(&router, "POST", "/api/runs/r1/answers", Some(other)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "the cost shown was for another model: {told}");
        let library = Library::open(&data.path().join("library")).unwrap();
        assert!(library.questions("r1").unwrap().is_empty(), "nothing refused is listed");

        // a job of another kind is running: busy
        let busy = with_key(engine(&data));
        let stop = crate::jobs::Cancel::new();
        let waiting = stop.clone();
        busy.job
            .start("a run", move |_| {
                while !waiting.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(crate::jobs::Finished::Nothing)
            })
            .unwrap();
        let busy = self::router(busy);
        let (status, told) = ask(&busy, "POST", "/api/runs/r1/answers", Some(request(json!(5.0)))).await;
        stop.cancel();
        assert_eq!(status, StatusCode::CONFLICT, "{told}");
        assert!(library.questions("r1").unwrap().is_empty());

        // accepted, and free: the job starts and the question is listed
        let (status, started) = ask(&router, "POST", "/api/runs/r1/answers", Some(request(json!(5.0)))).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        assert!(started["job"].as_u64().is_some());
        let (_, listed) = ask(&router, "GET", "/api/runs/r1/answers", None).await;
        assert_eq!(listed[0]["id"], started["id"]);
        assert_eq!((listed[0]["question"].clone(), listed[0]["answered"].clone()), (json!(QUESTION), json!(false)));
        assert_eq!(listed[0]["community"], Value::Null);
    }

    #[tokio::test]
    async fn asking_is_refused_when_the_question_or_the_passages_changed_since_the_estimate() {
        let data = data();
        let router = router(with_key(engine(&data)));
        let id = estimated(&router, QUESTION).await;
        let request = |id: &Value, question: &str| {
            Some(json!({"id": id, "question": question, "model": "claude-opus-5-5", "accepted_usd": 5.0}).to_string())
        };
        let refused = |told: &Value| told["error"].as_str().unwrap().contains("changed since the estimate");

        // the question was edited after the estimate was shown
        let (status, told) = ask(&router, "POST", "/api/runs/r1/answers", request(&id, "Why does merging help?")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{told}");
        assert!(refused(&told), "{told}");

        // a PDF was read since: the paper is now answered from its full text, not its abstract
        let library = Library::open(&data.path().join("library")).unwrap();
        let read = ["Results", "Merging fine-tuned models counts shared directions twice in every layer we measured."];
        library.keep_pdf("icml-2026-3", &pdf_with(&read)).unwrap();
        let (status, told) = ask(&router, "POST", "/api/runs/r1/answers", request(&id, QUESTION)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{told}");
        assert!(refused(&told), "{told}");
        let id = estimated(&router, QUESTION).await;

        // a paper was added to the run since
        let mut added = paper("2601.00004");
        added.r#abstract = "Merging counts shared directions twice, which we correct.".to_owned();
        library.add_papers(&[added], None, Known::Keep).unwrap();
        let nodes = json!({"nodes": [{"id": "2601.00001"}, {"id": "2601.00002"}, {"id": "icml-2026-3"}, {"id": "2601.00004"}], "edges": []});
        std::fs::write(data.path().join("runs").join("r1").join("graph.json"), nodes.to_string()).unwrap();
        let (status, told) = ask(&router, "POST", "/api/runs/r1/answers", request(&id, QUESTION)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{told}");
        assert!(refused(&told), "{told}");
        assert!(library.questions("r1").unwrap().is_empty(), "nothing refused is listed");

        // asked for again, the estimate is what is accepted
        let id = estimated(&router, QUESTION).await;
        let (status, started) = ask(&router, "POST", "/api/runs/r1/answers", request(&id, QUESTION)).await;
        assert_eq!((status, started["id"].clone()), (StatusCode::OK, id), "{started}");
    }

    #[tokio::test]
    async fn a_redo_forgets_the_answer_kept_with_the_question_even_one_of_an_older_prompt_and_says_when_there_was_none()
    {
        let data = data();
        let library = Library::open(&data.path().join("library")).unwrap();
        // a question kept with the key of an older prompt: not the key it would be made with now
        let then = AnswerKey::from_stored("a".repeat(64));
        let asked = Asked {
            run: "r1".to_owned(),
            key: then.as_str().to_owned(),
            community: None,
            question: QUESTION.to_owned(),
            provider: crate::model::language::Provider::Anthropic,
            model: "claude-opus-5-5".to_owned(),
            passages: Vec::new(),
            asked_at: String::new(),
        };
        library.keep_question(&asked).unwrap();
        let slot = Slot {
            feature: Feature::Answers,
            key: then.clone(),
            subject: crate::library::Subject::Run("r1".to_owned()),
        };
        library.keep(&slot, "claude-opus-5-5", 0, &json!({"outcome": "not_in_papers", "statements": []})).unwrap();
        let router = router(engine(&data));
        let path = format!("/api/runs/r1/answers/{}/redo", then.as_str());
        let (status, redone) = ask(&router, "POST", &path, None).await;
        assert_eq!((status, redone["forgotten"].clone()), (StatusCode::OK, json!(true)), "{redone}");
        assert!(
            library.kept::<Value>(Feature::Answers, &then).unwrap().is_none(),
            "the answer the listing showed is gone"
        );
        assert_eq!(ask(&router, "POST", &path, None).await.1["forgotten"], json!(false), "nothing left to forget");
    }

    #[test]
    fn a_question_with_a_kept_answer_stands_as_its_answer_checked_whatever_job_runs() {
        let counts = AnswerCounts { written: 2, shown: 0, not_found: 2, malformed: 0 };
        let asking = about(Feature::Answers, "k");
        let unverified = Standing::of(Some(Answer::Unverified { counts }), Some(&about(Feature::Answers, "x")), "k");
        assert_eq!(unverified.state(), AnswerState::Unverified);
        assert_eq!(Standing::of(Some(Answer::NotInPapers), None, "k").state(), AnswerState::NotInPapers);
        let answered = Answer::Answered { statements: Vec::new(), counts };
        assert_eq!(Standing::of(Some(answered), Some(&asking), "k").state(), AnswerState::Answered);
    }

    fn about(feature: Feature, item: &str) -> About {
        About { feature, subject: Subject::Run("r1".to_owned()), item: Some(item.to_owned()) }
    }

    #[test]
    fn a_question_without_an_answer_is_being_asked_only_while_its_own_job_runs() {
        assert_eq!(Standing::of(None, Some(&about(Feature::Answers, "k")), "k"), Standing::Asking);
        assert_eq!(Standing::of(None, Some(&about(Feature::Answers, "other")), "k"), Standing::Unanswered);
        let other = about(Feature::Stances, "k");
        assert_eq!(Standing::of(None, Some(&other), "k"), Standing::Unanswered, "another feature's job");
        let nothing = About { item: None, ..about(Feature::Answers, "k") };
        assert_eq!(Standing::of(None, Some(&nothing), "k"), Standing::Unanswered);
        assert_eq!(Standing::of(None, None, "k"), Standing::Unanswered, "no job running: failed, stopped or forgotten");
    }

    #[tokio::test]
    async fn a_question_just_started_is_shown_as_being_asked_before_its_job_has_logged_a_line() {
        let data = data();
        let engine = engine(&data);
        let library = engine.library().unwrap();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let key = "a-question-id".to_owned();
        let asked = Asked {
            run: "r1".to_owned(),
            key: key.clone(),
            community: None,
            question: QUESTION.to_owned(),
            provider: Provider::Anthropic,
            model: "claude-opus-5-5".to_owned(),
            passages: Vec::new(),
            asked_at: String::new(),
        };
        library.keep_question(&asked).unwrap();
        // started as the route starts it: what it is about is set before its work runs at all
        let about =
            About { feature: Feature::Answers, subject: Subject::Run("r1".to_owned()), item: Some(key.clone()) };
        engine
            .job
            .start_about(JOB, about, move |_| {
                wait.recv().ok();
                Ok(Finished::Nothing)
            })
            .unwrap();
        let router = router(engine);
        let (status, view) = ask(&router, "GET", &format!("/api/runs/r1/answers/{key}"), None).await;
        assert_eq!(status, StatusCode::OK, "{view}");
        assert_eq!(view["state"], "asking");
        let (_, told) = ask(&router, "GET", "/api/status", None).await;
        assert_eq!((told["feature"].as_str(), told["item"].as_str()), (Some("answers"), Some(key.as_str())));
        assert_eq!(told["subject"], json!({"run": "r1"}));
        go.send(()).unwrap();
    }

    #[tokio::test]
    async fn a_kept_answer_is_shown_checked_again_and_can_be_redone_or_deleted() {
        let data = data();
        let shared: Shared = Arc::new(engine(&data));
        let library = shared.library().unwrap();
        let scope = Scope { run: "r1".to_owned(), community: Some("c0".to_owned()) };
        let Ready::Prepared { prepared, .. } = prepared(&shared, &library, &scope, QUESTION).unwrap() else { panic!() };
        library
            .keep_question(&Asked {
                run: "r1".to_owned(),
                key: prepared.key.as_str().to_owned(),
                community: scope.community.clone(),
                question: QUESTION.to_owned(),
                provider: prepared.chosen.provider(),
                model: prepared.chosen.model().to_owned(),
                passages: prepared.given.clone(),
                asked_at: String::new(),
            })
            .unwrap();
        let claim = |sentence: &str, paper: &str, quote: &str| json!({"statement": {"sentence": sentence, "about": "one_paper"}, "quotes": [{"paper": paper, "quote": quote}]});
        let said = Said {
            outcome: SaidOutcome::Answered,
            statements: vec![
                claim(
                    "Merging counts shared directions twice.",
                    "2601.00001",
                    "merging task vectors counts shared directions twice",
                ),
                claim("Merging always helps.", "icml-2026-3", "merging always improves every task by ten points"),
                claim("The spectrum inflates.", "icml-2026-3", "inflates the spectrum of shared directions"),
            ],
        };
        let slot = Slot { feature: Feature::Answers, key: prepared.key.clone(), subject: prepared.subject.clone() };
        library.keep(&slot, prepared.chosen.model(), answers::PROMPT, &said).unwrap();

        let router = router(engine(&data));
        let path = format!("/api/runs/r1/answers/{}", prepared.key.as_str());
        let (status, view) = ask(&router, "GET", &path, None).await;
        assert_eq!(status, StatusCode::OK, "{view}");
        assert_eq!(view["state"], "answered", "{view}");
        crate::api::contract::assert_fits("AnswerView", &view, &[]);
        assert_eq!(view["statements"].as_array().unwrap().len(), 2, "{view}");
        assert_eq!(view["statements"][0]["statement"]["sentence"], "Merging counts shared directions twice.");
        assert_eq!(view["statements"][0]["evidence"][0]["established"]["kind"], "verified");
        assert_eq!(view["counts"], json!({"written": 3, "shown": 2, "not_found": 1, "malformed": 0}));
        assert_eq!(view["cited_papers"], json!(["2601.00001", "icml-2026-3"]));
        assert!(view["passages"].as_array().unwrap().iter().any(|found| found["cited"] == true));
        let (_, listed) = ask(&router, "GET", "/api/runs/r1/answers", None).await;
        assert_eq!((listed[0]["answered"].clone(), listed[0]["community"].clone()), (json!(true), json!("c0")));
        // the estimate knows it is kept: nothing to pay
        let estimate = format!("/api/runs/r1/answers/estimate?q={}&community=c0", query(QUESTION));
        let (_, told) = ask(&router, "GET", &estimate, None).await;
        assert_eq!((told["kept"].clone(), told["id"].clone()), (json!(true), json!(prepared.key.as_str())));
        assert_eq!(told["cost"], Value::Null);

        let (status, redone) = ask(&router, "POST", &format!("{path}/redo"), None).await;
        assert_eq!((status, redone["forgotten"].clone()), (StatusCode::OK, json!(true)));
        assert_eq!(ask(&router, "GET", &path, None).await.1["state"], "unanswered");
        assert_eq!(ask(&router, "DELETE", &path, None).await.0, StatusCode::OK);
        assert_eq!(ask(&router, "GET", &path, None).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_same_question_started_from_several_tabs_at_once_is_asked_once_and_stays_listed() {
        use crate::api::other_providers::{ACCEPTED, Chosen as Asked, Scripted, accepting, call, choose, finished};

        let data = data();
        let transport = Scripted::new(|_| {
            std::thread::sleep(Duration::from_millis(300)); // still being asked while the other clicks arrive
            json!({"outcome": "answered", "statements": [{"statement": {"sentence": "Merging counts shared directions twice.", "about": "one_paper"},
                "quotes": [{"paper": "2601.00001", "quote": "merging task vectors counts shared directions twice"}]}]})
            .to_string()
        });
        choose(data.path(), Asked::OpenAi);
        let router = crate::api::other_providers::router(data.path(), Asked::OpenAi, &transport);
        let estimate = format!("/api/runs/r1/answers/estimate?q={}", query(QUESTION));
        let (_, told) = call(&router, "GET", &estimate, None).await;
        let mut request = accepting(&told["cost"], ACCEPTED);
        request["id"] = told["id"].clone();
        request["question"] = json!(QUESTION);
        let clicks: Vec<_> = (0..6)
            .map(|_| {
                let (router, request) = (router.clone(), request.clone());
                tokio::spawn(async move { call(&router, "POST", "/api/runs/r1/answers", Some(request)).await })
            })
            .collect();
        let mut jobs = Vec::new();
        for click in clicks {
            let (status, started) = click.await.unwrap();
            assert!(status == StatusCode::OK || status == StatusCode::CONFLICT, "{status}: {started}");
            if status == StatusCode::OK && !started["job"].is_null() {
                jobs.push(started["job"].clone());
            }
        }
        assert_eq!(jobs.len(), 1, "one job asks it");
        assert_eq!(finished(&router, &jobs[0]).await["state"], "done");
        assert_eq!(transport.sent().len(), 1, "asked once");
        let (_, listed) = call(&router, "GET", "/api/runs/r1/answers", None).await;
        assert_eq!(listed.as_array().unwrap().len(), 1, "the question being asked is still listed: {listed}");
        assert_eq!(listed[0]["answered"], true, "{listed}");
    }

    #[tokio::test]
    async fn every_answer_of_these_routes_fits_its_type_in_the_contracts() {
        use crate::api::contract::assert_fits;

        let data = data();
        let router = router(with_key(engine(&data)));
        let (_, found) =
            ask(&router, "GET", &format!("/api/runs/r1/passages/found?q={}", query("merging")), None).await;
        assert!(!found["passages"].as_array().unwrap().is_empty(), "{found}");
        assert_fits("PassagesFound", &found, &[]);
        assert_fits("SearchCoverage", &found["coverage"], &[]);
        assert_fits("PassageFound", &found["passages"][0], &[]);

        let (_, told) =
            ask(&router, "GET", &format!("/api/runs/r1/answers/estimate?q={}", query(QUESTION)), None).await;
        assert_eq!(told["state"], "ready", "{told}");
        assert_fits("AnswerEstimate", &told, &[]);
        assert_fits("AskingCost", &told["cost"], &[]);

        let request = json!({"id": told["id"], "question": QUESTION, "provider": told["provider"], "model": told["model"], "accepted_usd": told["most_usd"]});
        let (status, started) = ask(&router, "POST", "/api/runs/r1/answers", Some(request.to_string())).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        assert_fits("AnswerStarted", &started, &[]);

        let (_, listed) = ask(&router, "GET", "/api/runs/r1/answers", None).await;
        assert_fits("AnswerSummary", &listed[0], &[]);
        let (_, view) =
            ask(&router, "GET", &format!("/api/runs/r1/answers/{}", started["id"].as_str().unwrap()), None).await;
        assert_fits("AnswerView", &view, &[]);
        assert_fits("AnswerCounts", &view["counts"], &[]);
    }
}
