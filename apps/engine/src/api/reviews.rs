//! A paper's reviews, read from the page the user opened, and what the reviews of a run or a community come to.
//!
//! ```text
//! POST /api/papers/{id}/reviews    {text, url}: the text of the paper's reviews page as the reader shows it,
//!                                  and the page's address. Answers what is kept now ([`Read`])
//! GET  /api/papers/{id}/reviews    the kept notes and their figures ([`PaperReviews`]), or null
//! GET  /api/runs/{run}/reviews/summary                          the run's [`ReviewSummary`]
//! GET  /api/runs/{run}/communities/{community}/reviews/summary  one community's (by its tree path)
//! ```
//!
//! The engine never fetches OpenReview: the page is the one the user is looking at, and the text is what the
//! interface read off it. That text is untrusted: it is capped at [`MAX_TEXT`] characters, read as text only, and
//! kept only when the address it came from is the paper's own discussion.
//!
//! The answer to the POST keeps the shape of the old scores-only route in its `ratings` (the interface's
//! `Ratings`), so a caller that wants only the scores reads that.

use std::collections::BTreeMap;
use std::path::PathBuf;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::routing::{MethodRouter, get};
use serde::{Deserialize, Serialize};

use super::routes::{ApiError, Shared, on_the_side, one_name};
use crate::analysis::reviews::{PaperFigures, ReviewSummary, Reviewed, paper_figures, summarise, venue_of};
use crate::library::store::read_json;
use crate::library::{Keeping, KeptReviews, Library, Ratings};
use crate::text::discussion::{Discussion, read_discussion};
use crate::text::reviews::{MAX_TEXT, forum, read_ratings};

/// The routes of this module, for [`super::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![
        ("/api/papers/{id}/reviews", get(kept).post(read)),
        ("/api/runs/{run}/reviews/summary", get(run_summary)),
        ("/api/runs/{run}/communities/{community}/reviews/summary", get(community_summary)),
    ]
}

/// A paper's kept reviews: where and when they were read, the notes, and what their scores come to.
#[derive(Serialize)]
pub struct PaperReviews {
    pub paper_id: String,
    pub address: String,
    pub read_at: String,
    #[serde(flatten)]
    pub discussion: Discussion,
    pub figures: PaperFigures,
}

impl PaperReviews {
    fn of(kept: KeptReviews) -> Self {
        let venue = venue_of(&kept.paper_id, &kept.discussion);
        let figures = paper_figures(&Reviewed::new(&kept.paper_id, venue.as_ref(), &kept.discussion));
        Self {
            paper_id: kept.paper_id,
            address: kept.address,
            read_at: kept.read_at,
            discussion: kept.discussion,
            figures,
        }
    }
}

/// What came of handing over a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Read and kept: it now stands for the paper's reviews.
    Kept,
    /// Read, but what is kept has more reviews (the page is still loading, or replies are collapsed).
    KeptFuller,
    /// The page shown is not this paper's discussion (the reader wandered off), or the paper has none.
    NotItsPage,
    /// The page has no notes: not loaded yet, or the reviews are not public.
    NoReviews,
}

/// The answer to a page handed over: what came of it, and what is kept now.
#[derive(Serialize)]
pub struct Read {
    pub outcome: Outcome,
    /// The scores alone, as the old route answered them; null when none were ever read.
    pub ratings: Option<Ratings>,
    pub reviews: Option<PaperReviews>,
}

#[derive(Deserialize)]
struct Page {
    text: String,
    url: String,
}

async fn kept(
    State(engine): State<Shared>,
    Path(paper_id): Path<String>,
) -> Result<Json<Option<PaperReviews>>, ApiError> {
    let kept = on_the_side(move || {
        let library = engine.library()?;
        library.paper(&paper_id)?.ok_or(ApiError::NotFound)?;
        Ok(library.reviews(&paper_id)?)
    })
    .await?;
    Ok(Json(kept.map(PaperReviews::of)))
}

async fn read(
    State(engine): State<Shared>,
    Path(paper_id): Path<String>,
    body: Result<Json<Page>, JsonRejection>,
) -> Result<Json<Read>, ApiError> {
    let Json(page) = body?;
    if page.text.chars().nth(MAX_TEXT).is_some() {
        return Err(ApiError::Invalid(format!("the page text is longer than {MAX_TEXT} characters")));
    }
    let read = on_the_side(move || {
        let library = engine.library()?;
        let paper = library.paper(&paper_id)?.ok_or(ApiError::NotFound)?;
        let as_it_is = |outcome| -> Result<Read, ApiError> {
            let (ratings, reviews) = library.ratings_and_reviews(&paper_id)?;
            Ok(Read { outcome, ratings, reviews: reviews.map(PaperReviews::of) })
        };
        let discussion = paper.url.as_deref().and_then(forum);
        if discussion.is_none() || forum(&page.url) != discussion {
            return as_it_is(Outcome::NotItsPage);
        }
        let Some(read) = read_discussion(&page.text) else {
            return as_it_is(Outcome::NoReviews);
        };
        let ratings = read_ratings(&page.text).map(|read| Ratings::from(&read));
        let outcome = match library.keep_reviews(&paper_id, &page.url, &page.text, &read, ratings)? {
            Keeping::Replaced(_) => Outcome::Kept,
            Keeping::KeptFuller(_) => Outcome::KeptFuller,
        };
        as_it_is(outcome)
    })
    .await?;
    Ok(Json(read))
}

/// The run's graph and community tree, as the pipeline writes them: only what says which papers are where.
#[derive(Deserialize)]
struct Graph {
    nodes: Vec<Node>,
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
    tree_path: String,
    /// Indices into the graph's nodes.
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

/// The ids of a run's papers, or of one of its communities' papers.
fn papers_of(engine: &Shared, run: &str, community: Option<&str>) -> Result<Vec<String>, ApiError> {
    let folder = run_folder(engine, run)?;
    let graph: Graph = read_json(&folder.join("graph.json"))?.ok_or(ApiError::NotFound)?;
    let Some(tree_path) = community else {
        let mut ids: Vec<String> = graph.nodes.into_iter().map(|node| node.id).collect();
        ids.sort_unstable();
        ids.dedup();
        return Ok(ids);
    };
    let tree: Communities = read_json(&folder.join("communities.json"))?.ok_or(ApiError::NotFound)?;
    let community = find(&tree.communities, tree_path).ok_or(ApiError::NotFound)?;
    let mut ids: Vec<String> =
        community.members.iter().filter_map(|member| graph.nodes.get(*member)).map(|node| node.id.clone()).collect();
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

/// What the reviews of a run or community come to, with the titles of the papers it names.
#[derive(Serialize)]
pub struct Summary {
    #[serde(flatten)]
    pub summary: ReviewSummary,
    /// By paper id: every paper the summary ranks or quotes.
    pub titles: BTreeMap<String, String>,
}

/// The titles of the papers `summary` names.
fn titles(library: &Library, summary: &ReviewSummary) -> Result<BTreeMap<String, String>, ApiError> {
    let ranked = [&summary.most_disagreed, &summary.most_agreed, &summary.highest_rated];
    let themes = summary.praise.iter().chain(&summary.criticism);
    let mut named: Vec<&str> =
        ranked.iter().flat_map(|papers| papers.iter().map(|paper| paper.paper_id.as_str())).collect();
    named.extend(themes.flat_map(|theme| theme.sources.iter().map(|source| source.paper_id.as_str())));
    named.sort_unstable();
    named.dedup();
    let papers = library.papers(&named)?;
    Ok(named.iter().zip(papers).filter_map(|(id, paper)| Some(((*id).to_owned(), paper?.title))).collect())
}

/// What the reviews of `ids` come to.
fn summary_of(library: &Library, ids: &[String]) -> Result<Summary, ApiError> {
    let wanted: Vec<&str> = ids.iter().map(String::as_str).collect();
    let kept = library.reviews_of(&wanted)?;
    let venues: Vec<_> = kept.iter().map(|kept| venue_of(&kept.paper_id, &kept.discussion)).collect();
    let reviewed: Vec<Reviewed<'_>> = kept
        .iter()
        .zip(&venues)
        .map(|(kept, venue)| Reviewed::new(&kept.paper_id, venue.as_ref(), &kept.discussion))
        .collect();
    let summary = summarise(ids.len(), &reviewed);
    Ok(Summary { titles: titles(library, &summary)?, summary })
}

async fn run_summary(State(engine): State<Shared>, Path(run): Path<String>) -> Result<Json<Summary>, ApiError> {
    let summary = on_the_side(move || {
        let ids = papers_of(&engine, &run, None)?;
        summary_of(&*engine.library()?, &ids)
    })
    .await?;
    Ok(Json(summary))
}

async fn community_summary(
    State(engine): State<Shared>,
    Path((run, community)): Path<(String, String)>,
) -> Result<Json<Summary>, ApiError> {
    let summary = on_the_side(move || {
        let ids = papers_of(&engine, &run, Some(&community))?;
        summary_of(&*engine.library()?, &ids)
    })
    .await?;
    Ok(Json(summary))
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
    use crate::library::Known;
    use crate::library::papers::tests::paper;
    use crate::settings::Choices;
    use crate::text::discussion::fixtures::{ICLR_2024, NEURIPS_2023};

    const AT: &str = "127.0.0.1:8765";
    const FORUM: &str = "https://openreview.net/forum?id=abc123XYZ";

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

    /// The engine as served on this computer, with a library of three papers and a run of them.
    fn engine() -> (axum::Router, tempfile::TempDir) {
        let data = tempfile::tempdir().unwrap();
        {
            let library = crate::library::Library::open(&data.path().join("library")).unwrap();
            let mut papers = vec![paper("iclr-2024-1"), paper("neurips-2023-2"), paper("2601.00003")];
            papers[0].url = Some(FORUM.to_owned());
            papers[1].url = Some("https://openreview.net/forum?id=other456".to_owned());
            library.add_papers(&papers, None, Known::Keep).unwrap();
        }
        let run = data.path().join("runs").join("r1");
        std::fs::create_dir_all(&run).unwrap();
        let nodes =
            json!({"nodes": [{"id": "iclr-2024-1"}, {"id": "neurips-2023-2"}, {"id": "2601.00003"}], "edges": []});
        std::fs::write(run.join("graph.json"), nodes.to_string()).unwrap();
        let tree = json!({"communities": [
            {"tree_path": "c0", "members": [0, 2], "children": [{"tree_path": "c0.0", "members": [2], "children": []}]},
            {"tree_path": "c1", "members": [1], "children": []}]});
        std::fs::write(run.join("communities.json"), tree.to_string()).unwrap();
        let listen = Listen::from_environment(&Environment::from_pairs([
            ("CONSTELLATE_TOKEN", STRONG),
            ("CONSTELLATE_ORIGIN", "http://127.0.0.1:3000"),
        ]))
        .unwrap();
        let (router, _) = served(&listen, AT, Engine::new(data.path().to_path_buf(), choices()));
        (router, data)
    }

    async fn ask(router: &axum::Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, AT)
            .header(header::AUTHORIZATION, format!("Bearer {STRONG}"))
            .header(header::CONTENT_TYPE, "application/json");
        let request = request.body(body.map_or(Body::empty(), |body| Body::from(body.to_string()))).unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn a_page_handed_over_is_kept_and_its_notes_and_figures_are_served() {
        let (router, _data) = engine();
        let (status, nothing) = ask(&router, "GET", "/api/papers/iclr-2024-1/reviews", None).await;
        assert_eq!((status, nothing), (StatusCode::OK, Value::Null));
        let (status, read) =
            ask(&router, "POST", "/api/papers/iclr-2024-1/reviews", Some(json!({"text": ICLR_2024, "url": FORUM})))
                .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(read["outcome"], "kept");
        assert_eq!(read["ratings"]["reviews"], 2); // the old route's answer, unchanged
        assert_eq!(read["ratings"]["ratings"]["Rating"]["values"], json!([8, 6]));
        let (_, kept) = ask(&router, "GET", "/api/papers/iclr-2024-1/reviews", None).await;
        assert_eq!(kept["address"], FORUM);
        assert_eq!(kept["notes"].as_array().unwrap().len(), 5);
        assert_eq!(kept["notes"][2]["kind"], "official_review");
        let rating = &kept["figures"]["fields"][0];
        assert_eq!((rating["field"].clone(), rating["mean"].clone()), (json!("Rating"), json!(7.0)));
        assert_eq!(rating["scale"], json!({"minimum": 1, "maximum": 10, "source": "published"}));
        let (_, again) =
            ask(&router, "POST", "/api/papers/iclr-2024-1/reviews", Some(json!({"text": "Loading...", "url": FORUM})))
                .await;
        assert_eq!((again["outcome"].clone(), again["ratings"]["reviews"].clone()), (json!("no_reviews"), json!(2)));
    }

    #[tokio::test]
    async fn a_page_that_is_not_the_papers_or_too_long_or_of_no_paper_is_refused() {
        let (router, _data) = engine();
        let elsewhere = json!({"text": NEURIPS_2023, "url": "https://openreview.net/forum?id=SOMEONEELSE"});
        let (status, read) = ask(&router, "POST", "/api/papers/iclr-2024-1/reviews", Some(elsewhere)).await;
        assert_eq!(
            (status, read["outcome"].clone(), read["reviews"].clone()),
            (StatusCode::OK, json!("not_its_page"), Value::Null)
        );
        let arxiv = json!({"text": NEURIPS_2023, "url": FORUM});
        assert_eq!(
            ask(&router, "POST", "/api/papers/2601.00003/reviews", Some(arxiv)).await.1["outcome"],
            "not_its_page"
        );
        let long = json!({"text": "x".repeat(super::MAX_TEXT + 1), "url": FORUM});
        assert_eq!(
            ask(&router, "POST", "/api/papers/iclr-2024-1/reviews", Some(long)).await.0,
            StatusCode::BAD_REQUEST
        );
        let unknown = json!({"text": ICLR_2024, "url": FORUM});
        assert_eq!(ask(&router, "POST", "/api/papers/nowhere/reviews", Some(unknown)).await.0, StatusCode::NOT_FOUND);
        assert_eq!(ask(&router, "GET", "/api/papers/nowhere/reviews", None).await.0, StatusCode::NOT_FOUND);
        assert_eq!(
            ask(&router, "POST", "/api/papers/iclr-2024-1/reviews", Some(json!({"text": 5}))).await.0,
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn a_run_and_each_community_say_how_many_of_their_papers_have_reviews() {
        let (router, _data) = engine();
        ask(&router, "POST", "/api/papers/iclr-2024-1/reviews", Some(json!({"text": ICLR_2024, "url": FORUM}))).await;
        let (status, run) = ask(&router, "GET", "/api/runs/r1/reviews/summary", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (run["papers"].clone(), run["papers_with_reviews"].clone(), run["reviews"].clone()),
            (json!(3), json!(1), json!(2))
        );
        assert_eq!(run["ratings"][0]["venue"], json!({"conference": "ICLR", "year": 2024}));
        assert_eq!(run["titles"], json!({"iclr-2024-1": "title of iclr-2024-1"}));
        let (_, first) = ask(&router, "GET", "/api/runs/r1/communities/c0/reviews/summary", None).await;
        assert_eq!((first["papers"].clone(), first["papers_with_reviews"].clone()), (json!(2), json!(1)));
        let (_, deeper) = ask(&router, "GET", "/api/runs/r1/communities/c0.0/reviews/summary", None).await;
        assert_eq!((deeper["papers"].clone(), deeper["papers_with_reviews"].clone()), (json!(1), json!(0)));
        assert_eq!(
            ask(&router, "GET", "/api/runs/r1/communities/c9/reviews/summary", None).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(ask(&router, "GET", "/api/runs/none/reviews/summary", None).await.0, StatusCode::NOT_FOUND);
        assert_eq!(ask(&router, "GET", "/api/runs/..%2Fdata/reviews/summary", None).await.0, StatusCode::NOT_FOUND);
    }
}
