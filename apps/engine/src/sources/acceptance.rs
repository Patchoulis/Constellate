//! How a conference accepted a paper, in tiers that hold across conferences.
//!
//! Conferences word their decisions differently: "Accept (oral)", "Accept (spotlight)", "Accept (regular)",
//! "Highlight", "Award Candidate", plain "Accept". Anything not singled out is an ordinary acceptance, a poster.
//! A paper that did not come from a conference's list (an arXiv paper, even one whose comment says "oral") was
//! not judged by anyone here, and says so rather than borrowing a tier.

use serde::{Serialize, Serializer};

use super::{Origin, Paper};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Acceptance {
    Oral,
    Spotlight,
    Poster,
    /// Not from a conference's list, so there is no decision to read.
    NotListed,
}

impl Acceptance {
    /// The tiers a conference paper can be in, best first.
    pub const TIERS: [Self; 3] = [Self::Oral, Self::Spotlight, Self::Poster];

    /// The tier a conference's decision puts a paper in.
    pub fn from_decision(decision: &str) -> Self {
        let decision = decision.to_lowercase();
        let says = |words: &[&str]| words.iter().any(|word| decision.contains(word));
        if says(&["oral", "award", "best paper"]) {
            Self::Oral
        } else if says(&["spotlight", "highlight"]) {
            Self::Spotlight
        } else {
            Self::Poster
        }
    }

    /// The name the interface knows (`Acceptance` in packages/contracts), or none for a paper not listed.
    pub fn name(self) -> Option<&'static str> {
        match self {
            Self::Oral => Some("oral"),
            Self::Spotlight => Some("spotlight"),
            Self::Poster => Some("poster"),
            Self::NotListed => None,
        }
    }
}

/// How this paper was accepted; a conference's decision is kept in the record's comment.
pub fn acceptance(paper: &Paper) -> Acceptance {
    match paper.origin() {
        Origin::Arxiv => Acceptance::NotListed,
        Origin::Conference { .. } => Acceptance::from_decision(&paper.comment),
    }
}

/// As the interface reads it: the tier's name, or `null`.
impl Serialize for Acceptance {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.name().serialize(serializer)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::super::fake;
    use super::*;

    fn paper(id: &str, comment: &str) -> Paper {
        Paper { comment: comment.to_owned(), ..fake::paper(id) }
    }

    #[test]
    fn acceptance_is_read_the_same_whatever_the_conference_calls_it() {
        for (decision, tier) in [
            ("Accept (oral)", Acceptance::Oral),
            ("Accept (Oral)", Acceptance::Oral),
            ("Award Candidate", Acceptance::Oral),
            ("Best Paper Honorable Mention", Acceptance::Oral),
            ("Accept (spotlight)", Acceptance::Spotlight),
            ("Highlight", Acceptance::Spotlight),
            ("Accept (poster)", Acceptance::Poster),
            ("Accept (regular)", Acceptance::Poster),
            ("Accept", Acceptance::Poster),
            ("", Acceptance::Poster),
        ] {
            assert_eq!(acceptance(&paper("cvpr-2026-123", decision)), tier, "{decision:?}");
        }
    }

    #[test]
    fn an_arxiv_paper_has_no_acceptance_state() {
        assert_eq!(acceptance(&paper("2512.01234", "Accepted as an oral at NeurIPS 2025")), Acceptance::NotListed);
    }

    #[test]
    fn the_interface_gets_the_tiers_name_or_null() {
        assert_eq!(serde_json::to_value(Acceptance::Spotlight).unwrap(), "spotlight");
        assert_eq!(serde_json::to_value(Acceptance::NotListed).unwrap(), serde_json::Value::Null);
        assert_eq!(Acceptance::TIERS.map(Acceptance::name), [Some("oral"), Some("spotlight"), Some("poster")]);
    }
}
