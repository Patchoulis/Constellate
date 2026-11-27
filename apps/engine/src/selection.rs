//! Which papers a run is about: a set of conferences, or a submission-date range with optional categories.
//!
//! The one place a request is checked and turned into a [`Selection`]. A selection is one of two things and
//! never a mixture, so it is an enum: a date range has no conferences, and conferences have no dates.

use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

/// Conferences spelled out in a run's name; a longer list is cut short and told apart by a few hash letters.
const MOST_NAMED: usize = 3;

#[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
static NAME_AND_YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^.+ \d{4}$").unwrap());
/// An arXiv category as arXiv writes them: `cs.LG`, `hep-th`, `astro-ph.CO`, `cond-mat.mes-hall`. Nothing else may
/// be one, since a category goes into the run's folder name and into the query sent to arXiv.
#[allow(clippy::unwrap_used)]
static CATEGORY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z]+(?:-[A-Za-z]+)*(?:\.[A-Za-z]+(?:-[A-Za-z]+)*)?$").unwrap());

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    /// Papers accepted at these conferences, each written "Name Year".
    Conferences { venues: Vec<String>, include_workshops: bool },
    /// Papers submitted to arXiv from `start` to `end`, in these categories (all, when none are given).
    Dates { start: NaiveDate, end: NaiveDate, categories: Vec<String> },
}

/// Why a request is not a selection, in words for the person who made it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Invalid {
    #[error("Write each conference as \"Name Year\", e.g. NeurIPS 2025")]
    Venue,
    #[error("Give conferences, or a start date and an end date (YYYY-MM-DD)")]
    Dates,
    #[error("The start date is after the end date")]
    Backwards,
    #[error("Write each category as arXiv does, e.g. cs.LG or hep-th")]
    Category,
}

/// The values that say something: trimmed, and without the empty ones.
fn clean(values: &[String]) -> Vec<String> {
    values.iter().map(|value| value.trim()).filter(|value| !value.is_empty()).map(str::to_owned).collect()
}

impl Selection {
    /// Conferences win when both are given; otherwise both dates are needed.
    pub fn parse(
        venues: &[String],
        start: Option<&str>,
        end: Option<&str>,
        categories: &[String],
        include_workshops: bool,
    ) -> Result<Self, Invalid> {
        let venues = clean(venues);
        if !venues.is_empty() {
            if venues.iter().any(|venue| !NAME_AND_YEAR.is_match(venue)) {
                return Err(Invalid::Venue);
            }
            return Ok(Self::Conferences { venues, include_workshops });
        }
        let date = |text: Option<&str>| text.and_then(|text| NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok());
        let (Some(start), Some(end)) = (date(start), date(end)) else {
            return Err(Invalid::Dates);
        };
        if start > end {
            return Err(Invalid::Backwards);
        }
        let categories = clean(categories);
        if categories.iter().any(|category| !CATEGORY.is_match(category)) {
            return Err(Invalid::Category);
        }
        Ok(Self::Dates { start, end, categories })
    }

    /// The folder name of the run: the same selection always gets the same name.
    pub fn name(&self) -> String {
        match self {
            Self::Conferences { venues, .. } => {
                let letters = |venue: &String| -> String {
                    venue.chars().filter(|c| c.is_alphanumeric() || *c == '_').flat_map(char::to_lowercase).collect()
                };
                let mut names: Vec<String> = venues.iter().map(letters).collect();
                if names.len() > MOST_NAMED {
                    // a long list would not fit in a file name
                    let mut sorted = names.clone();
                    sorted.sort();
                    let digest = Sha1::digest(sorted.join(",").as_bytes());
                    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
                    let more = names.len() - MOST_NAMED;
                    names.truncate(MOST_NAMED);
                    names.push(format!("and{more}more"));
                    names.push(hex[..6].to_owned());
                }
                names.join("_")
            }
            Self::Dates { start, end, categories } => {
                let kinds = if categories.is_empty() { String::new() } else { format!("_{}", categories.join("-")) };
                format!("arxiv_{start}_{end}{kinds}")
            }
        }
    }

    /// What a paper counts as in a community's mix: its conferences among the selected ones, or its primary
    /// category.
    pub fn tags(&self, paper_venues: &[String], primary_category: &str) -> Vec<String> {
        match self {
            Self::Conferences { venues, .. } => {
                let mut tags: Vec<String> =
                    paper_venues.iter().filter(|venue| venues.contains(venue)).cloned().collect();
                tags.sort();
                tags.dedup();
                tags
            }
            Self::Dates { .. } => vec![primary_category.to_owned()],
        }
    }

    /// As it is stored with a run and sent to the interface: every field, the ones that do not apply empty.
    pub fn to_json(&self) -> Value {
        match self {
            Self::Conferences { venues, include_workshops } => json!({
                "venues": venues, "start": null, "end": null, "categories": [], "include_workshops": include_workshops,
            }),
            Self::Dates { start, end, categories } => json!({
                "venues": [], "start": start.to_string(), "end": end.to_string(), "categories": categories,
                "include_workshops": false,
            }),
        }
    }
}

impl Serialize for Selection {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn all(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn conferences(values: &[&str]) -> Selection {
        Selection::parse(&all(values), None, None, &[], false).unwrap()
    }

    #[test]
    fn conferences_are_a_name_and_a_year() {
        let selection = conferences(&[" NeurIPS 2025 ", "", "ICML 2026"]);
        assert_eq!(
            selection,
            Selection::Conferences { venues: all(&["NeurIPS 2025", "ICML 2026"]), include_workshops: false }
        );
        for wrong in ["NeurIPS", "2025", "NeurIPS 25", "NeurIPS 2025 oral"] {
            assert_eq!(Selection::parse(&all(&[wrong]), None, None, &[], false), Err(Invalid::Venue), "{wrong}");
        }
    }

    #[test]
    fn a_date_range_needs_both_dates_in_order() {
        let parsed = Selection::parse(&[], Some("2026-01-05"), Some("2026-01-11"), &all(&["cs.LG", " "]), true);
        let Ok(Selection::Dates { start, end, categories }) = parsed else { panic!("not a date range: {parsed:?}") };
        assert_eq!(
            (start.to_string(), end.to_string(), categories),
            ("2026-01-05".into(), "2026-01-11".into(), all(&["cs.LG"]))
        );

        assert_eq!(Selection::parse(&[], Some("2026-01-11"), Some("2026-01-05"), &[], false), Err(Invalid::Backwards));
        for (start, end) in [
            (None, None),
            (Some("2026-01-05"), None),
            (Some("yesterday"), Some("2026-01-11")),
            (Some("2026-02-30"), Some("2026-03-01")),
        ] {
            assert_eq!(Selection::parse(&[], start, end, &[], false), Err(Invalid::Dates), "{start:?} {end:?}");
        }
        assert_eq!(Invalid::Dates.to_string(), "Give conferences, or a start date and an end date (YYYY-MM-DD)");
    }

    #[test]
    fn a_category_is_one_arxiv_writes_and_nothing_that_could_leave_the_runs_folder() {
        let dates = |categories: &[&str]| {
            Selection::parse(&[], Some("2026-01-05"), Some("2026-01-11"), &all(categories), false)
        };
        for good in ["cs.LG", "stat.ML", "hep-th", "astro-ph.CO", "cond-mat.mes-hall", "q-bio.NC", "math"] {
            assert!(dates(&[good]).is_ok(), "{good}");
        }
        for bad in ["../../outside", "cs/LG", "cs.LG\\x", "cs.LG OR all:x", "cs.LG:", "C:", ".hidden", "cs..LG", "-"] {
            assert_eq!(dates(&["cs.LG", bad]), Err(Invalid::Category), "{bad}");
        }
        let named = dates(&["cs.LG", "../../outside"]).map(|selection| selection.name());
        assert!(named.is_err(), "a run would be named {named:?}");
    }

    #[test]
    fn conferences_win_over_dates_given_with_them() {
        let both =
            Selection::parse(&all(&["ICLR 2026"]), Some("2026-01-05"), Some("2026-01-11"), &all(&["cs.LG"]), true);
        assert_eq!(both, Ok(Selection::Conferences { venues: all(&["ICLR 2026"]), include_workshops: true }));
    }

    #[test]
    fn a_runs_folder_name_is_the_same_as_it_always_was() {
        // these names are folders that exist on people's disks
        assert_eq!(conferences(&["NeurIPS 2025", "ICML 2026"]).name(), "neurips2025_icml2026");
        assert_eq!(conferences(&["AAAI/ACM AIES 2026"]).name(), "aaaiacmaies2026");
        let dates =
            Selection::parse(&[], Some("2026-01-05"), Some("2026-01-11"), &all(&["cs.LG", "stat.ML"]), false).unwrap();
        assert_eq!(dates.name(), "arxiv_2026-01-05_2026-01-11_cs.LG-stat.ML");
        let plain = Selection::parse(&[], Some("2026-01-05"), Some("2026-01-11"), &[], false).unwrap();
        assert_eq!(plain.name(), "arxiv_2026-01-05_2026-01-11");
    }

    #[test]
    fn a_long_list_of_conferences_gets_a_short_stable_name() {
        let many = conferences(&["NeurIPS 2025", "ICML 2026", "ICLR 2026", "CVPR 2026", "AISTATS 2026"]);
        let name = many.name();
        assert!(name.starts_with("neurips2025_icml2026_iclr2026_and2more_"), "{name}");
        assert_eq!(name.len(), "neurips2025_icml2026_iclr2026_and2more_".len() + 6);
        // the letters depend on which conferences, not on their order
        let reordered = conferences(&["NeurIPS 2025", "ICML 2026", "ICLR 2026", "AISTATS 2026", "CVPR 2026"]);
        assert_eq!(reordered.name(), name);
        let other = conferences(&["NeurIPS 2025", "ICML 2026", "ICLR 2026", "CVPR 2026", "MLSys 2026"]);
        assert_ne!(other.name(), name);
    }

    #[test]
    fn a_paper_is_tagged_by_its_selected_conferences_or_its_category() {
        let selection = conferences(&["NeurIPS 2025", "ICML 2026"]);
        assert_eq!(selection.tags(&all(&["ICML 2026", "ICLR 2026", "ICML 2026"]), "cs.LG"), all(&["ICML 2026"]));
        assert_eq!(selection.tags(&[], "cs.LG"), Vec::<String>::new());
        let dates = Selection::parse(&[], Some("2026-01-05"), Some("2026-01-11"), &[], false).unwrap();
        assert_eq!(dates.tags(&all(&["ICML 2026"]), "cs.CL"), all(&["cs.CL"]));
    }

    #[test]
    fn it_is_stored_with_every_field() {
        assert_eq!(
            serde_json::to_value(conferences(&["NeurIPS 2025"])).unwrap(),
            json!({"venues": ["NeurIPS 2025"], "start": null, "end": null, "categories": [], "include_workshops": false})
        );
        let dates = Selection::parse(&[], Some("2026-01-05"), Some("2026-01-11"), &all(&["cs.LG"]), false).unwrap();
        assert_eq!(dates.to_json()["start"], "2026-01-05");
        assert_eq!(dates.to_json()["venues"], json!([]));
    }
}
