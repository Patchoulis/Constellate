//! Where a paper's published PDF is, and fetching it safely.
//!
//! Where the PDF comes from depends on who published the paper:
//!
//! ```text
//!     NeurIPS             proceedings.neurips.cc
//!     ICML                proceedings.mlr.press (PMLR; one volume per year)
//!     CVPR, ICCV, WACV    openaccess.thecvf.com
//!     arXiv               arxiv.org
//!     ICLR and others     nowhere a program may fetch from (OpenReview does not serve them to programs)
//! ```
//!
//! A conference's papers are matched to its proceedings by title, through an index built from the proceedings'
//! listing pages. A title that two papers share is left out of the index: a guess there would show the wrong
//! paper. The index is fetched again after [`INDEX_DAYS`] (proceedings appear track by track), and one that could
//! not be fetched is not asked for again for [`RETRY`], so a missing list costs one request, not one per paper.
//! What the index is at any moment is an [`IndexState`], which the library keeps.
//!
//! A PDF is downloaded only from the hosts papers are published on, over https, with every redirect checked
//! before it is followed, within a size cap and an overall time limit, and only if it is a PDF.

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use url::Url;

use super::polite::{self, Politeness};
use super::{Fetch, GetError, Limits, Origin, Paper, any_https, get_following};
use crate::threads::{Outcome, Progress};

/// The hosts a PDF may come from, a leading "www." aside.
pub const PDF_HOSTS: [&str; 5] = [
    "arxiv.org",
    "proceedings.neurips.cc",
    "proceedings.mlr.press",
    "raw.githubusercontent.com",
    "openaccess.thecvf.com",
];
pub const MAX_PDF: u64 = 80 * 1024 * 1024;
/// For the whole download, every redirect included.
pub const PDF_TIME: Duration = Duration::from_secs(180);
/// A conference's index of PDFs is fetched again after this many days...
pub const INDEX_DAYS: i64 = 30;
/// ...and an index that could not be fetched, after this long.
pub const RETRY: TimeDelta = TimeDelta::hours(1);
const PAGE_LIMITS: Limits = Limits { max_bytes: 64 << 20, time: Duration::from_secs(60) };

const NEURIPS: &str = "https://proceedings.neurips.cc";
const PMLR: &str = "https://proceedings.mlr.press";
const CVF: &str = "https://openaccess.thecvf.com";

#[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
#[allow(clippy::unwrap_used)]
static ENTITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"&(?:#([0-9]{1,7})|#[xX]([0-9a-fA-F]{1,6})|([A-Za-z][A-Za-z0-9]{1,31}));").unwrap());
#[allow(clippy::unwrap_used)]
static PMLR_TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<p class="title">(.*?)</p>"#).unwrap());
#[allow(clippy::unwrap_used)]
static PMLR_PDF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<a href="([^"]+\.pdf)""#).unwrap());
#[allow(clippy::unwrap_used)]
static CVF_PAPER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<dt class="ptitle">\s*(?:<br>)?\s*<a href="(/content/[^"]+\.html)">(.*?)</a>"#).unwrap()
});

/// The named characters titles are written with. A letter with an accent stands for the letter alone, since the
/// accent is dropped from a title's key anyway.
fn named(name: &str) -> Option<String> {
    let plain = match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" | "rsquo" | "lsquo" => "'",
        "ldquo" | "rdquo" => "\"",
        "nbsp" => " ",
        "ndash" | "mdash" | "minus" => "-",
        "hellip" => "...",
        // characters with no letters or digits in ASCII: a title's key has nothing of them
        "szlig" | "aelig" | "AElig" | "oelig" | "OElig" | "eth" | "ETH" | "thorn" | "THORN" | "times" | "middot"
        | "alpha" | "beta" | "gamma" | "delta" | "epsilon" | "zeta" | "eta" | "theta" | "iota" | "kappa" | "lambda"
        | "mu" | "nu" | "xi" | "omicron" | "pi" | "rho" | "sigma" | "tau" | "upsilon" | "phi" | "chi" | "psi"
        | "omega" | "Gamma" | "Delta" | "Theta" | "Lambda" | "Xi" | "Pi" | "Sigma" | "Phi" | "Psi" | "Omega"
        | "infin" | "deg" | "micro" | "copy" | "reg" | "trade" | "laquo" | "raquo" | "bull" => "",
        _ => {
            let accents = ["acute", "grave", "uml", "circ", "tilde", "cedil", "ring", "slash", "caron"];
            let letter = name.chars().next().filter(char::is_ascii_alphabetic)?;
            return accents.contains(&&name[1..]).then(|| letter.to_string());
        }
    };
    Some(plain.to_owned())
}

/// HTML's character references read as the characters they stand for; one this does not know stays as written.
fn unescape(text: &str) -> String {
    ENTITY
        .replace_all(text, |found: &regex::Captures| {
            let code = match (found.get(1), found.get(2), found.get(3)) {
                (Some(decimal), _, _) => decimal.as_str().parse().ok(),
                (_, Some(hex), _) => u32::from_str_radix(hex.as_str(), 16).ok(),
                (_, _, Some(name)) => return named(name.as_str()).unwrap_or_else(|| found[0].to_owned()),
                _ => None,
            };
            code.and_then(char::from_u32).map_or_else(|| found[0].to_owned(), String::from)
        })
        .into_owned()
}

/// A title as it is compared between a conference's list and its proceedings: markup and accents dropped, then
/// letters and digits only, in lower case.
pub fn title_key(title: &str) -> String {
    let plain = unescape(&TAG.replace_all(title, " "));
    plain.nfkd().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// {title key: PDF address}, from a conference's proceedings.
pub type Titles = BTreeMap<String, String>;

/// The index of `(title, address)` pairs, without the titles that two different papers share.
pub fn by_title(pairs: impl IntoIterator<Item = (String, String)>) -> Titles {
    let mut found: HashMap<String, Option<String>> = HashMap::new();
    for (title, address) in pairs {
        let key = title_key(&title);
        if key.is_empty() {
            continue;
        }
        let entry = found.entry(key).or_insert_with(|| Some(address.clone()));
        if entry.as_deref() != Some(address.as_str()) {
            *entry = None; // shared: no guess
        }
    }
    found.into_iter().filter_map(|(key, address)| Some((key, address?))).collect()
}

/// Who publishes a conference's PDFs where a program may fetch them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Publisher {
    Neurips,
    /// ICML, through the Proceedings of Machine Learning Research.
    Pmlr,
    /// The Computer Vision Foundation's open access site, under the conference's name in capitals.
    Cvf(&'static str),
}

impl Publisher {
    /// The publisher of a conference, from the slug its paper ids start with.
    pub fn of(slug: &str) -> Option<Self> {
        match slug {
            "neurips" => Some(Self::Neurips),
            "icml" => Some(Self::Pmlr),
            "cvpr" => Some(Self::Cvf("CVPR")),
            "iccv" => Some(Self::Cvf("ICCV")),
            "wacv" => Some(Self::Cvf("WACV")),
            _ => None,
        }
    }
}

/// Where a paper's PDF may be fetched from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PdfSource<'a> {
    /// arXiv's own address for it.
    Arxiv(&'a str),
    /// Its conference's proceedings: look its title up in that year's index.
    Proceedings { publisher: Publisher, slug: &'a str, year: &'a str },
    /// Nowhere a program may fetch from.
    Nowhere,
}

pub fn pdf_source(paper: &Paper) -> PdfSource<'_> {
    if let Some(address) = paper.pdf_url.as_deref().filter(|address| address.starts_with("https://arxiv.org/")) {
        return PdfSource::Arxiv(address);
    }
    match paper.origin() {
        Origin::Conference { slug, year } => match Publisher::of(slug) {
            Some(publisher) => PdfSource::Proceedings { publisher, slug, year },
            None => PdfSource::Nowhere,
        },
        Origin::Arxiv => PdfSource::Nowhere,
    }
}

/// A pattern made for one year; the year is four digits, so it always compiles, but nothing is assumed.
fn for_year(pattern: &str, year: &str) -> Option<Regex> {
    Regex::new(&pattern.replace("{year}", &regex::escape(year))).ok()
}

/// The pages a NeurIPS year is split into by track, in newer years; older years list every paper on one page.
pub fn neurips_tracks(listing: &str, year: &str) -> Vec<String> {
    let Some(track) = for_year(r#"href="(/paper_files/paper/{year}/vol[^"]+)""#, year) else { return Vec::new() };
    let mut pages: Vec<String> = track.captures_iter(listing).map(|found| format!("{NEURIPS}{}", &found[1])).collect();
    pages.sort();
    pages.dedup();
    pages
}

pub fn neurips_papers(page: &str, year: &str) -> Vec<(String, String)> {
    let pattern = r#"(?s)href="/paper_files/paper/{year}/hash/([0-9a-f]+)-Abstract-([\w-]+)\.html">(.*?)</a>"#;
    let Some(paper) = for_year(pattern, year) else { return Vec::new() };
    paper
        .captures_iter(page)
        .map(|found| {
            let address = format!("{NEURIPS}/paper_files/paper/{year}/file/{}-Paper-{}.pdf", &found[1], &found[2]);
            (found[3].to_owned(), address)
        })
        .collect()
}

/// The PMLR volume that holds a year's ICML, from PMLR's front page; none until it is published.
pub fn pmlr_volume(front: &str, year: &str) -> Option<String> {
    let volume = for_year(r#"href="(v\d+)"><b>Volume \d+</b></a>\s*Proceedings of ICML {year}\b"#, year)?;
    volume.captures(front).map(|found| format!("{PMLR}/{}/", &found[1]))
}

/// A PMLR volume's papers, read one block per paper, so that a paper without a PDF is not given its neighbour's.
pub fn pmlr_papers(page: &str, volume: &str) -> Vec<(String, String)> {
    let Ok(base) = Url::parse(volume) else { return Vec::new() };
    page.split(r#"<div class="paper">"#)
        .skip(1)
        .filter_map(|block| {
            let title = PMLR_TITLE.captures(block)?.get(1)?.as_str().to_owned();
            let pdf = base.join(PMLR_PDF.captures(block)?.get(1)?.as_str()).ok()?;
            Some((title, pdf.to_string()))
        })
        .collect()
}

pub fn cvf_papers(page: &str) -> Vec<(String, String)> {
    CVF_PAPER
        .captures_iter(page)
        .map(|found| {
            let pdf = found[1].replace("/html/", "/papers/").replace(".html", ".pdf");
            (found[2].to_owned(), format!("{CVF}{pdf}"))
        })
        .collect()
}

/// A listing page's text; anything but a 200 means it cannot be read now.
fn page(fetch: &dyn Fetch, address: &str) -> Result<String, String> {
    match get_following(fetch, address, PAGE_LIMITS, &any_https) {
        Ok((_, response)) if response.status == 200 => Ok(response.text()),
        Ok((_, response)) => Err(format!("{address} answered HTTP {}", response.status)),
        Err(error) => Err(error.to_string()),
    }
}

/// A conference's index for one year, from its publisher's listing pages.
pub fn fetch_titles(fetch: &dyn Fetch, publisher: Publisher, year: &str) -> Result<Titles, String> {
    match publisher {
        Publisher::Neurips => {
            let listing = page(fetch, &format!("{NEURIPS}/paper_files/paper/{year}"))?;
            let mut pairs = neurips_papers(&listing, year);
            for track in neurips_tracks(&listing, year) {
                pairs.extend(neurips_papers(&page(fetch, &track)?, year));
            }
            Ok(by_title(pairs))
        }
        Publisher::Pmlr => match pmlr_volume(&page(fetch, &format!("{PMLR}/"))?, year) {
            Some(volume) => Ok(by_title(pmlr_papers(&page(fetch, &volume)?, &volume))),
            None => Ok(Titles::new()),
        },
        Publisher::Cvf(name) => Ok(by_title(cvf_papers(&page(fetch, &format!("{CVF}/{name}{year}?day=all"))?))),
    }
}

/// An index as it was fetched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fetched {
    pub on: DateTime<Utc>,
    pub titles: Titles,
}

/// Where a conference's index of PDFs stands, for one year.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexState {
    NeverFetched,
    Fetched(Fetched),
    /// The last attempt found nothing (not published yet, or no connection); an index fetched before it is still
    /// what there is.
    Unavailable {
        at: DateTime<Utc>,
        why: String,
        kept: Option<Fetched>,
    },
}

impl IndexState {
    /// Is it time to fetch the index (again)?
    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        match self {
            Self::NeverFetched => true,
            Self::Fetched(fetched) => now - fetched.on >= TimeDelta::days(INDEX_DAYS),
            Self::Unavailable { at, .. } => now - *at >= RETRY,
        }
    }

    /// The state after an attempt at `now`. An index with nothing in it is not a finished index: it is asked for
    /// again like a failure, and never replaces one that had papers.
    pub fn after(self, now: DateTime<Utc>, attempt: Result<Titles, String>) -> Self {
        let kept = match self {
            Self::NeverFetched => None,
            Self::Fetched(fetched) => Some(fetched),
            Self::Unavailable { kept, .. } => kept,
        };
        match attempt {
            Ok(titles) if !titles.is_empty() => Self::Fetched(Fetched { on: now, titles }),
            Ok(_) => Self::Unavailable { at: now, why: "nothing is listed yet".to_owned(), kept },
            Err(why) => Self::Unavailable { at: now, why, kept },
        }
    }

    /// Fetch the index if it is due; otherwise leave it as it is.
    pub fn refreshed(self, fetch: &dyn Fetch, publisher: Publisher, year: &str, now: DateTime<Utc>) -> Self {
        if self.is_due(now) { self.after(now, fetch_titles(fetch, publisher, year)) } else { self }
    }

    pub fn titles(&self) -> Option<&Titles> {
        match self {
            Self::Fetched(fetched) | Self::Unavailable { kept: Some(fetched), .. } => Some(&fetched.titles),
            Self::NeverFetched | Self::Unavailable { kept: None, .. } => None,
        }
    }

    /// The PDF of the paper with this title, if the index has it.
    pub fn address_of(&self, title: &str) -> Option<&str> {
        self.titles()?.get(&title_key(title)).map(String::as_str)
    }
}

/// Why a PDF was not downloaded.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PdfError {
    #[error(transparent)]
    Get(#[from] GetError),
    #[error("{0} did not return a PDF (HTTP {1})")]
    Status(String, u16),
    #[error("{0} did not return a PDF")]
    NotAPdf(String),
}

/// A host papers are published on, over https, on its usual port.
pub fn publisher_host(address: &Url) -> bool {
    let host = address.host_str().unwrap_or_default();
    address.scheme() == "https"
        && address.port().is_none()
        && PDF_HOSTS.contains(&host.strip_prefix("www.").unwrap_or(host))
}

/// A PDF's bytes, from one of the hosts papers are published on, within [`MAX_PDF`] and [`PDF_TIME`].
pub fn download_pdf(fetch: &dyn Fetch, address: &str) -> Result<Vec<u8>, PdfError> {
    download_within(fetch, address, Limits { max_bytes: MAX_PDF, time: PDF_TIME })
}

/// Many PDFs downloaded at once, each as [`download_pdf`] does it, within each host's limits and pauses
/// ([`polite`]): one answer per address, in their order. Every PDF of the batch is held in memory until the
/// batch is done, so a caller hands addresses over a few dozen at a time (the text of a batch read with
/// [`crate::text::pdf::pdf_texts`] while the next downloads).
pub fn download_pdfs(
    fetch: &(dyn Fetch + Sync),
    addresses: &[String],
    politeness: &Politeness,
    progress: &Progress,
) -> Vec<Outcome<Result<Vec<u8>, PdfError>>> {
    polite::fetch_all(fetch, addresses, politeness, progress, download_pdf)
}

fn download_within(fetch: &dyn Fetch, address: &str, limits: Limits) -> Result<Vec<u8>, PdfError> {
    let (address, response) = get_following(fetch, address, limits, &publisher_host)?;
    if response.status != 200 {
        return Err(PdfError::Status(address, response.status));
    }
    if !response.body.starts_with(b"%PDF") {
        return Err(PdfError::NotAPdf(address));
    }
    Ok(response.body)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use chrono::TimeZone;

    use super::super::FetchError;
    use super::super::fake::{Fake, ok, paper, redirect};
    use super::*;

    fn html(body: &str) -> super::super::Response {
        ok("text/html", body)
    }

    fn titled(id: &str, title: &str) -> Paper {
        Paper { title: title.to_owned(), ..paper(id) }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 26, 12, 0, 0).unwrap()
    }

    /// The paper's PDF address, with its conference's index fetched through `fetch` as needed.
    fn pdf_url(fetch: &dyn Fetch, paper: &Paper) -> Option<String> {
        match pdf_source(paper) {
            PdfSource::Arxiv(address) => Some(address.to_owned()),
            PdfSource::Proceedings { publisher, year, .. } => IndexState::NeverFetched
                .refreshed(fetch, publisher, year, now())
                .address_of(&paper.title)
                .map(str::to_owned),
            PdfSource::Nowhere => None,
        }
    }

    #[test]
    fn a_title_is_compared_by_its_letters_and_digits_alone() {
        assert_eq!(title_key("A <i>Great</i> Paper"), "agreatpaper");
        assert_eq!(title_key("Don&#39;t Stop: Naïve Baselines"), title_key("Don't stop: Naive baselines"));
        assert_eq!(title_key("Caf&eacute; &amp; Cr&#xE8;me"), "cafecreme");
        assert_eq!(title_key("&unknown; GPT-4"), "unknowngpt4");
        assert_eq!(title_key("Stra&szlig;e &alpha;-Nets"), title_key("Straße α-Nets"));
    }

    #[test]
    fn pdf_addresses_come_from_where_each_conference_publishes() {
        let fake = Fake::new()
            .with(
                "https://proceedings.neurips.cc/paper_files/paper/2025",
                html(r#"<a href="/paper_files/paper/2025/vol38-main">main</a>"#),
            )
            .with(
                "https://proceedings.neurips.cc/paper_files/paper/2025/vol38-main",
                html(r#"<a href="/paper_files/paper/2025/hash/abc123-Abstract-Conference.html">A <i>Great</i> Paper</a>"#),
            )
            .with(
                "https://proceedings.mlr.press/",
                html(r#"<li><a href="v306"><b>Volume 306</b></a> Proceedings of ICML 2026</li>"#),
            )
            .with(
                "https://proceedings.mlr.press/v306/",
                html(
                    r#"<div class="paper"><p class="title">Another Paper</p><p class="links">[<a href="x.html">abs</a>][<a href="https://raw.githubusercontent.com/mlresearch/v306/main/assets/a26/a26.pdf">Download PDF</a>]</p></div>"#,
                ),
            )
            .with(
                "https://openaccess.thecvf.com/CVPR2026?day=all",
                html(
                    r#"<dt class="ptitle"><br><a href="/content/CVPR2026/html/Doe_Vision_CVPR_2026_paper.html">Vision Paper</a></dt>"#,
                ),
            );
        assert_eq!(
            pdf_url(&fake, &titled("neurips-2025-1", "A Great Paper")).as_deref(),
            Some("https://proceedings.neurips.cc/paper_files/paper/2025/file/abc123-Paper-Conference.pdf")
        );
        assert!(pdf_url(&fake, &titled("icml-2026-2", "Another  paper")).unwrap().ends_with("/a26/a26.pdf"));
        assert_eq!(
            pdf_url(&fake, &titled("cvpr-2026-3", "Vision Paper")).as_deref(),
            Some("https://openaccess.thecvf.com/content/CVPR2026/papers/Doe_Vision_CVPR_2026_paper.pdf")
        );
        assert_eq!(pdf_url(&fake, &titled("neurips-2025-9", "Not In The Proceedings")), None);
        let asked = fake.asked().len();
        assert_eq!(pdf_url(&fake, &titled("iclr-2026-4", "A Great Paper")), None); // nowhere to fetch from
        assert_eq!(fake.asked().len(), asked);
        let arxiv = Paper { pdf_url: Some("https://arxiv.org/pdf/2512.01234".to_owned()), ..paper("2512.01234") };
        assert_eq!(pdf_url(&fake, &arxiv).as_deref(), Some("https://arxiv.org/pdf/2512.01234"));
        // a conference paper's OpenReview link is not somewhere to fetch from
        let openreview = Paper { pdf_url: Some("https://openreview.net/pdf?id=x".to_owned()), ..paper("iclr-2026-5") };
        assert_eq!(pdf_source(&openreview), PdfSource::Nowhere);
    }

    #[test]
    fn a_paper_without_a_pdf_is_not_given_its_neighbours() {
        let page = concat!(
            r#"<div class="paper"><p class="title">First Paper</p><p class="links">[<a href="a.html">abs</a>]</p></div>"#,
            r#"<div class="paper"><p class="title">Don&#39;t Stop: Naïve Baselines</p><p class="links">[<a href="b26.pdf">Download PDF</a>]</p></div>"#,
            r#"<div class="paper"><p class="title">Same Title</p><p class="links">[<a href="c.pdf">PDF</a>]</p></div>"#,
            r#"<div class="paper"><p class="title">Same Title</p><p class="links">[<a href="d.pdf">PDF</a>]</p></div>"#,
        );
        let index = by_title(pmlr_papers(page, "https://proceedings.mlr.press/v306/"));
        assert!(!index.contains_key(&title_key("First Paper")));
        assert_eq!(index[&title_key("Don't stop: Naive baselines")], "https://proceedings.mlr.press/v306/b26.pdf");
        assert!(!index.contains_key(&title_key("Same Title"))); // two papers, one title: no guess
    }

    #[test]
    fn the_same_paper_listed_twice_under_one_address_keeps_its_place() {
        let address = "https://openaccess.thecvf.com/x.pdf".to_owned();
        let index = by_title([("Twice".to_owned(), address.clone()), ("Twice".to_owned(), address.clone())]);
        assert_eq!(index.get("twice"), Some(&address));
    }

    #[test]
    fn icml_before_its_volume_is_published_has_an_empty_index() {
        let fake = Fake::new().with("https://proceedings.mlr.press/", html("<li>Proceedings of ICML 2025</li>"));
        assert_eq!(fetch_titles(&fake, Publisher::Pmlr, "2026"), Ok(Titles::new()));
        let state = IndexState::NeverFetched.after(now(), Ok(Titles::new()));
        assert!(matches!(state, IndexState::Unavailable { kept: None, .. }), "{state:?}");
    }

    #[test]
    fn a_list_that_cannot_be_fetched_is_asked_for_once_and_the_paper_is_just_not_readable() {
        let fake = Fake::new().failing(
            "https://openaccess.thecvf.com/WACV2031?day=all",
            FetchError::Unreachable("https://openaccess.thecvf.com".to_owned(), "no network".to_owned()),
        );
        let mut state = IndexState::NeverFetched;
        for minutes in [0, 5, 30] {
            state = state.refreshed(&fake, Publisher::Cvf("WACV"), "2031", now() + TimeDelta::minutes(minutes));
            assert_eq!(state.address_of("Anything"), None);
        }
        assert_eq!(fake.asked().len(), 1);
        assert!(state.is_due(now() + RETRY));
    }

    #[test]
    fn an_index_that_was_fetched_is_kept_through_a_failure_and_fetched_again_a_month_later() {
        let titles: Titles = [("vision".to_owned(), "https://openaccess.thecvf.com/v.pdf".to_owned())].into();
        let fetched = IndexState::NeverFetched.after(now(), Ok(titles.clone()));
        assert!(!fetched.is_due(now() + TimeDelta::days(INDEX_DAYS - 1)));
        assert!(fetched.is_due(now() + TimeDelta::days(INDEX_DAYS)));
        let later = now() + TimeDelta::days(INDEX_DAYS);
        let failed = fetched.after(later, Err("HTTP 503".to_owned()));
        assert_eq!(failed.address_of("Vision"), Some("https://openaccess.thecvf.com/v.pdf"));
        let emptied = failed.after(later + RETRY, Ok(Titles::new()));
        assert_eq!(emptied.titles(), Some(&titles)); // an empty answer never replaces papers
        let round_trip: IndexState = serde_json::from_value(serde_json::to_value(&emptied).unwrap()).unwrap();
        assert_eq!(round_trip, emptied);
    }

    const PDF: &str = "%PDF-1.7 ...";
    const SMALL: Limits = Limits { max_bytes: 1024, time: Duration::from_secs(60) };

    #[test]
    fn only_a_pdf_of_sane_size_from_a_publisher_is_accepted() {
        let start = "https://arxiv.org/pdf/1";
        let pdf = || ok("application/pdf", PDF);
        let cases: Vec<(Fake, bool)> = vec![
            (Fake::new().with(start, pdf()), true),
            (Fake::new().with(start, redirect(302, "/pdf/1v2")).with("https://arxiv.org/pdf/1v2", pdf()), true),
            (
                Fake::new()
                    .with(start, redirect(301, "https://www.arxiv.org/pdf/1"))
                    .with("https://www.arxiv.org/pdf/1", pdf()),
                true,
            ),
            // every hop of a redirect must be a publisher's host, over https: checked before it is fetched
            (
                Fake::new()
                    .with(start, redirect(302, "https://evil.example/x.pdf"))
                    .with("https://evil.example/x.pdf", pdf()),
                false,
            ),
            (
                Fake::new()
                    .with(start, redirect(301, "http://127.0.0.1:8765/api/runs"))
                    .with("http://127.0.0.1:8765/api/runs", pdf()),
                false,
            ),
            (
                Fake::new().with(start, redirect(302, "http://arxiv.org/pdf/1")).with("http://arxiv.org/pdf/1", pdf()),
                false,
            ),
            (Fake::new().with(start, redirect(302, "https://arxiv.org:8443/pdf/1")), false),
            (Fake::new().with(start, html("<html>not a pdf</html>")), false),
            (Fake::new().with(start, super::super::Response { status: 404, ..pdf() }), false),
            (Fake::new().with(start, ok("application/pdf", &format!("%PDF{}", "0".repeat(4096)))), false), // too large
            (Fake::new().with(start, redirect(302, start)), false),                                        // a loop
            (Fake::new().failing(start, FetchError::TimedOut(start.to_owned())), false),                   // too slow
        ];
        for (number, (fake, fine)) in cases.into_iter().enumerate() {
            let downloaded = download_within(&fake, start, SMALL);
            if fine {
                assert_eq!(downloaded, Ok(PDF.as_bytes().to_vec()), "case {number}");
            } else {
                assert!(downloaded.is_err(), "case {number}");
            }
            let asked = fake.asked();
            assert!(
                !asked.iter().any(|a| a.contains("evil")
                    || a.contains("127.0.0.1")
                    || a.starts_with("http://")
                    || a.contains(":8443")),
                "case {number}: {asked:?}"
            );
        }
    }

    #[test]
    fn an_address_elsewhere_is_never_fetched() {
        let fake = Fake::new();
        for address in
            ["https://evil.example/a.pdf", "http://arxiv.org/pdf/1", "file:///C:/secret.pdf", "not an address"]
        {
            assert!(matches!(download_pdf(&fake, address), Err(PdfError::Get(GetError::NotAllowed(_)))), "{address}");
        }
        assert!(fake.asked().is_empty());
    }

    #[test]
    fn a_batch_of_pdfs_is_downloaded_with_each_answer_in_its_place_and_a_bad_one_its_own() {
        let pdf = || ok("application/pdf", PDF);
        let addresses: Vec<String> = [
            "https://proceedings.mlr.press/v1/a.pdf",
            "https://evil.example/b.pdf",
            "https://proceedings.mlr.press/v1/c.pdf",
            "https://openaccess.thecvf.com/d.pdf",
        ]
        .map(str::to_owned)
        .to_vec();
        let fake = Fake::new()
            .with(&addresses[0], pdf())
            .with(&addresses[1], pdf())
            .with(&addresses[2], html("<html>a login page</html>"))
            .with(&addresses[3], pdf());
        let quick = Politeness { arxiv: polite::Rule { at_once: 1, pause: Duration::ZERO }, ..Politeness::default() };
        let downloaded = download_pdfs(&fake, &addresses, &quick, &Progress::new());
        assert_eq!(downloaded[0], Outcome::Done(Ok(PDF.as_bytes().to_vec())));
        assert!(matches!(&downloaded[1], Outcome::Done(Err(PdfError::Get(GetError::NotAllowed(_))))));
        assert!(matches!(&downloaded[2], Outcome::Done(Err(PdfError::NotAPdf(_)))));
        assert_eq!(downloaded[3], Outcome::Done(Ok(PDF.as_bytes().to_vec())));
        assert!(!fake.asked().iter().any(|asked| asked.contains("evil")));
    }
}
