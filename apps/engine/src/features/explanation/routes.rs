//! "Explain this" in the reader: a passage of a paper explained by the model, the paper's own definitions of what
//! it uses, and passages of nearby papers about the same thing ([`crate::features::explanation`]).
//!
//! ```text
//! POST   /api/papers/{id}/explanation/prepare  {address}: what explaining the passage there involves, free:
//!                                              the passage, the definitions found, the kept explanation if
//!                                              there is one, else the estimate ([`Prepared`])
//! POST   /api/papers/{id}/explanation          {address, provider, model, accepted_usd | accepted_tokens}: ask,
//!                                              as a tracked job ({started, job}), refused when the model or its
//!                                              worst case changed since the estimate was shown; nothing is asked
//!                                              when an explanation is kept
//! DELETE /api/papers/{id}/explanation          {address}: redo: the kept explanation is forgotten
//! GET    /api/papers/{id}/explanations         the passages with an explanation kept for the model chosen
//! POST   /api/runs/{run}/papers/{id}/related   {address}: passages of nearby papers sharing its words ([`Related`])
//! ```
//!
//! `address` is a passage's address as `GET /api/papers/{id}/passages` gives it (the reader resolves a selection
//! to one with the locate route first): the passage explained is the one that address starts in. The model is
//! the one chosen in the settings, of the provider chosen there ([`Engine::chosen`](crate::api::routes::Engine::chosen)).

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::routing::{MethodRouter, get, post};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::routes::{ApiError, Shared, on_the_side, one_name};
use crate::evidence::{Evidence, PaperText};
use crate::features::explanation::definitions::{Definition, Term, definitions};
use crate::features::explanation::related::{Candidate, Place, Searched, related};
use crate::features::explanation::{
    self, Checked, NEARBY_PAPERS, PURPOSE, Prompt, Question, Said, estimate, explain, item, prompt, schema, slot,
};
use crate::jobs::Finished;
use crate::library::core::store::read_json;
use crate::library::{Feature, Library, PaperReading, Subject};
use crate::model::batch::Asking;
use crate::model::chosen::{Accepted, Chosen, Terms, budget};
use crate::model::language::{Estimate, Provider};
use crate::text::passages::{Address, Kind, Passages};

/// The longest section heading taken in an address: a heading is a line, not a page.
const LONGEST_SECTION: usize = 1_000;

/// The routes of this module, for [`crate::api::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![
        ("/api/papers/{id}/explanation/prepare", post(prepare)),
        ("/api/papers/{id}/explanation", post(start).delete(redo)),
        ("/api/papers/{id}/explanations", get(explained)),
        ("/api/runs/{run}/papers/{id}/related", post(find_related)),
    ]
}

/// `ExplanationRequest` in the contracts.
#[derive(Deserialize)]
struct Request {
    address: Address,
}

fn take(body: Result<Json<Request>, JsonRejection>) -> Result<Address, ApiError> {
    let Json(Request { address }) = body?;
    if address.section.chars().nth(LONGEST_SECTION).is_some() {
        return Err(ApiError::Invalid(format!("the section is longer than {LONGEST_SECTION} characters")));
    }
    Ok(address)
}

/// Why a paper has no text to explain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum NoText {
    /// The library keeps no PDF of it.
    NoPdf,
    /// Its PDF could not be read.
    Unreadable,
}

/// A paper's passages, or why it has none; not found for a paper the library does not have.
fn read(library: &Library, paper: &str) -> Result<Result<Passages, NoText>, ApiError> {
    match library.paper_texts(&[paper])?.pop() {
        Some(PaperReading::Read(passages)) => Ok(Ok(passages)),
        Some(PaperReading::NoPdf { .. }) => Ok(Err(NoText::NoPdf)),
        Some(PaperReading::Unreadable { .. }) => Ok(Err(NoText::Unreadable)),
        Some(PaperReading::NotInLibrary) | None => Err(ApiError::NotFound),
    }
}

/// The passage `address` starts in.
fn passage_at(passages: &Passages, address: &Address) -> Result<usize, ApiError> {
    passages
        .passages
        .iter()
        .position(|passage| {
            passage.address.page == address.page
                && passage.address.start <= address.start
                && address.start < passage.address.end
        })
        .ok_or(ApiError::NotFound)
}

/// Everything about one passage's explanation that is known before asking.
struct Opened {
    library: Arc<Library>,
    paper: String,
    passages: Passages,
    index: usize,
    question: Question,
    chosen: Chosen,
    definitions: Vec<Definition>,
    prompt: Prompt,
}

impl Opened {
    fn address(&self) -> &Address {
        &self.passages.passages[self.index].address
    }

    fn slot(&self) -> crate::library::Slot {
        slot(&self.paper, &self.chosen, self.address(), self.question)
    }

    fn kept(&self) -> Result<Option<crate::library::Kept<Said>>, ApiError> {
        Ok(self.library.kept::<Said>(Feature::Explanation, &self.slot().key)?)
    }
}

fn open(engine: &Shared, paper: &str, address: &Address) -> Result<Result<Opened, NoText>, ApiError> {
    let library = engine.library()?;
    let record = library.paper(paper)?.ok_or(ApiError::NotFound)?;
    let passages = match read(&library, paper)? {
        Ok(passages) => passages,
        Err(why) => return Ok(Err(why)),
    };
    let index = passage_at(&passages, address)?;
    let question = Question::of(passages.passages[index].kind).ok_or_else(|| {
        ApiError::Invalid(
            "a title or a heading cannot be explained: choose a paragraph, an equation or a caption".into(),
        )
    })?;
    let chosen = engine.chosen()?;
    let found = definitions(&passages, index);
    let prompt = prompt(paper, &record.title, &record.r#abstract, &passages, index, &found);
    Ok(Ok(Opened { library, paper: paper.to_owned(), passages, index, question, chosen, definitions: found, prompt }))
}

/// A term the passage uses and the sentence that introduces it, as evidence: `DefinedTerm` in the contracts.
#[derive(Serialize)]
struct Defined {
    term: Term,
    evidence: Evidence,
}

/// The passage explained.
#[derive(Serialize)]
struct Explaining {
    kind: Kind,
    address: Address,
}

/// A kept explanation, checked again now: `Explanation` in the contracts.
#[derive(Serialize)]
struct Explained {
    /// The model that wrote it, as the API named it.
    model: String,
    kept_at: String,
    #[serde(flatten)]
    checked: Checked,
}

/// The answer to `prepare`: `ExplanationPrepared` in the contracts.
#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum Prepared {
    Ready(Box<Ready>),
    /// The paper has no text to explain.
    NoText {
        why: NoText,
    },
}

/// A passage that can be explained, and what explaining it involves.
#[derive(Serialize)]
struct Ready {
    paper: String,
    passage: Explaining,
    question: Question,
    /// The passage's text as extracted may be garbled (an equation, a table): the answer must say so.
    garbled: bool,
    /// The model that would be asked, of its provider: the one chosen in the settings.
    provider: Provider,
    model: String,
    /// Whether it can be asked at all (its provider's key is set); never the key.
    api_key_set: bool,
    definitions: Vec<Defined>,
    #[serde(flatten)]
    answer: Answer,
}

/// The passage's explanation kept, or what asking for one would cost: one or the other, never both or neither.
/// Told as the two fields `explained` and `estimate`, the other null, as the interface reads them.
#[derive(Serialize)]
#[serde(untagged)]
enum Answer {
    /// Kept from before: shown at no cost.
    Kept { explained: Explained, estimate: Null },
    /// Not asked yet: what asking would cost (`AskingCost` in the contracts).
    NotAsked { explained: Null, estimate: Estimate },
}

/// A field told as null.
#[derive(Serialize)]
struct Null;

fn prepared(opened: &Opened) -> Result<Prepared, ApiError> {
    let passage = &opened.passages.passages[opened.index];
    let defined = opened
        .definitions
        .iter()
        .filter_map(|found| {
            let evidence = Evidence::at(&opened.paper, &opened.passages, &found.address)?;
            Some(Defined { term: found.term.clone(), evidence })
        })
        .collect();
    let answer = match opened.kept()? {
        // its quotes checked again now: a kept answer is the model's words, never evidence
        Some(kept) => Answer::Kept {
            explained: Explained {
                checked: explanation::check(&kept.answer, &opened.paper, &opened.passages),
                model: kept.model,
                kept_at: kept.kept_at,
            },
            estimate: Null,
        },
        None => {
            let schema = schema();
            let item = item(opened.slot(), &opened.chosen, &opened.prompt, &schema);
            Answer::NotAsked { explained: Null, estimate: estimate(&opened.library, &item, &opened.chosen)?.cost }
        }
    };
    Ok(Prepared::Ready(Box::new(Ready {
        paper: opened.paper.clone(),
        passage: Explaining { kind: passage.kind, address: passage.address.clone() },
        question: opened.question,
        garbled: Question::garbled(passage.kind),
        provider: opened.chosen.provider(),
        model: opened.chosen.model().to_owned(),
        api_key_set: opened.chosen.can_ask(),
        definitions: defined,
        answer,
    })))
}

async fn prepare(
    State(engine): State<Shared>,
    Path(paper): Path<String>,
    body: Result<Json<Request>, JsonRejection>,
) -> Result<Json<Prepared>, ApiError> {
    let address = take(body)?;
    let answer = on_the_side(move || match open(&engine, &paper, &address)? {
        Ok(opened) => prepared(&opened),
        Err(why) => Ok(Prepared::NoText { why }),
    })
    .await?;
    Ok(Json(answer))
}

/// The answer to starting: whether a job was started, and which.
#[derive(Serialize)]
struct Started {
    started: bool,
    /// The job's number (`Status.id`), to follow and to stop.
    job: Option<u64>,
}

fn no_text(why: NoText) -> ApiError {
    ApiError::Invalid(match why {
        NoText::NoPdf => "the library keeps no PDF of this paper, so it has no text to explain".to_owned(),
        NoText::Unreadable => "this paper's PDF could not be read, so it has no text to explain".to_owned(),
    })
}

/// `ExplanationStart` in the contracts: the passage, and the model (of its provider) and the worst case the
/// reader was shown, in dollars or, for a model with no price, in tokens, so that what is paid is never more than
/// what they agreed to.
#[derive(Deserialize)]
struct Start {
    address: Address,
    #[serde(default)]
    provider: Option<Provider>,
    model: String,
    #[serde(alias = "most_usd")]
    accepted_usd: Option<f64>,
    #[serde(default)]
    accepted_tokens: Option<u64>,
}

async fn start(
    State(engine): State<Shared>,
    Path(paper): Path<String>,
    body: Result<Json<Start>, JsonRejection>,
) -> Result<Json<Started>, ApiError> {
    let Json(seen) = body?;
    let address = take(Ok(Json(Request { address: seen.address.clone() })))?;
    let started = on_the_side(move || {
        let opened = open(&engine, &paper, &address)?.map_err(no_text)?;
        if opened.kept()?.is_some() {
            return Ok(Started { started: false, job: None });
        }
        let endpoint = opened.chosen.endpoint().map_err(|why| ApiError::Invalid(why.to_string()))?;
        opened.chosen.as_shown(seen.provider, &seen.model).map_err(|why| ApiError::Invalid(why.to_string()))?;
        let schema = schema();
        let shown = {
            let item = item(opened.slot(), &opened.chosen, &opened.prompt, &schema);
            estimate(&opened.library, &item, &opened.chosen)?
        };
        // never more than the worst case the reader was shown before the click
        let accepted =
            Accepted::of(seen.accepted_usd, seen.accepted_tokens).map_err(|why| ApiError::Invalid(why.to_string()))?;
        // naming nothing is not accepting: for a model with no price (a price shown is refused unless sent back) it
        // would run on a worst case never agreed to
        if accepted == Accepted::Shown && shown.cost.calls > 0 && shown.cost.most_usd.is_none() {
            return Err(ApiError::Invalid(
                "say what was accepted of the estimate shown (accepted_usd or accepted_tokens)".to_owned(),
            ));
        }
        let accepted =
            budget(&shown.cost, accepted, Terms::AsShown).map_err(|why| ApiError::Invalid(why.to_string()))?;
        let transport = engine.transport.clone();
        let about = crate::jobs::About {
            feature: Feature::Explanation,
            subject: Subject::Paper(opened.paper.clone()),
            item: None,
        };
        let job = engine.job.start_about(PURPOSE, about, move |tracker| {
            let item = item(opened.slot(), &opened.chosen, &opened.prompt, &schema);
            let asking = Asking {
                library: &opened.library,
                transport: &*transport,
                endpoint,
                recorder: &*opened.library,
                in_flight: 1,
            };
            explain(asking, &item, &accepted, tracker).map_err(|why| why.to_string())?;
            Ok(Finished::Feature { feature: Feature::Explanation, subject: Subject::Paper(opened.paper.clone()) })
        })?;
        Ok(Started { started: true, job: Some(job) })
    })
    .await?;
    Ok(Json(started))
}

async fn redo(
    State(engine): State<Shared>,
    Path(paper): Path<String>,
    body: Result<Json<Request>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let address = take(body)?;
    let forgotten = on_the_side(move || {
        let opened = open(&engine, &paper, &address)?.map_err(no_text)?;
        // that one explanation only: the paper's others stay
        Ok(opened.library.forget_key(Feature::Explanation, &opened.slot().key)?)
    })
    .await?;
    // false when there was no explanation to forget, said rather than a redo that did nothing
    Ok(Json(serde_json::json!({ "redone": forgotten })))
}

/// The passages of a paper with an explanation kept: `ExplainedPassages` in the contracts.
#[derive(Serialize)]
struct Marks {
    model: String,
    explained: Vec<Address>,
}

async fn explained(State(engine): State<Shared>, Path(paper): Path<String>) -> Result<Json<Marks>, ApiError> {
    let marks = on_the_side(move || {
        let library = engine.library()?;
        library.paper(&paper)?.ok_or(ApiError::NotFound)?;
        let chosen = engine.chosen()?;
        let model = chosen.model().to_owned();
        let Ok(passages) = read(&library, &paper)? else { return Ok(Marks { model, explained: Vec::new() }) };
        let mut explained = Vec::new();
        for passage in &passages.passages {
            let Some(question) = Question::of(passage.kind) else { continue };
            let key = slot(&paper, &chosen, &passage.address, question).key;
            if library.kept::<Said>(Feature::Explanation, &key)?.is_some() {
                explained.push(passage.address.clone());
            }
        }
        Ok(Marks { model, explained })
    })
    .await?;
    Ok(Json(marks))
}

/// The run's graph and community tree, as the pipeline writes them: which papers are where, and which are close.
#[derive(Deserialize)]
struct Graph {
    nodes: Vec<Node>,
    /// (node, node, similarity): each paper's few closest papers.
    #[serde(default)]
    edges: Vec<(usize, usize, f64)>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
}

#[derive(Deserialize)]
struct Communities {
    communities: Vec<Community>,
}

#[derive(Deserialize)]
struct Community {
    /// Indices into the graph's nodes.
    members: Vec<usize>,
    #[serde(default)]
    children: Vec<Community>,
}

/// The smallest community of `tree` that has `node`.
fn smallest(tree: &[Community], node: usize) -> Option<&Community> {
    tree.iter()
        .find(|community| community.members.contains(&node))
        .map(|community| smallest(&community.children, node).unwrap_or(community))
}

/// The papers of the run near `paper`, the nearest first: its neighbours in the graph by similarity, then the
/// other papers of its smallest community by id, at most [`NEARBY_PAPERS`]. Not found when the paper is not in
/// the run.
fn nearby(engine: &Shared, run: &str, paper: &str) -> Result<Vec<String>, ApiError> {
    let folder = engine.data.join("runs").join(one_name(run)?);
    let graph: Graph = read_json(&folder.join("graph.json"))?.ok_or(ApiError::NotFound)?;
    let node = graph.nodes.iter().position(|node| node.id == paper).ok_or(ApiError::NotFound)?;
    let mut neighbours: Vec<(usize, f64)> = graph
        .edges
        .iter()
        .filter_map(|&(from, to, similarity)| match (from == node, to == node) {
            (true, false) => Some((to, similarity)),
            (false, true) => Some((from, similarity)),
            _ => None,
        })
        .filter(|(other, _)| *other < graph.nodes.len())
        .collect();
    neighbours.sort_by(|a, b| b.1.total_cmp(&a.1).then(graph.nodes[a.0].id.cmp(&graph.nodes[b.0].id)));
    let mut ids: Vec<String> = Vec::new();
    let add = |id: &str, ids: &mut Vec<String>| {
        if id != paper && !ids.iter().any(|known| known == id) && ids.len() < NEARBY_PAPERS {
            ids.push(id.to_owned());
        }
    };
    for (other, _) in neighbours {
        add(&graph.nodes[other].id, &mut ids);
    }
    let tree: Option<Communities> = read_json(&folder.join("communities.json"))?;
    if let Some(community) = tree.as_ref().and_then(|tree| smallest(&tree.communities, node)) {
        let mut members: Vec<&str> = community
            .members
            .iter()
            .filter_map(|member| graph.nodes.get(*member))
            .map(|node| node.id.as_str())
            .collect();
        members.sort_unstable();
        for member in members {
            add(member, &mut ids);
        }
    }
    Ok(ids)
}

/// A passage of another paper about the same thing: `RelatedPassage` in the contracts.
#[derive(Serialize)]
struct Near {
    paper: String,
    evidence: Evidence,
    /// The words it shares with the passage, the rarest first.
    words: Vec<String>,
}

/// What was searched and found: `RelatedPassages` in the contracts.
#[derive(Serialize)]
struct Related {
    /// Nearby papers looked at, at most `bound`.
    nearby: usize,
    bound: usize,
    /// Of those, searched by their text, by their abstract only (no PDF read), and not in the library.
    by_text: usize,
    by_abstract: usize,
    not_in_library: usize,
    found: Vec<Near>,
}

async fn find_related(
    State(engine): State<Shared>,
    Path((run, paper)): Path<(String, String)>,
    body: Result<Json<Request>, JsonRejection>,
) -> Result<Json<Related>, ApiError> {
    let address = take(body)?;
    let answer = on_the_side(move || {
        let library = engine.library()?;
        library.paper(&paper)?.ok_or(ApiError::NotFound)?;
        let passages = read(&library, &paper)?.map_err(no_text)?;
        let query = &passages.passages[passage_at(&passages, &address)?].text;
        let ids = nearby(&engine, &run, &paper)?;
        let wanted: Vec<&str> = ids.iter().map(String::as_str).collect();
        // read here, on this request's own thread, before any work goes to the pool
        let readings = library.paper_texts(&wanted)?;
        let by_paper: HashMap<&str, &PaperReading> = wanted.iter().copied().zip(&readings).collect();
        let candidates: Vec<Candidate<'_>> = wanted
            .iter()
            .zip(&readings)
            .filter_map(|(paper, reading)| {
                let text = match reading {
                    PaperReading::Read(passages) => Searched::Passages(passages),
                    PaperReading::NoPdf { abstract_text, .. } | PaperReading::Unreadable { abstract_text, .. } => {
                        Searched::Abstract(abstract_text)
                    }
                    PaperReading::NotInLibrary => return None,
                };
                Some(Candidate { paper, text })
            })
            .collect();
        let count =
            |read: bool| readings.iter().filter(|reading| matches!(reading, PaperReading::Read(_)) == read).count();
        let not_in_library = readings.iter().filter(|reading| matches!(reading, PaperReading::NotInLibrary)).count();
        let found = related(query, &candidates)
            .into_iter()
            .filter_map(|found| {
                let evidence = match (&found.place, by_paper.get(found.paper.as_str())?) {
                    (Place::Sentence(address), PaperReading::Read(passages)) => {
                        Evidence::at(&found.paper, passages, address)?
                    }
                    (
                        Place::Abstract(sentence),
                        PaperReading::NoPdf { title, abstract_text }
                        | PaperReading::Unreadable { title, abstract_text, .. },
                    ) => {
                        let text = PaperText::Unread { title: title.clone(), abstract_text: abstract_text.clone() };
                        Evidence::locate(&found.paper, &text, sentence).ok()?
                    }
                    _ => return None,
                };
                Some(Near { paper: found.paper, evidence, words: found.words })
            })
            .collect();
        Ok(Related {
            nearby: ids.len(),
            bound: NEARBY_PAPERS,
            by_text: count(true),
            by_abstract: count(false) - not_in_library,
            not_in_library,
            found,
        })
    })
    .await?;
    Ok(Json(answer))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::api::config::tests::STRONG;
    use crate::api::config::{Environment, Listen};
    use crate::api::routes::Engine;
    use crate::api::security::LARGEST_BODY;
    use crate::api::tests::served;
    use crate::library::papers::passages::fixtures::pdf_with;
    use crate::library::papers::tests::paper;
    use crate::library::{Known, Library};
    use crate::settings::Choices;

    const AT: &str = "127.0.0.1:8765";
    const SAID: &str = "We show that kept passages are served to the reader of the paper.";

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

    /// A library of five papers: three with a PDF, one without, one whose PDF is not one; a run of four of them.
    fn data() -> tempfile::TempDir {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(&data.path().join("library")).unwrap();
        let mut papers: Vec<_> = ["2601.00001", "2601.00002", "2601.00003", "iclr-2026-4", "hep-th/9901005"]
            .iter()
            .map(|id| paper(id))
            .collect();
        papers[3].r#abstract = "Abstracts can say that kept passages are served to the reader too.".to_owned();
        library.add_papers(&papers, None, Known::Keep).unwrap();
        // a blank line between blocks: a title, a heading, a paragraph
        let pdf = |title: &str, heading: &str, text: &str| pdf_with(&[title, "", heading, "", text]);
        library.keep_pdf("2601.00001", &pdf("Kept Passages", "1. Introduction", SAID)).unwrap();
        let serving = pdf("Serving Papers", "2. Method", "Our reader serves the kept passages of every paper.");
        library.keep_pdf("2601.00002", &serving).unwrap();
        let folding = pdf("Folding", "3. Results", "Protein folding is predicted from sequence.");
        library.keep_pdf("2601.00003", &folding).unwrap();
        library.keep_pdf("hep-th/9901005", b"%PDF-1.4 and then nothing a reader could use").unwrap();
        let run = data.path().join("runs").join("r1");
        std::fs::create_dir_all(&run).unwrap();
        let graph = json!({
            "nodes": [{"id": "2601.00001"}, {"id": "2601.00002"}, {"id": "2601.00003"}, {"id": "iclr-2026-4"}],
            "edges": [[0, 2, 0.9], [1, 0, 0.7]],
        });
        std::fs::write(run.join("graph.json"), graph.to_string()).unwrap();
        let tree = json!({"communities": [{"tree_path": "c0", "members": [0, 1, 2, 3], "children": [
            {"tree_path": "c0.0", "members": [0, 3], "children": []}]}]});
        std::fs::write(run.join("communities.json"), tree.to_string()).unwrap();
        data
    }

    fn serve(data: &tempfile::TempDir, engine: impl FnOnce(&mut Engine)) -> axum::Router {
        let listen = Listen::from_environment(&Environment::from_pairs([
            ("CONSTELLATE_TOKEN", STRONG),
            ("CONSTELLATE_ORIGIN", "http://127.0.0.1:3000"),
        ]))
        .unwrap();
        let mut built = Engine::new(data.path().to_path_buf(), choices());
        engine(&mut built);
        served(&listen, AT, built).0
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

    /// The address of the passage of `paper` that says `words`, as the reader has it.
    async fn address_of(router: &axum::Router, paper: &str, words: &str) -> Value {
        let (_, read) = ask(router, "GET", &format!("/api/papers/{paper}/passages"), None).await;
        let passage = read["passages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|passage| passage["text"].as_str().unwrap().contains(words))
            .unwrap_or_else(|| panic!("{read}"));
        passage["address"].clone()
    }

    fn body(address: &Value) -> Option<String> {
        Some(json!({ "address": address }).to_string())
    }

    /// Asking, with the model and the worst case the reader was shown.
    fn shown(address: &Value, model: &str, most_usd: Value) -> Option<String> {
        Some(json!({ "address": address, "model": model, "most_usd": most_usd }).to_string())
    }

    #[tokio::test]
    async fn another_provider_is_priced_asked_in_its_own_shape_kept_and_asked_again_when_the_provider_changes() {
        use crate::api::other_providers::{
            ACCEPTED, Chosen as Asked, MODEL, Scripted, accepting, call, choose, finished, priced_for, router, sent_to,
        };

        let data = data();
        let transport = Scripted::new(|_| {
            json!({"statements": [{"statement": {"part": "explanation", "symbol": "", "text": "Passages are kept.", "source": "paper"},
                "quotes": [{"paper": "2601.00001", "quote": "kept passages are served to the reader"}]}]})
            .to_string()
        });
        let router = router(data.path(), Asked::OpenAi, &transport);
        let address = address_of(&router, "2601.00001", "kept passages").await;
        let prepare = "/api/papers/2601.00001/explanation/prepare";
        let mut sent = 0;
        for chosen in [Asked::OpenAi, Asked::ThisComputer] {
            choose(data.path(), chosen);
            let (_, told) = call(&router, "POST", prepare, Some(json!({"address": address}))).await;
            assert_eq!(
                (told["provider"].clone(), told["model"].clone()),
                (json!(chosen.provider().id()), json!(MODEL)),
                "{told}"
            );
            assert_eq!((told["explained"].clone(), told["api_key_set"].clone()), (Value::Null, json!(true)));
            let cost = told["estimate"].clone();
            priced_for(&cost, chosen);
            assert_eq!(transport.sent().len(), sent, "preparing sends nothing");

            let mut asked = accepting(&cost, ACCEPTED);
            asked["address"] = address.clone();
            // naming nothing accepted is refused, also for a model with no price
            let mut bare = asked.clone();
            bare.as_object_mut().unwrap().retain(|field, _| !field.starts_with("accepted"));
            let (status, refused) = call(&router, "POST", "/api/papers/2601.00001/explanation", Some(bare)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{chosen:?}: {refused}");
            assert_eq!(transport.sent().len(), sent, "{chosen:?}: nothing accepted, nothing sent");
            let (status, started) =
                call(&router, "POST", "/api/papers/2601.00001/explanation", Some(asked.clone())).await;
            assert_eq!((status, started["started"].clone()), (StatusCode::OK, json!(true)), "{started}");
            let done = finished(&router, &started["job"]).await;
            assert_eq!(done["state"], "done", "{done}");
            let made = json!({"kind": "feature", "feature": "explanation", "subject": {"paper": "2601.00001"}});
            assert_eq!((done["outcome"].clone(), done["result"].clone()), (made, Value::Null), "no run to open");
            let all = transport.sent();
            sent_to(&all[sent..], chosen);
            sent = all.len();

            let (_, told) = call(&router, "POST", prepare, Some(json!({"address": address}))).await;
            assert_eq!(told["estimate"], Value::Null, "kept: nothing to pay");
            assert_eq!(told["explained"]["statements"][0]["evidence"][0]["established"]["kind"], "verified", "{told}");
            let (_, again) = call(&router, "POST", "/api/papers/2601.00001/explanation", Some(asked)).await;
            assert_eq!(again, json!({"started": false, "job": null}));
            assert_eq!(transport.sent().len(), sent, "kept: not asked again");
        }
        // a redo forgets that one explanation, of this provider's model only
        let (_, redone) =
            call(&router, "DELETE", "/api/papers/2601.00001/explanation", Some(json!({"address": address}))).await;
        assert_eq!(redone, json!({"redone": true}));
        assert_ne!(call(&router, "POST", prepare, Some(json!({"address": address}))).await.1["estimate"], Value::Null);
        choose(data.path(), Asked::OpenAi);
        assert_eq!(call(&router, "POST", prepare, Some(json!({"address": address}))).await.1["estimate"], Value::Null);
    }

    #[tokio::test]
    async fn preparing_tells_the_passage_and_the_estimate_and_asks_nothing() {
        let data = data();
        let router = serve(&data, |_| {});
        let address = address_of(&router, "2601.00001", "kept passages").await;
        let (status, told) = ask(&router, "POST", "/api/papers/2601.00001/explanation/prepare", body(&address)).await;
        assert_eq!(status, StatusCode::OK, "{told}");
        assert_eq!(told["state"], "ready");
        assert_eq!((told["question"].clone(), told["garbled"].clone()), (json!("passage"), json!(false)));
        assert_eq!(told["model"], "claude-opus-5-5", "the model of the settings");
        assert_eq!(told["passage"]["address"], address);
        assert_eq!(told["explained"], Value::Null);
        let estimate = &told["estimate"];
        assert!(estimate["usd"].as_f64().unwrap() > 0.0 && estimate["usd"].as_f64() <= estimate["most_usd"].as_f64());
        assert!(estimate["most_tokens"].as_u64().unwrap() > 100, "the worst case in tokens");
        assert_eq!(
            (estimate["provider"].clone(), estimate["model"].clone()),
            (json!("anthropic"), json!("claude-opus-5-5"))
        );
        assert_eq!(estimate["counting"]["kind"], "assumed");
        // nothing was asked: nothing is recorded in the ledger
        let library = Library::open(&data.path().join("library")).unwrap();
        assert_eq!(library.spending().unwrap().total.calls, 0);
    }

    #[tokio::test]
    async fn a_kept_explanation_is_shown_marked_free_to_open_again_and_set_aside_by_a_redo() {
        let data = data();
        let router = serve(&data, |_| {});
        let address = address_of(&router, "2601.00001", "kept passages").await;
        {
            let library = Library::open(&data.path().join("library")).unwrap();
            let parsed: Address = serde_json::from_value(address.clone()).unwrap();
            let said = Said {
                statements: vec![
                    json!({"statement": {"part": "explanation", "symbol": "", "text": "The reader serves passages.",
                        "source": "paper"}, "quotes": [{"paper": "2601.00001", "quote": "kept passages are served to the reader"}]}),
                    json!({"statement": {"part": "prerequisite", "symbol": "", "text": "A PDF has pages.",
                        "source": "background"}, "quotes": []}),
                    json!({"statement": {"part": "explanation", "symbol": "", "text": "Invented.", "source": "paper"},
                        "quotes": [{"paper": "2601.00001", "quote": "this sentence is not anywhere in the paper"}]}),
                ],
            };
            let slot = slot(
                "2601.00001",
                &crate::model::chosen::Chosen::claude("claude-opus-5-5"),
                &parsed,
                Question::Passage,
            );
            library.keep(&slot, "claude-opus-5-5", crate::features::explanation::PROMPT, &said).unwrap();
        }
        let (_, told) = ask(&router, "POST", "/api/papers/2601.00001/explanation/prepare", body(&address)).await;
        let explained = &told["explained"];
        assert_eq!(told["estimate"], Value::Null, "kept: nothing to pay");
        assert_eq!(explained["model"], "claude-opus-5-5");
        assert_eq!(explained["statements"][0]["source"], "paper");
        assert_eq!(explained["statements"][0]["evidence"][0]["quote"], "kept passages are served to the reader");
        assert_eq!(explained["statements"][1]["source"], "background");
        assert_eq!(explained["statements"].as_array().unwrap().len(), 2);
        assert_eq!(explained["left_out"], json!({"not_found": 1, "malformed": 0, "past_bound": 0}));
        // nothing is asked for what is kept, key or no key
        let asked = shown(&address, "claude-opus-5-5", json!(1.0));
        let (status, started) = ask(&router, "POST", "/api/papers/2601.00001/explanation", asked).await;
        assert_eq!((status, started), (StatusCode::OK, json!({"started": false, "job": null})));
        let (_, marks) = ask(&router, "GET", "/api/papers/2601.00001/explanations", None).await;
        assert_eq!(marks, json!({"model": "claude-opus-5-5", "explained": [address]}));

        let (status, redone) = ask(&router, "DELETE", "/api/papers/2601.00001/explanation", body(&address)).await;
        assert_eq!((status, redone), (StatusCode::OK, json!({"redone": true})));
        let (_, twice) = ask(&router, "DELETE", "/api/papers/2601.00001/explanation", body(&address)).await;
        assert_eq!(twice, json!({"redone": false}), "nothing left to forget, and said so");
        let (_, again) = ask(&router, "POST", "/api/papers/2601.00001/explanation/prepare", body(&address)).await;
        assert_eq!(again["explained"], Value::Null);
        assert!(again["estimate"].is_object());
        assert_eq!(ask(&router, "GET", "/api/papers/2601.00001/explanations", None).await.1["explained"], json!([]));
    }

    #[tokio::test]
    async fn explaining_needs_a_key_and_waits_for_the_running_job() {
        let data = data();
        let router = serve(&data, |_| {});
        let address = address_of(&router, "2601.00001", "kept passages").await;
        let (_, prepared) = ask(&router, "POST", "/api/papers/2601.00001/explanation/prepare", body(&address)).await;
        let most = prepared["estimate"]["most_usd"].clone();
        let asked = || shown(&address, "claude-opus-5-5", most.clone());
        let (status, told) = ask(&router, "POST", "/api/papers/2601.00001/explanation", asked()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(told["error"].as_str().unwrap().contains("No API key"), "{told}");
        // without what the reader was shown, nothing is asked
        let (status, _) = ask(&router, "POST", "/api/papers/2601.00001/explanation", body(&address)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // a key, to an address nothing listens on; another job running: nothing is started, nothing sent
        let busy = serve(&data, |engine| {
            engine.set_claude_key("test-key-not-real-0123456789", Some("http://127.0.0.1:9"));
            engine
                .job
                .start_tracked("a run", |tracker| {
                    while !tracker.cancelled() {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Ok(crate::jobs::Finished::Nothing)
                })
                .unwrap();
        });
        let (status, told) = ask(&busy, "POST", "/api/papers/2601.00001/explanation", asked()).await;
        assert_eq!(status, StatusCode::CONFLICT, "{told}");
        // another model than the one shown, or a worst case above the one shown: refused before any job
        let cheaper = json!(most.as_f64().unwrap() / 2.0);
        for (model, most) in
            [("claude-haiku-5-5", most.clone()), ("claude-opus-5-5", cheaper), ("claude-opus-5-5", Value::Null)]
        {
            let (status, told) =
                ask(&busy, "POST", "/api/papers/2601.00001/explanation", shown(&address, model, most)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{told}");
            assert!(told["error"].as_str().unwrap().contains("estimate before asking"), "{told}");
        }
        let (_, status) = ask(&busy, "GET", "/api/status", None).await;
        assert_eq!(status["kind"], "a run");
        let library = Library::open(&data.path().join("library")).unwrap();
        assert_eq!(library.spending().unwrap().total.calls, 0);
    }

    #[tokio::test]
    async fn a_paper_without_text_a_place_outside_it_and_a_bad_request_are_told_apart() {
        let data = data();
        let router = serve(&data, |_| {});
        let address = address_of(&router, "2601.00001", "kept passages").await;
        let prepare = |paper: &str| format!("/api/papers/{paper}/explanation/prepare");
        let (status, told) = ask(&router, "POST", &prepare("iclr-2026-4"), body(&address)).await;
        assert_eq!((status, told), (StatusCode::OK, json!({"state": "no_text", "why": "no_pdf"})));
        let (_, told) = ask(&router, "POST", &prepare("hep-th%2F9901005"), body(&address)).await;
        assert_eq!(told, json!({"state": "no_text", "why": "unreadable"}));
        let asked = shown(&address, "claude-opus-5-5", json!(1.0));
        let (status, _) = ask(&router, "POST", "/api/papers/iclr-2026-4/explanation", asked).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "no text to explain");
        assert_eq!(ask(&router, "POST", &prepare("nowhere"), body(&address)).await.0, StatusCode::NOT_FOUND);
        assert_eq!(ask(&router, "GET", "/api/papers/nowhere/explanations", None).await.0, StatusCode::NOT_FOUND);
        let mut elsewhere = address.clone();
        elsewhere["page"] = json!(40);
        assert_eq!(ask(&router, "POST", &prepare("2601.00001"), body(&elsewhere)).await.0, StatusCode::NOT_FOUND);
        let mut long = address.clone();
        long["section"] = json!("x".repeat(LONGEST_SECTION + 1));
        assert_eq!(ask(&router, "POST", &prepare("2601.00001"), body(&long)).await.0, StatusCode::BAD_REQUEST);
        for bad in [json!({"text": "explain this"}).to_string(), "[1]".to_owned()] {
            assert_eq!(ask(&router, "POST", &prepare("2601.00001"), Some(bad)).await.0, StatusCode::BAD_REQUEST);
        }
        let huge = json!({"address": address, "padding": "x".repeat(LARGEST_BODY)}).to_string();
        assert_eq!(ask(&router, "POST", &prepare("2601.00001"), Some(huge)).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn related_passages_come_from_nearby_papers_with_the_words_they_share() {
        let data = data();
        let router = serve(&data, |_| {});
        let address = address_of(&router, "2601.00001", "kept passages").await;
        let (status, told) = ask(&router, "POST", "/api/runs/r1/papers/2601.00001/related", body(&address)).await;
        assert_eq!(status, StatusCode::OK, "{told}");
        assert_eq!((told["nearby"].clone(), told["bound"].clone()), (json!(3), json!(NEARBY_PAPERS)));
        assert_eq!(
            (told["by_text"].clone(), told["by_abstract"].clone(), told["not_in_library"].clone()),
            (json!(2), json!(1), json!(0))
        );
        let found = told["found"].as_array().unwrap();
        let papers: Vec<&str> = found.iter().map(|found| found["paper"].as_str().unwrap()).collect();
        assert!(papers.contains(&"2601.00002") && papers.contains(&"iclr-2026-4"), "{told}");
        assert!(!papers.contains(&"2601.00003"), "protein folding is about something else");
        let from_pdf = found.iter().find(|found| found["paper"] == "2601.00002").unwrap();
        assert_eq!(from_pdf["evidence"]["established"]["kind"], "verified");
        assert!(from_pdf["words"].as_array().unwrap().contains(&json!("passage")));
        let from_abstract = found.iter().find(|found| found["paper"] == "iclr-2026-4").unwrap();
        assert_eq!(from_abstract["evidence"]["established"]["kind"], "abstract");

        let related = |run: &str, paper: &str| format!("/api/runs/{run}/papers/{paper}/related");
        assert_eq!(ask(&router, "POST", &related("r2", "2601.00001"), body(&address)).await.0, StatusCode::NOT_FOUND);
        assert_eq!(ask(&router, "POST", &related("..", "2601.00001"), body(&address)).await.0, StatusCode::NOT_FOUND);
        // a paper of the library that is not in the run
        let mut other = address.clone();
        other["section"] = json!("");
        let (status, _) = ask(&router, "POST", &related("r1", "iclr-2026-4"), body(&other)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "no text of its own to search with");
    }

    #[tokio::test]
    async fn every_answer_of_the_explanation_routes_fits_its_type_in_the_contracts() {
        use crate::api::contract::assert_fits;

        let data = data();
        let router = serve(&data, |_| {});
        let address = address_of(&router, "2601.00001", "kept passages").await;
        let prepare = "/api/papers/2601.00001/explanation/prepare";
        let (_, not_asked) = ask(&router, "POST", prepare, body(&address)).await;
        assert!(not_asked["explained"].is_null() && not_asked["estimate"].is_object(), "{not_asked}");
        assert_fits("ExplanationPrepared", &not_asked, &[]);
        let (_, no_text) = ask(&router, "POST", "/api/papers/iclr-2026-4/explanation/prepare", body(&address)).await;
        assert_fits("ExplanationPrepared", &no_text, &[]);
        {
            let library = Library::open(&data.path().join("library")).unwrap();
            let parsed: Address = serde_json::from_value(address.clone()).unwrap();
            let said = Said {
                statements: vec![
                    json!({"statement": {"part": "explanation", "symbol": "", "text": "Passages are served.",
                        "source": "paper"}, "quotes": [{"paper": "2601.00001", "quote": "kept passages are served to the reader"}]}),
                    json!({"statement": {"part": "symbol", "symbol": "x", "text": "A PDF has pages.",
                        "source": "background"}, "quotes": []}),
                ],
            };
            let chosen = crate::model::chosen::Chosen::claude("claude-opus-5-5");
            let slot = slot("2601.00001", &chosen, &parsed, Question::Passage);
            library.keep(&slot, "claude-opus-5-5", crate::features::explanation::PROMPT, &said).unwrap();
        }
        let (_, kept) = ask(&router, "POST", prepare, body(&address)).await;
        assert!(kept["explained"].is_object() && kept["estimate"].is_null(), "{kept}");
        assert_fits("ExplanationPrepared", &kept, &[]);
        let asked = shown(&address, "claude-opus-5-5", json!(1.0));
        let (_, started) = ask(&router, "POST", "/api/papers/2601.00001/explanation", asked).await;
        assert_fits("ExplanationStarted", &started, &[]);
        let (_, marks) = ask(&router, "GET", "/api/papers/2601.00001/explanations", None).await;
        assert_fits("ExplainedPassages", &marks, &[]);
        let (_, redone) = ask(&router, "DELETE", "/api/papers/2601.00001/explanation", body(&address)).await;
        assert_fits("ExplanationRedone", &redone, &[]);
        let (_, related) = ask(&router, "POST", "/api/runs/r1/papers/2601.00001/related", body(&address)).await;
        assert!(!related["found"].as_array().unwrap().is_empty());
        assert_fits("RelatedPassages", &related, &[]);
    }

    #[test]
    fn the_nearby_papers_are_the_graphs_neighbours_then_the_smallest_community_bounded() {
        let data = data();
        let engine = Arc::new(Engine::new(data.path().to_path_buf(), choices()));
        assert_eq!(nearby(&engine, "r1", "2601.00001").unwrap(), ["2601.00003", "2601.00002", "iclr-2026-4"]);
        assert_eq!(nearby(&engine, "r1", "iclr-2026-4").unwrap(), ["2601.00001"]);
        assert!(matches!(nearby(&engine, "r1", "2601.99999"), Err(ApiError::NotFound)));
        // a large community: at most the bound
        let run = data.path().join("runs").join("big");
        std::fs::create_dir_all(&run).unwrap();
        let nodes: Vec<Value> = (0..100).map(|n| json!({"id": format!("p{n:03}")})).collect();
        std::fs::write(run.join("graph.json"), json!({"nodes": nodes, "edges": []}).to_string()).unwrap();
        let tree =
            json!({"communities": [{"tree_path": "c0", "members": (0..100).collect::<Vec<_>>(), "children": []}]});
        std::fs::write(run.join("communities.json"), tree.to_string()).unwrap();
        let ids = nearby(&engine, "big", "p050").unwrap();
        assert_eq!(ids.len(), NEARBY_PAPERS);
        assert_eq!(ids[0], "p000");
        assert!(!ids.contains(&"p050".to_owned()));
    }
}
