//! Where papers come from: the conferences' own lists, arXiv, and the proceedings that publish their PDFs.
//!
//! Each source is split in two. What to ask for and how to read the answer is plain code over text and bytes, and
//! is tested on canned answers. Asking goes through [`Fetch`], one small trait whose only production
//! implementation is [`Http`]; the tests serve their answers through a fake one, so no test touches the network.
//!
//! | module          | what lives there                                                                   |
//! |-----------------|------------------------------------------------------------------------------------|
//! | [`citations`]   | cited works that are not in the library, looked up on arXiv                         |
//! | [`catalog`]     | the conferences and arXiv categories a run can be about, grouped by field           |
//! | [`arxiv`]       | arXiv's search API: the query, its pages, and its Atom answer read into papers      |
//! | [`conferences`] | a conference's official list of accepted papers, or arXiv comments where it has none |
//! | [`acceptance`]  | how a conference accepted a paper, the same whatever the conference calls it        |
//! | [`proceedings`] | where each conference publishes its PDFs, and downloading one safely                |
//! | [`download`]    | a selection cut into the slices that are fetched, and fetching one                  |
//! | [`polite`]      | many addresses fetched at once, within each host's limits and pauses                |
//!
//! Nothing here keeps anything. Every function returns papers and outcomes, and the library decides what to
//! store; an outcome says whether what came back is complete, so that a failure is never stored as a finish.
//!
//! OpenReview is never contacted: it serves a bot challenge to programs, and Constellate does not work around it.
//! Its addresses appear in records only as links for the reader the user opens.

pub mod acceptance;
pub mod arxiv;
pub mod catalog;
pub mod citations;
pub mod conferences;
pub mod download;
pub mod polite;
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
    /// How long a refusal (429, 503) asks to be left alone, when it says so in seconds (`Retry-After`).
    pub retry_after: Option<Duration>,
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

/// Any https address on the public internet: what lists, pages and the arXiv API are fetched from. A redirect to
/// plain http is refused, and so is one to this computer or its local network (`localhost`, `127.0.0.1`,
/// `192.168.x.x`, `169.254.169.254`...): a site answering with such a redirect must not make the engine ask the
/// user's own machines anything. A public name that resolves to such an address is caught when it is resolved
/// ([`PublicOnly`]).
pub fn any_https(address: &Url) -> bool {
    address.scheme() == "https" && is_public(address)
}

/// Is this address one on the public internet? Not this computer, its local network, link-local addresses (the
/// cloud's metadata service), multicast or reserved ranges, written in any form (IPv4 inside IPv6 included).
fn is_public_ip(ip: std::net::IpAddr) -> bool {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    let public_v4 = |ip: Ipv4Addr| {
        let [first, second, ..] = ip.octets();
        let shared = first == 100 && (64..128).contains(&second); // carrier-grade NAT, 100.64.0.0/10
        let benchmarking = first == 198 && (18..20).contains(&second); // 198.18.0.0/15
        !(ip.is_private()
            || ip.is_loopback()
            || ip.is_link_local()
            || ip.is_unspecified()
            || ip.is_broadcast()
            || ip.is_documentation()
            || ip.is_multicast()
            || shared
            || benchmarking
            || first == 0
            || first >= 240) // reserved, 240.0.0.0/4
    };
    let public_v6 = |ip: Ipv6Addr| {
        let segments = ip.segments();
        let first = segments[0];
        let unique_local = (first & 0xfe00) == 0xfc00;
        let link_local = (first & 0xffc0) == 0xfe80;
        let site_local = (first & 0xffc0) == 0xfec0;
        let multicast = (first & 0xff00) == 0xff00;
        // an IPv4 address carried inside: ::ffff:a.b.c.d, the old ::a.b.c.d, NAT64's 64:ff9b::a.b.c.d and 6to4's
        // 2002:aabb:ccdd:: all reach that IPv4 address in the end
        let octets = ip.octets();
        let last = Ipv4Addr::new(octets[12], octets[13], octets[14], octets[15]);
        let carried = if let Some(v4) = ip.to_ipv4_mapped() {
            Some(v4)
        } else if (segments[..6] == [0; 6] && !ip.is_loopback() && !ip.is_unspecified())
            || segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0]
        {
            Some(last)
        } else if first == 0x2002 {
            Some(Ipv4Addr::new(octets[2], octets[3], octets[4], octets[5]))
        } else {
            None
        };
        match carried {
            Some(v4) => public_v4(v4),
            None => !(ip.is_loopback() || ip.is_unspecified() || unique_local || link_local || site_local || multicast),
        }
    };
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => public_v6(ip),
    }
}

/// `text` as an address a paper's record may keep and the window may open: `https` or `http`, with a host and no
/// user name or password before it (`https://neurips.cc@evil.example/` names `evil.example`). Anything else a
/// source wrote (`javascript:`, `data:`, `file:`) is no address at all.
pub fn web_address(text: &str) -> Option<String> {
    let address = Url::parse(text.trim()).ok()?;
    let fine = matches!(address.scheme(), "https" | "http")
        && address.host_str().is_some_and(|host| !host.is_empty())
        && address.username().is_empty()
        && address.password().is_none();
    fine.then(|| address.to_string())
}

/// Does the address name a host on the public internet, by its name or its numbers?
fn is_public(address: &Url) -> bool {
    match address.host() {
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            let local = ["localhost", "local", "internal", "home.arpa"];
            !name.is_empty() && !local.iter().any(|end| name == *end || name.ends_with(&format!(".{end}")))
        }
        Some(url::Host::Ipv4(ip)) => is_public_ip(ip.into()),
        Some(url::Host::Ipv6(ip)) => is_public_ip(ip.into()),
        None => false,
    }
}

/// Names resolved as the system resolves them, keeping only public addresses ([`is_public_ip`]): the address
/// actually connected to is checked, not only the name, so a site's name (or a redirect's) that resolves to this
/// computer or its network is never connected to, whatever check its address passed before. A proxy configured
/// on this computer (`HTTPS_PROXY`) is the user's own and is resolved as it is.
#[derive(Debug, Default)]
pub(crate) struct PublicOnly(ureq::unversioned::resolver::DefaultResolver);

impl ureq::unversioned::resolver::Resolver for PublicOnly {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let found = self.0.resolve(uri, config, timeout)?;
        if config.proxy().is_some_and(|proxy| Some(proxy.host()) == uri.host()) {
            return Ok(found);
        }
        let mut kept = self.0.empty();
        for address in found.iter().filter(|address| is_public_ip(address.ip())) {
            kept.push(*address);
        }
        if kept.is_empty() { Err(ureq::Error::HostNotFound) } else { Ok(kept) }
    }
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
        let config = ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(15)))
            .tls_config(tls)
            .build();
        let connector = ureq::unversioned::transport::DefaultConnector::new();
        Self { agent: ureq::Agent::with_parts(config, connector, PublicOnly::default()) }
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
        // the date form of Retry-After is not read: a refusal without seconds is waited out by the default
        let retry_after =
            header("retry-after").and_then(|value| value.trim().parse::<u64>().ok()).map(Duration::from_secs);
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
        Ok(Response { status: response.status().as_u16(), content_type, location, retry_after, body })
    }
}

/// Canned answers for the tests: an address it was not given answers 404, and every address asked is noted. It
/// may be asked from many threads at once, and then also notes how many requests each host had at once at most,
/// and when each request began; [`Fake::slow`] makes every answer take a while, so that requests overlap.
#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod fake {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

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
        /// Answers given before the standing one, first first, for an address asked more than once.
        first: Mutex<HashMap<String, Vec<Response>>>,
        asked: Mutex<Vec<String>>,
        delay: Duration,
        hosts: Mutex<HashMap<String, Load>>,
    }

    /// What one host was asked: requests under way now, the most at once, and when each began.
    #[derive(Default)]
    struct Load {
        now: usize,
        most: usize,
        began: Vec<Instant>,
    }

    pub fn ok(content_type: &str, body: &str) -> Response {
        Response {
            status: 200,
            content_type: Some(content_type.to_owned()),
            location: None,
            retry_after: None,
            body: body.into(),
        }
    }

    pub fn status(status: u16) -> Response {
        Response {
            status,
            content_type: Some("text/html".to_owned()),
            location: None,
            retry_after: None,
            body: Vec::new(),
        }
    }

    pub fn redirect(status: u16, location: &str) -> Response {
        Response {
            status,
            content_type: None,
            location: Some(location.to_owned()),
            retry_after: None,
            body: Vec::new(),
        }
    }

    fn host_of(url: &str) -> String {
        url::Url::parse(url).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_default()
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

        /// `answer` the first time `url` is asked (then the next of these, then the standing answer).
        pub fn first(self, url: &str, answer: Response) -> Self {
            self.first.lock().unwrap().entry(url.to_owned()).or_default().push(answer);
            self
        }

        /// Every answer takes `delay`.
        pub fn slow(mut self, delay: Duration) -> Self {
            self.delay = delay;
            self
        }

        pub fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }

        /// The most requests `host` had at once (`*`: all hosts together).
        pub fn most_at_once(&self, host: &str) -> usize {
            self.hosts.lock().unwrap().get(host).map_or(0, |load| load.most)
        }

        /// When each request to `host` began, in order.
        pub fn began(&self, host: &str) -> Vec<Instant> {
            let mut began = self.hosts.lock().unwrap().get(host).map(|load| load.began.clone()).unwrap_or_default();
            began.sort();
            began
        }
    }

    impl Fetch for Fake {
        fn get(&self, url: &str, limits: Limits) -> Result<Response, FetchError> {
            self.asked.lock().unwrap().push(url.to_owned());
            // each host by its name, and every host together as "*"
            let hosts = [host_of(url), "*".to_owned()];
            for host in &hosts {
                let mut counts = self.hosts.lock().unwrap();
                let load = counts.entry(host.clone()).or_default();
                load.now += 1;
                load.most = load.most.max(load.now);
                load.began.push(Instant::now());
            }
            std::thread::sleep(self.delay);
            let queued = self
                .first
                .lock()
                .unwrap()
                .get_mut(url)
                .filter(|queued| !queued.is_empty())
                .map(|queued| queued.remove(0));
            let answer = match queued {
                Some(answer) => Ok(answer),
                None => self.answers.get(url).cloned().unwrap_or_else(|| Ok(status(404))),
            };
            for host in hosts {
                self.hosts.lock().unwrap().entry(host).or_default().now -= 1;
            }
            let mut answer = answer?;
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
    fn a_redirect_to_this_computer_or_its_network_is_refused_before_it_is_fetched() {
        for inside in [
            "https://localhost/api/runs",
            "https://LOCALHOST./x",
            "https://app.localhost/x",
            "https://127.0.0.1:8765/api/runs",
            "https://10.0.0.7/x",
            "https://192.168.1.1/admin",
            "https://172.16.4.2/x",
            "https://169.254.169.254/latest/meta-data/",
            "https://100.64.0.1/x",
            "https://0.0.0.0/x",
            "https://[::1]/x",
            "https://[fd00::1]/x",
            "https://[fe80::1]/x",
            "https://[::ffff:127.0.0.1]/x",
            "https://printer.local/x",
            "https://router.home.arpa/x",
        ] {
            let fake = Fake::new().with("https://example.org/a", redirect(302, inside));
            let refused = get_following(&fake, "https://example.org/a", LIMITS, &any_https);
            assert!(matches!(refused, Err(GetError::NotAllowed(_))), "{inside}: {refused:?}");
            assert_eq!(fake.asked(), ["https://example.org/a"], "{inside}");
        }
        for outside in ["https://arxiv.org/x", "https://8.8.8.8/x", "https://[2606:4700::1]/x", "https://neurips.cc./x"]
        {
            assert!(any_https(&Url::parse(outside).unwrap()), "{outside}");
        }
    }

    #[test]
    fn an_address_of_this_computer_or_its_network_written_in_any_form_is_not_public() {
        for inside in [
            "::127.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b::a9fe:a9fe",
            "2002:7f00:1::",
            "2002:c0a8:101::1",
            "fec0::1",
            "ff02::1",
            "224.0.0.1",
            "240.0.0.1",
            "198.18.0.1",
        ] {
            assert!(!is_public_ip(inside.parse().unwrap()), "{inside}");
        }
        for outside in ["8.8.8.8", "2606:4700::1", "64:ff9b::808:808", "2002:808:808::1"] {
            assert!(is_public_ip(outside.parse().unwrap()), "{outside}");
        }
    }

    #[test]
    fn a_name_that_resolves_to_this_computer_is_never_connected_to() {
        // `localhost` stands for any public-looking name a site's DNS points at 127.0.0.1: the check is made on
        // the address resolved, so the listener here must see no connection at all
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let quick = Limits { max_bytes: 1024, time: Duration::from_secs(3) };
        let fetched = Http::new().get(&format!("http://localhost:{port}/"), quick);
        assert!(fetched.is_err(), "{fetched:?}");
        assert!(listener.accept().is_err(), "the engine connected to this computer");
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
