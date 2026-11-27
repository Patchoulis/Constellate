//! What the list of runs keeps about each run between starts: its summary and how it has been opened
//! ([`crate::runs`]). The runs' results themselves stay files in the runs folder, where the Python kept them.

use rusqlite::params;

use super::{Library, LibraryError, json_column};
use crate::runs::index::{Facts, SummaryRecord};
use crate::runs::usage::{Moment, Usage};

impl Library {
    /// Every run summary kept.
    pub fn run_summaries(&self) -> Result<Vec<SummaryRecord>, LibraryError> {
        self.read(|connection| {
            let mut statement = connection.prepare("SELECT name, of_time, facts FROM run_summaries")?;
            let rows = statement.query_map([], |row| {
                let facts: Facts = json_column(row, 2)?;
                Ok(SummaryRecord { name: row.get(0)?, of: Moment(row.get(1)?), facts })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Keep these summaries, replacing what was kept for the same runs.
    pub fn keep_run_summaries(&self, records: &[SummaryRecord]) -> Result<(), LibraryError> {
        if records.is_empty() {
            return Ok(());
        }
        self.write(|transaction| {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO run_summaries (name, of_time, facts) VALUES (?1, ?2, ?3)
                 ON CONFLICT (name) DO UPDATE SET of_time = excluded.of_time, facts = excluded.facts",
            )?;
            for record in records {
                statement.execute(params![record.name, record.of.0, serde_json::to_string(&record.facts)?])?;
            }
            Ok(())
        })
    }

    /// How every run has been opened.
    pub fn run_usage(&self) -> Result<Vec<(String, Usage)>, LibraryError> {
        self.read(|connection| {
            let mut statement = connection.prepare("SELECT name, usage FROM run_usage")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, json_column(row, 1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    pub fn keep_run_usage(&self, name: &str, usage: &Usage) -> Result<(), LibraryError> {
        self.write(|transaction| {
            transaction.execute(
                "INSERT INTO run_usage (name, usage) VALUES (?1, ?2)
                 ON CONFLICT (name) DO UPDATE SET usage = excluded.usage",
                params![name, serde_json::to_string(usage)?],
            )?;
            Ok(())
        })
    }

    /// Forget what is kept about a run that was deleted. Papers, embeddings and everything else of the library
    /// stay: other runs use them, and making the same run again reuses them.
    pub fn forget_run(&self, name: &str) -> Result<(), LibraryError> {
        self.write(|transaction| {
            transaction.execute("DELETE FROM run_summaries WHERE name = ?1", [name])?;
            transaction.execute("DELETE FROM run_usage WHERE name = ?1", [name])?;
            Ok(())
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn facts(title: &str) -> Facts {
        Facts { title: title.into(), papers: 3, communities: 1, modularity: 0.5, created: Moment(10), bytes: 99 }
    }

    #[test]
    fn summaries_and_usage_are_kept_replaced_and_forgotten_with_their_run() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let record = |title: &str, of: i64| SummaryRecord { name: "run".into(), of: Moment(of), facts: facts(title) };
        library.keep_run_summaries(&[record("first", 1)]).unwrap();
        library.keep_run_summaries(&[record("second", 2)]).unwrap();
        assert_eq!(library.run_summaries().unwrap(), [record("second", 2)]);
        let usage = Usage::Never.opened(Moment(5));
        library.keep_run_usage("run", &usage).unwrap();
        assert_eq!(library.run_usage().unwrap(), [("run".to_owned(), usage)]);
        library.forget_run("run").unwrap();
        assert!(library.run_summaries().unwrap().is_empty() && library.run_usage().unwrap().is_empty());
    }
}
