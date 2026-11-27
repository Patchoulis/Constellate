//! Finding a cited work on arXiv when it is not in the user's library: optional enrichment of a citation network
//! ([`crate::analysis::citations`]), which works without it.
//!
//! arXiv is the one outside service used, because the app already gathers papers from it and arXiv allows
//! programs to query its API, asking for one request every three seconds. A reference with an arXiv id but no
//! title read is looked up by its id (`id_list=`), for its title; one with a title and no id by a title search
//! (`search_query=ti:"..."`), whose answers are taken only when the title is the same ([`same_title`]): the
//! same letters and digits, or nearly the same words from a year at most one apart. Two different papers that
//! both pass are ambiguous and neither is taken.
//!
//! What to ask and how to read the answer are plain functions; the asking goes through a function the caller
//! passes in ([`Get`]), so every test runs on recorded answers and no test touches the network.
//!
//! The pace is not kept here: every request to arXiv the engine makes waits for its turn at arXiv's one pace
//! ([`super::polite::ARXIV_PACE`], taken by the network), first come first served with the PDFs and the listings,
//! and left alone after a refusal for as long as arXiv asks. A refusal ends the lookups at once ([`send`]): the
//! entries not asked about are asked about next time, and the error says when to try again.
//!
//! The address asked is the one its caller passes ([`enrich`]'s `base`): arXiv's own API, [`super::arxiv::API`].

use std::time::Duration;

use crate::analysis::citations::{Link, LinkedReference, Linker, same_title};
use crate::text::pdf::references::{Confidence, Reference, title_words};

/// arXiv asks for no more than one request every three seconds.
pub const PAUSE: Duration = Duration::from_secs(3);
/// Answers asked for per title search: the same title is near the top or not there.
const SEARCH_RESULTS: usize = 5;
/// Words of a title put in a search: arXiv's phrase search needs no more to find it.
const SEARCH_WORDS: usize = 12;

/// An answer to a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// The `Retry-After` header, when the answer carried one.
    pub retry_after: Option<Duration>,
    pub body: String,
}

/// How a request is sent: an address in, an answer or why there was none out.
pub type Get<'a> = dyn FnMut(&str) -> Result<Response, String> + 'a;

/// What is asked about a reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Its title, by its arXiv id.
    Id(String),
    /// Its arXiv id, by its title.
    Title(String),
}

/// What arXiv's answer says about a reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Found {
        arxiv: String,
        title: String,
        year: Option<u16>,
    },
    NotFound,
    /// More than one paper of the same title: none is taken.
    Ambiguous {
        arxiv: Vec<String>,
    },
}

/// Why a look-up stopped.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LookupError {
    #[error("arXiv is refusing requests for now; try again in {} seconds", retry_after.as_secs())]
    RateLimited { retry_after: Duration },
    #[error("arXiv answered {0}")]
    Failed(u16),
    #[error("arXiv could not be reached: {0}")]
    Unreachable(String),
}

/// What to ask about a reference, if anything: an unresolved one with a title, or one known only by its id.
pub fn ask_for(linked: &LinkedReference) -> Option<Ask> {
    let reference = &linked.reference;
    match (&linked.link, &reference.title) {
        (Link::OnArxiv { arxiv }, None) => Some(Ask::Id(arxiv.clone())),
        (Link::Unresolved, Some(title)) if title_words(title).len() >= 2 => Some(Ask::Title(title.clone())),
        _ => None,
    }
}

/// The address that asks it.
pub fn url_for(base: &str, ask: &Ask) -> String {
    match ask {
        Ask::Id(arxiv) => format!("{base}?id_list={}&max_results=1", encoded(arxiv)),
        Ask::Title(title) => {
            // a phrase of plain words: arXiv's query language gives punctuation meanings of its own
            let words: Vec<String> = title_words(title).into_iter().take(SEARCH_WORDS).collect();
            format!(
                "{base}?search_query={}&max_results={SEARCH_RESULTS}",
                encoded(&format!("ti:\"{}\"", words.join(" ")))
            )
        }
    }
}

fn encoded(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => (byte as char).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// One paper of an Atom feed from arXiv.
struct Entry {
    arxiv: String,
    title: String,
    year: Option<u16>,
}

/// The text between `<tag>` and `</tag>`, the first time it occurs in `xml`.
fn element<'x>(xml: &'x str, tag: &str) -> Option<&'x str> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    Some(&xml[start..end])
}

fn unescaped(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// The papers of an arXiv answer. An error comes as an entry whose id is not a paper's, and is not one.
fn entries(feed: &str) -> Vec<Entry> {
    feed.split("<entry>")
        .skip(1)
        .filter_map(|entry| {
            let id = element(entry, "id")?.trim();
            let arxiv = id.split_once("arxiv.org/abs/")?.1;
            // without its version: "2505.11792v3" is "2505.11792"
            let arxiv = arxiv
                .rsplit_once('v')
                .filter(|(_, version)| version.chars().all(|c| c.is_ascii_digit()))
                .map_or(arxiv, |(id, _)| id);
            let title = unescaped(&element(entry, "title")?.split_whitespace().collect::<Vec<_>>().join(" "));
            let year = element(entry, "published").and_then(|date| date.get(..4)).and_then(|year| year.parse().ok());
            Some(Entry { arxiv: arxiv.to_owned(), title, year })
        })
        .collect()
}

/// What arXiv's answer to `ask` says about `reference`.
pub fn read_answer(ask: &Ask, reference: &Reference, feed: &str) -> Answer {
    let entries = entries(feed);
    match ask {
        Ask::Id(arxiv) => entries.into_iter().find(|entry| entry.arxiv == *arxiv).map_or(Answer::NotFound, |entry| {
            Answer::Found { arxiv: entry.arxiv, title: entry.title, year: entry.year }
        }),
        Ask::Title(title) => {
            let mut same: Vec<Entry> = entries
                .into_iter()
                .filter(|entry| same_title(title, reference.year, &entry.title, entry.year))
                .collect();
            same.dedup_by(|a, b| a.arxiv == b.arxiv);
            match same.len() {
                0 => Answer::NotFound,
                1 => same.pop().map_or(Answer::NotFound, |entry| Answer::Found {
                    arxiv: entry.arxiv,
                    title: entry.title,
                    year: entry.year,
                }),
                _ => Answer::Ambiguous { arxiv: same.into_iter().map(|entry| entry.arxiv).collect() },
            }
        }
    }
}

/// Send one request (at arXiv's pace, which `get` keeps) and answer its feed. A refusal (429, 503) is
/// [`LookupError::RateLimited`], saying how long arXiv asked to be left alone (as long as it says, else
/// [`super::polite::FIRST_BACK_OFF`]; an hour at most).
pub fn send(url: &str, get: &mut Get) -> Result<String, LookupError> {
    let response = get(url).map_err(LookupError::Unreachable)?;
    match response.status {
        200 => Ok(response.body),
        429 | 503 => {
            let wait = response.retry_after.unwrap_or(super::polite::FIRST_BACK_OFF).min(Duration::from_secs(3600));
            Err(LookupError::RateLimited { retry_after: wait })
        }
        status => Err(LookupError::Failed(status)),
    }
}

/// Look up on arXiv each reference of `paper` that is worth asking about, and link it again with what was found.
/// Stops at the first refusal, keeping what was found so far; the error says when to try again.
pub fn enrich(
    paper: &mut [LinkedReference],
    library: &Linker,
    base: &str,
    get: &mut Get,
) -> Result<usize, LookupError> {
    let mut found = 0;
    for linked in paper.iter_mut() {
        let Some(ask) = ask_for(linked) else { continue };
        let feed = send(&url_for(base, &ask), get)?;
        if let Answer::Found { arxiv, title, year } = read_answer(&ask, &linked.reference, &feed) {
            let reference = &mut linked.reference;
            reference.arxiv = Some(arxiv);
            reference.title = reference.title.take().or(Some(title));
            reference.year = reference.year.or(year);
            reference.confidence = Confidence::Identified;
            linked.link = library.link(reference);
            found += 1;
        }
    }
    Ok(found)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::analysis::citations::LibraryPaper;

    /// arXiv's answer to a title search, as recorded (the abstract and the authors trimmed away).
    const TITLE_SEARCH: &str = r#"<?xml version='1.0' encoding='UTF-8'?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>https://arxiv.org/api/WlG9jUTUH4gGtp4TzqPl7oMtiqQ</id>
  <title>arXiv Query: search_query=ti:"Solver informed rl Grounding large language models for authentic optimization modeling"&amp;id_list=&amp;start=0&amp;max_results=3</title>
  <opensearch:totalResults>1</opensearch:totalResults>
  <entry>
    <id>http://arxiv.org/abs/2505.11792v3</id>
    <title>Solver-Informed RL: Grounding Large Language Models for Authentic
      Optimization Modeling</title>
    <updated>2025-12-22T09:39:43Z</updated>
    <published>2025-05-17T02:32:03Z</published>
  </entry>
</feed>"#;

    /// arXiv's answer when nothing matches, and when the query is malformed.
    const NOTHING: &str =
        r#"<feed xmlns="http://www.w3.org/2005/Atom"><opensearch:totalResults>0</opensearch:totalResults></feed>"#;
    const ERROR: &str = r#"<feed><entry><id>http://arxiv.org/api/errors#incorrect_id_format_for_1234</id><title>Error</title></entry></feed>"#;

    fn unresolved(title: &str, year: u16) -> LinkedReference {
        LinkedReference {
            reference: Reference {
                raw: format!("Chen, Y. {title}. arXiv, {year}."),
                authors: vec!["Chen, Y.".to_owned()],
                title: Some(title.to_owned()),
                year: Some(year),
                venue: None,
                arxiv: None,
                doi: None,
                url: None,
                confidence: Confidence::TitleParsed,
            },
            link: Link::Unresolved,
        }
    }

    fn answer(status: u16, body: &str) -> Response {
        Response { status, retry_after: None, body: body.to_owned() }
    }

    #[test]
    fn a_title_search_is_a_phrase_of_plain_words() {
        let ask = ask_for(&unresolved("Solver-informed rl: Grounding large language models", 2025)).unwrap();
        assert_eq!(
            url_for("https://export.arxiv.org/api/query", &ask),
            "https://export.arxiv.org/api/query?search_query=ti%3A%22solver%20informed%20rl%20grounding%20large%20language%20models%22&max_results=5"
        );
        assert_eq!(
            url_for("https://example.org/q", &Ask::Id("cs/0501001".to_owned())),
            "https://example.org/q?id_list=cs/0501001&max_results=1"
        );
    }

    #[test]
    fn an_answer_is_taken_only_when_its_title_is_the_same() {
        let linked =
            unresolved("Solver-informed rl: Grounding large language models for authentic optimization modeling", 2025);
        let ask = ask_for(&linked).unwrap();
        assert_eq!(
            read_answer(&ask, &linked.reference, TITLE_SEARCH),
            Answer::Found {
                arxiv: "2505.11792".to_owned(),
                title: "Solver-Informed RL: Grounding Large Language Models for Authentic Optimization Modeling"
                    .to_owned(),
                year: Some(2025),
            }
        );
        let other = unresolved("Solver-informed rl for scheduling", 2025);
        assert_eq!(read_answer(&ask_for(&other).unwrap(), &other.reference, TITLE_SEARCH), Answer::NotFound);
        assert_eq!(read_answer(&ask, &linked.reference, NOTHING), Answer::NotFound);
        assert_eq!(read_answer(&Ask::Id("1234".to_owned()), &linked.reference, ERROR), Answer::NotFound);
        let twice = TITLE_SEARCH.replace("</feed>", "<entry><id>http://arxiv.org/abs/2601.00002v1</id><title>Solver-Informed RL: Grounding Large Language Models for Authentic Optimization Modeling</title><published>2026-01-01T00:00:00Z</published></entry></feed>");
        assert_eq!(
            read_answer(&ask, &linked.reference, &twice),
            Answer::Ambiguous { arxiv: vec!["2505.11792".to_owned(), "2601.00002".to_owned()] }
        );
    }

    #[test]
    fn a_refusal_ends_the_lookups_at_once_and_says_when_to_try_again() {
        let answers = RefCell::new(vec![
            answer(200, NOTHING),
            Response { retry_after: Some(Duration::from_secs(7)), ..answer(429, "") },
            answer(503, ""),
        ]);
        let asked = RefCell::new(0);
        let mut get = |_: &str| -> Result<Response, String> {
            *asked.borrow_mut() += 1;
            Ok(answers.borrow_mut().remove(0))
        };
        assert_eq!(send("first", &mut get).unwrap(), NOTHING);
        assert_eq!(send("second", &mut get), Err(LookupError::RateLimited { retry_after: Duration::from_secs(7) }));
        assert_eq!(
            send("third", &mut get),
            Err(LookupError::RateLimited { retry_after: super::super::polite::FIRST_BACK_OFF }),
            "a refusal that does not say how long"
        );
        assert_eq!(*asked.borrow(), 3, "nothing asked twice: the pace waits, not the lookup");
        let mut broken = |_: &str| -> Result<Response, String> { Ok(answer(500, "")) };
        assert_eq!(send("broken", &mut broken), Err(LookupError::Failed(500)));
        let mut gone = |_: &str| -> Result<Response, String> { Err("no connection".to_owned()) };
        assert_eq!(send("gone", &mut gone), Err(LookupError::Unreachable("no connection".to_owned())));
    }

    #[test]
    fn what_is_found_on_arxiv_is_linked_again() {
        let library: Vec<LibraryPaper> = vec![];
        let linker = Linker::new(&library);
        let mut get = |url: &str| -> Result<Response, String> {
            assert!(url.starts_with("https://export.arxiv.org/api/query?search_query="));
            Ok(answer(200, TITLE_SEARCH))
        };
        let mut paper = vec![
            unresolved("Solver-informed rl: Grounding large language models for authentic optimization modeling", 2025),
            LinkedReference { link: Link::InLibrary { paper: "x".to_owned() }, ..unresolved("Already known", 2020) },
        ];
        let found = enrich(&mut paper, &linker, super::super::arxiv::API, &mut get).unwrap();
        assert_eq!(found, 1);
        assert_eq!(paper[0].link, Link::OnArxiv { arxiv: "2505.11792".to_owned() });
        assert_eq!(paper[0].reference.confidence, Confidence::Identified);
        assert_eq!(paper[1].link, Link::InLibrary { paper: "x".to_owned() }, "a resolved reference is not asked about");
    }
}
