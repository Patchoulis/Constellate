//! The columns of a run's extraction table ([`crate::features::extraction`]), kept so that the questions a user
//! wrote survive a restart.
//!
//! They are not a model's answer, so they are not kept with the answers ([`crate::library::money::answers`]): an answer's key
//! changes with the passages reader's version, and a user's columns must not be lost when it does. One row per
//! run, the columns as JSON in their order.

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::library::{Library, LibraryError, now};

impl Library {
    /// The columns kept for `run`, if any were ever saved. Columns that no longer read as `T` count as none
    /// (the table then starts from its defaults again).
    pub fn extraction_columns<T: DeserializeOwned>(&self, run: &str) -> Result<Option<T>, LibraryError> {
        let content: Option<String> = self.read(|connection| {
            Ok(connection
                .prepare_cached("SELECT columns FROM extraction_columns WHERE run = ?1")?
                .query_row([run], |row| row.get(0))
                .optional()?)
        })?;
        Ok(content.and_then(|content| serde_json::from_str(&content).ok()))
    }

    /// Keep `columns` as the columns of `run`, replacing those kept before.
    pub fn keep_extraction_columns<T: Serialize>(&self, run: &str, columns: &T) -> Result<(), LibraryError> {
        let content = serde_json::to_string(columns)?;
        self.write(|transaction| {
            transaction
                .prepare_cached(
                    "INSERT INTO extraction_columns (run, columns, changed_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT (run) DO UPDATE SET columns = excluded.columns, changed_at = excluded.changed_at",
                )?
                .execute(params![run, content, now()])?;
            Ok(())
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_runs_columns_are_kept_replaced_and_apart_from_other_runs() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        assert_eq!(library.extraction_columns::<Vec<String>>("r1").unwrap(), None);
        library.keep_extraction_columns("r1", &vec!["task".to_owned(), "datasets".to_owned()]).unwrap();
        library.keep_extraction_columns("r2", &vec!["compute".to_owned()]).unwrap();
        library.keep_extraction_columns("r1", &vec!["datasets".to_owned()]).unwrap();
        assert_eq!(library.extraction_columns::<Vec<String>>("r1").unwrap(), Some(vec!["datasets".to_owned()]));
        assert_eq!(library.extraction_columns::<Vec<String>>("r2").unwrap(), Some(vec!["compute".to_owned()]));
        // reopened: kept on disk
        drop(library);
        let library = Library::open(folder.path()).unwrap();
        assert_eq!(library.extraction_columns::<Vec<String>>("r2").unwrap(), Some(vec!["compute".to_owned()]));
        assert_eq!(library.extraction_columns::<Vec<u32>>("r2").unwrap(), None, "no longer readable: none");
    }
}
