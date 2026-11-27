//! The questions asked of a run or of one of its communities (feature: cited answers), kept so that the
//! interface can list a run's questions and reopen one without asking the model again.
//!
//! The model's answer itself is kept like every feature's, in `kept_answers` under its [`AnswerKey`]
//! ([`Library::keep`], through `model::batch::ask_all`). What `kept_answers` cannot do is list what was asked of a
//! run, in order, with the question's words and the passages the model was given: that is this table. Deleting a
//! question takes its kept answer with it, in one transaction; a redo takes only the answer ([`Library::forget`]
//! clears every answer about a subject, which would take all of a community's questions at once, so this clears
//! one by its key).

use rusqlite::{OptionalExtension, params};

use super::{AnswerKey, Feature, Library, LibraryError, json_column, now};
use crate::text::retrieval::Hit;

/// The most questions of one run listed.
pub const LISTED: usize = 100;

/// One question asked, as kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Asked {
    pub run: String,
    /// The kept answer's key (hex), which is also the question's id in its run.
    pub key: String,
    /// The community's tree path; none for the whole run.
    pub community: Option<String>,
    pub question: String,
    /// The model it was asked of.
    pub model: String,
    /// The passages the model was given, best first.
    pub passages: Vec<Hit>,
    /// When it was asked, UTC.
    pub asked_at: String,
}

const COLUMNS: &str = "run, answer_key, community, question, model, passages, asked_at";

fn asked_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<Asked> {
    Ok(Asked {
        run: row.get(0)?,
        key: row.get(1)?,
        community: row.get(2)?,
        question: row.get(3)?,
        model: row.get(4)?,
        passages: json_column(row, 5)?,
        asked_at: row.get(6)?,
    })
}

impl Library {
    /// Keep a question asked (replacing one with the same key, which is then asked again: it moves to the top).
    /// Its `asked_at` is now.
    pub fn keep_question(&self, asked: &Asked) -> Result<(), LibraryError> {
        let passages = serde_json::to_string(&asked.passages)?;
        self.write(|transaction| {
            transaction
                .prepare_cached(&format!(
                    "INSERT INTO asked_questions ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT (run, answer_key) DO UPDATE SET
                         community = excluded.community, question = excluded.question, model = excluded.model,
                         passages = excluded.passages, asked_at = excluded.asked_at"
                ))?
                .execute(params![
                    asked.run,
                    asked.key,
                    asked.community,
                    asked.question,
                    asked.model,
                    passages,
                    now()
                ])?;
            Ok(())
        })
    }

    /// The questions asked of `run` (of the whole run and of its communities), the latest first, at most
    /// [`LISTED`].
    pub fn questions(&self, run: &str) -> Result<Vec<Asked>, LibraryError> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(&format!(
                "SELECT {COLUMNS} FROM asked_questions WHERE run = ?1 ORDER BY asked_at DESC, answer_key LIMIT ?2"
            ))?;
            let listed = i64::try_from(LISTED).unwrap_or(i64::MAX);
            let rows = statement.query_map(params![run, listed], asked_of)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// One question of `run`, by its key.
    pub fn question(&self, run: &str, key: &str) -> Result<Option<Asked>, LibraryError> {
        self.read(|connection| {
            Ok(connection
                .prepare_cached(&format!("SELECT {COLUMNS} FROM asked_questions WHERE run = ?1 AND answer_key = ?2"))?
                .query_row(params![run, key], asked_of)
                .optional()?)
        })
    }

    /// Forget a question of `run` and its kept answer; false when there was no such question.
    pub fn forget_question(&self, run: &str, key: &str) -> Result<bool, LibraryError> {
        self.write(|transaction| {
            let gone = transaction
                .prepare_cached("DELETE FROM asked_questions WHERE run = ?1 AND answer_key = ?2")?
                .execute(params![run, key])?;
            if gone > 0 {
                forget_answer(transaction, key)?;
            }
            Ok(gone > 0)
        })
    }

    /// Forget the kept answer of one question (a redo: it is asked again, at the cost shown again), keeping the
    /// question. False when no answer was kept.
    pub fn forget_answer(&self, key: &AnswerKey) -> Result<bool, LibraryError> {
        self.write(|transaction| Ok(forget_answer(transaction, key.as_str())? > 0))
    }
}

fn forget_answer(connection: &rusqlite::Connection, key: &str) -> Result<usize, LibraryError> {
    Ok(connection
        .prepare_cached("DELETE FROM kept_answers WHERE feature = ?1 AND answer_key = ?2")?
        .execute(params![Feature::Answers.name(), key])?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::library::{Slot, Subject};
    use crate::text::retrieval::Unit;

    fn asked(run: &str, key: &AnswerKey, question: &str) -> Asked {
        Asked {
            run: run.to_owned(),
            key: key.as_str().to_owned(),
            community: Some("c0".to_owned()),
            question: question.to_owned(),
            model: "claude-opus-5-5".to_owned(),
            passages: vec![Hit { paper: "p".to_owned(), unit: Unit::Passage { index: 3 }, score: 1.5 }],
            asked_at: String::new(),
        }
    }

    #[test]
    fn questions_are_listed_by_run_and_deleted_with_their_answer_and_nothing_else() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let key = |question: &str| AnswerKey::new(Feature::Answers, "claude-opus-5-5", 1, &["p"], question);
        let (first, second, other) = (key("first"), key("second"), key("other run"));
        for (run, key, question) in [("r1", &first, "first"), ("r1", &second, "second"), ("r2", &other, "other")] {
            library.keep_question(&asked(run, key, question)).unwrap();
            let slot = Slot { feature: Feature::Answers, key: key.clone(), subject: Subject::Run(run.to_owned()) };
            library.keep(&slot, "claude-opus-5-5", 1, &question).unwrap();
        }
        let listed = library.questions("r1").unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|asked| asked.run == "r1" && !asked.asked_at.is_empty()));
        let one = library.question("r1", first.as_str()).unwrap().unwrap();
        assert_eq!((one.question.as_str(), one.passages.len()), ("first", 1));
        assert_eq!(library.question("r2", first.as_str()).unwrap(), None, "a question of another run");

        // a redo forgets the answer and keeps the question
        assert!(library.forget_answer(&second).unwrap());
        assert_eq!(library.kept::<String>(Feature::Answers, &second).unwrap(), None);
        assert!(library.question("r1", second.as_str()).unwrap().is_some());

        assert!(library.forget_question("r1", first.as_str()).unwrap());
        assert!(!library.forget_question("r1", first.as_str()).unwrap());
        assert_eq!(library.kept::<String>(Feature::Answers, &first).unwrap(), None);
        assert!(library.kept::<String>(Feature::Answers, &other).unwrap().is_some());
        assert_eq!(library.questions("r1").unwrap().len(), 1);
    }
}
