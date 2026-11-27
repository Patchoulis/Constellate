//! What is about to be asked of a provider, written before it is sent, so that a call paid for is never lost
//! without a trace.
//!
//! An answer and the ledger row of what it cost are written together, once the answer is in
//! ([`Library::keep_paid`]). Between the provider billing a call and that transaction, a crash (the app closed,
//! the computer off) would lose both, and the next run would ask and pay again without the user ever seeing that
//! money was spent. So before a call is sent, an intent is written in a small transaction of its own: what is
//! about to be asked (feature, key, subject, provider, model), the most it may cost (what the budget holds for
//! it) and when. It is removed in the transaction that keeps the answer and records its cost, or with the cost of
//! a call that gave no usable answer, or when the call is known not to have been billed.
//!
//! "Known not to have been billed" is decided conservatively by the caller (`model::batch`): refused by the budget,
//! stopped before sending, a reply that is an error (which the APIs do not bill), a connection that could not be
//! made. A connection lost after the request went out, a timeout waiting for the answer, or a panic leave the
//! intent where it is.
//!
//! An intent left behind is a call whose outcome is unknown: it may have been billed. [`Library::unsettled`] says
//! how many there are and the most they may have cost, in the ledger's terms, and the interface and the start of
//! the engine show it as such. Such a call is not asked again by itself: only when the user starts that work
//! again, and its intent is then replaced by the new one ([`Library::intend`]).
//!
//! The table is bounded: intents older than [`FOLD_AFTER_DAYS`] days, or beyond the newest [`KEPT_INTENTS`], are
//! folded into one summary row (feature `folded`), which keeps their count, the most they may have cost and the
//! day of the oldest, so that nothing they say is lost while the table cannot grow without end.

use chrono::{DateTime, Duration as Days, SecondsFormat, Utc};
use rusqlite::{Connection, params};
use serde::Serialize;

use super::{Library, LibraryError, Slot, now};

/// Intents older than this many days are folded into the summary row.
pub const FOLD_AFTER_DAYS: i64 = 30;
/// At most this many intents are kept one by one; older ones are folded into the summary row.
pub const KEPT_INTENTS: i64 = 1_000;
/// The summary row's feature: never a feature's name (`Feature::name`).
const FOLDED: &str = "folded";

/// A call about to be sent: what [`Library::intend`] writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Intent<'a> {
    pub slot: &'a Slot,
    /// The provider's id (`anthropic`, `openai`).
    pub provider: &'a str,
    pub model: &'a str,
    /// The most it may take, in tokens of every kind: what the budget holds for it.
    pub most_tokens: u64,
    /// The most it may cost, in millionths of a US dollar; `None` when its price is not known.
    pub most_micros: Option<u64>,
}

/// Calls asked whose outcome is not known, added up as the ledger adds up calls: "asked, outcome unknown: may
/// have been billed, about this much at most". `Unsettled` in the contracts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Unsettled {
    pub calls: i64,
    /// The most the priced ones may have cost, in millionths of a US dollar.
    pub most_micros: i64,
    /// Those whose price is not known: `most_micros` leaves them out, `most_tokens` counts them.
    pub unpriced_calls: i64,
    /// The most all of them may have taken, in tokens of every kind.
    pub most_tokens: i64,
    /// When the oldest was asked (UTC, RFC 3339), if there is one.
    pub since: Option<String>,
}

impl Unsettled {
    /// The most the priced ones may have cost, in US dollars.
    pub fn most_usd(&self) -> f64 {
        self.most_micros as f64 / 1e6
    }

    /// As the engine says it at start: `None` when there is none.
    pub fn said(&self) -> Option<String> {
        if self.calls == 0 {
            return None;
        }
        let many = if self.calls == 1 { "call was" } else { "calls were" };
        let mut said = format!(
            "{} {many} asked of the language model and the outcome is unknown: they may have been billed, about \
             {:.4} US dollars at most",
            self.calls,
            self.most_usd()
        );
        if self.unpriced_calls > 0 {
            said.push_str(&format!(
                " ({} of them of a model whose price is not known, {} tokens at most in all)",
                self.unpriced_calls, self.most_tokens
            ));
        }
        said.push_str(". They are not asked again until that work is started again.");
        Some(said)
    }
}

fn signed(count: u64) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

impl Library {
    /// Write that `intent` is about to be sent, replacing an intent left for the same slot (asked again), in a
    /// transaction of its own: it is on disk before the request goes out. Old intents are folded at the same time.
    pub fn intend(&self, intent: &Intent<'_>) -> Result<(), LibraryError> {
        self.write(|transaction| {
            let at = now();
            fold(transaction, &at)?;
            transaction
                .prepare_cached(
                    "INSERT INTO asking (feature, answer_key, subject, provider, model, calls, unpriced_calls,
                                         most_tokens, most_micros, asked_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8, ?9)
                     ON CONFLICT (feature, answer_key) DO UPDATE SET
                         subject = excluded.subject, provider = excluded.provider, model = excluded.model,
                         calls = 1, unpriced_calls = excluded.unpriced_calls, most_tokens = excluded.most_tokens,
                         most_micros = excluded.most_micros, asked_at = excluded.asked_at",
                )?
                .execute(params![
                    intent.slot.feature.name(),
                    intent.slot.key.as_str(),
                    intent.slot.subject.stored(),
                    intent.provider,
                    intent.model,
                    i64::from(intent.most_micros.is_none()),
                    signed(intent.most_tokens),
                    intent.most_micros.map_or(0, signed),
                    at
                ])?;
            Ok(())
        })
    }

    /// The call for `slot` is known not to have been billed (refused, not sent, answered with an error): its intent
    /// goes. False when there was none.
    pub fn not_billed(&self, slot: &Slot) -> Result<bool, LibraryError> {
        self.write(|transaction| Ok(settle(transaction, slot)? > 0))
    }

    /// Record `calls`, which were paid for and gave no answer to keep, and remove the intent for `slot`, in one
    /// transaction: the ledger now knows what it cost.
    pub fn record_paid(&self, slot: &Slot, calls: &[super::ledger::Call<'_>]) -> Result<(), LibraryError> {
        self.write(|transaction| {
            let at = now();
            for call in calls {
                super::ledger::put_calls(transaction, &at, call, 1)?;
            }
            settle(transaction, slot)?;
            Ok(())
        })
    }

    /// Calls asked whose outcome is not known (intents left behind), added up.
    pub fn unsettled(&self) -> Result<Unsettled, LibraryError> {
        self.read(|connection| {
            Ok(connection.query_row(
                "SELECT COALESCE(SUM(calls), 0), COALESCE(SUM(most_micros), 0), COALESCE(SUM(unpriced_calls), 0),
                        COALESCE(SUM(most_tokens), 0), MIN(asked_at)
                 FROM asking",
                [],
                |row| {
                    Ok(Unsettled {
                        calls: row.get(0)?,
                        most_micros: row.get(1)?,
                        unpriced_calls: row.get(2)?,
                        most_tokens: row.get(3)?,
                        since: row.get(4)?,
                    })
                },
            )?)
        })
    }
}

/// Remove the intent for `slot`, in a transaction of the caller's: how many rows went.
pub(super) fn settle(connection: &Connection, slot: &Slot) -> Result<usize, LibraryError> {
    Ok(connection
        .prepare_cached("DELETE FROM asking WHERE feature = ?1 AND answer_key = ?2")?
        .execute(params![slot.feature.name(), slot.key.as_str()])?)
}

/// Fold the intents older than [`FOLD_AFTER_DAYS`] before `at`, or beyond the newest [`KEPT_INTENTS`], into the
/// summary row.
fn fold(connection: &Connection, at: &str) -> Result<(), LibraryError> {
    let cutoff = DateTime::parse_from_rfc3339(at)
        .map(|at| (at.with_timezone(&Utc) - Days::days(FOLD_AFTER_DAYS)).to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_default();
    // the asked_at of the newest intent past the ones kept one by one, if there are that many
    let beyond: Option<String> = connection
        .prepare_cached("SELECT asked_at FROM asking WHERE feature <> ?1 ORDER BY asked_at DESC LIMIT 1 OFFSET ?2")?
        .query_row(params![FOLDED, KEPT_INTENTS], |row| row.get(0))
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            error => Err(error),
        })?;
    // folded: older than the cutoff, or no newer than the first one past those kept
    let old = "feature <> ?1 AND (asked_at < ?2 OR asked_at <= ?3)";
    let beyond = beyond.unwrap_or_default();
    connection
        .prepare_cached(&format!(
            "INSERT INTO asking (feature, answer_key, subject, provider, model, calls, unpriced_calls, most_tokens,
                                 most_micros, asked_at)
             SELECT ?1, '', '', '', '', SUM(calls), SUM(unpriced_calls), SUM(most_tokens), SUM(most_micros),
                    MIN(asked_at)
             FROM asking WHERE {old} HAVING COUNT(*) > 0
             ON CONFLICT (feature, answer_key) DO UPDATE SET
                 calls = asking.calls + excluded.calls,
                 unpriced_calls = asking.unpriced_calls + excluded.unpriced_calls,
                 most_tokens = asking.most_tokens + excluded.most_tokens,
                 most_micros = asking.most_micros + excluded.most_micros,
                 asked_at = MIN(asking.asked_at, excluded.asked_at)"
        ))?
        .execute(params![FOLDED, cutoff, beyond])?;
    connection.prepare_cached(&format!("DELETE FROM asking WHERE {old}"))?.execute(params![FOLDED, cutoff, beyond])?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::library::{AnswerKey, Feature, Subject};
    use crate::model::language::Provider;

    fn slot(question: &str) -> Slot {
        Slot {
            feature: Feature::Answers,
            key: AnswerKey::new(Feature::Answers, Provider::Anthropic, "claude-opus-5-5", 1, &["p"], question),
            subject: Subject::Paper("p".to_owned()),
        }
    }

    fn intent(slot: &Slot, micros: Option<u64>) -> Intent<'_> {
        Intent { slot, provider: "anthropic", model: "claude-opus-5-5", most_tokens: 5_000, most_micros: micros }
    }

    fn library() -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        (folder, library)
    }

    #[test]
    fn an_intent_is_told_as_unsettled_until_its_answer_is_kept_or_it_is_known_not_billed() {
        let (_folder, library) = library();
        assert_eq!(library.unsettled().unwrap(), Unsettled::default());
        assert_eq!(Unsettled::default().said(), None);
        let (first, second) = (slot("one"), slot("two"));
        library.intend(&intent(&first, Some(40_000))).unwrap();
        library.intend(&intent(&second, None)).unwrap();
        let unsettled = library.unsettled().unwrap();
        assert_eq!((unsettled.calls, unsettled.most_micros, unsettled.unpriced_calls), (2, 40_000, 1));
        assert_eq!(unsettled.most_tokens, 10_000);
        let said = unsettled.said().unwrap();
        assert!(said.contains("2 calls were asked") && said.contains("may have been billed"), "{said}");
        assert!(said.contains("0.0400 US dollars at most") && said.contains("10000 tokens"), "{said}");
        library.keep_paid(&first, "claude-opus-5-5", 1, "{}", &[]).unwrap();
        assert!(library.not_billed(&second).unwrap());
        assert_eq!(library.unsettled().unwrap().calls, 0, "kept with its cost, and known not billed");
    }

    #[test]
    fn asking_again_replaces_the_intent_left_for_the_same_question() {
        let (_folder, library) = library();
        let first = slot("one");
        library.intend(&intent(&first, Some(40_000))).unwrap();
        library.intend(&intent(&first, Some(30_000))).unwrap();
        let unsettled = library.unsettled().unwrap();
        assert_eq!((unsettled.calls, unsettled.most_micros), (1, 30_000));
    }

    #[test]
    fn a_call_paid_for_with_no_answer_to_keep_is_recorded_and_its_intent_goes_together() {
        let (_folder, library) = library();
        let first = slot("one");
        library.intend(&intent(&first, Some(40_000))).unwrap();
        let call = super::super::ledger::Call {
            model: "claude-opus-5-5",
            purpose: "questions",
            input_tokens: 10,
            output_tokens: 10,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            price_micros: Some(3),
        };
        library.record_paid(&first, &[call]).unwrap();
        assert_eq!(library.unsettled().unwrap().calls, 0);
        assert_eq!(library.spending().unwrap().total.calls, 1);
    }

    #[test]
    fn old_intents_are_folded_into_one_row_that_keeps_their_count_and_cost() {
        let (_folder, library) = library();
        let (old, recent) = (slot("old"), slot("recent"));
        library.intend(&intent(&old, Some(10))).unwrap();
        library
            .write(|transaction| {
                transaction.execute("UPDATE asking SET asked_at = '2020-01-01T00:00:00Z'", [])?;
                Ok(())
            })
            .unwrap();
        library.intend(&intent(&recent, Some(20))).unwrap();
        let rows: Vec<(String, i64, i64)> = library
            .read(|connection| {
                let mut statement =
                    connection.prepare("SELECT feature, calls, most_micros FROM asking ORDER BY feature")?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .unwrap();
        assert_eq!(rows, [("answers".to_owned(), 1, 20), ("folded".to_owned(), 1, 10)]);
        let unsettled = library.unsettled().unwrap();
        assert_eq!((unsettled.calls, unsettled.most_micros), (2, 30));
        assert_eq!(unsettled.since.as_deref(), Some("2020-01-01T00:00:00Z"), "the oldest is still told");
    }

    #[test]
    fn intents_written_from_many_threads_are_all_counted_and_all_settle() {
        let (_folder, library) = library();
        let slots: Vec<Slot> = (0..64).map(|n| slot(&format!("question {n}"))).collect();
        std::thread::scope(|scope| {
            for chunk in slots.chunks(8) {
                let library = &library;
                scope.spawn(move || {
                    for slot in chunk {
                        library.intend(&intent(slot, Some(1))).unwrap();
                    }
                });
            }
        });
        assert_eq!(library.unsettled().unwrap().calls, 64);
        std::thread::scope(|scope| {
            for chunk in slots.chunks(8) {
                let library = &library;
                scope.spawn(move || {
                    for slot in chunk {
                        library.keep_paid(slot, "claude-opus-5-5", 1, "{}", &[]).unwrap();
                    }
                });
            }
        });
        assert_eq!(library.unsettled().unwrap(), Unsettled::default());
    }
}
