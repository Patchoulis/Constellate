//! A paper's PDF on demand: the one kept in the library, or fetched now from where the paper is published and
//! kept.
//!
//! What asking for a paper's PDF comes to is one [`PdfOutcome`]: kept, fetched now, or one of the reasons there is
//! none (no address known, the source refused or could not be reached, too large, not a PDF). The route that
//! serves it turns each into its status and a message the reader can show; a job counts them.
//!
//! Fetching goes through [`crate::sources`] only: the publishers' hosts ([`PDF_HOSTS`]), https on the usual port,
//! public addresses only (the resolver refuses this computer and its network), every redirect checked before it
//! is followed, at most [`crate::sources::proceedings::MAX_PDF`] bytes and
//! [`crate::sources::proceedings::PDF_TIME`] for all of it, and only bytes that start as a PDF does. A
//! conference paper's address comes from its proceedings' index, fetched as needed and kept
//! ([`crate::library::papers::proceedings`]).
//!
//! # Politely, and once
//!
//! Every request to a publisher (an index page, a PDF) passes one [`Gate`] per site, shared by everyone who
//! fetches PDFs on demand, so that a reader opening papers while a job reads a run's declarations together keep
//! each publisher's rule ([`crate::sources::polite::PUBLISHER`]): two at once. A site that refuses (429, 503) is
//! left alone for as long as it asks, else [`FIRST_BACK_OFF`]. A request to arXiv waits instead for its turn at
//! arXiv's one pace, first come first served with everything else the engine sends to arXiv (the network takes it:
//! [`crate::sources::polite::ARXIV_PACE`]).
//!
//! Two requests for the same paper's PDF at once fetch it once: the first claims the paper and the second waits
//! for what the first got (the same scheme as the library's reading of passages). One index is fetched at a time.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use url::Url;

use crate::library::{Library, LibraryError, Paper};
use crate::sources::polite::{FIRST_BACK_OFF, PUBLISHER, Rule};
use crate::sources::proceedings::{self, IndexState, PDF_HOSTS, PdfError, PdfSource};
use crate::sources::{Fetch, FetchError, GetError, Limits, Response};

/// What asking for a paper's PDF came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfOutcome {
    /// The library had it.
    Kept(PathBuf),
    /// It was fetched now, and is kept from now on.
    FetchedNow(PathBuf),
    /// No address a program may fetch it from is known: not on arXiv, and its conference publishes nowhere a
    /// program may fetch from (or does not list it).
    NoAddress,
    /// The source answered, but not with the PDF: an error status, a refusal to be asked so often, an address
    /// off the publishers' hosts.
    Refused { why: String },
    /// The source could not be reached, or took too long.
    Unreachable { why: String },
    /// The source's file is larger than any paper.
    TooLarge,
    /// The source sent something that is not a PDF.
    NotAPdf { address: String },
}

impl PdfOutcome {
    /// The file, when there is one now.
    pub fn file(&self) -> Option<&PathBuf> {
        match self {
            Self::Kept(path) | Self::FetchedNow(path) => Some(path),
            Self::NoAddress
            | Self::Refused { .. }
            | Self::Unreachable { .. }
            | Self::TooLarge
            | Self::NotAPdf { .. } => None,
        }
    }

    /// Why there is no PDF, in words the reader can show; `None` when there is one.
    pub fn why_not(&self) -> Option<String> {
        match self {
            Self::Kept(_) | Self::FetchedNow(_) => None,
            Self::NoAddress => Some(
                "This paper's PDF is not published anywhere Constellate may fetch it from: open it from its own page"
                    .to_owned(),
            ),
            Self::Refused { why } => Some(format!("The PDF could not be fetched: {why}")),
            Self::Unreachable { why } => Some(format!("The PDF could not be fetched: {why}. Try again later")),
            Self::TooLarge => {
                Some(format!("The PDF is larger than {} MB, more than any paper", proceedings::MAX_PDF / (1024 * 1024)))
            }
            Self::NotAPdf { address } => Some(format!("{address} did not send a PDF")),
        }
    }
}

/// The longest a site's `Retry-After` is obeyed for, as the batch downloader caps it.
const LONGEST_BACK_OFF: Duration = Duration::from_secs(3600);
/// The longest a fetch waits for its turn: past that it is told the site asked to be left alone, rather than
/// holding a reader (or a job's cancel) for as long as the site asked.
const LONGEST_TURN_WAIT: Duration = Duration::from_secs(30);

/// One site's turn-taking.
struct Site {
    rule: Rule,
    busy: usize,
    next: Instant,
}

/// Who may ask which site now: [`Rule`]s by site, shared by every fetch on demand.
#[derive(Default)]
pub struct Gate {
    sites: Mutex<HashMap<String, Site>>,
    changed: Condvar,
}

/// The site an address is on, as its rule is kept: arXiv as one, others by host without `www.`.
fn site_of(address: &str) -> String {
    let host = Url::parse(address).ok().and_then(|url| url.host_str().map(str::to_ascii_lowercase)).unwrap_or_default();
    if host == "arxiv.org" || host.ends_with(".arxiv.org") {
        return "arxiv.org".to_owned();
    }
    host.strip_prefix("www.").map_or(host.clone(), str::to_owned)
}

/// A turn at a site, given back when dropped (also on a panic), with the pause after it.
struct Turn<'a> {
    gate: &'a Gate,
    site: String,
    /// How long the site asked to be left alone after this request, if it refused.
    back_off: Option<Duration>,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let mut sites = self.gate.sites.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(site) = sites.get_mut(&self.site) {
            site.busy = site.busy.saturating_sub(1);
            let pause = self.back_off.unwrap_or(site.rule.pause).max(site.rule.pause);
            site.next = site.next.max(Instant::now() + pause);
        }
        drop(sites);
        self.gate.changed.notify_all();
    }
}

impl Gate {
    /// Wait for a turn at the site of `address`; or, when the site asked to be left alone for longer than
    /// [`LONGEST_TURN_WAIT`], say how much longer instead of waiting (a reader must not hang on it).
    fn turn(&self, address: &str) -> Result<Turn<'_>, Duration> {
        let name = site_of(address);
        let mut sites = self.sites.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            let now = Instant::now();
            let site = sites.entry(name.clone()).or_insert_with(|| Site { rule: PUBLISHER, busy: 0, next: now });
            if site.busy < site.rule.at_once && site.next <= now {
                site.busy += 1;
                return Ok(Turn { gate: self, site: name, back_off: None });
            }
            if site.next > now + LONGEST_TURN_WAIT {
                return Err(site.next - now);
            }
            let wait = if site.next > now { site.next - now } else { Duration::from_millis(100) };
            sites = self.changed.wait_timeout(sites, wait).unwrap_or_else(PoisonError::into_inner).0;
        }
    }
}

/// [`Fetch`] through the [`Gate`]: every request to a publisher, redirects included, waits for its turn at its
/// site; one to arXiv for its turn at arXiv's one pace, which the fetch given keeps (the network does).
struct Gated<'a> {
    fetch: &'a (dyn Fetch + Send + Sync),
    gate: &'a Gate,
}

impl Fetch for Gated<'_> {
    fn get(&self, url: &str, limits: Limits) -> Result<Response, FetchError> {
        if site_of(url) == "arxiv.org" {
            return self.fetch.get(url, limits);
        }
        let mut turn = self.gate.turn(url).map_err(|left| {
            FetchError::Unreachable(
                url.to_owned(),
                format!("it asked to be left alone for {} more seconds", left.as_secs()),
            )
        })?;
        let answer = self.fetch.get(url, limits);
        if let Ok(response) = &answer
            && matches!(response.status, 429 | 503)
        {
            turn.back_off = Some(response.retry_after.unwrap_or(FIRST_BACK_OFF).min(LONGEST_BACK_OFF));
        }
        answer
    }
}

/// A fetch under way, which whoever else asks for the same paper waits for.
#[derive(Default)]
struct Pending {
    outcome: Mutex<Option<Result<PdfOutcome, String>>>,
    ready: Condvar,
}

impl Pending {
    fn wait(&self) -> Result<PdfOutcome, String> {
        let mut outcome = self.outcome.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(outcome) = outcome.as_ref() {
                return outcome.clone();
            }
            outcome = self.ready.wait(outcome).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// The claim on one paper's fetch; dropped, it hands what came of it to whoever waits (an error when the fetch
/// was cut short by a panic) and lets the paper be claimed again.
struct Claim<'a> {
    pdfs: &'a Pdfs,
    paper_id: String,
    pending: Arc<Pending>,
    outcome: Option<Result<PdfOutcome, String>>,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        let outcome = self.outcome.take().unwrap_or_else(|| Err("the fetch stopped unexpectedly".to_owned()));
        let mut claimed = self.pdfs.claimed.lock().unwrap_or_else(PoisonError::into_inner);
        if claimed.get(&self.paper_id).is_some_and(|pending| Arc::ptr_eq(pending, &self.pending)) {
            claimed.remove(&self.paper_id);
        }
        drop(claimed);
        *self.pending.outcome.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
        self.pending.ready.notify_all();
    }
}

/// One conference's index as last read, behind a lock of its own (fetching it holds up no other conference).
type IndexSlot = Arc<Mutex<Option<Arc<IndexState>>>>;

/// Papers' PDFs on demand, shared by every request and job of the engine.
pub struct Pdfs {
    fetch: Arc<dyn Fetch + Send + Sync>,
    gate: Gate,
    claimed: Mutex<HashMap<String, Arc<Pending>>>,
    /// The conferences' indexes as last read from the library, so that a request does not decode thousands of
    /// titles again; and the lock that lets one index be fetched at a time.
    indexes: Mutex<HashMap<String, IndexSlot>>,
}

/// The network, in the program; nothing at all in tests, which give their own fake when they fetch.
impl Default for Pdfs {
    #[cfg(not(test))]
    fn default() -> Self {
        Self::new(Arc::new(crate::sources::Http::new()))
    }

    #[cfg(test)]
    fn default() -> Self {
        Self::new(Arc::new(NoNetwork))
    }
}

/// What a test's engine fetches through unless it says otherwise: nothing reaches the network.
#[cfg(test)]
struct NoNetwork;

#[cfg(test)]
impl Fetch for NoNetwork {
    fn get(&self, url: &str, _: Limits) -> Result<Response, FetchError> {
        Err(FetchError::Unreachable(url.to_owned(), "a test reaches no network".to_owned()))
    }
}

impl Pdfs {
    /// Fetching through `fetch`: the network ([`crate::sources::Http`]), or a test's fake.
    pub fn new(fetch: Arc<dyn Fetch + Send + Sync>) -> Self {
        Self { fetch, gate: Gate::default(), claimed: Mutex::default(), indexes: Mutex::default() }
    }

    /// What is fetched through when a paper is asked about outside the library (its references looked up on
    /// arXiv): the network in the program, a test's fake in a test.
    pub fn fetch(&self) -> &(dyn Fetch + Send + Sync) {
        self.fetch.as_ref()
    }

    /// The PDF of `paper_id`: the kept one, or fetched now and kept. [`LibraryError::UnknownPaper`] for a paper
    /// the library does not have. Blocks on the network: call it off the async threads (a request's blocking
    /// thread, a job).
    pub fn pdf(&self, library: &Library, paper_id: &str) -> Result<PdfOutcome, LibraryError> {
        let record = library.paper(paper_id)?.ok_or_else(|| LibraryError::UnknownPaper(paper_id.to_owned()))?;
        let path = library.pdf_file(paper_id);
        if path.is_file() {
            return Ok(PdfOutcome::Kept(path));
        }
        let mut claim = {
            let mut claimed = self.claimed.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(pending) = claimed.get(paper_id) {
                let pending = Arc::clone(pending);
                drop(claimed);
                return match pending.wait() {
                    // fetched by the other request: kept now, as far as this one is concerned
                    Ok(PdfOutcome::FetchedNow(path)) => Ok(PdfOutcome::Kept(path)),
                    Ok(outcome) => Ok(outcome),
                    Err(why) => Ok(PdfOutcome::Unreachable { why }),
                };
            }
            let pending = Arc::new(Pending::default());
            claimed.insert(paper_id.to_owned(), Arc::clone(&pending));
            Claim { pdfs: self, paper_id: paper_id.to_owned(), pending, outcome: None }
        };
        let outcome = self.fetch_and_keep(library, &record);
        claim.outcome = Some(outcome.as_ref().map(Clone::clone).map_err(ToString::to_string));
        outcome
    }

    fn fetch_and_keep(&self, library: &Library, record: &Paper) -> Result<PdfOutcome, LibraryError> {
        let Some(address) = self.address(library, record)? else { return Ok(PdfOutcome::NoAddress) };
        let gated = Gated { fetch: self.fetch.as_ref(), gate: &self.gate };
        match proceedings::download_pdf(&gated, &address) {
            Ok(bytes) => Ok(PdfOutcome::FetchedNow(library.keep_pdf(&record.id, &bytes)?)),
            Err(error) => Ok(outcome_of(error)),
        }
    }

    /// Where the paper's PDF may be fetched from, if anywhere: its arXiv address, or its conference's index
    /// (fetched now if it is due, and kept). Python's `pdf_url`.
    pub fn address(&self, library: &Library, record: &Paper) -> Result<Option<String>, LibraryError> {
        let paper = as_source(record);
        let (publisher, conference, year) = match proceedings::pdf_source(&paper) {
            PdfSource::Arxiv(address) => return Ok(Some(address.to_owned())),
            PdfSource::Nowhere => return Ok(None),
            PdfSource::Proceedings { publisher, slug, year } => (publisher, format!("{slug}-{year}"), year),
        };
        // one lock per conference, so that fetching one index holds up no other
        let slot = Arc::clone(
            self.indexes.lock().unwrap_or_else(PoisonError::into_inner).entry(conference.clone()).or_default(),
        );
        let mut kept = slot.lock().unwrap_or_else(PoisonError::into_inner);
        let state = match kept.as_ref() {
            Some(state) => Arc::clone(state),
            None => Arc::new(library.proceedings_index(&conference)?),
        };
        let now = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now());
        let state = if state.is_due(now) {
            let gated = Gated { fetch: self.fetch.as_ref(), gate: &self.gate };
            let refreshed = (*state).clone().refreshed(&gated, publisher, year, now);
            library.keep_proceedings_index(&conference, &refreshed)?;
            Arc::new(refreshed)
        } else {
            state
        };
        *kept = Some(Arc::clone(&state));
        drop(kept);
        Ok(state.address_of(&paper.title).map(str::to_owned))
    }

    /// Can the paper's PDF be had: kept, on arXiv, or in its conference's proceedings as far as is known now
    /// (Python's `readable`). Nothing is fetched to tell, so a paper's record never waits on the network: an
    /// index not fetched yet counts as having the paper, and asking for the PDF then says why not if it does not.
    pub fn readable(&self, library: &Library, record: &Paper) -> Result<bool, LibraryError> {
        if library.pdf_file(&record.id).is_file() {
            return Ok(true);
        }
        let paper = as_source(record);
        Ok(match proceedings::pdf_source(&paper) {
            PdfSource::Arxiv(_) => true,
            PdfSource::Nowhere => false,
            PdfSource::Proceedings { slug, year, .. } => {
                let state = library.proceedings_index(&format!("{slug}-{year}"))?;
                state.titles().is_none() || state.address_of(&paper.title).is_some()
            }
        })
    }
}

/// The library's record as the sources read it.
pub fn as_source(record: &Paper) -> crate::sources::Paper {
    crate::sources::Paper {
        id: record.id.clone(),
        title: record.title.clone(),
        abstract_text: record.r#abstract.clone(),
        authors: record.authors.clone(),
        published: record.published.clone(),
        updated: record.updated.clone(),
        comment: record.comment.clone(),
        primary_category: record.primary_category.clone(),
        categories: record.categories.clone(),
        url: record.url.clone().unwrap_or_default(),
        pdf_url: record.pdf_url.clone(),
    }
}

fn outcome_of(error: PdfError) -> PdfOutcome {
    match error {
        PdfError::NotAPdf(address) => PdfOutcome::NotAPdf { address },
        PdfError::Status(address, status @ (429 | 503)) => {
            PdfOutcome::Refused { why: format!("{} asks to be asked less often (HTTP {status})", site_of(&address)) }
        }
        PdfError::Status(address, status) => {
            PdfOutcome::Refused { why: format!("{} answered HTTP {status}", site_of(&address)) }
        }
        PdfError::Get(GetError::TooLarge(_)) => PdfOutcome::TooLarge,
        PdfError::Get(GetError::NotAllowed(address)) => PdfOutcome::Refused {
            why: format!("{address} is not on a host papers are published on ({})", PDF_HOSTS.join(", ")),
        },
        PdfError::Get(GetError::TooManyRedirects(address)) => {
            PdfOutcome::Refused { why: format!("{} redirects too many times", site_of(&address)) }
        }
        PdfError::Get(why @ (GetError::TooSlow(_) | GetError::Fetch(_))) => {
            PdfOutcome::Unreachable { why: why.to_string() }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::library::Known;
    use crate::library::papers::tests::paper;
    use crate::sources::fake::{Fake, ok, redirect, status};
    use crate::sources::polite::{ArxivPace, Paced};

    const ARXIV_PDF: &str = "https://arxiv.org/pdf/2601.00001";

    fn library_with(papers: &[Paper]) -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        library.add_papers(papers, None, Known::Keep).unwrap();
        (folder, library)
    }

    fn on_arxiv(id: &str) -> Paper {
        Paper { pdf_url: Some(format!("https://arxiv.org/pdf/{id}")), ..paper(id) }
    }

    fn pdf(body: &str) -> Response {
        ok("application/pdf", &format!("%PDF-1.4 {body}"))
    }

    #[test]
    fn a_kept_pdf_is_served_without_asking_anyone() {
        let (_folder, library) = library_with(&[on_arxiv("2601.00001")]);
        library.keep_pdf("2601.00001", b"%PDF-1.4 kept").unwrap();
        let fake = Arc::new(Fake::new());
        let pdfs = Pdfs::new(fake.clone());
        assert!(matches!(pdfs.pdf(&library, "2601.00001").unwrap(), PdfOutcome::Kept(_)));
        assert!(fake.asked().is_empty());
    }

    #[test]
    fn a_missing_pdf_is_fetched_once_kept_and_then_kept() {
        let (_folder, library) = library_with(&[on_arxiv("2601.00001")]);
        let fake = Arc::new(Fake::new().with(ARXIV_PDF, pdf("fetched")));
        let pdfs = Pdfs::new(fake.clone());
        let PdfOutcome::FetchedNow(path) = pdfs.pdf(&library, "2601.00001").unwrap() else { panic!() };
        assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-1.4 fetched");
        assert_eq!(path, library.pdf_file("2601.00001"));
        assert!(matches!(pdfs.pdf(&library, "2601.00001").unwrap(), PdfOutcome::Kept(_)));
        assert_eq!(fake.asked(), [ARXIV_PDF]);
    }

    #[test]
    fn each_reason_there_is_no_pdf_is_its_own_outcome() {
        let papers = [
            paper("iclr-2026-1"),
            on_arxiv("2601.00002"),
            on_arxiv("2601.00003"),
            on_arxiv("2601.00004"),
            on_arxiv("2601.00005"),
            on_arxiv("2601.00006"),
        ];
        let (_folder, library) = library_with(&papers);
        let fake = Fake::new()
            .with("https://arxiv.org/pdf/2601.00002", status(404))
            .with("https://arxiv.org/pdf/2601.00003", ok("text/html", "<html>a captcha</html>"))
            .with("https://arxiv.org/pdf/2601.00004", redirect(302, "https://evil.example.org/x.pdf"))
            .with("https://arxiv.org/pdf/2601.00005", ok("application/pdf", &"x".repeat(10)))
            .failing(
                "https://arxiv.org/pdf/2601.00006",
                FetchError::Unreachable("arxiv.org".into(), "no connection".into()),
            );
        let pdfs = Pdfs::new(Arc::new(fake));
        let asked = |id: &str| pdfs.pdf(&library, id).unwrap();
        assert_eq!(asked("iclr-2026-1"), PdfOutcome::NoAddress);
        assert!(matches!(asked("2601.00002"), PdfOutcome::Refused { why } if why.contains("404")));
        assert!(matches!(asked("2601.00003"), PdfOutcome::NotAPdf { .. }));
        assert!(matches!(asked("2601.00004"), PdfOutcome::Refused { why } if why.contains("evil.example.org")));
        assert!(matches!(asked("2601.00005"), PdfOutcome::NotAPdf { .. }));
        assert!(matches!(asked("2601.00006"), PdfOutcome::Unreachable { .. }));
        for id in ["iclr-2026-1", "2601.00002", "2601.00006"] {
            assert!(!library.pdf_file(id).exists(), "nothing kept for {id}");
            assert!(asked(id).why_not().is_some());
        }
        assert!(matches!(pdfs.pdf(&library, "nowhere"), Err(LibraryError::UnknownPaper(_))));
    }

    #[test]
    fn a_pdf_larger_than_any_paper_is_too_large() {
        assert_eq!(outcome_of(PdfError::Get(GetError::TooLarge("x".into()))), PdfOutcome::TooLarge);
        assert!(PdfOutcome::TooLarge.why_not().unwrap().contains("80 MB"));
    }

    #[test]
    fn two_requests_for_one_paper_at_once_fetch_it_once() {
        let (_folder, library) = library_with(&[on_arxiv("2601.00001")]);
        let fake = Arc::new(Fake::new().with(ARXIV_PDF, pdf("once")).slow(Duration::from_millis(150)));
        let pdfs = Pdfs::new(fake.clone());
        let outcomes: Vec<PdfOutcome> = std::thread::scope(|scope| {
            let askers: Vec<_> = (0..4).map(|_| scope.spawn(|| pdfs.pdf(&library, "2601.00001").unwrap())).collect();
            askers.into_iter().map(|asker| asker.join().unwrap()).collect()
        });
        assert_eq!(fake.asked(), [ARXIV_PDF], "fetched once");
        assert_eq!(outcomes.iter().filter(|outcome| matches!(outcome, PdfOutcome::FetchedNow(_))).count(), 1);
        assert!(outcomes.iter().all(|outcome| outcome.file().is_some()));
        assert!(pdfs.claimed.lock().unwrap().is_empty(), "no claim left behind");
    }

    #[test]
    fn arxiv_is_asked_one_request_at_a_time_and_at_its_pace_by_everyone() {
        static PACE: ArxivPace = ArxivPace::new(Duration::from_millis(300));
        let papers: Vec<Paper> = (1..=2).map(|n| on_arxiv(&format!("2601.0000{n}"))).collect();
        let (_folder, library) = library_with(&papers);
        let fake = Arc::new(
            Fake::new()
                .with("https://arxiv.org/pdf/2601.00001", pdf("one"))
                .with("https://arxiv.org/pdf/2601.00002", pdf("two")),
        );
        // as the network does it
        let pdfs = Pdfs::new(Arc::new(Paced { inner: Arc::clone(&fake), pace: &PACE }));
        std::thread::scope(|scope| {
            for paper in &papers {
                let (pdfs, library) = (&pdfs, &library);
                scope.spawn(move || pdfs.pdf(library, &paper.id).unwrap());
            }
        });
        let began = fake.began("arxiv.org");
        assert_eq!((began.len(), fake.most_at_once("arxiv.org")), (2, 1));
        assert!(began[1] - began[0] >= Duration::from_millis(300), "{:?}", began[1] - began[0]);
    }

    #[test]
    fn a_conference_papers_address_comes_from_its_index_fetched_once_and_kept() {
        let neurips = Paper { title: "A Great Paper".into(), ..paper("neurips-2025-1") };
        let other = Paper { title: "Not In The Proceedings".into(), ..paper("neurips-2025-2") };
        let (_folder, library) = library_with(&[neurips.clone(), other.clone()]);
        let listing = "https://proceedings.neurips.cc/paper_files/paper/2025";
        let entry = r#"<a href="/paper_files/paper/2025/hash/abc-Abstract-Conference.html">A <i>Great</i> Paper</a>"#;
        let file = "https://proceedings.neurips.cc/paper_files/paper/2025/file/abc-Paper-Conference.pdf";
        let fake = Arc::new(Fake::new().with(listing, ok("text/html", entry)).with(file, pdf("neurips")));
        let pdfs = Pdfs::new(fake.clone());
        // before its index is fetched, a conference paper may have a PDF; telling so fetches nothing
        assert!(pdfs.readable(&library, &other).unwrap() && fake.asked().is_empty());
        assert!(matches!(pdfs.pdf(&library, "neurips-2025-1").unwrap(), PdfOutcome::FetchedNow(_)));
        assert_eq!(pdfs.pdf(&library, "neurips-2025-2").unwrap(), PdfOutcome::NoAddress);
        assert_eq!(fake.asked(), [listing, file], "the index asked for once");
        assert!(pdfs.readable(&library, &neurips).unwrap() && !pdfs.readable(&library, &other).unwrap());
        // kept: another engine (a restart) does not ask for it again
        let again = Pdfs::new(Arc::new(Fake::new()));
        assert_eq!(again.address(&library, &neurips).unwrap().as_deref(), Some(file));
    }

    #[test]
    fn a_site_that_refuses_is_left_alone_for_as_long_as_it_asks() {
        let gate = Gate::default();
        {
            let mut turn = gate.turn("https://proceedings.mlr.press/v1/a.pdf").unwrap();
            turn.back_off = Some(Duration::from_millis(300));
        }
        let started = Instant::now();
        drop(gate.turn("https://proceedings.mlr.press/v1/b.pdf").unwrap());
        assert!(started.elapsed() >= Duration::from_millis(250), "{:?}", started.elapsed());
        // another site is not held up by it
        let started = Instant::now();
        drop(gate.turn("https://openaccess.thecvf.com/x.pdf").unwrap());
        assert!(started.elapsed() < Duration::from_millis(100));
    }
}
