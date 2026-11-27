//! The conferences and arXiv categories a run can be about, grouped by field, as the run form's menus show them.
//!
//! Conferences come in two kinds, and the difference matters when comparing them:
//!
//! ```text
//!   official list       the conference's own site publishes every accepted paper (see conferences.rs). Complete.
//!   arXiv comments      no such list; papers are found by the conference's name in arXiv comments
//!                       ("Accepted at ACL 2025"). Only the papers that are on arXiv and say so: a partial,
//!                       self-selected sample.
//! ```
//!
//! Fields outside computer science publish in journals, not conference proceedings; for those the arXiv
//! categories (used with a date range) are the real source. Chemistry has no conference here for that reason.
//!
//! The other menus (models, clustering methods, acceptance filters, declaration flags) belong to the modules that
//! define those things, and are added to the interface's catalogue there.

use serde_json::{Value, json};

/// Where a conference's accepted papers are found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum List {
    /// The host of the site that publishes the complete list.
    Official(&'static str),
    /// No list anywhere: arXiv papers whose comment names the conference.
    FromArxivComments,
}

use List::{FromArxivComments as ArxivOnly, Official};

/// Field, then each conference's name and where its papers are listed.
pub const CONFERENCES: &[(&str, &[(&str, List)])] = &[
    (
        "Machine learning",
        &[
            ("NeurIPS", Official("neurips.cc")),
            ("ICML", Official("icml.cc")),
            ("ICLR", Official("iclr.cc")),
            ("AISTATS", Official("virtual.aistats.org")),
            ("MLSys", Official("mlsys.org")),
            ("AAAI", ArxivOnly),
            ("IJCAI", ArxivOnly),
            ("UAI", ArxivOnly),
            ("COLT", ArxivOnly),
            ("KDD", ArxivOnly),
            ("SIGIR", ArxivOnly),
        ],
    ),
    (
        "Computer vision",
        &[
            ("CVPR", Official("cvpr.thecvf.com")),
            ("ICCV", Official("iccv.thecvf.com")),
            ("ECCV", Official("eccv.ecva.net")),
            ("WACV", Official("wacv.thecvf.com")),
            ("BMVC", ArxivOnly),
        ],
    ),
    (
        "Language and speech",
        &[
            ("ACL", ArxivOnly),
            ("EMNLP", ArxivOnly),
            ("NAACL", ArxivOnly),
            ("COLING", ArxivOnly),
            ("Interspeech", ArxivOnly),
            ("ICASSP", ArxivOnly),
        ],
    ),
    ("Robotics", &[("ICRA", ArxivOnly), ("IROS", ArxivOnly), ("CoRL", ArxivOnly), ("RSS", ArxivOnly)]),
    ("Medicine", &[("MICCAI", ArxivOnly), ("MIDL", ArxivOnly)]),
    ("Physics", &[("ICRC", ArxivOnly), ("Quark Matter", ArxivOnly), ("Lattice", ArxivOnly), ("ICHEP", ArxivOnly)]),
];

/// Field, then each arXiv category and what it is.
pub const CATEGORIES: &[(&str, &[(&str, &str)])] = &[
    (
        "Machine learning",
        &[
            ("cs.LG", "Machine learning"),
            ("stat.ML", "Statistical ML"),
            ("cs.AI", "Artificial intelligence"),
            ("cs.NE", "Neural and evolutionary computing"),
        ],
    ),
    (
        "Vision, language, robotics",
        &[
            ("cs.CV", "Computer vision"),
            ("cs.CL", "Computation and language"),
            ("cs.RO", "Robotics"),
            ("eess.AS", "Audio and speech"),
            ("cs.IR", "Information retrieval"),
        ],
    ),
    (
        "Chemistry and materials",
        &[
            ("physics.chem-ph", "Chemical physics"),
            ("cond-mat.mtrl-sci", "Materials science"),
            ("cond-mat.soft", "Soft condensed matter"),
            ("q-bio.BM", "Biomolecules"),
        ],
    ),
    (
        "Physics",
        &[
            ("quant-ph", "Quantum physics"),
            ("hep-th", "High energy theory"),
            ("hep-ph", "High energy phenomenology"),
            ("cond-mat.str-el", "Strongly correlated electrons"),
            ("cond-mat.stat-mech", "Statistical mechanics"),
            ("gr-qc", "General relativity"),
            ("astro-ph.CO", "Cosmology"),
            ("physics.comp-ph", "Computational physics"),
        ],
    ),
    (
        "Biology and medicine",
        &[
            ("q-bio.QM", "Quantitative methods"),
            ("q-bio.GN", "Genomics"),
            ("q-bio.NC", "Neurons and cognition"),
            ("eess.IV", "Image and video processing"),
            ("physics.med-ph", "Medical physics"),
        ],
    ),
    (
        "Mathematics and statistics",
        &[
            ("math.OC", "Optimization and control"),
            ("math.ST", "Statistics theory"),
            ("stat.ME", "Methodology"),
            ("math.PR", "Probability"),
            ("cs.IT", "Information theory"),
        ],
    ),
];

/// Where this conference's papers are listed, whatever case its name is written in; a conference not in the
/// catalogue has no known list, so it is looked for in arXiv comments.
pub fn list_of(conference: &str) -> List {
    CONFERENCES
        .iter()
        .flat_map(|(_, venues)| venues.iter())
        .find(|(name, _)| name.eq_ignore_ascii_case(conference))
        .map_or(List::FromArxivComments, |(_, list)| *list)
}

/// The site that publishes this conference's accepted papers, if there is one.
pub fn official_host(conference: &str) -> Option<&'static str> {
    match list_of(conference) {
        List::Official(host) => Some(host),
        List::FromArxivComments => None,
    }
}

/// The `conferences` and `categories` of the interface's `Catalog` (packages/contracts): groups of items, each
/// with a note that says how complete a conference's papers are.
pub fn as_json() -> Value {
    let conferences: Vec<Value> = CONFERENCES
        .iter()
        .map(|(field, venues)| {
            let items: Vec<Value> = venues
                .iter()
                .map(|(name, list)| {
                    let note = match list {
                        List::Official(_) => "official list",
                        List::FromArxivComments => "from arXiv comments: partial",
                    };
                    json!({"name": name, "note": note})
                })
                .collect();
            json!({"field": field, "items": items})
        })
        .collect();
    let categories: Vec<Value> = CATEGORIES
        .iter()
        .map(|(field, categories)| {
            let items: Vec<Value> = categories.iter().map(|(name, what)| json!({"name": name, "note": what})).collect();
            json!({"field": field, "items": items})
        })
        .collect();
    json!({"conferences": conferences, "categories": categories})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conferences_list_is_found_whatever_case_its_name_is_written_in() {
        assert_eq!(official_host("neurips"), Some("neurips.cc"));
        assert_eq!(official_host("CVPR"), Some("cvpr.thecvf.com"));
        assert_eq!(official_host("ACL"), None);
        assert_eq!(list_of("Some Workshop"), List::FromArxivComments);
    }

    #[test]
    fn the_interface_gets_groups_of_named_items_with_notes() {
        let catalog = as_json();
        assert_eq!(catalog["conferences"][0]["field"], "Machine learning");
        assert_eq!(catalog["conferences"][0]["items"][0], json!({"name": "NeurIPS", "note": "official list"}));
        assert_eq!(
            catalog["conferences"][0]["items"][5],
            json!({"name": "AAAI", "note": "from arXiv comments: partial"})
        );
        assert_eq!(catalog["categories"][3]["items"][0], json!({"name": "quant-ph", "note": "Quantum physics"}));
        let count = |key: &str| -> usize {
            catalog[key]
                .as_array()
                .into_iter()
                .flatten()
                .map(|group| group["items"].as_array().map_or(0, Vec::len))
                .sum()
        };
        assert_eq!((count("conferences"), count("categories")), (32, 31));
    }

    #[test]
    fn no_conference_is_listed_twice() {
        let mut names: Vec<String> =
            CONFERENCES.iter().flat_map(|(_, venues)| venues.iter()).map(|(name, _)| name.to_lowercase()).collect();
        let all = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all);
    }
}
