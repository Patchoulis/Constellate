//! What the library keeps for a paper's PDF on demand ([`crate::papers`]): each conference's index of where its
//! proceedings keep their PDFs, and which PDF a paper's declarations were read from.
//!
//! An index is kept as the [`IndexState`] it is in, as JSON, by conference and year (`neurips-2025`), so that the
//! next start knows when it was fetched and does not fetch it again for [`crate::sources::proceedings::INDEX_DAYS`]
//! (or, after a failure, for [`crate::sources::proceedings::RETRY`]).
//!
//! A reading of declarations is valid for one version of the reader and one PDF: it is kept with the PDF's
//! SHA-256, and one kept without (moved in from the Python backend, which did not keep the PDFs it read) is taken
//! as still valid, since it was read from the publisher's same PDF by the same version of the reading.

use std::collections::HashMap;

use rusqlite::{OptionalExtension, params};

use super::readings::Reading;
use crate::library::{Library, LibraryError, json_column, now, placeholders};
use crate::sources::proceedings::IndexState;

/// Papers asked about in one statement: well under SQLite's limit on bound values.
const IDS_AT_ONCE: usize = 500;

/// A kept reading of a paper's declarations, with the PDF it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeptDeclarations {
    pub reading: Reading,
    /// The SHA-256 of the PDF read; `None` for a reading moved in from the Python backend.
    pub pdf_hash: Option<String>,
}

impl KeptDeclarations {
    /// Is this still the reading of the PDF with `hash`?
    pub fn is_of(&self, hash: &str) -> bool {
        self.pdf_hash.as_deref().is_none_or(|kept| kept == hash)
    }
}

fn kept_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<KeptDeclarations> {
    let outcome: String = row.get(0)?;
    let reading = match outcome.as_str() {
        "no_pdf" => Reading::NoPdf,
        "unreadable" => Reading::Unreadable,
        _ => Reading::Read(json_column(row, 1)?),
    };
    Ok(KeptDeclarations { reading, pdf_hash: row.get(2)? })
}

impl Library {
    /// The index of `conference` ("neurips-2025") as last kept; never fetched when there is none.
    pub fn proceedings_index(&self, conference: &str) -> Result<IndexState, LibraryError> {
        self.read(|connection| {
            let kept: Option<String> = connection
                .query_row("SELECT state FROM proceedings_indexes WHERE conference = ?1", [conference], |row| {
                    row.get(0)
                })
                .optional()?;
            // a state that cannot be read back is as good as none: the index is fetched again
            Ok(kept.and_then(|state| serde_json::from_str(&state).ok()).unwrap_or(IndexState::NeverFetched))
        })
    }

    pub fn keep_proceedings_index(&self, conference: &str, state: &IndexState) -> Result<(), LibraryError> {
        let state = serde_json::to_string(state)?;
        self.write(|transaction| {
            transaction.execute(
                "INSERT INTO proceedings_indexes (conference, state, changed_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (conference) DO UPDATE SET state = excluded.state, changed_at = excluded.changed_at",
                params![conference, state, now()],
            )?;
            Ok(())
        })
    }

    /// What `reader` found in each of `paper_ids` that it has read, with the PDF it read.
    pub fn kept_declarations(
        &self,
        paper_ids: &[&str],
        reader: u32,
    ) -> Result<HashMap<String, KeptDeclarations>, LibraryError> {
        self.read(|connection| {
            let mut found = HashMap::new();
            for ids in paper_ids.chunks(IDS_AT_ONCE) {
                let mut statement = connection.prepare(&format!(
                    "SELECT outcome, content, pdf_hash, paper_id FROM declarations
                     WHERE reader = ?1 AND paper_id IN ({})",
                    placeholders(ids.len())
                ))?;
                let values = std::iter::once(&reader as &dyn rusqlite::ToSql)
                    .chain(ids.iter().map(|id| id as &dyn rusqlite::ToSql));
                let rows = statement
                    .query_map(rusqlite::params_from_iter(values), |row| Ok((row.get(3)?, kept_from_row(row)?)))?;
                for row in rows {
                    let (id, kept) = row?;
                    found.insert(id, kept);
                }
            }
            Ok(found)
        })
    }

    /// Keep what `reader` found in several papers, each with the hash of the PDF it read, in one transaction.
    pub fn keep_declarations_of(&self, reader: u32, readings: &[(&str, &str, Reading)]) -> Result<(), LibraryError> {
        self.write(|transaction| {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO declarations (paper_id, reader, outcome, content, read_at, pdf_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (paper_id) DO UPDATE SET
                     reader = excluded.reader, outcome = excluded.outcome, content = excluded.content,
                     read_at = excluded.read_at, pdf_hash = excluded.pdf_hash",
            )?;
            for (paper_id, hash, reading) in readings {
                let (outcome, content) = match reading {
                    Reading::Read(declarations) => ("read", Some(serde_json::to_string(declarations)?)),
                    Reading::NoPdf => ("no_pdf", None),
                    Reading::Unreadable => ("unreadable", None),
                };
                statement.execute(params![paper_id, reader, outcome, content, now(), hash])?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::library::papers::tests::paper;
    use crate::library::{Declarations, Known};
    use crate::sources::proceedings::by_title;

    fn library_with(ids: &[&str]) -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        library.add_papers(&ids.iter().map(|id| paper(id)).collect::<Vec<_>>(), None, Known::Keep).unwrap();
        (folder, library)
    }

    #[test]
    fn an_index_is_kept_as_the_state_it_is_in_and_none_is_never_fetched() {
        let (_folder, library) = library_with(&[]);
        assert_eq!(library.proceedings_index("neurips-2025").unwrap(), IndexState::NeverFetched);
        let titles = by_title([("A Great Paper".to_owned(), "https://proceedings.neurips.cc/a.pdf".to_owned())]);
        let state = IndexState::NeverFetched.after(Utc.with_ymd_and_hms(2026, 11, 26, 12, 0, 0).unwrap(), Ok(titles));
        library.keep_proceedings_index("neurips-2025", &state).unwrap();
        assert_eq!(library.proceedings_index("neurips-2025").unwrap(), state);
        assert_eq!(library.proceedings_index("icml-2025").unwrap(), IndexState::NeverFetched);
    }

    #[test]
    fn a_reading_is_kept_with_its_pdf_and_one_moved_in_without_counts_for_any() {
        let (_folder, library) = library_with(&["a", "b"]);
        let read =
            Reading::Read(Declarations { checklist: vec![], statements: vec![], flags: vec!["closed_code".into()] });
        library.keep_declarations_of(2, &[("a", "hash-a", read.clone())]).unwrap();
        library.keep_declarations(2, &[("b", read.clone())]).unwrap(); // as the import keeps them
        let kept = library.kept_declarations(&["a", "b", "c"], 2).unwrap();
        assert_eq!(kept.len(), 2);
        assert!(kept["a"].is_of("hash-a") && !kept["a"].is_of("hash-other"));
        assert!(kept["b"].is_of("anything"), "moved in from the Python backend");
        assert_eq!(kept["a"].reading, read);
        assert!(library.kept_declarations(&["a"], 3).unwrap().is_empty(), "another version of the reader");
    }
}
