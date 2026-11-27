//! The arXiv API: every paper matching a search query, as library records (metadata and abstract, no PDF).
//!
//! arXiv asks programs for one request every three seconds (kept by the network for every request to arXiv:
//! [`super::polite::ARXIV_PACE`]), and a query is read in pages of [`PAGE`], oldest
//! first. The API sometimes answers with an error or with an empty page that is not the end, so a page is asked
//! for again, each time after a longer pause (or as long as a refusal's `Retry-After` asks, when that is longer),
//! before the query is given up on.

use std::sync::LazyLock;
use std::time::Duration;

use chrono::NaiveDate;
use regex::Regex;
use roxmltree::{Document, Node};

use super::{Fetch, Limits, Origin, Paper, any_https, get_following};

pub const API: &str = "https://export.arxiv.org/api/query";
/// Papers per page: the most arXiv recommends asking for at once.
pub const PAGE: usize = 500;
/// The pause before a page is asked for again, times the attempts so far: beyond arXiv's pace between requests.
pub const DELAY: Duration = Duration::from_millis(3100);
const ATTEMPTS: u32 = 6;
/// A refusal's `Retry-After` is waited for, up to this long: arXiv asks for minutes at most, and an absurd value is
/// not waited for for hours.
const LONGEST_ASKED: Duration = Duration::from_secs(5 * 60);
const LIMITS: Limits = Limits { max_bytes: 64 << 20, time: Duration::from_secs(90) };

const ATOM: &str = "http://www.w3.org/2005/Atom";
const ARXIV: &str = "http://arxiv.org/schemas/atom";
const OPENSEARCH: &str = "http://a9.com/-/spec/opensearch/1.1/";

#[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
static VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"v\d+$").unwrap());

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ArxivError {
    #[error("arXiv's answer could not be read: {0}")]
    Unreadable(String),
    #[error("the arXiv API kept failing at offset {offset} for the query {search} ({last})")]
    KeptFailing { offset: usize, search: String, last: String },
}

/// One page of an answer: how many papers match in all, and this page's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub total: usize,
    pub papers: Vec<Paper>,
}

/// Was this record downloaded from arXiv, and not from a conference's list? A conference record's "published" is
/// when the paper is presented, not when it was submitted, so the two are not compared by date.
pub fn from_arxiv(paper: &Paper) -> bool {
    paper.origin() == Origin::Arxiv
}

/// Papers submitted on one day, in any of these categories (all, when none are given). One query per day keeps
/// each one clear of the API's limit on deep paging.
pub fn submitted_on(day: NaiveDate, categories: &[String]) -> String {
    let stamp = day.format("%Y%m%d");
    let range = format!("submittedDate:[{stamp}0000 TO {stamp}2359]");
    if categories.is_empty() {
        return range;
    }
    let any: Vec<String> = categories.iter().map(|category| format!("cat:{category}")).collect();
    format!("{range} AND ({})", any.join(" OR "))
}

/// Papers whose comment mentions a conference ("NeurIPS 2025"); arXiv's matching is loose, so what comes back
/// is checked again ([`super::conferences::names_conference`]).
pub fn comment_mentions(venue: &str) -> String {
    format!("co:\"{venue}\"")
}

/// Form-encoded as a browser would: letters, digits and `-_.~` as they are, a space as `+`, the rest as `%XX`.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => char::from(byte).to_string(),
            b' ' => "+".to_owned(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The address of one page of a query, oldest submissions first.
pub fn page_url(search: &str, start: usize) -> String {
    format!(
        "{API}?search_query={}&start={start}&max_results={PAGE}&sortBy=submittedDate&sortOrder=ascending",
        encode(search)
    )
}

/// A child element's text with its white space made single spaces, or "" when it is missing.
fn text_of(node: Node, namespace: &str, name: &str) -> String {
    node.children().find(|child| child.has_tag_name((namespace, name))).map(collapsed).unwrap_or_default()
}

fn collapsed(node: Node) -> String {
    let text: String = node.descendants().filter(Node::is_text).filter_map(|text| text.text()).collect();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One Atom answer read into papers. Entries that are not papers (arXiv reports errors as an entry whose id is
/// not an /abs/ address) are left out.
pub fn parse(xml: &str) -> Result<Page, ArxivError> {
    let document = Document::parse(xml).map_err(|error| ArxivError::Unreadable(error.to_string()))?;
    let feed = document.root_element();
    // a count that is there but is not a number is a broken answer, not an empty one
    let total = match text_of(feed, OPENSEARCH, "totalResults").as_str() {
        "" => 0,
        count => count.parse().map_err(|_| ArxivError::Unreadable(format!("a count of {count:?} results")))?,
    };
    let papers = feed
        .children()
        .filter(|child| child.has_tag_name((ATOM, "entry")))
        .filter_map(|entry| {
            let address = text_of(entry, ATOM, "id");
            let (_, id) = address.split_once("/abs/")?;
            let id = VERSION.replace(id, "").into_owned();
            let terms = |namespace: &str, name: &str| -> Vec<String> {
                entry
                    .children()
                    .filter(|child| child.has_tag_name((namespace, name)))
                    .filter_map(|child| child.attribute("term"))
                    .map(str::to_owned)
                    .collect()
            };
            Some(Paper {
                title: text_of(entry, ATOM, "title"),
                abstract_text: text_of(entry, ATOM, "summary"),
                authors: entry
                    .children()
                    .filter(|child| child.has_tag_name((ATOM, "author")))
                    .map(|author| text_of(author, ATOM, "name"))
                    .collect(),
                published: text_of(entry, ATOM, "published"),
                updated: text_of(entry, ATOM, "updated"),
                comment: text_of(entry, ARXIV, "comment"),
                primary_category: terms(ARXIV, "primary_category").into_iter().next().unwrap_or_default(),
                categories: terms(ATOM, "category"),
                url: format!("https://arxiv.org/abs/{id}"),
                pdf_url: Some(format!("https://arxiv.org/pdf/{id}")),
                id,
            })
        })
        .collect();
    Ok(Page { total, papers })
}

/// One page, asked for again with a growing pause while the API errs or answers an empty page before the end.
fn page(
    fetch: &dyn Fetch,
    pause: &dyn Fn(Duration),
    search: &str,
    start: usize,
    log: &dyn Fn(&str),
) -> Result<Page, ArxivError> {
    let mut last = String::new();
    // how long the last refusal asked to be left alone (`Retry-After`), when it said
    let mut asked: Option<Duration> = None;
    for attempt in 1..=ATTEMPTS {
        // the pace between requests is arXiv's one pace, which the network keeps; this is the patience after a
        // failure, growing with each, and at least what a refusal asked for
        if attempt > 1 {
            pause((DELAY * (attempt - 1)).max(asked.take().unwrap_or_default().min(LONGEST_ASKED)));
        }
        let answer = match get_following(fetch, &page_url(search, start), LIMITS, &any_https) {
            Ok((_, response)) if response.status == 200 => parse(&response.text()).map_err(|error| error.to_string()),
            Ok((_, response)) => {
                if matches!(response.status, 429 | 503) {
                    asked = response.retry_after;
                }
                Err(format!("HTTP {}", response.status))
            }
            Err(error) => Err(error.to_string()),
        };
        match answer {
            Ok(page) if !page.papers.is_empty() || start >= page.total => return Ok(page),
            Ok(_) => last = "empty page".to_owned(),
            Err(error) => last = error,
        }
        log(&format!("    retry {attempt} at offset {start}: {last}"));
    }
    Err(ArxivError::KeptFailing { offset: start, search: search.to_owned(), last })
}

/// Every paper matching an arXiv search query, oldest first. `pause` waits (a real sleep, outside the tests);
/// it is called before a page is asked for again. The requests themselves keep arXiv's one pace, which the
/// network takes for every request to arXiv ([`super::polite::ARXIV_PACE`]).
pub fn query(
    fetch: &dyn Fetch,
    pause: &dyn Fn(Duration),
    search: &str,
    log: &dyn Fn(&str),
) -> Result<Vec<Paper>, ArxivError> {
    let mut papers = Vec::new();
    let mut start = 0;
    loop {
        let page = page(fetch, pause, search, start, log)?;
        papers.extend(page.papers);
        start += PAGE;
        log(&format!("    {}/{}", start.min(page.total), page.total));
        if start >= page.total {
            return Ok(papers);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use std::cell::RefCell;

    use super::super::fake::{Fake, ok, paper, status};
    use super::*;

    /// An Atom answer with these entries, each `(id address, title, comment)`.
    pub(crate) fn feed(total: usize, entries: &[(&str, &str, &str)]) -> String {
        let entries: String = entries
            .iter()
            .map(|(id, title, comment)| {
                format!(
                    r#"<entry><id>{id}</id><updated>2026-01-06T10:00:00Z</updated>
                    <published>2026-01-05T09:00:00Z</published><title>{title}</title>
                    <summary>  An abstract
                      over two lines.  </summary>
                    <author><name>Ada One</name></author><author><name> Bo  Two </name></author>
                    <arxiv:comment>{comment}</arxiv:comment>
                    <arxiv:primary_category term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
                    <category term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
                    <category term="stat.ML" scheme="http://arxiv.org/schemas/atom"/></entry>"#
                )
            })
            .collect();
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
            <feed xmlns="http://www.w3.org/2005/Atom" xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/"
                  xmlns:arxiv="http://arxiv.org/schemas/atom">
              <title>ArXiv Query</title><opensearch:totalResults>{total}</opensearch:totalResults>{entries}</feed>"#
        )
    }

    fn quiet(_: &str) {}

    #[test]
    fn an_atom_answer_is_read_into_papers() {
        let xml = feed(
            2,
            &[
                ("http://arxiv.org/abs/2601.00001v2", "A Title\n   That Wraps", "Accepted at ICML 2026"),
                ("http://arxiv.org/api/errors#incorrect_id_format", "Error", ""),
            ],
        );
        let page = parse(&xml).unwrap();
        assert_eq!(page.total, 2);
        let [found] = page.papers.as_slice() else { panic!("{:?}", page.papers) };
        assert_eq!(
            *found,
            Paper {
                id: "2601.00001".to_owned(),
                title: "A Title That Wraps".to_owned(),
                abstract_text: "An abstract over two lines.".to_owned(),
                authors: vec!["Ada One".to_owned(), "Bo Two".to_owned()],
                published: "2026-01-05T09:00:00Z".to_owned(),
                updated: "2026-01-06T10:00:00Z".to_owned(),
                comment: "Accepted at ICML 2026".to_owned(),
                primary_category: "cs.LG".to_owned(),
                categories: vec!["cs.LG".to_owned(), "stat.ML".to_owned()],
                url: "https://arxiv.org/abs/2601.00001".to_owned(),
                pdf_url: Some("https://arxiv.org/pdf/2601.00001".to_owned()),
            }
        );
        assert!(from_arxiv(found));
        assert!(!from_arxiv(&paper("icml-2026-1")));
    }

    #[test]
    fn an_answer_that_is_not_xml_is_unreadable() {
        assert!(matches!(parse("<html><body>Rate exceeded"), Err(ArxivError::Unreadable(_))));
    }

    #[test]
    fn a_day_is_one_query_restricted_to_its_categories() {
        let day = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        assert_eq!(submitted_on(day, &[]), "submittedDate:[202601050000 TO 202601052359]");
        let categories = ["cs.LG".to_owned(), "stat.ML".to_owned()];
        assert_eq!(
            submitted_on(day, &categories),
            "submittedDate:[202601050000 TO 202601052359] AND (cat:cs.LG OR cat:stat.ML)"
        );
        assert_eq!(
            page_url(&submitted_on(day, &categories[..1]), 500),
            "https://export.arxiv.org/api/query?search_query=submittedDate%3A%5B202601050000+TO+202601052359%5D+AND+\
             %28cat%3Acs.LG%29&start=500&max_results=500&sortBy=submittedDate&sortOrder=ascending"
        );
        assert_eq!(comment_mentions("CVPR 2026"), "co:\"CVPR 2026\"");
    }

    #[test]
    fn every_page_is_read_and_every_request_waits_its_turn() {
        let ids: Vec<String> = (0..PAGE + 1).map(|n| format!("http://arxiv.org/abs/2601.{n:05}v1")).collect();
        let entries: Vec<(&str, &str, &str)> = ids.iter().map(|id| (id.as_str(), "T", "")).collect();
        let fake = Fake::new()
            .with(&page_url("q", 0), ok("application/atom+xml", &feed(PAGE + 1, &entries[..PAGE])))
            .with(&page_url("q", PAGE), ok("application/atom+xml", &feed(PAGE + 1, &entries[PAGE..])));
        let pauses = RefCell::new(Vec::new());
        let papers = query(&fake, &|pause| pauses.borrow_mut().push(pause), "q", &quiet).unwrap();
        assert_eq!(papers.len(), PAGE + 1);
        assert_eq!(papers[PAGE].id, format!("2601.{PAGE:05}"));
        assert!(pauses.borrow().is_empty(), "the pace is the network's: no pause of its own between pages");
    }

    #[test]
    fn a_failing_page_is_asked_again_after_longer_pauses_and_then_given_up_on() {
        let fake = Fake::new().with(&page_url("q", 0), status(503));
        let pauses = RefCell::new(Vec::new());
        let lines = RefCell::new(Vec::new());
        let failed = query(&fake, &|pause| pauses.borrow_mut().push(pause), "q", &|line| {
            lines.borrow_mut().push(line.to_owned());
        });
        assert_eq!(
            failed,
            Err(ArxivError::KeptFailing { offset: 0, search: "q".to_owned(), last: "HTTP 503".to_owned() })
        );
        assert_eq!(*pauses.borrow(), (1..ATTEMPTS).map(|attempt| DELAY * attempt).collect::<Vec<_>>());
        assert_eq!(lines.borrow()[0], "    retry 1 at offset 0: HTTP 503");
    }

    #[test]
    fn a_refusal_is_waited_out_as_long_as_its_retry_after_asks_before_the_page_is_asked_again() {
        let refused = super::super::Response { retry_after: Some(Duration::from_secs(40)), ..status(429) };
        let absurd = super::super::Response { retry_after: Some(Duration::from_secs(86_400)), ..status(503) };
        let url = page_url("q", 0);
        let fake = Fake::new().first(&url, refused).first(&url, absurd).with(&url, status(500));
        let pauses = RefCell::new(Vec::new());
        let _ = query(&fake, &|pause| pauses.borrow_mut().push(pause), "q", &quiet);
        let pauses = pauses.borrow();
        // no pause before the first ask (arXiv's one pace, kept by the network, spaces it); before each retry, the
        // patience of that attempt or what the refusal asked for, whichever is longer
        assert_eq!(pauses[..4], [Duration::from_secs(40), LONGEST_ASKED, DELAY * 3, DELAY * 4], "{pauses:?}");
    }

    #[test]
    fn an_empty_page_before_the_end_is_a_failure_but_an_empty_answer_is_not() {
        let fake = Fake::new().with(&page_url("q", 0), ok("application/atom+xml", &feed(3, &[])));
        let failed = query(&fake, &|_| {}, "q", &quiet);
        assert!(matches!(failed, Err(ArxivError::KeptFailing { ref last, .. }) if last == "empty page"), "{failed:?}");
        let nothing = Fake::new().with(&page_url("q", 0), ok("application/atom+xml", &feed(0, &[])));
        assert_eq!(query(&nothing, &|_| {}, "q", &quiet), Ok(Vec::new()));
    }
}
