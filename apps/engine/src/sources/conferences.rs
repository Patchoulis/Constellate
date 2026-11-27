//! Accepted papers of a conference, from the conference's own site.
//!
//! NeurIPS, ICML, ICLR, CVPR and others ([`super::catalog`] lists them) publish the full list of accepted papers
//! as data files on their virtual-conference sites:
//!
//! ```text
//!     https://<host>/static/virtual/data/<slug>-<year>-orals-posters.json   every poster and oral
//!     https://<host>/static/virtual/data/<slug>-<year>-abstracts.json       {event id: abstract}, newer years only
//!                                                                           (older years carry the abstract inline)
//! ```
//!
//! That is the complete, official list, unlike matching "Accepted at ..." in arXiv comments, which only finds the
//! papers that are on arXiv and say so; for a conference without a list, that is what there is.
//!
//! Asking for a list has three outcomes, and they are kept apart: the list; this conference publishes none (the
//! site answers 404, and only then); or it could not be had now (a 503, a 429, no connection, an answer that is
//! not the list). Only the second may fall back to arXiv comments and be remembered as done. Treating the third
//! like the second would store a partial sample as if it were the official list, and never fetch the list again.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use regex::{Regex, RegexBuilder};
use serde_json::{Map, Value};

use super::arxiv::{self, ArxivError};
use super::catalog::official_host;
use super::{Fetch, Limits, Paper, any_https, get_following};

/// A large conference's list is tens of megabytes, and these sites can be slow.
const LIMITS: Limits = Limits { max_bytes: 256 << 20, time: Duration::from_secs(300) };

#[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
static FORUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[?&]id=([\w-]+)").unwrap());

/// The two data files a conference publishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    OralsPosters,
    Abstracts,
}

pub fn list_url(host: &str, slug: &str, year: &str, part: Part) -> String {
    let part = match part {
        Part::OralsPosters => "orals-posters",
        Part::Abstracts => "abstracts",
    };
    format!("https://{host}/static/virtual/data/{slug}-{year}-{part}.json")
}

/// What asking for one data file came to.
#[derive(Clone, Debug, PartialEq)]
enum Asked {
    Got(Value),
    /// The site says there is no such file (HTTP 404).
    NotPublished,
    /// Anything else: worth asking again later.
    NotNow(String),
}

fn ask(fetch: &dyn Fetch, url: &str) -> Asked {
    match get_following(fetch, url, LIMITS, &any_https) {
        Ok((_, response)) if response.status == 200 => {
            let json = response.content_type.as_deref().is_some_and(|kind| kind.contains("json"));
            match json.then(|| serde_json::from_slice(&response.body)) {
                Some(Ok(value)) => Asked::Got(value),
                _ => Asked::NotNow(format!("{url} answered with something that is not the list")),
            }
        }
        Ok((_, response)) if response.status == 404 => Asked::NotPublished,
        Ok((_, response)) => Asked::NotNow(format!("{url} answered HTTP {}", response.status)),
        Err(error) => Asked::NotNow(error.to_string()),
    }
}

/// Whether the papers' abstracts came with them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Abstracts {
    /// The abstracts file was read.
    Fetched,
    /// There is no abstracts file: older years carry each abstract in the list itself.
    NotPublished,
    /// The file could not be had now, so papers that do not carry their abstract have none yet.
    MissingForNow(String),
}

/// What asking for a conference's official list came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    /// Every accepted paper, once each.
    List { papers: Vec<Paper>, abstracts: Abstracts },
    /// This conference (or this year of it) publishes no list where one is looked for.
    NoList(String),
    /// The list may exist but could not be fetched now: ask again later, and do not fall back.
    NotNow(String),
}

/// The OpenReview forum id, which is the same for a paper's poster and its oral.
fn forum(entry: &Value) -> Option<String> {
    let link = entry.get("paper_url").and_then(Value::as_str)?;
    FORUM.captures(link).and_then(|found| found.get(1)).map(|id| id.as_str().to_owned())
}

fn collapsed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A field as text, when it is text and not empty.
fn field<'a>(entry: &'a Value, name: &str) -> Option<&'a str> {
    entry.get(name).and_then(Value::as_str).filter(|text| !text.is_empty())
}

/// An entry's event id, written as a number or as text depending on the site.
fn event_id(entry: &Value) -> Option<String> {
    match entry.get("id")? {
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        _ => None,
    }
}

/// A paper's decision as the library keeps it: the list's decision, then the kind of each of its other events
/// ("Oral", "Spotlight") that the decision does not already name, best first. Some lists name a paper's oral only
/// by its talk (a decision of "Accept Poster", or none, and an "Oral" event), and acceptance.rs reads the two
/// together per conference and year.
pub fn decision_with_events(decision: &str, events: &[String]) -> String {
    let mut events: Vec<&String> = events.iter().collect();
    events.sort_by_key(|kind| (["Oral", "Spotlight"].iter().position(|best| best == kind).unwrap_or(2), kind.as_str()));
    let mut parts: Vec<&str> = if decision.is_empty() { Vec::new() } else { vec![decision] };
    for kind in events {
        if !decision.to_lowercase().contains(&kind.to_lowercase()) && !parts.contains(&kind.as_str()) {
            parts.push(kind);
        }
    }
    parts.join("; ")
}

/// A list's entries as papers. A paper with an oral is listed again for the talk, often without its paper link
/// (or with a made-up one), so posters are read first and two entries are the same paper when they share the
/// link OR the title; the kind of the other entries is kept with the decision ([`decision_with_events`]).
pub fn read_entries(
    entries: &[Value],
    abstracts: &Map<String, Value>,
    host: &str,
    slug: &str,
    year: &str,
) -> Vec<Paper> {
    let mut entries: Vec<&Value> = entries.iter().collect();
    entries.sort_by_key(|entry| field(entry, "eventtype") != Some("Poster"));
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut papers = Vec::new();
    // for each paper, its events other than its poster
    let mut events: Vec<Vec<String>> = Vec::new();
    for entry in entries {
        let (Some(id), Some(name)) = (event_id(entry), field(entry, "name")) else { continue };
        let forum = forum(entry);
        let title_key: String =
            name.chars().flat_map(char::to_lowercase).filter(|c| c.is_alphanumeric() || *c == '_').collect();
        let keys: Vec<String> = std::iter::once(title_key).chain(forum.clone()).collect();
        let kind = field(entry, "eventtype").filter(|kind| *kind != "Poster").map(str::to_owned);
        if let Some(index) = keys.iter().find_map(|key| seen.get(key).copied()) {
            events[index].extend(kind);
            continue;
        }
        for key in keys {
            seen.insert(key, papers.len());
        }
        events.push(kind.into_iter().collect());
        let topic = field(entry, "topic").unwrap_or_default();
        let when = field(entry, "starttime").map_or_else(|| format!("{year}-01-01"), str::to_owned);
        let abstract_text =
            field(entry, "abstract").or_else(|| abstracts.get(&id).and_then(Value::as_str)).unwrap_or_default();
        papers.push(Paper {
            id: format!("{slug}-{year}-{id}"),
            title: collapsed(name),
            abstract_text: collapsed(abstract_text),
            authors: entry
                .get("authors")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|author| author.get("fullname").and_then(Value::as_str))
                .map(str::to_owned)
                .collect(),
            published: when.clone(),
            updated: when,
            comment: field(entry, "decision").unwrap_or_default().to_owned(),
            primary_category: topic.to_owned(),
            categories: if topic.is_empty() { Vec::new() } else { vec![topic.to_owned()] },
            // the list is the conference site's to write, and the address is opened from the window: only a web
            // address on it, never `javascript:` or one that hides its host behind a user name
            url: field(entry, "paper_url").and_then(super::web_address).unwrap_or_else(|| {
                let page = field(entry, "virtualsite_url").unwrap_or_default();
                let page = if page.starts_with('/') && !page.starts_with("//") { page } else { "" };
                super::web_address(&format!("https://{host}{page}")).unwrap_or_default()
            }),
            // a link for the reader to show; nothing here fetches from OpenReview
            pdf_url: forum.map(|forum| format!("https://openreview.net/pdf?id={forum}")),
        });
    }
    for (paper, events) in papers.iter_mut().zip(&events) {
        paper.comment = decision_with_events(&paper.comment, events);
    }
    papers
}

/// Every accepted paper of a conference, e.g. "NeurIPS 2025", from its official list.
pub fn accepted_papers(fetch: &dyn Fetch, venue: &str, log: &dyn Fn(&str)) -> Listing {
    let Some((name, year)) = venue.rsplit_once(' ') else {
        return Listing::NoList(format!("{venue} is not a conference and a year"));
    };
    let Some(host) = official_host(name) else {
        return Listing::NoList(format!("no official list is known for {name}"));
    };
    let slug = name.to_lowercase();
    let list = list_url(host, &slug, year, Part::OralsPosters);
    let entries = match ask(fetch, &list) {
        Asked::Got(Value::Object(mut value)) if matches!(value.get("results"), Some(Value::Array(_))) => {
            match value.remove("results") {
                Some(Value::Array(entries)) => entries,
                _ => Vec::new(),
            }
        }
        Asked::Got(_) => return Listing::NotNow(format!("{list} is not a list of papers")),
        Asked::NotPublished => return Listing::NoList(format!("{list} answered HTTP 404")),
        Asked::NotNow(why) => return Listing::NotNow(why),
    };
    let (texts, abstracts) = match ask(fetch, &list_url(host, &slug, year, Part::Abstracts)) {
        Asked::Got(Value::Object(texts)) => (texts, Abstracts::Fetched),
        Asked::Got(_) => (Map::new(), Abstracts::MissingForNow("the abstracts file is not a table".to_owned())),
        Asked::NotPublished => (Map::new(), Abstracts::NotPublished),
        Asked::NotNow(why) => (Map::new(), Abstracts::MissingForNow(why)),
    };
    let papers = read_entries(&entries, &texts, host, &slug, year);
    let without = papers.iter().filter(|paper| paper.abstract_text.is_empty()).count();
    log(&format!("    {} accepted papers on {host} ({without} without an abstract)", papers.len()));
    Listing::List { papers, abstracts }
}

/// Does an arXiv comment say the paper is at this conference ("Accepted at CVPR 2026", "CVPR'26" does not
/// count: the year is written out)? A workshop of the conference counts only when asked for.
pub fn names_conference(comment: &str, venue: &str, include_workshops: bool) -> bool {
    let Some((name, year)) = venue.rsplit_once(' ') else { return false };
    let pattern = format!(r"{}\W*{}", regex::escape(name), regex::escape(year));
    let Ok(names) = RegexBuilder::new(&pattern).case_insensitive(true).build() else { return false };
    names.is_match(comment) && (include_workshops || !comment.to_lowercase().contains("workshop"))
}

/// For a conference without a list: arXiv papers whose comment names it. A partial, self-selected sample.
pub fn from_arxiv_comments(
    fetch: &dyn Fetch,
    pause: &dyn Fn(Duration),
    venue: &str,
    include_workshops: bool,
    log: &dyn Fn(&str),
) -> Result<Vec<Paper>, ArxivError> {
    let papers = arxiv::query(fetch, pause, &arxiv::comment_mentions(venue), log)?;
    Ok(papers.into_iter().filter(|paper| names_conference(&paper.comment, venue, include_workshops)).collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;

    use super::super::FetchError;
    use super::super::fake::{Fake, ok, status};
    use super::*;

    const LIST: &str = "https://neurips.cc/static/virtual/data/neurips-2025-orals-posters.json";
    const ABSTRACTS: &str = "https://neurips.cc/static/virtual/data/neurips-2025-abstracts.json";

    fn quiet(_: &str) {}

    fn list() -> String {
        json!({"count": 4, "results": [
            {"id": 7, "name": "Fast  Things", "eventtype": "Oral", "decision": "Accept (oral)",
             "paper_url": "https://openreview.net/forum?id=abc-1"},
            {"id": 3, "name": "Fast Things", "eventtype": "Poster", "decision": "Accept (oral)", "topic": "Optimization",
             "authors": [{"fullname": "Ada One"}, {"fullname": "Bo Two"}], "starttime": "2025-12-03T11:00:00",
             "paper_url": "https://openreview.net/forum?id=abc-1"},
            {"id": 4, "name": "Slow things!", "eventtype": "Oral", "decision": "Accept (spotlight)",
             "virtualsite_url": "/virtual/2025/poster/4"},
            {"id": 5, "name": "Slow Things", "eventtype": "Poster", "abstract": "Inline\n abstract.",
             "paper_url": "https://openreview.net/forum?id=zzz"},
            {"id": 6, "name": "A Third Paper", "eventtype": "Poster", "paper_url": "https://openreview.net/forum?id=abc-1"},
        ]})
        .to_string()
    }

    #[test]
    fn the_official_list_is_read_with_its_abstracts_and_each_paper_once() {
        let fake = Fake::new()
            .with(LIST, ok("application/json", &list()))
            .with(ABSTRACTS, ok("application/json; charset=utf-8", r#"{"3": "From  the file.", "7": "Talk."}"#));
        let Listing::List { papers, abstracts } = accepted_papers(&fake, "NeurIPS 2025", &quiet) else { panic!() };
        assert_eq!(abstracts, Abstracts::Fetched);
        // the oral of "Fast Things" shares its link, "Slow things!" shares a title, "A Third Paper" a link
        let ids: Vec<&str> = papers.iter().map(|paper| paper.id.as_str()).collect();
        assert_eq!(ids, ["neurips-2025-3", "neurips-2025-5"]);
        let fast = &papers[0];
        assert_eq!(fast.title, "Fast Things");
        assert_eq!(fast.abstract_text, "From the file.");
        assert_eq!(fast.authors, ["Ada One", "Bo Two"]);
        assert_eq!((fast.published.as_str(), fast.comment.as_str()), ("2025-12-03T11:00:00", "Accept (oral)"));
        assert_eq!(
            (fast.primary_category.as_str(), fast.categories.as_slice()),
            ("Optimization", &["Optimization".to_owned()][..])
        );
        assert_eq!(fast.pdf_url.as_deref(), Some("https://openreview.net/pdf?id=abc-1"));
        let slow = &papers[1];
        assert_eq!((slow.abstract_text.as_str(), slow.published.as_str()), ("Inline abstract.", "2025-01-01"));
        assert!(slow.categories.is_empty());
        // nothing was asked of OpenReview: its addresses are only written down
        assert!(fake.asked().iter().all(|address| !address.contains("openreview")));
    }

    #[test]
    fn a_papers_talk_is_kept_with_its_decision_when_the_decision_does_not_name_it() {
        let Listing::List { papers, .. } =
            accepted_papers(&Fake::new().with(LIST, ok("application/json", &list())), "NeurIPS 2025", &quiet)
        else {
            panic!()
        };
        // "Fast Things" says oral already; "Slow Things" has no decision, only its oral talk
        assert_eq!(papers[0].comment, "Accept (oral)");
        assert_eq!(papers[1].comment, "Oral");
        // as WACV 2026 and ICML 2026 list their orals
        let entries = [
            json!({"id": 1, "name": "A", "eventtype": "Poster", "decision": "Accept Poster"}),
            json!({"id": 2, "name": "A", "eventtype": "Oral", "decision": "Accept Poster"}),
            json!({"id": 3, "name": "B", "eventtype": "Spotlight"}),
            json!({"id": 4, "name": "B", "eventtype": "Oral"}),
            json!({"id": 5, "name": "C", "eventtype": "Poster", "decision": "Accept (spotlight)"}),
            json!({"id": 6, "name": "C", "eventtype": "Spotlight", "decision": "Accept (spotlight)"}),
        ];
        let comments: Vec<String> = read_entries(&entries, &Map::new(), "wacv.thecvf.com", "wacv", "2026")
            .into_iter()
            .map(|p| p.comment)
            .collect();
        assert_eq!(comments, ["Accept Poster; Oral", "Accept (spotlight)", "Oral; Spotlight"]);
    }

    #[test]
    fn a_paper_without_a_link_gets_its_page_on_the_conference_site() {
        let entries = [json!({"id": "x9", "name": "Solo", "virtualsite_url": "/virtual/2025/poster/9"})];
        let papers = read_entries(&entries, &Map::new(), "neurips.cc", "neurips", "2025");
        assert_eq!(papers[0].url, "https://neurips.cc/virtual/2025/poster/9");
        assert_eq!((papers[0].id.as_str(), papers[0].pdf_url.clone()), ("neurips-2025-x9", None));
    }

    #[test]
    fn a_link_the_list_gives_is_kept_only_when_it_is_a_web_address_that_names_its_host() {
        let entries = [
            json!({"id": 1, "name": "A", "paper_url": "javascript:alert(document.domain)//openreview.net",
                   "virtualsite_url": "/virtual/2025/poster/1"}),
            json!({"id": 2, "name": "B", "paper_url": "https://openreview.net@evil.example/forum"}),
            json!({"id": 3, "name": "C", "paper_url": "data:text/html,<script>alert(1)</script>"}),
            json!({"id": 4, "name": "D", "virtualsite_url": "@evil.example/x"}),
            json!({"id": 5, "name": "E", "virtualsite_url": "//evil.example/x"}),
            json!({"id": 6, "name": "F", "paper_url": "https://openreview.net/forum?id=abc"}),
        ];
        let papers = read_entries(&entries, &Map::new(), "neurips.cc", "neurips", "2025");
        let urls: Vec<&str> = papers.iter().map(|paper| paper.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://neurips.cc/virtual/2025/poster/1",
                "https://neurips.cc/",
                "https://neurips.cc/",
                "https://neurips.cc/",
                "https://neurips.cc/",
                "https://openreview.net/forum?id=abc",
            ]
        );
    }

    #[test]
    fn a_missing_abstracts_file_is_told_apart_from_one_that_could_not_be_had() {
        let older = Fake::new().with(LIST, ok("application/json", &list()));
        let Listing::List { abstracts, .. } = accepted_papers(&older, "NeurIPS 2025", &quiet) else { panic!() };
        assert_eq!(abstracts, Abstracts::NotPublished);

        let busy = Fake::new().with(LIST, ok("application/json", &list())).with(ABSTRACTS, status(503));
        let Listing::List { abstracts, .. } = accepted_papers(&busy, "NeurIPS 2025", &quiet) else { panic!() };
        assert_eq!(abstracts, Abstracts::MissingForNow(format!("{ABSTRACTS} answered HTTP 503")));
    }

    #[test]
    fn only_a_404_means_the_conference_publishes_no_list() {
        let none = Fake::new();
        assert_eq!(
            accepted_papers(&none, "NeurIPS 2025", &quiet),
            Listing::NoList(format!("{LIST} answered HTTP 404"))
        );

        for (busy, why) in [
            (Fake::new().with(LIST, status(503)), format!("{LIST} answered HTTP 503")),
            (Fake::new().with(LIST, status(429)), format!("{LIST} answered HTTP 429")),
            (
                Fake::new().with(LIST, ok("text/html", "<html>")),
                format!("{LIST} answered with something that is not the list"),
            ),
            (Fake::new().with(LIST, ok("application/json", "{}")), format!("{LIST} is not a list of papers")),
            (
                Fake::new().failing(LIST, FetchError::TimedOut(LIST.to_owned())),
                format!("{LIST} took too long to answer"),
            ),
        ] {
            assert_eq!(accepted_papers(&busy, "NeurIPS 2025", &quiet), Listing::NotNow(why));
        }
    }

    #[test]
    fn a_conference_without_a_known_list_is_not_asked_for_one() {
        let fake = Fake::new();
        assert_eq!(
            accepted_papers(&fake, "ACL 2025", &quiet),
            Listing::NoList("no official list is known for ACL".to_owned())
        );
        assert!(fake.asked().is_empty());
    }

    #[test]
    fn an_arxiv_comment_names_a_conference_only_with_its_year() {
        assert!(names_conference("Accepted at CVPR 2026 (highlight)", "CVPR 2026", false));
        assert!(names_conference("to appear in cvpr-2026", "CVPR 2026", false));
        assert!(!names_conference("Accepted at CVPR 2025", "CVPR 2026", false));
        assert!(!names_conference("CVPR'26", "CVPR 2026", false));
        assert!(!names_conference("CVPR 2026 Workshop on Things", "CVPR 2026", false));
        assert!(names_conference("CVPR 2026 Workshop on Things", "CVPR 2026", true));
        assert!(names_conference("Quark Matter 2025 proceedings", "Quark Matter 2025", false));
    }

    #[test]
    fn without_a_list_the_papers_come_from_arxiv_comments_that_name_the_conference() {
        let feed = super::super::arxiv::tests::feed(
            3,
            &[
                ("http://arxiv.org/abs/2601.00001v1", "Kept", "Accepted at ACL 2025"),
                ("http://arxiv.org/abs/2601.00002v1", "Workshop", "ACL 2025 workshop"),
                ("http://arxiv.org/abs/2601.00003v1", "Other year", "ACL 2024; extended for 2025"),
            ],
        );
        let address = arxiv::page_url(&arxiv::comment_mentions("ACL 2025"), 0);
        let fake = Fake::new().with(&address, ok("application/atom+xml", &feed));
        let papers = from_arxiv_comments(&fake, &|_| {}, "ACL 2025", false, &quiet).unwrap();
        assert_eq!(papers.iter().map(|paper| paper.title.as_str()).collect::<Vec<_>>(), ["Kept"]);
    }
}
