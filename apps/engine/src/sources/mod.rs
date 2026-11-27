//! Where papers come from: the conferences' own lists, arXiv, and the proceedings that publish their PDFs.
//!
//! Each source is split in two. What to ask for and how to read the answer is plain code over text and bytes, and
//! is tested on canned answers. Asking goes through [`Fetch`], one small trait whose only production
//! implementation is [`Http`]; the tests serve their answers through a fake one, so no test touches the network.
//!
//! | module          | what lives there                                                                   |
//! |-----------------|------------------------------------------------------------------------------------|
//! | [`catalog`]     | the conferences and arXiv categories a run can be about, grouped by field           |
//! | [`arxiv`]       | arXiv's search API: the query, its pages, and its Atom answer read into papers      |
//! | [`conferences`] | a conference's official list of accepted papers, or arXiv comments where it has none |
//! | [`acceptance`]  | how a conference accepted a paper, the same whatever the conference calls it        |
//! | [`proceedings`] | where each conference publishes its PDFs, and downloading one safely                |
//! | [`download`]    | a selection cut into the slices that are fetched, and fetching one                  |
//!
//! Nothing here keeps anything. Every function returns papers and outcomes, and the library decides what to
//! store; an outcome says whether what came back is complete, so that a failure is never stored as a finish.
//!
//! OpenReview is never contacted: it serves a bot challenge to programs, and Constellate does not work around it.
//! Its addresses appear in records only as links for the reader the user opens.

pub mod acceptance;
pub mod arxiv;
pub mod catalog;
pub mod conferences;
pub mod download;
pub mod proceedings;

use std::io::Read;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};
use url::Url;

/// How every request introduces itself: the program and its version, nothing about the person using it.
pub const USER_AGENT: &str = concat!("Constellate/", env!("CARGO_PKG_VERSION"), " (research paper library)");

/// Redirects followed before an address is given up on.
const MOST_HOPS: usize = 6;

#[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
static CONFERENCE_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([a-z]+)-(\d{4})-\d+$").unwrap());

/// A paper as the library keeps it, field for field what the Python backend wrote to `papers.jsonl`, so that the
/// two read each other's records. Which conferences a paper is tagged with is the library's business, not the
/// source's, and is not here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paper {
    /// An arXiv id ("2512.01234"), or "<conference>-<year>-<number>" for a paper from a conference's list.
    pub id: String,
    pub title: String,
    #[serde(rename = "abstract")]
    pub abstract_text: String,
    pub authors: Vec<String>,
    /// When it was submitted to arXiv, or when a conference presents it.
    pub published: String,
    pub updated: String,
    /// arXiv's comment field, or a conference's decision ("Accept (oral)").
    pub comment: String,
    pub primary_category: String,
    pub categories: Vec<String>,
    pub url: String,
    /// Where a PDF is said to be; for conference papers this is a link to show, not one a program fetches.
    pub pdf_url: Option<String>,
}

/// Where a paper's record came from, read off its id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin<'a> {
    Arxiv,
    /// A conference's own list: `slug` is the conference in lower case ("neurips"), `year` its year.
    Conference {
        slug: &'a str,
        year: &'a str,
    },
}

impl Paper {
    pub fn origin(&self) -> Origin<'_> {
        match CONFERENCE_ID.captures(&self.id) {
            Some(found) => match (found.get(1), found.get(2)) {
                (Some(slug), Some(year)) => Origin::Conference { slug: slug.as_str(), year: year.as_str() },
                _ => Origin::Arxiv,
            },
            None => Origin::Arxiv,
        }
    }
}

/// How much one request may bring back and how long it may take, all told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_bytes: u64,
    pub time: Duration,
}

/// An answer as it came: any status, not only success, because what a 404 means differs from what a 503 means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub content_type: Option<String>,
    /// Where a redirect points, as written (it may be relative).
    pub location: Option<String>,
    /// At most `max_bytes + 1` bytes of the body: one more than allowed is how a caller tells "too large".
    pub body: Vec<u8>,
}

impl Response {
    pub fn is_redirect(&self) -> bool {
        matches!(self.status, 301 | 302 | 303 | 307 | 308)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Why nothing came back at all (an answer with an error status is a [`Response`], not this).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error("{0} could not be reached: {1}")]
    Unreachable(String, String),
    #[error("{0} took too long to answer")]
    TimedOut(String),
    #[error("{0} is not an address that can be fetched")]
    BadAddress(String),
}

/// The one way anything here reaches the network.
///
/// An implementation never follows a redirect (it returns it, so that every hop can be checked before it is
/// taken), reads at most `limits.max_bytes + 1` bytes of a body, gives up after `limits.time` in all, and sends
/// nothing but the request itself and [`USER_AGENT`].
pub trait Fetch {
    fn get(&self, url: &str, limits: Limits) -> Result<Response, FetchError>;
}

/// Why an address, followed through its redirects, gave no answer that can be used.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GetError {
    #[error("{0} is not an address this may be fetched from")]
    NotAllowed(String),
    #[error("{0} redirects too many times")]
    TooManyRedirects(String),
    #[error("{0} is too large to fetch")]
    TooLarge(String),
    #[error("{0} took too long to fetch")]
    TooSlow(String),
    #[error(transparent)]
    Fetch(#[from] FetchError),
}

/// Any https address: what lists, pages and the arXiv API are fetched from. A redirect to plain http is refused.
pub fn any_https(address: &Url) -> bool {
    address.scheme() == "https"
}

/// The answer at `address`, with redirects followed by hand: every hop, the first included, is checked with
/// `allowed` before it is fetched; the size limit holds for each answer and the time limit for all of them
/// together. Answers the address
/// that finally answered with its response, whatever its status.
pub fn get_following(
    fetch: &dyn Fetch,
    address: &str,
    limits: Limits,
    allowed: &dyn Fn(&Url) -> bool,
) -> Result<(String, Response), GetError> {
    let deadline = Instant::now() + limits.time;
    let mut address = Url::parse(address).map_err(|_| GetError::NotAllowed(address.to_owned()))?;
    for _ in 0..MOST_HOPS {
        if !allowed(&address) {
            return Err(GetError::NotAllowed(address.to_string()));
        }
        let time = deadline.saturating_duration_since(Instant::now());
        if time.is_zero() {
            return Err(GetError::TooSlow(address.to_string()));
        }
        let response = fetch.get(address.as_str(), Limits { time, ..limits })?;
        if response.body.len() as u64 > limits.max_bytes {
            return Err(GetError::TooLarge(address.to_string()));
        }
        if !response.is_redirect() {
            return Ok((address.to_string(), response));
        }
        // a redirect that says nowhere would only be the same address again
        let Some(next) = response.location.as_deref() else {
            return Err(GetError::TooManyRedirects(address.to_string()));
        };
        address = address.join(next).map_err(|_| GetError::NotAllowed(next.to_owned()))?;
    }
    Err(GetError::TooManyRedirects(address.to_string()))
}

/// [`Fetch`] over the network, through `ureq` and rustls.
pub struct Http {
    agent: ureq::Agent,
}

impl Default for Http {
    fn default() -> Self {
        let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::Rustls).build();
        let agent = ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(15)))
            .tls_config(tls)
            .build()
            .into();
        Self { agent }
    }
}

impl Http {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Fetch for Http {
    fn get(&self, url: &str, limits: Limits) -> Result<Response, FetchError> {
        let failed = |error: ureq::Error| match error {
            ureq::Error::Timeout(_) => FetchError::TimedOut(url.to_owned()),
            ureq::Error::BadUri(_) | ureq::Error::Http(_) => FetchError::BadAddress(url.to_owned()),
            other => FetchError::Unreachable(url.to_owned(), other.to_string()),
        };
        let mut response =
            self.agent.get(url).config().timeout_global(Some(limits.time)).build().call().map_err(failed)?;
        let header = |name: &str| response.headers().get(name).and_then(|value| value.to_str().ok()).map(str::to_owned);
        let (content_type, location) = (header("content-type"), header("location"));
        let mut body = Vec::new();
        response.body_mut().as_reader().take(limits.max_bytes.saturating_add(1)).read_to_end(&mut body).map_err(
            |error| {
                // ureq hands its own timeout over as an io error of kind Other, wrapping it
                let timed_out = error.kind() == std::io::ErrorKind::TimedOut
                    || matches!(
                        error.get_ref().and_then(|inner| inner.downcast_ref::<ureq::Error>()),
                        Some(ureq::Error::Timeout(_))
                    );
                if timed_out {
                    FetchError::TimedOut(url.to_owned())
                } else {
                    FetchError::Unreachable(url.to_owned(), error.to_string())
                }
            },
        )?;
        Ok(Response { status: response.status().as_u16(), content_type, location, body })
    }
}

/// Canned answers for the tests: an address it was not given answers 404, and every address asked is noted.
#[cfg(test)]
pub(crate) mod fake {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::{Fetch, FetchError, Limits, Paper, Response};

    /// A paper with ordinary values for everything the test does not care about.
    pub fn paper(id: &str) -> Paper {
        Paper {
            id: id.to_owned(),
            title: "A title".to_owned(),
            abstract_text: "An abstract.".to_owned(),
            authors: Vec::new(),
            published: "2026-01-02T00:00:00Z".to_owned(),
            updated: "2026-01-02T00:00:00Z".to_owned(),
            comment: String::new(),
            primary_category: "cs.LG".to_owned(),
            categories: vec!["cs.LG".to_owned()],
            url: String::new(),
            pdf_url: None,
        }
    }

    #[derive(Default)]
    pub struct Fake {
        answers: HashMap<String, Result<Response, FetchError>>,
        pub asked: RefCell<Vec<String>>,
    }

    pub fn ok(content_type: &str, body: &str) -> Response {
        Response { status: 200, content_type: Some(content_type.to_owned()), location: None, body: body.into() }
    }

    pub fn status(status: u16) -> Response {
        Response { status, content_type: Some("text/html".to_owned()), location: None, body: Vec::new() }
    }

    pub fn redirect(status: u16, location: &str) -> Response {
        Response { status, content_type: None, location: Some(location.to_owned()), body: Vec::new() }
    }

    impl Fake {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with(mut self, url: &str, answer: Response) -> Self {
            self.answers.insert(url.to_owned(), Ok(answer));
            self
        }

        pub fn failing(mut self, url: &str, error: FetchError) -> Self {
            self.answers.insert(url.to_owned(), Err(error));
            self
        }

        pub fn asked(&self) -> Vec<String> {
            self.asked.borrow().clone()
        }
    }

    impl Fetch for Fake {
        fn get(&self, url: &str, limits: Limits) -> Result<Response, FetchError> {
            self.asked.borrow_mut().push(url.to_owned());
            let mut answer = self.answers.get(url).cloned().unwrap_or_else(|| Ok(status(404)))?;
            // as the real one does: never more than one byte past the limit
            answer.body.truncate(usize::try_from(limits.max_bytes.saturating_add(1)).unwrap_or(usize::MAX));
            Ok(answer)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::fake::{Fake, ok, paper, redirect};
    use super::*;

    const LIMITS: Limits = Limits { max_bytes: 1024, time: Duration::from_secs(60) };

    #[test]
    fn a_papers_origin_is_read_off_its_id() {
        assert_eq!(paper("neurips-2025-123").origin(), Origin::Conference { slug: "neurips", year: "2025" });
        for arxiv in ["2512.01234", "hep-th/9901001", "cvpr-26-1", "CVPR-2026-1", "cvpr-2026-1a"] {
            assert_eq!(paper(arxiv).origin(), Origin::Arxiv, "{arxiv}");
        }
    }

    #[test]
    fn a_record_reads_and_writes_the_same_json_as_the_python_backend() {
        let line = r#"{"id": "2512.01234", "title": "T", "abstract": "A", "authors": ["X"], "published": "p",
            "updated": "u", "comment": "", "primary_category": "cs.LG", "categories": ["cs.LG"],
            "url": "https://arxiv.org/abs/2512.01234", "pdf_url": null, "venues": ["NeurIPS 2025"]}"#;
        let read: Paper = serde_json::from_str(line).unwrap();
        assert_eq!(read.abstract_text, "A");
        assert_eq!(serde_json::to_value(&read).unwrap()["abstract"], "A");
    }

    #[test]
    fn a_relative_redirect_is_followed_from_where_it_was_given() {
        let fake = Fake::new()
            .with("https://example.org/a", redirect(302, "/b"))
            .with("https://example.org/b", ok("text/plain", "here"));
        let (address, response) = get_following(&fake, "https://example.org/a", LIMITS, &any_https).unwrap();
        assert_eq!((address.as_str(), response.text().as_str()), ("https://example.org/b", "here"));
    }

    #[test]
    fn a_redirect_to_plain_http_is_refused_before_it_is_fetched() {
        let fake = Fake::new().with("https://example.org/a", redirect(301, "http://example.org/a"));
        let refused = get_following(&fake, "https://example.org/a", LIMITS, &any_https);
        assert_eq!(refused, Err(GetError::NotAllowed("http://example.org/a".to_owned())));
        assert_eq!(fake.asked(), ["https://example.org/a"]);
    }

    #[test]
    fn a_redirect_loop_ends() {
        let fake = Fake::new().with("https://example.org/a", redirect(302, "https://example.org/a"));
        let refused = get_following(&fake, "https://example.org/a", LIMITS, &any_https);
        assert!(matches!(refused, Err(GetError::TooManyRedirects(_))), "{refused:?}");
        assert_eq!(fake.asked().len(), MOST_HOPS);
    }

    #[test]
    fn an_answer_larger_than_allowed_is_refused() {
        let fake = Fake::new().with("https://example.org/a", ok("text/plain", &"x".repeat(2000)));
        let refused = get_following(&fake, "https://example.org/a", LIMITS, &any_https);
        assert!(matches!(refused, Err(GetError::TooLarge(_))), "{refused:?}");
    }

    #[test]
    fn the_user_agent_names_the_program_and_nobody() {
        assert!(USER_AGENT.starts_with("Constellate/"));
        assert!(!USER_AGENT.contains('@'));
    }
}
