//! How a conference accepted a paper, in tiers that hold across conferences, and in the conference's own words.
//!
//! Conferences name their tiers differently, and the same name changes meaning from year to year: ICLR 2023
//! called its orals "notable-top-5%", CVPR calls its spotlights "Highlight", and at ICML 2021 every paper without
//! a long talk was a "Spotlight", which there meant an ordinary acceptance. So a decision is not read by the words
//! in it but looked up in [`TERMS`]: per conference and year, every term its list was seen to use, the tier it is
//! counted as, how the conference writes it, and what share of the list's papers it was. docs/acceptance-tiers.md
//! has where each row comes from.
//!
//! ```text
//!   the conference and year have the term            that row
//!   another year of the conference has it            the latest such row, said to be borrowed from that year
//!   no year has it                                   read from its wording, said to be unknown, and collected
//!                                                    (`unknown_terms`) for a maintainer to add:
//!                                                      oral, award, best paper, outstanding, long talk  -> oral
//!                                                      spotlight, highlight, notable                    -> spotlight
//!                                                      anything else                                    -> poster
//!   no decision at all                               a poster, with no term
//! ```
//!
//! A decision is the list's `decision`, plus the kind of any other event the list has for the same paper when the
//! decision does not already say so ("Accept Poster; Oral": see conferences.rs), because some lists name a
//! paper's oral only by its talk. A paper that did not come from a conference's list (an arXiv paper, even one
//! whose comment says "oral") was not judged by anyone here, and says so rather than borrowing a tier.

use std::collections::BTreeMap;

use serde::{Serialize, Serializer};

use super::catalog::CONFERENCES;
use super::{Origin, Paper};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Acceptance {
    Oral,
    Spotlight,
    Poster,
    /// Not from a conference's list, so there is no decision to read.
    NotListed,
}

use Acceptance::{Oral, Poster, Spotlight};

impl Acceptance {
    /// The tiers a conference paper can be in, best first.
    pub const TIERS: [Self; 3] = [Self::Oral, Self::Spotlight, Self::Poster];

    /// The name the interface knows (`Acceptance` in packages/contracts), or none for a paper not listed.
    pub fn name(self) -> Option<&'static str> {
        match self {
            Self::Oral => Some("oral"),
            Self::Spotlight => Some("spotlight"),
            Self::Poster => Some("poster"),
            Self::NotListed => None,
        }
    }

    /// The terms of every conference counted as this tier, as the conferences write them, each once.
    pub fn equivalents(self) -> Vec<&'static str> {
        let mut called: Vec<&'static str> = Vec::new();
        for row in TERMS.iter().filter(|row| row.3 == self) {
            if !called.iter().any(|known| known.eq_ignore_ascii_case(row.4)) {
                called.push(row.4);
            }
        }
        called
    }
}

/// As the interface reads it: the tier's name, or `null`.
impl Serialize for Acceptance {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.name().serialize(serializer)
    }
}

/// A conference's term for how it accepted a paper (`AcceptanceTerm` in packages/contracts).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Term {
    /// The conference and year, as the catalogue names it: "CVPR 2024".
    pub conference: String,
    /// The conference's own word for it, as it writes it: "Highlight", "Notable top 5%".
    pub called: String,
    /// What it is there: its share of the list's papers, and what it involves.
    pub means: String,
    /// False when no year of the conference was seen to use it, so the tier was read from its wording.
    pub known: bool,
}

/// How a conference paper was accepted: the tier it counts as, and how the conference put it.
#[derive(Clone, Debug, PartialEq)]
pub struct Accepted {
    pub tier: Acceptance,
    /// None for a paper not listed, or one whose list gives no decision.
    pub term: Option<Term>,
}

/// Conference (lower case, as in a paper's id), year, the term as the list writes it (lower case: lists change
/// its case between tracks and years), tier, the conference's wording for it, the percentage of the list's
/// papers given that wording that year, and what it is there.
type Row = (&'static str, u16, &'static str, Acceptance, &'static str, f32, &'static str);

/// Every term the conferences' lists were seen to use, from reading those lists (docs/acceptance-tiers.md).
#[rustfmt::skip]
pub const TERMS: &[Row] = &[
    ("neurips", 2021, "accept (oral)", Oral, "Oral", 2.4, "presented as a talk"),
    ("neurips", 2021, "accept (spotlight)", Spotlight, "Spotlight", 11.1, "a short spotlight talk"),
    ("neurips", 2021, "accept (poster)", Poster, "Poster", 86.5, ""),
    ("neurips", 2022, "accept; spotlight", Spotlight, "Featured", 19.8, "a lightning talk in a Featured Papers Panel; decisions named no tier that year"),
    ("neurips", 2022, "spotlight", Spotlight, "Featured", 19.8, "a lightning talk in a Featured Papers Panel; decisions named no tier that year"),
    ("neurips", 2022, "accept", Poster, "Accept", 78.7, "decisions named no tier that year"),
    ("neurips", 2023, "accept (oral)", Oral, "Oral", 2.1, "presented as a talk; main and Datasets and Benchmarks tracks"),
    ("neurips", 2023, "accept (spotlight)", Spotlight, "Spotlight", 11.2, "main and Datasets and Benchmarks tracks"),
    ("neurips", 2023, "accept (poster)", Poster, "Poster", 86.0, ""),
    ("neurips", 2023, "accept", Poster, "Accept", 0.6, "ML Reproducibility Challenge reports"),
    ("neurips", 2023, "accept (outstanding paper)", Poster, "Outstanding paper (Reproducibility Challenge)", 0.0, "an award of the ML Reproducibility Challenge, not of NeurIPS; presented as a poster"),
    ("neurips", 2023, "accept (outstanding paper honorable mention)", Poster, "Honorable mention (Reproducibility Challenge)", 0.0, "an award of the ML Reproducibility Challenge, not of NeurIPS; presented as a poster"),
    ("neurips", 2024, "accept (oral)", Oral, "Oral", 1.6, "presented as a talk; main and Datasets and Benchmarks tracks"),
    ("neurips", 2024, "accept (spotlight)", Spotlight, "Spotlight", 8.4, "main and Datasets and Benchmarks tracks"),
    ("neurips", 2024, "accept (poster)", Poster, "Poster", 89.6, ""),
    ("neurips", 2025, "accept (oral)", Oral, "Oral", 1.6, "presented as a talk; main, Datasets and Benchmarks, and position paper tracks"),
    ("neurips", 2025, "accept (spotlight)", Spotlight, "Spotlight", 12.7, "main and Datasets and Benchmarks tracks"),
    ("neurips", 2025, "accept (poster)", Poster, "Poster", 84.9, ""),
    ("neurips", 2025, "accept", Poster, "Accept", 0.5, "position paper track"),
    ("neurips", 2026, "accept (oral)", Oral, "Oral", 1.5, "presented as a talk; main, Evaluations and Datasets, and position paper tracks"),
    ("neurips", 2026, "accept (spotlight)", Spotlight, "Spotlight", 3.7, "main, Evaluations and Datasets, and position paper tracks"),
    ("neurips", 2026, "accept (poster)", Poster, "Poster", 92.4, ""),
    ("neurips", 2026, "accept", Poster, "Accept", 1.3, "position paper track"),
    ("icml", 2021, "oral", Oral, "Oral", 14.0, "a long talk"),
    ("icml", 2021, "spotlight", Poster, "Spotlight", 86.0, "in 2021 every paper without a long talk gave a short spotlight talk: not singled out"),
    ("icml", 2022, "accept for long presentation; oral", Oral, "Long presentation", 9.6, "a long talk"),
    ("icml", 2022, "accept for short presentation; spotlight", Poster, "Short presentation", 90.4, "every other accepted paper gave a short talk: not singled out"),
    ("icml", 2023, "accept (oral & poster)", Oral, "Oral", 8.3, "presented as a talk and a poster"),
    ("icml", 2023, "accept (poster)", Poster, "Poster", 91.7, ""),
    ("icml", 2024, "accept (oral)", Oral, "Oral", 5.5, "presented as a talk"),
    ("icml", 2024, "accept (spotlight)", Spotlight, "Spotlight", 7.3, "a spotlight poster"),
    ("icml", 2024, "accept (poster)", Poster, "Poster", 87.3, ""),
    ("icml", 2025, "accept (oral)", Oral, "Oral", 3.6, "presented as a talk"),
    ("icml", 2025, "accept (spotlight poster)", Spotlight, "Spotlight poster", 6.7, ""),
    ("icml", 2025, "accept (poster)", Poster, "Poster", 89.7, ""),
    ("icml", 2026, "accept (spotlight); oral", Oral, "Oral", 2.5, "chosen for a talk from among the spotlights"),
    ("icml", 2026, "oral", Oral, "Oral", 0.0, "a talk listed without a decision"),
    ("icml", 2026, "accept (spotlight)", Spotlight, "Spotlight", 6.1, "main and position paper tracks"),
    ("icml", 2026, "accept (regular)", Poster, "Regular", 90.3, ""),
    ("icml", 2026, "accept (poster)", Poster, "Poster", 0.1, "journal-track and other invited posters"),
    ("iclr", 2021, "oral", Oral, "Oral", 6.2, "presented as a talk"),
    ("iclr", 2021, "spotlight", Spotlight, "Spotlight", 13.3, "a short spotlight talk"),
    ("iclr", 2021, "poster", Poster, "Poster", 80.6, ""),
    ("iclr", 2022, "accept (oral)", Oral, "Oral", 5.0, "presented as a talk"),
    ("iclr", 2022, "accept (spotlight)", Spotlight, "Spotlight", 15.9, "a short spotlight talk"),
    ("iclr", 2022, "accept (poster)", Poster, "Poster", 79.1, ""),
    ("iclr", 2023, "accept: notable-top-5%; oral", Oral, "Notable top 5%", 5.7, "the conference's name that year for its orals"),
    ("iclr", 2023, "accept: notable-top-5%", Oral, "Notable top 5%", 5.7, "the conference's name that year for its orals"),
    ("iclr", 2023, "accept: notable-top-25%; oral", Spotlight, "Notable top 25%", 17.7, "the conference's name that year for its spotlights"),
    ("iclr", 2023, "accept: notable-top-25%", Spotlight, "Notable top 25%", 17.7, "the conference's name that year for its spotlights"),
    ("iclr", 2023, "accept: poster", Poster, "Poster", 75.9, ""),
    ("iclr", 2023, "accept (poster)", Poster, "Poster", 0.6, "journal-track and other invited posters"),
    ("iclr", 2024, "accept (oral)", Oral, "Oral", 3.7, "presented as a talk"),
    ("iclr", 2024, "accept (spotlight)", Spotlight, "Spotlight", 16.0, "a spotlight poster"),
    ("iclr", 2024, "accept (poster)", Poster, "Poster", 80.3, ""),
    ("iclr", 2025, "accept (oral)", Oral, "Oral", 5.6, "presented as a talk"),
    ("iclr", 2025, "accept (spotlight)", Spotlight, "Spotlight", 9.9, ""),
    ("iclr", 2025, "accept (poster)", Poster, "Poster", 83.2, ""),
    ("iclr", 2025, "accept", Poster, "Blog post", 1.3, "the blog post track"),
    ("iclr", 2026, "accept (oral)", Oral, "Oral", 4.1, "presented as a talk; there was no spotlight tier that year"),
    ("iclr", 2026, "accept (poster)", Poster, "Poster", 95.9, ""),
    ("aistats", 2021, "oral", Oral, "Oral", 10.5, "presented as a talk"),
    ("aistats", 2021, "poster", Poster, "Poster", 89.5, ""),
    ("aistats", 2022, "accept (oral)", Oral, "Oral", 8.9, "presented as a talk"),
    ("aistats", 2022, "accept (poster)", Poster, "Poster", 91.1, ""),
    ("aistats", 2023, "accept (oral)", Oral, "Oral", 6.5, "presented as a talk"),
    ("aistats", 2023, "accept (poster)", Poster, "Poster", 93.5, ""),
    ("aistats", 2024, "accept (oral)", Oral, "Oral", 8.6, "presented as a talk"),
    ("aistats", 2024, "accept (poster)", Poster, "Poster", 91.4, ""),
    ("aistats", 2025, "accept (oral)", Oral, "Oral", 6.3, "presented as a talk"),
    ("aistats", 2025, "accept (poster)", Poster, "Poster", 92.4, ""),
    ("aistats", 2026, "accept (poster); oral", Oral, "Oral", 1.4, "presented as a talk"),
    ("aistats", 2026, "accept (poster)", Poster, "Poster", 98.6, ""),
    ("mlsys", 2021, "accept; oral", Poster, "Accept", 100.0, "every MLSys paper gives a talk: not singled out"),
    ("mlsys", 2022, "accept; oral", Poster, "Accept", 100.0, "every MLSys paper gives a talk: not singled out"),
    ("mlsys", 2023, "accept", Poster, "Accept", 100.0, ""),
    ("mlsys", 2024, "accept", Poster, "Accept", 89.2, ""),
    ("mlsys", 2024, "conditional accept", Poster, "Conditional accept", 10.8, ""),
    ("mlsys", 2025, "accept", Poster, "Accept", 78.7, ""),
    ("mlsys", 2025, "accept with shepherding", Poster, "Accept with shepherding", 21.3, ""),
    ("mlsys", 2026, "conditional accept; oral", Poster, "Conditional accept", 78.9, "every MLSys paper gives a talk: not singled out"),
    ("cvpr", 2023, "accept", Poster, "Accept", 100.0, "the list does not name 2023's highlights or orals"),
    ("cvpr", 2024, "accept: oral", Oral, "Oral", 3.3, "presented as a talk"),
    ("cvpr", 2024, "accept: poster (highlight)", Spotlight, "Highlight", 11.9, "a poster singled out as a highlight"),
    ("cvpr", 2024, "accept: poster", Poster, "Poster", 84.8, ""),
    ("cvpr", 2025, "oral", Oral, "Oral", 3.3, "presented as a talk; the list does not name 2025's highlights"),
    ("cvpr", 2026, "accept (oral)", Oral, "Oral", 4.0, "presented as a talk"),
    ("cvpr", 2026, "accept (highlight)", Spotlight, "Highlight", 9.0, "a poster singled out as a highlight"),
    ("cvpr", 2026, "accept (poster)", Poster, "Poster", 45.7, ""),
    ("iccv", 2025, "oral", Oral, "Oral", 2.4, "presented as a talk"),
    ("iccv", 2025, "highlight", Spotlight, "Highlight", 9.7, "a poster singled out as a highlight"),
    ("iccv", 2025, "poster", Poster, "Poster", 87.9, ""),
    ("eccv", 2024, "oral", Oral, "Oral", 8.4, "presented as a talk"),
    ("eccv", 2026, "accept poster; oral", Oral, "Oral", 1.0, "presented as a talk"),
    ("eccv", 2026, "accept poster; spotlight", Spotlight, "Spotlight", 4.8, "a short spotlight talk"),
    ("eccv", 2026, "accept poster", Poster, "Poster", 94.2, ""),
    ("wacv", 2026, "accept poster; oral", Oral, "Oral", 10.2, "presented as a talk"),
    ("wacv", 2026, "accept poster", Poster, "Poster", 89.8, ""),
];

/// A word that puts a term never seen before in a tier, best first; anything else is a poster.
const WORDING: [(Acceptance, &[&str]); 2] = [
    (Oral, &["oral", "award", "best paper", "outstanding", "long talk", "long presentation"]),
    (Spotlight, &["spotlight", "highlight", "notable"]),
];

/// The conference's name as the catalogue writes it ("neurips" -> "NeurIPS").
fn conference_name(slug: &str) -> String {
    CONFERENCES
        .iter()
        .flat_map(|(_, venues)| venues.iter())
        .find(|(name, _)| name.eq_ignore_ascii_case(slug))
        .map_or_else(|| slug.to_uppercase(), |(name, _)| (*name).to_owned())
}

/// "11.9% of the papers in its list", or under 0.1%.
fn share(percent: f32) -> String {
    if percent < 0.1 {
        "under 0.1% of the papers in its list".to_owned()
    } else {
        format!("{percent:.1}% of the papers in its list")
    }
}

/// A term never seen, as it is written without "Accept": "Accept (Best Paper)" -> "Best Paper".
fn as_written(decision: &str) -> String {
    let rest = match decision.get(.."accept".len()) {
        Some(start)
            if start.eq_ignore_ascii_case("accept") && !decision[start.len()..].starts_with(char::is_alphabetic) =>
        {
            &decision[start.len()..]
        }
        _ => decision,
    };
    let rest = rest.trim_matches(|c: char| c.is_whitespace() || "():;-".contains(c));
    if rest.is_empty() { decision.trim().to_owned() } else { rest.to_owned() }
}

/// How a decision of a conference's list, in a given year, is read.
pub fn read(slug: &str, year: &str, decision: &str) -> Accepted {
    let decision = decision.trim();
    if decision.is_empty() {
        return Accepted { tier: Poster, term: None };
    }
    let year: u16 = year.parse().unwrap_or(0);
    let conference = format!("{} {year}", conference_name(slug));
    let term = decision.to_lowercase();
    let same = |row: &&Row| row.0 == slug && row.2 == term;
    let found = TERMS
        .iter()
        .filter(same)
        .find(|row| row.1 == year)
        .map(|row| (row, None))
        .or_else(|| TERMS.iter().filter(same).max_by_key(|row| row.1).map(|row| (row, Some(row.1))));
    let worded = WORDING
        .iter()
        .find(|(_, words)| words.iter().any(|word| term.contains(word)))
        .map_or(Poster, |(tier, _)| *tier);
    if let Some(((_, _, _, tier, called, percent, what), borrowed)) = found {
        let mut means = share(*percent);
        if !what.is_empty() {
            means = format!("{means}; {what}");
        }
        if let Some(from) = borrowed {
            means = format!("read as at {} {from}, where it was {means}", conference_name(slug));
        }
        // A borrowed row that ranks the term below what its words say (ICML 2021's "Spotlight", NeurIPS 2023's
        // Reproducibility Challenge award) may have meant that only that year: keep its lower tier, never promote,
        // but collect it for a maintainer to confirm.
        let known = borrowed.is_none() || worded >= *tier;
        let term = Term { conference, called: (*called).to_owned(), means, known };
        return Accepted { tier: *tier, term: Some(term) };
    }
    let tier = worded;
    let means = "not yet in Constellate's table of this conference's terms, so its tier was read from its wording";
    Accepted {
        tier,
        term: Some(Term { conference, called: as_written(decision), means: means.to_owned(), known: false }),
    }
}

/// How this paper was accepted; a conference's decision is kept in the record's comment.
pub fn accepted(paper: &Paper) -> Accepted {
    match paper.origin() {
        Origin::Arxiv => Accepted { tier: Acceptance::NotListed, term: None },
        Origin::Conference { slug, year } => read(slug, year, &paper.comment),
    }
}

/// The tier alone.
pub fn acceptance(paper: &Paper) -> Acceptance {
    accepted(paper).tier
}

/// The decisions no row of [`TERMS`] knows, by conference and decision as written, with how many papers have
/// each: what a run reports so that a maintainer can add them.
pub fn unknown_terms<'a>(papers: impl IntoIterator<Item = &'a Paper>) -> BTreeMap<(String, String), usize> {
    let mut unknown = BTreeMap::new();
    for paper in papers {
        if let Some(Term { conference, known: false, .. }) = accepted(paper).term {
            *unknown.entry((conference, paper.comment.trim().to_owned())).or_insert(0) += 1;
        }
    }
    unknown
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::super::fake;
    use super::*;

    fn paper(id: &str, comment: &str) -> Paper {
        Paper { comment: comment.to_owned(), ..fake::paper(id) }
    }

    /// Every decision seen in the lists (docs/acceptance-tiers.md), as the list writes it, with the tier and the
    /// wording it must be shown with.
    #[rustfmt::skip]
    const SEEN: &[(&str, &str, Acceptance, &str)] = &[
        ("neurips-2021-1", "Accept (Oral)", Oral, "Oral"),
        ("neurips-2021-1", "Accept (Spotlight)", Spotlight, "Spotlight"),
        ("neurips-2021-1", "Accept (Poster)", Poster, "Poster"),
        ("neurips-2022-1", "Accept; Spotlight", Spotlight, "Featured"),
        ("neurips-2022-1", "Spotlight", Spotlight, "Featured"),
        ("neurips-2022-1", "Accept", Poster, "Accept"),
        ("neurips-2023-1", "Accept (oral)", Oral, "Oral"),
        ("neurips-2023-1", "Accept (Oral)", Oral, "Oral"),
        ("neurips-2023-1", "Accept (spotlight)", Spotlight, "Spotlight"),
        ("neurips-2023-1", "Accept (Spotlight)", Spotlight, "Spotlight"),
        ("neurips-2023-1", "Accept (poster)", Poster, "Poster"),
        ("neurips-2023-1", "Accept (Poster)", Poster, "Poster"),
        ("neurips-2023-1", "Accept", Poster, "Accept"),
        ("neurips-2023-1", "Accept (Outstanding Paper)", Poster, "Outstanding paper (Reproducibility Challenge)"),
        ("neurips-2023-1", "Accept (Outstanding Paper Honorable Mention)", Poster, "Honorable mention (Reproducibility Challenge)"),
        ("neurips-2024-1", "Accept (oral)", Oral, "Oral"),
        ("neurips-2024-1", "Accept (Oral)", Oral, "Oral"),
        ("neurips-2024-1", "Accept (spotlight)", Spotlight, "Spotlight"),
        ("neurips-2024-1", "Accept (Spotlight)", Spotlight, "Spotlight"),
        ("neurips-2024-1", "Accept (poster)", Poster, "Poster"),
        ("neurips-2024-1", "Accept (Poster)", Poster, "Poster"),
        ("neurips-2025-1", "Accept (oral)", Oral, "Oral"),
        ("neurips-2025-1", "Accept (Oral)", Oral, "Oral"),
        ("neurips-2025-1", "Accept (spotlight)", Spotlight, "Spotlight"),
        ("neurips-2025-1", "Accept (poster)", Poster, "Poster"),
        ("neurips-2025-1", "Accept (Poster)", Poster, "Poster"),
        ("neurips-2025-1", "Accept", Poster, "Accept"),
        ("neurips-2026-1", "Accept (oral)", Oral, "Oral"),
        ("neurips-2026-1", "Accept (Oral)", Oral, "Oral"),
        ("neurips-2026-1", "Accept (spotlight)", Spotlight, "Spotlight"),
        ("neurips-2026-1", "Accept (Spotlight)", Spotlight, "Spotlight"),
        ("neurips-2026-1", "Accept (poster)", Poster, "Poster"),
        ("neurips-2026-1", "Accept", Poster, "Accept"),
        ("icml-2021-1", "Oral", Oral, "Oral"),
        ("icml-2021-1", "Spotlight", Poster, "Spotlight"),
        ("icml-2022-1", "Accept for Long Presentation; Oral", Oral, "Long presentation"),
        ("icml-2022-1", "Accept for Short Presentation; Spotlight", Poster, "Short presentation"),
        ("icml-2023-1", "Accept (Oral & Poster)", Oral, "Oral"),
        ("icml-2023-1", "Accept (Poster)", Poster, "Poster"),
        ("icml-2024-1", "Accept (Oral)", Oral, "Oral"),
        ("icml-2024-1", "Accept (Spotlight)", Spotlight, "Spotlight"),
        ("icml-2024-1", "Accept (Poster)", Poster, "Poster"),
        ("icml-2025-1", "Accept (oral)", Oral, "Oral"),
        ("icml-2025-1", "Accept (spotlight poster)", Spotlight, "Spotlight poster"),
        ("icml-2025-1", "Accept (poster)", Poster, "Poster"),
        ("icml-2025-1", "Accept (Poster)", Poster, "Poster"),
        ("icml-2026-1", "Accept (spotlight); Oral", Oral, "Oral"),
        ("icml-2026-1", "Oral", Oral, "Oral"),
        ("icml-2026-1", "Accept (spotlight)", Spotlight, "Spotlight"),
        ("icml-2026-1", "Accept (regular)", Poster, "Regular"),
        ("icml-2026-1", "Accept (Poster)", Poster, "Poster"),
        ("iclr-2021-1", "Oral", Oral, "Oral"),
        ("iclr-2021-1", "Spotlight", Spotlight, "Spotlight"),
        ("iclr-2021-1", "Poster", Poster, "Poster"),
        ("iclr-2022-1", "Accept (Oral)", Oral, "Oral"),
        ("iclr-2022-1", "Accept (Spotlight)", Spotlight, "Spotlight"),
        ("iclr-2022-1", "Accept (Poster)", Poster, "Poster"),
        ("iclr-2023-1", "Accept: notable-top-5%; Oral", Oral, "Notable top 5%"),
        ("iclr-2023-1", "Accept: notable-top-5%", Oral, "Notable top 5%"),
        ("iclr-2023-1", "Accept: notable-top-25%; Oral", Spotlight, "Notable top 25%"),
        ("iclr-2023-1", "Accept: notable-top-25%", Spotlight, "Notable top 25%"),
        ("iclr-2023-1", "Accept: poster", Poster, "Poster"),
        ("iclr-2023-1", "Accept (Poster)", Poster, "Poster"),
        ("iclr-2024-1", "Accept (oral)", Oral, "Oral"),
        ("iclr-2024-1", "Accept (spotlight)", Spotlight, "Spotlight"),
        ("iclr-2024-1", "Accept (poster)", Poster, "Poster"),
        ("iclr-2024-1", "Accept (Poster)", Poster, "Poster"),
        ("iclr-2025-1", "Accept (Oral)", Oral, "Oral"),
        ("iclr-2025-1", "Accept (Spotlight)", Spotlight, "Spotlight"),
        ("iclr-2025-1", "Accept (Poster)", Poster, "Poster"),
        ("iclr-2025-1", "Accept", Poster, "Blog post"),
        ("iclr-2026-1", "Accept (Oral)", Oral, "Oral"),
        ("iclr-2026-1", "Accept (Poster)", Poster, "Poster"),
        ("aistats-2021-1", "Oral", Oral, "Oral"),
        ("aistats-2021-1", "Poster", Poster, "Poster"),
        ("aistats-2022-1", "Accept (Oral)", Oral, "Oral"),
        ("aistats-2022-1", "Accept (Poster)", Poster, "Poster"),
        ("aistats-2023-1", "Accept (Oral)", Oral, "Oral"),
        ("aistats-2023-1", "Accept (Poster)", Poster, "Poster"),
        ("aistats-2024-1", "Accept (Oral)", Oral, "Oral"),
        ("aistats-2024-1", "Accept (Poster)", Poster, "Poster"),
        ("aistats-2025-1", "Accept (Oral)", Oral, "Oral"),
        ("aistats-2025-1", "Accept (Poster)", Poster, "Poster"),
        ("aistats-2026-1", "Accept (Poster); Oral", Oral, "Oral"),
        ("aistats-2026-1", "Accept (Poster)", Poster, "Poster"),
        ("mlsys-2021-1", "Accept; Oral", Poster, "Accept"),
        ("mlsys-2022-1", "Accept; Oral", Poster, "Accept"),
        ("mlsys-2023-1", "Accept", Poster, "Accept"),
        ("mlsys-2024-1", "Accept", Poster, "Accept"),
        ("mlsys-2024-1", "Conditional Accept", Poster, "Conditional accept"),
        ("mlsys-2025-1", "Accept", Poster, "Accept"),
        ("mlsys-2025-1", "Accept with shepherding", Poster, "Accept with shepherding"),
        ("mlsys-2026-1", "Conditional Accept; Oral", Poster, "Conditional accept"),
        ("cvpr-2023-1", "Accept", Poster, "Accept"),
        ("cvpr-2024-1", "Accept: Oral", Oral, "Oral"),
        ("cvpr-2024-1", "Accept: Poster (Highlight)", Spotlight, "Highlight"),
        ("cvpr-2024-1", "Accept: Poster", Poster, "Poster"),
        ("cvpr-2025-1", "Oral", Oral, "Oral"),
        ("cvpr-2026-1", "Accept (Oral)", Oral, "Oral"),
        ("cvpr-2026-1", "Accept (Highlight)", Spotlight, "Highlight"),
        ("cvpr-2026-1", "Accept (Poster)", Poster, "Poster"),
        ("iccv-2025-1", "oral", Oral, "Oral"),
        ("iccv-2025-1", "highlight", Spotlight, "Highlight"),
        ("iccv-2025-1", "poster", Poster, "Poster"),
        ("eccv-2024-1", "Oral", Oral, "Oral"),
        ("eccv-2026-1", "Accept Poster; Oral", Oral, "Oral"),
        ("eccv-2026-1", "Accept Poster; Spotlight", Spotlight, "Spotlight"),
        ("eccv-2026-1", "Accept Poster", Poster, "Poster"),
        ("wacv-2026-1", "Accept Poster; Oral", Oral, "Oral"),
        ("wacv-2026-1", "Accept Poster", Poster, "Poster"),
    ];

    #[test]
    fn every_decision_seen_in_the_lists_is_read_as_its_conference_meant_it() {
        for (id, decision, tier, called) in SEEN {
            let read = accepted(&paper(id, decision));
            let term = read.term.unwrap();
            assert_eq!((read.tier, term.called.as_str(), term.known), (*tier, *called, true), "{id} {decision:?}");
        }
        assert!(
            unknown_terms(SEEN.iter().map(|(id, decision, ..)| paper(id, decision)).collect::<Vec<_>>().iter())
                .is_empty()
        );
    }

    #[test]
    fn the_same_word_means_different_tiers_at_different_conferences_and_years() {
        // "Spotlight" was ICML 2021's ordinary acceptance, and ICLR 2021's spotlight
        assert_eq!(acceptance(&paper("icml-2021-1", "Spotlight")), Poster);
        assert_eq!(acceptance(&paper("iclr-2021-1", "Spotlight")), Spotlight);
        // ICML 2026 named its orals only by their talk; MLSys gives every paper one
        assert_eq!(acceptance(&paper("icml-2026-1", "Accept (spotlight); Oral")), Oral);
        assert_eq!(acceptance(&paper("mlsys-2026-1", "Conditional Accept; Oral")), Poster);
    }

    #[test]
    fn the_hover_text_says_what_share_the_term_was_and_what_it_is() {
        let term = accepted(&paper("cvpr-2024-7", "Accept: Poster (Highlight)")).term.unwrap();
        assert_eq!(term.conference, "CVPR 2024");
        assert_eq!(term.called, "Highlight");
        assert_eq!(term.means, "11.9% of the papers in its list; a poster singled out as a highlight");
        let iclr = accepted(&paper("iclr-2023-1", "Accept: notable-top-5%")).term.unwrap();
        assert_eq!((iclr.called.as_str(), iclr.conference.as_str()), ("Notable top 5%", "ICLR 2023"));
    }

    #[test]
    fn a_year_not_in_the_table_borrows_the_latest_year_that_used_the_term() {
        let read = accepted(&paper("neurips-2027-1", "Accept (oral)"));
        assert_eq!(read.tier, Oral);
        let term = read.term.unwrap();
        assert!(term.known);
        assert_eq!(term.conference, "NeurIPS 2027");
        assert!(term.means.starts_with("read as at NeurIPS 2026, where it was 1.5% of the papers"), "{}", term.means);
        // a borrowed row lower than the term's words is kept, not promoted, and collected to be confirmed
        for (id, decision) in [("icml-2027-1", "Spotlight"), ("neurips-2027-1", "Accept (Outstanding Paper)")] {
            let read = accepted(&paper(id, decision));
            assert_eq!((read.tier, read.term.unwrap().known), (Poster, false), "{decision}");
        }
        assert!(accepted(&paper("mlsys-2027-1", "Accept")).term.unwrap().known);
    }

    #[test]
    fn a_term_never_seen_is_read_from_its_wording_and_collected() {
        for (decision, tier, called) in [
            ("Accept (Best Paper)", Oral, "Best Paper"),
            ("Award Candidate", Oral, "Award Candidate"),
            ("Accept: Outstanding Paper Award", Oral, "Outstanding Paper Award"),
            ("Accept (Highlight Paper)", Spotlight, "Highlight Paper"),
            ("Accept (regular)", Poster, "regular"),
            ("Accepted", Poster, "Accepted"),
            ("Weird new thing", Poster, "Weird new thing"),
        ] {
            let read = accepted(&paper("cvpr-2027-1", decision));
            let term = read.term.unwrap();
            assert_eq!((read.tier, term.called.as_str(), term.known), (tier, called, false), "{decision:?}");
            assert!(term.means.starts_with("not yet in Constellate's table"));
        }
        // a conference the table has never seen at all is read the same way
        assert_eq!(accepted(&paper("newconf-2026-1", "Accept (oral)")).tier, Oral);
        let papers = [
            paper("cvpr-2027-1", "Award Candidate"),
            paper("cvpr-2027-2", "Award Candidate"),
            paper("cvpr-2024-3", "Accept: Oral"),
        ];
        let unknown = unknown_terms(&papers);
        assert_eq!(
            unknown.into_iter().collect::<Vec<_>>(),
            [(("CVPR 2027".to_owned(), "Award Candidate".to_owned()), 2)]
        );
    }

    #[test]
    fn no_decision_is_a_poster_with_no_term() {
        assert_eq!(accepted(&paper("eccv-2024-1", "")), Accepted { tier: Poster, term: None });
        assert_eq!(accepted(&paper("eccv-2024-1", "Oral")).tier, Oral);
    }

    #[test]
    fn an_arxiv_paper_has_no_acceptance_state() {
        let read = accepted(&paper("2512.01234", "Accepted as an oral at NeurIPS 2025"));
        assert_eq!(read, Accepted { tier: Acceptance::NotListed, term: None });
    }

    #[test]
    fn the_interface_gets_the_tiers_name_or_null_and_the_term_as_an_object() {
        assert_eq!(serde_json::to_value(Spotlight).unwrap(), "spotlight");
        assert_eq!(serde_json::to_value(Acceptance::NotListed).unwrap(), serde_json::Value::Null);
        assert_eq!(Acceptance::TIERS.map(Acceptance::name), [Some("oral"), Some("spotlight"), Some("poster")]);
        let term = accepted(&paper("iccv-2025-1", "highlight")).term.unwrap();
        assert_eq!(
            serde_json::to_value(term).unwrap(),
            serde_json::json!({"conference": "ICCV 2025", "called": "Highlight",
                "means": "9.7% of the papers in its list; a poster singled out as a highlight", "known": true})
        );
    }

    #[test]
    fn each_tier_lists_the_terms_it_includes() {
        let spotlights = Spotlight.equivalents();
        for called in ["Spotlight", "Highlight", "Notable top 25%", "Spotlight poster", "Featured"] {
            assert!(spotlights.contains(&called), "{called}");
        }
        assert!(Oral.equivalents().contains(&"Long presentation"));
        assert!(!spotlights.contains(&"Oral"));
    }

    #[test]
    fn the_table_has_one_row_per_term_and_shares_that_are_percentages() {
        let mut keys: Vec<(&str, u16, &str)> = TERMS.iter().map(|row| (row.0, row.1, row.2)).collect();
        let all = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), all);
        assert!(TERMS.iter().all(|row| (0.0..=100.0).contains(&row.5) && row.2 == row.2.to_lowercase()));
    }
}
