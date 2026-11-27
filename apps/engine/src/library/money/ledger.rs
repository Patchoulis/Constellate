//! What the app has spent on the Claude API: one row per call, priced when it is made.
//!
//! The Python kept running totals per model and per purpose, added to in floating point on every call. Here a
//! call is a row and every total is a sum over rows, so totals cannot drift from the calls or from each other,
//! and a total by any other measure (by day, say) can be asked for later. Prices are whole millionths of a US
//! dollar, so a sum of thousands of tiny calls is exact. A call to a model whose price is not known has no price,
//! is counted in tokens only, and is counted as unpriced so that the total can say it is short.

use std::collections::BTreeMap;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, params};
use serde::Serialize;

use crate::library::{Library, LibraryError, now};

/// One call to the API, as made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call<'a> {
    pub model: &'a str,
    /// What it was for: naming communities, overviews, training the AI-text detector.
    pub purpose: &'a str,
    /// The prompt read at the full rate.
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// Read from and written to the prompt cache, each billed at its own rate.
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// What it cost in millionths of a US dollar; `None` for a model whose price is not known.
    pub price_micros: Option<i64>,
}

/// Calls added up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// Of the priced calls only.
    pub price_micros: i64,
    pub unpriced_calls: i64,
}

impl Totals {
    pub fn usd(&self) -> f64 {
        self.price_micros as f64 / 1e6
    }
}

/// Everything spent, in all and by model and by purpose.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Spending {
    /// The day of the first call, if there was one.
    pub since: Option<String>,
    pub total: Totals,
    pub models: BTreeMap<String, Totals>,
    pub purposes: BTreeMap<String, Totals>,
}

/// The sums every total is made of, after `SELECT`, before `FROM`.
const SUMS: &str = "COALESCE(SUM(calls), 0), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(price_micros), 0), COALESCE(SUM(CASE WHEN price_micros IS NULL THEN calls ELSE 0 END), 0),
                    COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_write_tokens), 0)";

fn totals_from(row: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<Totals> {
    Ok(Totals {
        calls: row.get(first)?,
        input_tokens: row.get(first + 1)?,
        output_tokens: row.get(first + 2)?,
        price_micros: row.get(first + 3)?,
        unpriced_calls: row.get(first + 4)?,
        cache_read_tokens: row.get(first + 5)?,
        cache_write_tokens: row.get(first + 6)?,
    })
}

/// `calls` calls, as one row (more than one only for totals moved in from the old ledger).
pub(in crate::library) fn put_calls(
    connection: &Connection,
    at: &str,
    call: &Call<'_>,
    calls: i64,
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO spending (at, model, purpose, calls, input_tokens, output_tokens, price_micros,
                               cache_read_tokens, cache_write_tokens)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            at,
            call.model,
            call.purpose,
            calls,
            call.input_tokens,
            call.output_tokens,
            call.price_micros,
            call.cache_read_tokens,
            call.cache_write_tokens
        ],
    )
}

fn grouped(connection: &Connection, column: &str) -> rusqlite::Result<BTreeMap<String, Totals>> {
    let mut statement = connection.prepare(&format!("SELECT {column}, {SUMS} FROM spending GROUP BY {column}"))?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, totals_from(row, 1)?)))?;
    rows.collect()
}

impl Library {
    /// Add one call to the ledger, stamped now.
    pub fn record_call(&self, call: &Call<'_>) -> Result<(), LibraryError> {
        self.write(|transaction| Ok(put_calls(transaction, &now(), call, 1).map(drop)?))
    }

    pub fn spending(&self) -> Result<Spending, LibraryError> {
        self.read(|connection| {
            let (since, total) = connection.query_row(&format!("SELECT MIN(at), {SUMS} FROM spending"), [], |row| {
                Ok((row.get::<_, Option<String>>(0)?, totals_from(row, 1)?))
            })?;
            Ok(Spending {
                since: since.map(|at| at.chars().take(10).collect()),
                total,
                models: grouped(connection, "model")?,
                purposes: grouped(connection, "purpose")?,
            })
        })
    }

    /// The calls recorded at `from` or later, added up: what was spent since then (since the engine started, say).
    /// Every row is stamped as [`now`] writes it, UTC to the second in RFC 3339, which sorts as text; `from` is
    /// written the same way, so the comparison is one of instants, to the second.
    pub fn spending_since(&self, from: DateTime<Utc>) -> Result<Totals, LibraryError> {
        let from = from.to_rfc3339_opts(SecondsFormat::Secs, true);
        self.read(|connection| {
            let sql = format!("SELECT {SUMS} FROM spending WHERE at >= ?1");
            Ok(connection.query_row(&sql, [from], |row| totals_from(row, 0))?)
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn call<'a>(model: &'a str, purpose: &'a str, price_micros: Option<i64>) -> Call<'a> {
        Call {
            model,
            purpose,
            input_tokens: 1_000,
            output_tokens: 200,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            price_micros,
        }
    }

    #[test]
    fn nothing_spent_is_all_zeros_and_no_first_day() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        assert_eq!(library.spending().unwrap(), Spending::default());
    }

    #[test]
    fn totals_are_sums_of_the_calls_by_model_and_by_purpose() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        library.record_call(&call("claude-opus-5-5", "naming communities", Some(8_000))).unwrap();
        library.record_call(&call("claude-opus-5-5", "community overviews", Some(8_000))).unwrap();
        library.record_call(&call("some-new-model", "naming communities", None)).unwrap();
        let spent = library.spending().unwrap();
        assert_eq!(
            spent.total,
            Totals {
                calls: 3,
                input_tokens: 3_000,
                output_tokens: 600,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                price_micros: 16_000,
                unpriced_calls: 1
            }
        );
        assert_eq!(spent.total.usd(), 0.016);
        assert_eq!(spent.models["claude-opus-5-5"].calls, 2);
        assert_eq!(spent.models["some-new-model"].unpriced_calls, 1);
        assert_eq!(spent.models["some-new-model"].price_micros, 0);
        assert_eq!(spent.purposes["naming communities"].calls, 2);
        assert_eq!(spent.purposes["community overviews"].price_micros, 8_000);
        assert_eq!(spent.since.unwrap().len(), "2026-11-26".len());
    }

    #[test]
    fn calls_recorded_from_many_threads_are_all_counted() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..25 {
                        library.record_call(&call("claude-haiku-5-5", "training the detector", Some(1))).unwrap();
                    }
                });
            }
        });
        let spent = library.spending().unwrap();
        assert_eq!((spent.total.calls, spent.total.price_micros), (200, 200));
    }

    #[test]
    fn spending_since_an_instant_counts_the_calls_from_then_on_and_none_before() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let at = |instant: &str| DateTime::parse_from_rfc3339(instant).unwrap().with_timezone(&Utc);
        library
            .write(|transaction| {
                put_calls(transaction, "2026-11-26T09:59:59Z", &call("claude-opus-5-5", "p", Some(5)), 1)?;
                put_calls(transaction, "2026-11-26T10:00:00Z", &call("claude-opus-5-5", "p", Some(7)), 2)?;
                Ok(put_calls(transaction, "2026-11-27T08:00:00Z", &call("mystery", "p", None), 1)?)
            })
            .unwrap();
        let since = library.spending_since(at("2026-11-26T10:00:00.250+00:00")).unwrap();
        assert_eq!((since.calls, since.price_micros, since.unpriced_calls), (3, 7, 1));
        assert_eq!(library.spending_since(at("2026-11-28T00:00:00Z")).unwrap(), Totals::default());
    }

    #[test]
    fn a_negative_count_is_refused_by_the_database() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let mut wrong = call("m", "p", Some(1));
        wrong.input_tokens = -1;
        assert!(library.record_call(&wrong).is_err());
        assert_eq!(library.spending().unwrap().total.calls, 0);
    }
}
