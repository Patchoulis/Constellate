//! What is kept on disk: the library, one SQLite database on the user's own computer.
//!
//! Everything fetched or computed is kept here by id, so that work that was done is not done again: papers and
//! the venues they are tagged with, embeddings, which download slices are finished, what was read from papers
//! (authors' declarations, reviewers' ratings), AI-text scores, community descriptions and overviews, what was
//! spent on the Claude API, settings and exchange rates.
//!
//! The Python backend kept these as loose files, each replaced atomically under a lock file. That encoded state
//! in "does the file exist", let a crash leave a set of files half-written (vectors without their index), and
//! left read-change-write races between processes. A database removes all three: several rows change in one
//! transaction or not at all, and SQLite itself arbitrates between threads and processes.
//!
//! | folder          | what lives there                                                          |
//! |-----------------|---------------------------------------------------------------------------|
//! | [`core`]        | the library itself: schema, place, file store, import, preferences, run records |
//! | [`papers`]      | what is kept about papers: records, embeddings, passages, readings, reviews, references, proceedings |
//! | [`money`]       | what asking the model costs: kept answers, calls about to be asked, the ledger |
//!
//! # The database
//!
//! One file, `library.sqlite3`, in the folder the caller names. It is opened in WAL mode, so readers never wait
//! for a writer and a writer never waits for readers; `synchronous = NORMAL`, which in WAL mode cannot corrupt
//! the database on a power cut (at worst the last transactions are lost); `foreign_keys` on, so nothing can be
//! kept about a paper the library does not have; and a busy timeout, so a second process (an old Python backend,
//! a second window) waits its turn instead of failing.
//!
//! # Threads
//!
//! A [`Library`] is shared by every thread of the engine (it is `Sync`), and that is safe by construction, not
//! by care: a rusqlite `Connection` is `Send` but not `Sync`, so the compiler refuses any use of one connection
//! from two threads at once, and the library never hands a connection out.
//!
//! * **One writer.** SQLite allows one writer at a time whatever is done, so the library has one connection that
//!   writes, behind a `Mutex`. Writes start `BEGIN IMMEDIATE`: they take the write lock at once, so two processes
//!   cannot both read and then both try to upgrade, which is the one case a busy timeout cannot settle. Whatever
//!   one write does (a paper's reviews: its page, its reading and its scores) is one transaction.
//! * **Readers of their own.** Each read takes a connection of its own from a small pool (opening one when none
//!   is idle), runs in one deferred transaction (so a read of several tables sees one moment), and gives it
//!   back. In WAL mode readers never wait for the writer nor the writer for them, so reads on many threads (the
//!   reviews of thousands of papers, while a page is being kept) run side by side. Readers are opened
//!   `query_only`: one cannot write even by mistake.
//! * **A thread that panics** poisons nothing. A write it was in the middle of rolls back as its transaction is
//!   dropped on unwinding, so the writer's lock is taken again as if it had not been poisoned; a reader it was
//!   using is dropped (closed), never given back half-used. Other threads carry on with the library as it was
//!   before the failed write.
//!
//! # PostgreSQL, should the engine ever be a shared server
//!
//! All SQL is in this module and in the `store.rs` of each feature ([`crate::features`]), whose `impl Library`
//! blocks go through the same `Library::read` and `Library::write`; it is written in the portable subset:
//! `INSERT ... ON CONFLICT (...) DO UPDATE /
//! DO NOTHING`, `BIGINT`, `DOUBLE PRECISION`, `TEXT`, `CHECK` constraints, no `INSERT OR REPLACE`, no
//! `WITHOUT ROWID`, no JSON functions in queries, no rowid. A port would change:
//!
//! * the driver (`postgres` or `sqlx`), the connection setup (the pragmas go; a pool comes in), and placeholders
//!   from `?1` to `$1` (including the unnumbered lists [`placeholders`] makes);
//! * `PRAGMA user_version` becomes a one-row `schema_version` table read and written in the migration's
//!   transaction;
//! * `BLOB` becomes `BYTEA` and `length(vector)` becomes `octet_length(vector)`; JSON columns may become `JSONB`;
//! * `ORDER BY id` needs `COLLATE "C"` to keep ordering by code point, as the Python did;
//! * `SUM` over `BIGINT` returns `NUMERIC` and needs a cast back to `BIGINT`;
//! * transactions. `BEGIN IMMEDIATE` makes every write here run alone, so the read-then-write steps (keeping
//!   ratings only when fuller, the import's "insert what is not there", the vector width check) cannot race. In
//!   PostgreSQL's default READ COMMITTED they can, and `SELECT ... FOR UPDATE` cannot lock a row that does not
//!   exist yet: writes would run SERIALIZABLE and be retried on a serialization failure (or take an advisory
//!   lock), and reads that span tables would run REPEATABLE READ.

pub mod core;
pub mod money;
pub mod papers;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, ErrorCode, Transaction, TransactionBehavior};

pub use self::core::import::{ImportError, Imported, import_files};
pub use self::core::preferences::Rate;
pub use crate::features::writing::store::{CitedPaper, Description, Overview, Point, Section, community_key};
pub use money::answers::{AnswerKey, Feature, Kept, Slot, Subject};
pub use money::ledger::{Call, RunSpending, Spending, Totals};
pub use papers::embeddings::NotEmbedded;
pub use papers::passages::PaperReading;
pub use papers::readings::{AiScore, Answer, ChecklistItem, Declarations, FieldRatings, Ratings, Reading, Statement};
pub use papers::reviews::{Keeping, KeptReviews, ReviewPage};
pub use papers::{Added, Known, Origin, Paper, SliceOutcome, text_hash};

/// The database's file name inside the library folder.
pub const DATABASE: &str = "library.sqlite3";
/// How long a write waits for another process's write to finish before giving up.
const BUSY: Duration = Duration::from_secs(10);
/// Readers kept open between reads; more are opened when more threads read at once, and closed after.
const IDLE_READERS: usize = 8;

/// What can go wrong with the library, always saying which database or which paper.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("{}: {source}", path.display())]
    Folder {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} could not be opened: {source}", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    /// The file is there but is not a SQLite database, or is damaged: left alone for a person to look at.
    #[error("{} is not a library database, or is damaged", path.display())]
    NotADatabase { path: PathBuf },
    /// Written by a newer version of the app: opening it with this one could lose what the newer one keeps.
    #[error("{} was written by a newer version of Constellate (schema {found}; this version knows up to {known})", path.display())]
    Newer { path: PathBuf, found: i64, known: i64 },
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    /// Something to be kept could not be written as JSON (it cannot happen with the types here, but is not
    /// unwrapped).
    #[error("could not write JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("paper {0} is not in the library")]
    UnknownPaper(String),
    #[error(transparent)]
    NotEmbedded(#[from] NotEmbedded),
    /// Vectors handed over do not fit: a different count than papers, or a different length than the model's.
    #[error("{0}")]
    Shape(String),
    /// A file kept beside the database (a paper's PDF) could not be written.
    #[error(transparent)]
    Store(#[from] self::core::store::StoreError),
}

/// The library: a handle on its database, shared by every thread of the engine (see the module's "Threads").
#[derive(Debug)]
pub struct Library {
    path: PathBuf,
    /// The one connection that writes.
    connection: Mutex<Connection>,
    /// Connections that only read, idle; each is used by one read at a time, taken out of here for it.
    readers: Mutex<Vec<Connection>>,
    /// The PDFs being read right now, so that two threads asking for the same paper read it once.
    in_reading: papers::passages::InReading,
}

impl Library {
    /// Open the library in `folder` (created if need be), bringing its schema up to date.
    pub fn open(folder: &Path) -> Result<Self, LibraryError> {
        fs::create_dir_all(folder).map_err(|source| LibraryError::Folder { path: folder.to_path_buf(), source })?;
        let path = folder.join(DATABASE);
        let connection = connect(&path)?;
        self::core::schema::migrate(&connection, self::core::schema::MIGRATIONS)
            .map_err(|error| located(error, &path))?;
        Ok(Self {
            path,
            connection: Mutex::new(connection),
            readers: Mutex::new(Vec::new()),
            in_reading: papers::passages::InReading::default(),
        })
    }

    /// The database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn connection(&self) -> MutexGuard<'_, Connection> {
        // a thread that panicked mid-transaction left nothing half-done: the transaction rolled back on unwind
        self.connection.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Run `work` as one write transaction: all of it is kept, or none of it.
    pub(crate) fn write<T>(
        &self,
        work: impl FnOnce(&Transaction<'_>) -> Result<T, LibraryError>,
    ) -> Result<T, LibraryError> {
        let mut connection = self.connection();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = work(&transaction)?;
        transaction.commit()?;
        Ok(result)
    }

    /// Run `work` on one consistent view of the database, on a reader of its own.
    pub(crate) fn read<T>(&self, work: impl FnOnce(&Connection) -> Result<T, LibraryError>) -> Result<T, LibraryError> {
        let idle = self.readers.lock().unwrap_or_else(PoisonError::into_inner).pop();
        let mut connection = match idle {
            Some(connection) => connection,
            None => connect_reader(&self.path)?,
        };
        let result = match connection.transaction() {
            Ok(transaction) => work(&transaction), // dropped, not committed: nothing was written
            Err(error) => Err(error.into()),
        };
        // not reached when `work` panics: the connection is then dropped with the unwinding, never reused; nor
        // is one whose transaction could not be rolled back
        let mut idle = self.readers.lock().unwrap_or_else(PoisonError::into_inner);
        if idle.len() < IDLE_READERS && connection.is_autocommit() {
            idle.push(connection);
        }
        result
    }
}

fn connect(path: &Path) -> Result<Connection, LibraryError> {
    let opened = |source| located(LibraryError::Open { path: path.to_path_buf(), source }, path);
    let connection = Connection::open(path).map_err(opened)?;
    connection.busy_timeout(BUSY).map_err(opened)?;
    // the first statement that reads the file: a file that is not a database is found out here
    let mode: String =
        connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0)).map_err(opened)?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(LibraryError::Open {
            path: path.to_path_buf(),
            source: rusqlite::Error::InvalidParameterName(format!("journal mode {mode} instead of WAL")),
        });
    }
    connection.pragma_update(None, "foreign_keys", true).map_err(opened)?;
    connection.pragma_update(None, "synchronous", "NORMAL").map_err(opened)?;
    Ok(connection)
}

/// A connection that only reads, to a database [`connect`] has set up.
fn connect_reader(path: &Path) -> Result<Connection, LibraryError> {
    let opened = |source| located(LibraryError::Open { path: path.to_path_buf(), source }, path);
    let connection = Connection::open(path).map_err(opened)?;
    connection.busy_timeout(BUSY).map_err(opened)?;
    connection.pragma_update(None, "query_only", true).map_err(opened)?;
    Ok(connection)
}

/// SQLite's "not a database" and "damaged", wherever they surface, said as such with the file's name.
fn located(error: LibraryError, path: &Path) -> LibraryError {
    let code = match &error {
        LibraryError::Open { source, .. } | LibraryError::Sql(source) => source.sqlite_error_code(),
        _ => None,
    };
    match code {
        Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt) => {
            LibraryError::NotADatabase { path: path.to_path_buf() }
        }
        _ => match error {
            LibraryError::Newer { found, known, .. } => LibraryError::Newer { path: path.to_path_buf(), found, known },
            other => other,
        },
    }
}

/// Now, as kept in the database: UTC, to the second, in RFC 3339 (which sorts as text).
pub(crate) fn now() -> String {
    DateTime::<Utc>::from(SystemTime::now()).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A JSON column read back into its type; a value that does not parse is reported as that column's error.
pub(crate) fn json_column<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    serde_json::from_str(&text)
        .map_err(|error| rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(error)))
}

/// `?, ?, ?` for a list of `count` values (never empty: callers return early on an empty list).
pub(crate) fn placeholders(count: usize) -> String {
    vec!["?"; count].join(", ")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_folder_is_made_and_the_database_is_set_up_as_described() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(&folder.path().join("not").join("yet")).unwrap();
        assert!(library.path().exists());
        let connection = library.connection();
        let mode: String = connection.pragma_query_value(None, "journal_mode", |row| row.get(0)).unwrap();
        let keys: bool = connection.pragma_query_value(None, "foreign_keys", |row| row.get(0)).unwrap();
        let synchronous: i64 = connection.pragma_query_value(None, "synchronous", |row| row.get(0)).unwrap();
        assert_eq!(mode, "wal");
        assert!(keys);
        assert_eq!(synchronous, 1); // NORMAL
    }

    #[test]
    fn a_file_that_is_not_a_database_is_refused_by_name_and_left_alone() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(DATABASE);
        let garbage = "these are not the pages of a database\n".repeat(200);
        fs::write(&path, &garbage).unwrap();
        let error = Library::open(folder.path()).unwrap_err();
        assert!(matches!(error, LibraryError::NotADatabase { .. }), "{error:?}");
        assert!(error.to_string().contains(DATABASE));
        assert_eq!(fs::read_to_string(&path).unwrap(), garbage);
    }

    #[test]
    fn a_database_whose_pages_were_overwritten_is_refused_or_fails_cleanly() {
        let folder = tempfile::tempdir().unwrap();
        {
            let library = Library::open(folder.path()).unwrap();
            library.change_settings(&serde_json::from_str(r#"{"max_links": 8}"#).unwrap()).unwrap();
            let connection = library.connection();
            connection.pragma_update(None, "wal_checkpoint", "TRUNCATE").unwrap();
        }
        let path = folder.path().join(DATABASE);
        let mut bytes = fs::read(&path).unwrap();
        for byte in bytes.iter_mut().skip(100) {
            *byte = 0xA5; // everything after the header: every page's structure is gone
        }
        fs::write(&path, &bytes).unwrap();
        match Library::open(folder.path()) {
            Err(error) => assert!(matches!(error, LibraryError::NotADatabase { .. }), "{error:?}"),
            Ok(library) => assert!(library.settings().is_err()), // never wrong answers, only an error
        }
    }

    #[test]
    fn a_folder_that_cannot_be_made_says_which() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("a file");
        fs::write(&file, "").unwrap();
        let error = Library::open(&file.join("library")).unwrap_err();
        assert!(matches!(error, LibraryError::Folder { .. }), "{error:?}");
    }

    #[test]
    fn a_library_is_shared_between_threads() {
        fn shared<T: Send + Sync>() {}
        shared::<Library>();
    }

    #[test]
    fn a_reader_cannot_write_and_reads_on_many_threads_run_at_once() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let refused = library.read(|connection| Ok(connection.execute("CREATE TABLE notes (id TEXT)", [])?));
        assert!(refused.is_err());
        let barrier = std::sync::Barrier::new(4);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    library
                        .read(|_| {
                            barrier.wait(); // every reader inside a read at the same moment: none waits for another
                            Ok(())
                        })
                        .unwrap()
                });
            }
        });
        assert!(library.readers.lock().unwrap().len() >= 2);
    }

    #[test]
    fn a_read_that_panics_leaves_the_readers_usable() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        library.read(|_| Ok(())).unwrap();
        let panicked = std::thread::scope(|scope| {
            scope.spawn(|| library.read::<()>(|_| panic!("a read failed half-way"))).join().is_err()
        });
        assert!(panicked);
        assert!(library.readers.lock().unwrap().is_empty(), "the panicking read's reader is not given back");
        assert!(library.settings().is_ok());
    }

    #[test]
    fn time_is_kept_in_utc_seconds() {
        let stamp = now();
        assert_eq!(stamp.len(), "2026-11-26T10:00:00Z".len());
        assert!(stamp.ends_with('Z'));
    }
}
