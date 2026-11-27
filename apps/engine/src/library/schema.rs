//! The schema, as a numbered list of migrations that is only ever appended to.
//!
//! A database records how many of them it has had in `PRAGMA user_version`. Opening applies the missing ones, in
//! order, in one transaction, so a database is always at one numbered version and never between two. A database
//! at a higher number than this code knows was written by a newer version of the app and is refused rather than
//! guessed at.
//!
//! To change the schema, add a migration at the end; never edit one that has been released, since databases out
//! there have already had it.
//!
//! Types are spelled the way PostgreSQL spells them where SQLite accepts the same words (`BIGINT`, `DOUBLE
//! PRECISION`); SQLite gives them the same meaning. Times are RFC 3339 text in UTC, which sorts as it reads.

use rusqlite::{Connection, TransactionBehavior};

use super::LibraryError;

/// Every migration, in order; `MIGRATIONS[n]` takes a database from version `n` to `n + 1`.
pub const MIGRATIONS: &[&str] = &[
    // 1: the library as the Python backend kept it in files, made into tables
    r"
    -- A paper, by its arXiv id or its conference id (<conference>-<year>-<number>). `origin` says which, so a
    -- date selection can ask for arXiv papers without a pattern match. Authors and categories are JSON arrays.
    -- `text_hash` is the hash of the title and abstract that embeddings and scores are checked against.
    CREATE TABLE papers (
        id               TEXT PRIMARY KEY,
        origin           TEXT NOT NULL CHECK (origin IN ('arxiv', 'conference')),
        title            TEXT NOT NULL,
        abstract         TEXT NOT NULL,
        authors          TEXT NOT NULL,
        published        TEXT NOT NULL,
        updated          TEXT NOT NULL,
        comment          TEXT NOT NULL,
        primary_category TEXT NOT NULL,
        categories       TEXT NOT NULL,
        url              TEXT,
        pdf_url          TEXT,
        text_hash        TEXT NOT NULL
    );
    CREATE INDEX papers_by_origin_and_published ON papers (origin, published);

    -- The venues a paper is listed at. A venue's list is replaced whole when it is fetched again.
    CREATE TABLE paper_venues (
        venue    TEXT NOT NULL,
        paper_id TEXT NOT NULL REFERENCES papers (id) ON DELETE CASCADE,
        PRIMARY KEY (venue, paper_id)
    );
    CREATE INDEX paper_venues_by_paper ON paper_venues (paper_id);

    -- One vector per model and paper, of the text whose hash is kept with it: little-endian float32.
    CREATE TABLE embeddings (
        model     TEXT NOT NULL,
        paper_id  TEXT NOT NULL REFERENCES papers (id) ON DELETE CASCADE,
        text_hash TEXT NOT NULL,
        vector    BLOB NOT NULL,
        PRIMARY KEY (model, paper_id)
    );
    CREATE INDEX embeddings_by_paper ON embeddings (paper_id);

    -- A slice of a selection that was fetched, and how completely.
    CREATE TABLE download_slices (
        slice      TEXT PRIMARY KEY,
        outcome    TEXT NOT NULL CHECK (outcome IN ('complete', 'arxiv_comments', 'incomplete')),
        fetched_at TEXT NOT NULL
    );

    -- What a version of the reader found in a paper's PDF. Only a paper that was read has content.
    CREATE TABLE declarations (
        paper_id TEXT PRIMARY KEY REFERENCES papers (id) ON DELETE CASCADE,
        reader   BIGINT NOT NULL,
        outcome  TEXT NOT NULL CHECK (outcome IN ('read', 'no_pdf', 'unreadable')),
        content  TEXT,
        read_at  TEXT NOT NULL,
        CHECK ((outcome = 'read' AND content IS NOT NULL) OR (outcome <> 'read' AND content IS NULL))
    );

    -- Reviewers' scores, read from a paper's reviews page (JSON).
    CREATE TABLE ratings (
        paper_id TEXT PRIMARY KEY REFERENCES papers (id) ON DELETE CASCADE,
        content  TEXT NOT NULL,
        read_at  TEXT NOT NULL
    );

    -- The AI-text estimate of a paper's abstract, by one classifier; no score: too short to judge.
    CREATE TABLE ai_scores (
        paper_id   TEXT PRIMARY KEY REFERENCES papers (id) ON DELETE CASCADE,
        text_hash  TEXT NOT NULL,
        classifier TEXT NOT NULL,
        score      DOUBLE PRECISION,
        scored_at  TEXT NOT NULL
    );

    -- Written about a community by a model, keyed by the hash of the model and the community's papers.
    CREATE TABLE descriptions (
        community_key TEXT PRIMARY KEY,
        name          TEXT NOT NULL,
        description   TEXT NOT NULL,
        written_at    TEXT NOT NULL
    );
    CREATE TABLE overviews (
        community_key TEXT PRIMARY KEY,
        model         TEXT NOT NULL,
        content       TEXT NOT NULL,
        written_at    TEXT NOT NULL
    );

    -- One row per call to the Claude API (more than one call only for totals moved from the old ledger).
    -- The price is in millionths of a US dollar; none when the model's price is not known.
    CREATE TABLE spending (
        at            TEXT NOT NULL,
        model         TEXT NOT NULL,
        purpose       TEXT NOT NULL,
        calls         BIGINT NOT NULL DEFAULT 1 CHECK (calls > 0),
        input_tokens  BIGINT NOT NULL CHECK (input_tokens >= 0),
        output_tokens BIGINT NOT NULL CHECK (output_tokens >= 0),
        price_micros  BIGINT CHECK (price_micros >= 0)
    );

    -- The user's settings, each a JSON value.
    CREATE TABLE settings (
        name  TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );

    -- What one US dollar buys, as last asked for; `rate_date` is the day the bank's rate is from.
    CREATE TABLE exchange_rates (
        code      TEXT PRIMARY KEY,
        rate      DOUBLE PRECISION NOT NULL CHECK (rate > 0),
        rate_date TEXT NOT NULL,
        asked     TEXT NOT NULL
    );

    -- Which parts of a files-based library were moved in, for those that cannot be told apart row by row.
    CREATE TABLE imported (
        source TEXT PRIMARY KEY,
        at     TEXT NOT NULL
    );
    ",
    // 2: a paper's reviews page as the reader showed it, and the notes read from it
    r"
    -- The text of a paper's reviews page as the user's reader showed it, and the page's address. Kept so that a
    -- newer reader can read it again, and so that positions in a reading point into something that is still there.
    -- `text_hash` (text_hash of papers.rs) tells a reading of this text from one of an earlier page.
    CREATE TABLE review_pages (
        paper_id  TEXT PRIMARY KEY REFERENCES papers (id) ON DELETE CASCADE,
        address   TEXT NOT NULL,
        text      TEXT NOT NULL,
        text_hash TEXT NOT NULL,
        read_at   TEXT NOT NULL
    );

    -- What a version of the reader (text/discussion.rs PARSER) found on a paper's page: its notes, as JSON. A paper
    -- has one row per version that read it; keeping a new page removes them all.
    CREATE TABLE review_readings (
        paper_id  TEXT NOT NULL REFERENCES review_pages (paper_id) ON DELETE CASCADE,
        parser    BIGINT NOT NULL,
        text_hash TEXT NOT NULL,
        reviews   BIGINT NOT NULL CHECK (reviews >= 0),
        content   TEXT NOT NULL,
        read_at   TEXT NOT NULL,
        PRIMARY KEY (paper_id, parser)
    );
    CREATE INDEX review_readings_by_parser ON review_readings (parser);
    ",
    // 3: what each call read from and wrote to the prompt cache, which is billed apart from the rest of its prompt
    r"
    ALTER TABLE spending ADD COLUMN cache_read_tokens BIGINT NOT NULL DEFAULT 0 CHECK (cache_read_tokens >= 0);
    ALTER TABLE spending ADD COLUMN cache_write_tokens BIGINT NOT NULL DEFAULT 0 CHECK (cache_write_tokens >= 0);
    ",
    // 4: what was read from a paper's PDF, so that a PDF is read once per version of the reader
    r"
    -- What a version of the passages reader (text/passages.rs READER) made of the PDF kept for a paper: its
    -- passages as JSON, or why it could not be read (remembered, so a PDF that cannot be read is not tried again
    -- and again). `pdf_hash` is the SHA-256 of the PDF's bytes; `pdf_size` and `pdf_modified` (nanoseconds since
    -- 1970) tell whether the file may have changed without reading it.
    CREATE TABLE paper_passages (
        paper_id     TEXT PRIMARY KEY REFERENCES papers (id) ON DELETE CASCADE,
        reader       BIGINT NOT NULL,
        pdf_hash     TEXT NOT NULL,
        pdf_size     BIGINT NOT NULL,
        pdf_modified BIGINT NOT NULL,
        outcome      TEXT NOT NULL CHECK (outcome IN ('read', 'unreadable')),
        content      TEXT NOT NULL,
        read_at      TEXT NOT NULL
    );
    ",
    // 5: what the language model answered for a feature, so that no answer is paid for twice
    r"
    -- A model's raw answer (its claims, before their quotes are checked: they are checked again when loaded) for
    -- one feature (library/answers.rs Feature), under a key made from everything the answer depends on.
    -- `subject` is what a redo clears it with: 'paper:<id>', 'run:<name>' or 'community:<run>/<tree path>'.
    CREATE TABLE kept_answers (
        feature    TEXT NOT NULL,
        answer_key TEXT NOT NULL,
        subject    TEXT NOT NULL,
        model      TEXT NOT NULL,
        prompt     BIGINT NOT NULL,
        reader     BIGINT NOT NULL,
        content    TEXT NOT NULL,
        kept_at    TEXT NOT NULL,
        PRIMARY KEY (feature, answer_key)
    );
    CREATE INDEX kept_answers_by_subject ON kept_answers (feature, subject);
    ",
    // 6 (citation contexts): each paper's citing sentences, and who cites whom among the library's papers
    r"
    -- What library/citation_contexts.rs read of a paper: every entry of its reference list that its text cites,
    -- with the citing sentences and their neighbours (`content`), and the same with only the entries that are
    -- papers of the library (`linked`). Valid while the paper's passages are of reader `reader` and of the PDF
    -- whose SHA-256 is `pdf_hash`; `version` is that of what is read (analysis/citation_contexts.rs CONTEXTS).
    CREATE TABLE citation_contexts (
        paper_id TEXT PRIMARY KEY REFERENCES papers (id) ON DELETE CASCADE,
        reader   BIGINT NOT NULL,
        version  BIGINT NOT NULL,
        pdf_hash TEXT NOT NULL,
        content  TEXT NOT NULL,
        linked   TEXT NOT NULL,
        read_at  TEXT NOT NULL
    );

    -- A paper whose kept contexts cite another paper of the library, in how many sentences: replaced with them.
    CREATE TABLE citation_links (
        citing    TEXT NOT NULL REFERENCES citation_contexts (paper_id) ON DELETE CASCADE,
        cited     TEXT NOT NULL REFERENCES papers (id) ON DELETE CASCADE,
        sentences BIGINT NOT NULL CHECK (sentences > 0),
        PRIMARY KEY (citing, cited)
    );
    CREATE INDEX citation_links_by_cited ON citation_links (cited);
    ",
];

/// Bring `connection` to the last of `migrations`. Refuses a database newer than the list.
pub(super) fn migrate(connection: &Connection, migrations: &[&str]) -> Result<(), LibraryError> {
    let known = i64::try_from(migrations.len()).unwrap_or(i64::MAX);
    let version = |connection: &Connection| -> rusqlite::Result<i64> {
        connection.pragma_query_value(None, "user_version", |row| row.get(0))
    };
    let refuse = |found| LibraryError::Newer { path: connection.path().unwrap_or_default().into(), found, known };
    let found = version(connection)?;
    if found > known {
        return Err(refuse(found));
    }
    if found == known {
        return Ok(());
    }
    // another process may be migrating the same file: the write lock is taken first, then the version read again
    let transaction = rusqlite::Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
    let found = version(&transaction)?;
    if found > known {
        return Err(refuse(found));
    }
    for migration in migrations.iter().skip(usize::try_from(found).unwrap_or(0)) {
        transaction.execute_batch(migration)?;
    }
    transaction.pragma_update(None, "user_version", known)?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::library::{DATABASE, Library};

    /// Every table, index and column, as SQLite describes them: two databases with the same are the same schema.
    fn schema_of(connection: &Connection) -> Vec<String> {
        let mut statement = connection
            .prepare("SELECT type, name, sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
            .unwrap();
        let rows = statement.query_map([], |row| {
            Ok(format!("{} {} {}", row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        });
        rows.unwrap().map(Result::unwrap).collect()
    }

    fn version_of(connection: &Connection) -> i64 {
        connection.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap()
    }

    #[test]
    fn a_fresh_database_and_one_at_each_earlier_version_arrive_at_the_same_schema() {
        let fresh = tempfile::tempdir().unwrap();
        let latest = schema_of(&Library::open(fresh.path()).unwrap().connection());
        assert!(latest.iter().any(|line| line.starts_with("table papers ")));
        for earlier in 0..MIGRATIONS.len() {
            let folder = tempfile::tempdir().unwrap();
            {
                let connection = Connection::open(folder.path().join(DATABASE)).unwrap();
                migrate(&connection, &MIGRATIONS[..earlier]).unwrap();
                assert_eq!(version_of(&connection), i64::try_from(earlier).unwrap());
            }
            let library = Library::open(folder.path()).unwrap();
            let connection = library.connection();
            assert_eq!(schema_of(&connection), latest, "from version {earlier}");
            assert_eq!(version_of(&connection), i64::try_from(MIGRATIONS.len()).unwrap());
        }
    }

    #[test]
    fn later_migrations_are_applied_in_order_and_keep_what_is_there() {
        let steps = [
            "CREATE TABLE notes (id TEXT PRIMARY KEY)",
            "ALTER TABLE notes ADD COLUMN said TEXT NOT NULL DEFAULT ''",
            "UPDATE notes SET said = 'moved' WHERE id = 'kept'",
        ];
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection, &steps[..1]).unwrap();
        connection.execute("INSERT INTO notes (id) VALUES ('kept')", []).unwrap();
        migrate(&connection, &steps).unwrap();
        let said: String =
            connection.query_row("SELECT said FROM notes WHERE id = 'kept'", [], |row| row.get(0)).unwrap();
        assert_eq!(said, "moved");
        assert_eq!(version_of(&connection), 3);
        migrate(&connection, &steps).unwrap(); // nothing left to do: nothing is done twice
        assert_eq!(version_of(&connection), 3);
    }

    #[test]
    fn a_migration_that_fails_leaves_the_database_at_its_old_version() {
        let steps = ["CREATE TABLE notes (id TEXT PRIMARY KEY)", "CREATE TABLE more (id TEXT)", "NOT SQL AT ALL"];
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection, &steps[..1]).unwrap();
        assert!(migrate(&connection, &steps).is_err());
        assert_eq!(version_of(&connection), 1);
        let tables: i64 = connection
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'more'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tables, 0); // the step that worked was rolled back with the one that did not
    }

    #[test]
    fn a_database_from_a_newer_version_is_refused_with_both_versions() {
        let folder = tempfile::tempdir().unwrap();
        drop(Library::open(folder.path()).unwrap());
        Connection::open(folder.path().join(DATABASE)).unwrap().pragma_update(None, "user_version", 99).unwrap();
        let error = Library::open(folder.path()).unwrap_err();
        match &error {
            LibraryError::Newer { path, found, known } => {
                assert_eq!((*found, *known), (99, i64::try_from(MIGRATIONS.len()).unwrap()));
                assert!(path.ends_with(DATABASE));
            }
            other => panic!("{other:?}"),
        }
        assert!(error.to_string().contains("newer version"));
    }
}
