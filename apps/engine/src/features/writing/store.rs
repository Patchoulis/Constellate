//! What a model wrote about a community: its name and description, and its overview.
//!
//! Both are paid for, so each is written once: they are kept under [`community_key`], the hash of the model and
//! the exact set of the community's papers, the key the Python used (`describe.py` and `overview.py`). A community
//! that gains or loses one paper is a different community and is written about again; the same papers found
//! again in another run are not.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::library::{Library, LibraryError, json_column, now};

/// `sha1(model + "|" + ",".join(sorted(paper ids)))` in hex: the same key for the same model and papers in any
/// order.
pub fn community_key<'a>(model: &str, paper_ids: impl IntoIterator<Item = &'a str>) -> String {
    let mut ids: Vec<&str> = paper_ids.into_iter().collect();
    ids.sort_unstable();
    let text = format!("{model}|{}", ids.join(","));
    Sha1::digest(text.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A community's name and what it is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Description {
    pub name: String,
    pub description: String,
}

/// A paper cited by a point of an overview.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitedPaper {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub title: String,
    pub insight: String,
    pub papers: Vec<CitedPaper>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub heading: String,
    pub points: Vec<Point>,
}

/// An overview of one community, as `overview.py` keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overview {
    pub title: String,
    pub summary: String,
    pub sections: Vec<Section>,
    /// The model that wrote it.
    pub model: String,
    /// How many papers the community had.
    pub papers: usize,
}

pub(crate) fn put_description(
    connection: &Connection,
    key: &str,
    description: &Description,
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO descriptions (community_key, name, description, written_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (community_key) DO UPDATE SET
             name = excluded.name, description = excluded.description, written_at = excluded.written_at",
        params![key, description.name, description.description, now()],
    )
}

pub(crate) fn put_overview(connection: &Connection, key: &str, overview: &Overview) -> Result<usize, LibraryError> {
    Ok(connection.execute(
        "INSERT INTO overviews (community_key, model, content, written_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (community_key) DO UPDATE SET
             model = excluded.model, content = excluded.content, written_at = excluded.written_at",
        params![key, overview.model, serde_json::to_string(overview)?, now()],
    )?)
}

impl Library {
    pub fn description(&self, key: &str) -> Result<Option<Description>, LibraryError> {
        self.read(|connection| {
            Ok(connection
                .query_row("SELECT name, description FROM descriptions WHERE community_key = ?1", [key], |row| {
                    Ok(Description { name: row.get(0)?, description: row.get(1)? })
                })
                .optional()?)
        })
    }

    pub fn keep_description(&self, key: &str, description: &Description) -> Result<(), LibraryError> {
        self.write(|transaction| Ok(put_description(transaction, key, description).map(drop)?))
    }

    pub fn overview(&self, key: &str) -> Result<Option<Overview>, LibraryError> {
        self.read(|connection| {
            Ok(connection
                .query_row("SELECT content FROM overviews WHERE community_key = ?1", [key], |row| json_column(row, 0))
                .optional()?)
        })
    }

    pub fn keep_overview(&self, key: &str, overview: &Overview) -> Result<(), LibraryError> {
        self.write(|transaction| put_overview(transaction, key, overview).map(drop))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn overview() -> Overview {
        Overview {
            title: "Verifiers replace human labels".into(),
            summary: "Most papers train against a checker.".into(),
            sections: vec![Section {
                heading: "Common methods".into(),
                points: vec![Point {
                    title: "Reward from verifier feedback".into(),
                    insight: "Three papers do it.".into(),
                    papers: vec![CitedPaper { id: "2603.00001".into(), title: "One".into() }],
                }],
            }],
            model: "claude-opus-5-5".into(),
            papers: 120,
        }
    }

    #[test]
    fn the_community_key_is_the_pythons_and_ignores_order() {
        // what describe.py and overview.py compute for these papers
        let key = community_key("claude-opus-5-5", ["2603.00002", "icml-2026-3", "2603.00001"]);
        assert_eq!(key, "cbc5f2238a5a430d5b98a842600a5c0028552694");
        assert_ne!(key, community_key("claude-haiku-5-5", ["2603.00002", "icml-2026-3", "2603.00001"]));
        assert_ne!(key, community_key("claude-opus-5-5", ["2603.00002", "icml-2026-3"]));
    }

    #[test]
    fn descriptions_and_overviews_are_read_back_by_their_key() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let key = community_key("claude-opus-5-5", ["a", "b", "c"]);
        assert_eq!(library.description(&key).unwrap(), None);
        let described = Description { name: "Verifier rewards".into(), description: "Papers on ...".into() };
        library.keep_description(&key, &described).unwrap();
        assert_eq!(library.description(&key).unwrap(), Some(described));
        assert_eq!(library.overview(&key).unwrap(), None);
        library.keep_overview(&key, &overview()).unwrap();
        assert_eq!(library.overview(&key).unwrap(), Some(overview()));
    }
}
