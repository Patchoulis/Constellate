//! A selection cut into the slices that are fetched, and what fetching one came to.
//!
//! A selection is fetched as a series of slices, each remembered by the library once it is finished:
//!
//! * a submission-date range: one arXiv query per day (optionally restricted to categories), so that no single
//!   query runs into the arXiv API's limit on deep paging;
//! * conferences: one slice per conference, from the conference's own list of accepted papers; only when a
//!   conference publishes no such list, from arXiv papers whose comment names it ("Accepted at CVPR 2026").
//!
//! A slice that finished once is not fetched again unless asked to redo it. What fetching a slice came to is a
//! [`Gathered`]: finished (remember it), unfinished (store the papers, fetch it again next time), or nothing now
//! (store nothing, try again later). Only a finished slice may be remembered as done.

use std::collections::HashSet;
use std::time::Duration;

use chrono::{NaiveDate, TimeDelta};

use super::conferences::{self, Abstracts, Listing};
use super::{Fetch, Paper, arxiv};
use crate::selection::Selection;

/// Days this recent can still gain papers on arXiv, so they are fetched again next time.
const STILL_GROWING: TimeDelta = TimeDelta::days(2);

/// One piece of a selection, fetched as a whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Slice {
    /// A conference ("NeurIPS 2025"): its papers are tagged with it.
    Conference { venue: String, include_workshops: bool },
    /// Papers submitted to arXiv on one day, in these categories (all, when none are given).
    Day { day: NaiveDate, categories: Vec<String> },
}

impl Slice {
    /// How the library remembers that this slice is finished: the same keys the Python backend wrote, so that
    /// nothing it finished is fetched again.
    pub fn key(&self) -> String {
        match self {
            Self::Conference { venue, include_workshops } => {
                let workshops = if *include_workshops { "True" } else { "False" };
                format!("venue|{venue}|{workshops}|official")
            }
            Self::Day { day, categories } => {
                let mut categories = categories.clone();
                categories.sort();
                format!("day|{day}|{}", categories.join(","))
            }
        }
    }

    /// What the log calls it.
    pub fn label(&self) -> String {
        match self {
            Self::Conference { venue, .. } => venue.clone(),
            Self::Day { day, .. } => day.to_string(),
        }
    }

    /// The conference its papers are tagged with, if it is one.
    pub fn venue(&self) -> Option<&str> {
        match self {
            Self::Conference { venue, .. } => Some(venue),
            Self::Day { .. } => None,
        }
    }
}

/// The slices a selection is fetched in: one per conference, or one per day of the range, in order.
pub fn slices(selection: &Selection) -> Vec<Slice> {
    match selection {
        Selection::Conferences { venues, include_workshops } => venues
            .iter()
            .map(|venue| Slice::Conference { venue: venue.clone(), include_workshops: *include_workshops })
            .collect(),
        Selection::Dates { start, end, categories } => start
            .iter_days()
            .take_while(|day| day <= end)
            .map(|day| Slice::Day { day, categories: categories.clone() })
            .collect(),
    }
}

/// The slices still to fetch: those not finished before, or all of them when redoing.
pub fn to_fetch<'a>(slices: &'a [Slice], done: &HashSet<String>, redo: bool) -> Vec<&'a Slice> {
    slices.iter().filter(|slice| redo || !done.contains(&slice.key())).collect()
}

/// Why a slice that brought papers is not finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StillOpen {
    /// A day this recent can still gain papers.
    RecentDay,
    /// The conference's list came without the abstracts file, which could not be had now.
    AbstractsMissing(String),
}

/// What fetching a slice came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gathered {
    /// Every paper the slice will have: store them and remember the slice as done.
    Finished(Vec<Paper>),
    /// Store these papers, but fetch the slice again next time.
    Unfinished { papers: Vec<Paper>, why: StillOpen },
    /// Nothing could be had now: store nothing, remember nothing, and try again later.
    NotNow(String),
}

/// Fetch one slice. `today` decides which days are recent; `pause` waits between arXiv's requests.
pub fn gather(
    slice: &Slice,
    fetch: &dyn Fetch,
    pause: &dyn Fn(Duration),
    today: NaiveDate,
    log: &dyn Fn(&str),
) -> Gathered {
    match slice {
        Slice::Conference { venue, include_workshops } => match conferences::accepted_papers(fetch, venue, log) {
            Listing::List { papers, abstracts: Abstracts::Fetched | Abstracts::NotPublished } => {
                Gathered::Finished(papers)
            }
            Listing::List { papers, abstracts: Abstracts::MissingForNow(why) } => {
                Gathered::Unfinished { papers, why: StillOpen::AbstractsMissing(why) }
            }
            Listing::NoList(why) => {
                log(&format!("    {why}; using arXiv papers whose comment names {venue} instead"));
                match conferences::from_arxiv_comments(fetch, pause, venue, *include_workshops, log) {
                    Ok(papers) => Gathered::Finished(papers),
                    Err(error) => Gathered::NotNow(error.to_string()),
                }
            }
            Listing::NotNow(why) => Gathered::NotNow(why),
        },
        Slice::Day { day, categories } => match arxiv::query(fetch, pause, &arxiv::submitted_on(*day, categories), log)
        {
            Ok(papers) if *day >= today - STILL_GROWING => Gathered::Unfinished { papers, why: StillOpen::RecentDay },
            Ok(papers) => Gathered::Finished(papers),
            Err(error) => Gathered::NotNow(error.to_string()),
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;

    use super::super::arxiv::tests::feed;
    use super::super::conferences::{Part, list_url};
    use super::super::fake::{Fake, ok, status};
    use super::*;

    fn quiet(_: &str) {}

    fn date(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    fn conference(venue: &str) -> Slice {
        Slice::Conference { venue: venue.to_owned(), include_workshops: false }
    }

    fn list() -> String {
        json!({"results": [{"id": 1, "name": "A Paper", "eventtype": "Poster"}]}).to_string()
    }

    #[test]
    fn conferences_are_one_slice_each_and_a_date_range_one_per_day() {
        let venues = vec!["NeurIPS 2025".to_owned(), "ACL 2025".to_owned()];
        let picked = slices(&Selection::Conferences { venues, include_workshops: true });
        assert_eq!(
            picked.iter().map(Slice::key).collect::<Vec<_>>(),
            ["venue|NeurIPS 2025|True|official", "venue|ACL 2025|True|official"]
        );
        assert_eq!(picked[0].venue(), Some("NeurIPS 2025"));

        let categories = vec!["stat.ML".to_owned(), "cs.LG".to_owned()];
        let days = slices(&Selection::Dates { start: date("2026-01-30"), end: date("2026-02-01"), categories });
        assert_eq!(days.iter().map(Slice::label).collect::<Vec<_>>(), ["2026-01-30", "2026-01-31", "2026-02-01"]);
        assert_eq!(days[0].key(), "day|2026-01-30|cs.LG,stat.ML");
        assert_eq!(days[0].venue(), None);
        let all = slices(&Selection::Dates { start: date("2026-01-30"), end: date("2026-01-30"), categories: vec![] });
        assert_eq!(all[0].key(), "day|2026-01-30|");
    }

    #[test]
    fn a_slice_finished_before_is_not_fetched_again_unless_redone() {
        let picked = [conference("NeurIPS 2025"), conference("ICML 2026")];
        let done: HashSet<String> = [picked[0].key()].into();
        assert_eq!(to_fetch(&picked, &done, false), [&picked[1]]);
        assert_eq!(to_fetch(&picked, &done, true).len(), 2);
    }

    #[test]
    fn a_conferences_list_with_its_abstracts_is_finished() {
        let fake = Fake::new()
            .with(&list_url("neurips.cc", "neurips", "2025", Part::OralsPosters), ok("application/json", &list()))
            .with(
                &list_url("neurips.cc", "neurips", "2025", Part::Abstracts),
                ok("application/json", r#"{"1": "Text."}"#),
            );
        let Gathered::Finished(papers) =
            gather(&conference("NeurIPS 2025"), &fake, &|_| {}, date("2026-01-01"), &quiet)
        else {
            panic!()
        };
        assert_eq!(papers[0].abstract_text, "Text.");
    }

    #[test]
    fn a_list_whose_abstracts_could_not_be_had_is_fetched_again() {
        let abstracts = list_url("neurips.cc", "neurips", "2025", Part::Abstracts);
        let fake = Fake::new()
            .with(&list_url("neurips.cc", "neurips", "2025", Part::OralsPosters), ok("application/json", &list()))
            .with(&abstracts, status(503));
        let gathered = gather(&conference("NeurIPS 2025"), &fake, &|_| {}, date("2026-01-01"), &quiet);
        let Gathered::Unfinished { papers, why } = gathered else { panic!("{gathered:?}") };
        assert_eq!((papers.len(), why), (1, StillOpen::AbstractsMissing(format!("{abstracts} answered HTTP 503"))));
    }

    #[test]
    fn a_list_that_could_not_be_fetched_is_neither_replaced_by_arxiv_nor_remembered() {
        for answer in [status(503), status(429), status(500)] {
            let fake = Fake::new().with(&list_url("neurips.cc", "neurips", "2025", Part::OralsPosters), answer.clone());
            let gathered = gather(&conference("NeurIPS 2025"), &fake, &|_| {}, date("2026-01-01"), &quiet);
            assert!(matches!(gathered, Gathered::NotNow(_)), "{answer:?}: {gathered:?}");
            assert!(fake.asked().iter().all(|address| !address.contains("arxiv")), "{:?}", fake.asked());
        }
    }

    #[test]
    fn a_conference_that_publishes_no_list_is_found_in_arxiv_comments() {
        let search = arxiv::page_url(&arxiv::comment_mentions("NeurIPS 2031"), 0);
        let answer = feed(1, &[("http://arxiv.org/abs/3101.00001v1", "Future", "Accepted at NeurIPS 2031")]);
        let fake = Fake::new().with(&search, ok("application/atom+xml", &answer)); // the list answers 404
        let gathered = gather(&conference("NeurIPS 2031"), &fake, &|_| {}, date("2031-01-01"), &quiet);
        let Gathered::Finished(papers) = gathered else { panic!("{gathered:?}") };
        assert_eq!(papers[0].id, "3101.00001");

        let failing = Fake::new().with(&search, status(503));
        let gathered = gather(&conference("NeurIPS 2031"), &failing, &|_| {}, date("2031-01-01"), &quiet);
        assert!(matches!(gathered, Gathered::NotNow(_)), "{gathered:?}");
    }

    #[test]
    fn a_recent_day_is_stored_but_fetched_again() {
        let day = Slice::Day { day: date("2026-11-24"), categories: vec![] };
        let search = arxiv::page_url(&arxiv::submitted_on(date("2026-11-24"), &[]), 0);
        let answer = feed(1, &[("http://arxiv.org/abs/2611.00001v1", "New", "")]);
        let fake = Fake::new().with(&search, ok("application/atom+xml", &answer));
        let recent = gather(&day, &fake, &|_| {}, date("2026-11-26"), &quiet);
        assert!(matches!(recent, Gathered::Unfinished { why: StillOpen::RecentDay, .. }), "{recent:?}");
        let settled = gather(&day, &fake, &|_| {}, date("2026-11-27"), &quiet);
        assert!(matches!(settled, Gathered::Finished(ref papers) if papers.len() == 1), "{settled:?}");
    }
}
