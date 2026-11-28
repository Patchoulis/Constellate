//! What the app has spent on the Claude API: one row per call, priced when it is made.
//!
//! The Python kept running totals per model and per purpose, added to in floating point on every call. Here a
//! call is a row and every total is a sum over rows, so totals cannot drift from the calls or from each other,
//! and a total by any other measure (by day, say) can be asked for later. Prices are whole millionths of a US
//! dollar, so a sum of thousands of tiny calls is exact. A call to a model whose price is not known has no price,
//! is counted in tokens only, and is counted as unpriced so that the total can say it is short.
//!
//! A row names the run it was made for, so that what a run cost is a sum over its rows too. Nothing kept from
//! before that column was added says which run a call was for (a run's opens are kept, its cost never was; an
//! answer kept for a run does not say which call paid for it), so those rows name no run rather than one guessed.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::library::{Library, LibraryError, now};

/// One call to the API, as made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call<'a> {
    /// The run it was made for, by name; none for a call of no run, and for every call recorded before calls were
    /// counted by run ([`Library::runs_counted_since`]).
    pub run: Option<&'a str>,
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
                               cache_read_tokens, cache_write_tokens, run)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            at,
            call.model,
            call.purpose,
            calls,
            call.input_tokens,
            call.output_tokens,
            call.price_micros,
            call.cache_read_tokens,
            call.cache_write_tokens,
            call.run
        ],
    )
}

fn grouped(connection: &Connection, column: &str) -> rusqlite::Result<BTreeMap<String, Totals>> {
    let mut statement = connection.prepare(&format!("SELECT {column}, {SUMS} FROM spending GROUP BY {column}"))?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, totals_from(row, 1)?)))?;
    rows.collect()
}

/// What one run cost: the calls recorded for it, in all and by purpose.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RunSpending {
    pub total: Totals,
    pub purposes: BTreeMap<String, Totals>,
    /// The day calls began to be counted by run, when calls were recorded before it: those have no run, so what a
    /// run made before that day cost then is in the total of everything spent and not in its own. None when every
    /// call recorded was counted by run.
    pub counted_since: Option<String>,
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

    /// When calls began to be counted by run (UTC, RFC 3339): the moment this library was first opened by a version
    /// that counts them.
    pub fn runs_counted_since(&self) -> Result<String, LibraryError> {
        self.read(|connection| {
            Ok(connection.query_row("SELECT at FROM spending_runs_since WHERE id = 1", [], |row| row.get(0))?)
        })
    }

    /// What the run named `run` cost: every call recorded for it, whenever it was made, so that a run deleted and
    /// made again under the same name goes on counting under it. A run's calls are never removed with it: the
    /// money was spent, and the total of everything spent never goes down.
    pub fn run_spending(&self, run: &str) -> Result<RunSpending, LibraryError> {
        self.read(|connection| {
            let total = connection
                .prepare_cached(&format!("SELECT {SUMS} FROM spending WHERE run = ?1"))?
                .query_row([run], |row| totals_from(row, 0))?;
            let mut statement = connection
                .prepare_cached(&format!("SELECT purpose, {SUMS} FROM spending WHERE run = ?1 GROUP BY purpose"))?;
            let purposes = statement
                .query_map([run], |row| Ok((row.get(0)?, totals_from(row, 1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let counted_since: Option<String> = connection
                .query_row(
                    "SELECT since.at FROM spending_runs_since AS since
                 WHERE since.id = 1 AND EXISTS (SELECT 1 FROM spending WHERE at < since.at)",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(RunSpending { total, purposes, counted_since: counted_since.map(|at| at.chars().take(10).collect()) })
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn call<'a>(model: &'a str, purpose: &'a str, price_micros: Option<i64>) -> Call<'a> {
        Call {
            run: None,
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

    fn for_run<'a>(run: &'a str, purpose: &'a str, price_micros: Option<i64>) -> Call<'a> {
        Call { run: Some(run), ..call("claude-opus-5-5", purpose, price_micros) }
    }

    #[test]
    fn a_run_costs_the_calls_recorded_for_it_by_purpose_and_no_others() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        library.record_call(&for_run("r1", "naming communities", Some(5_000))).unwrap();
        library.record_call(&for_run("r1", "community overviews", Some(7_000))).unwrap();
        library.record_call(&Call { model: "mystery", ..for_run("r1", "community overviews", None) }).unwrap();
        library.record_call(&for_run("r2", "naming communities", Some(100))).unwrap();
        library.record_call(&call("claude-opus-5-5", "training the AI-text detector", Some(9))).unwrap();
        let r1 = library.run_spending("r1").unwrap();
        assert_eq!((r1.total.calls, r1.total.price_micros, r1.total.unpriced_calls), (3, 12_000, 1));
        assert_eq!(r1.purposes["community overviews"].calls, 2);
        assert_eq!(r1.purposes["naming communities"].price_micros, 5_000);
        assert_eq!(library.run_spending("r2").unwrap().total.price_micros, 100);
        assert_eq!(library.run_spending("never made").unwrap().total, Totals::default());
        // the detector's call is in the total and in no run's
        assert_eq!(library.spending().unwrap().total.price_micros, 12_109);
        assert_eq!(r1.counted_since, None, "every call recorded was counted by run");
    }

    #[test]
    fn a_deleted_runs_calls_stay_in_the_total_and_in_its_name_and_a_reopened_library_says_the_same() {
        let folder = tempfile::tempdir().unwrap();
        {
            let library = Library::open(folder.path()).unwrap();
            library.record_call(&for_run("r1", "naming communities", Some(5_000))).unwrap();
            library.forget_run("r1").unwrap();
            // made again under the same name: counted on under it
            library.record_call(&for_run("r1", "naming communities", Some(2_000))).unwrap();
        }
        let reopened = Library::open(folder.path()).unwrap();
        assert_eq!(reopened.spending().unwrap().total.price_micros, 7_000);
        assert_eq!(reopened.run_spending("r1").unwrap().total.price_micros, 7_000);
    }

    #[test]
    fn calls_recorded_before_runs_were_counted_say_from_which_day_a_runs_figure_counts() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let since = library.runs_counted_since().unwrap();
        assert_eq!(since.len(), "2026-11-28T10:00:00Z".len());
        library.record_call(&for_run("r1", "naming communities", Some(5))).unwrap();
        assert_eq!(library.run_spending("r1").unwrap().counted_since, None);
        // a call from before (the Python's ledger, or this ledger before the run column) has no run
        library
            .write(|transaction| Ok(put_calls(transaction, "2026-01-01T00:00:00Z", &call("m", "p", Some(3)), 1)?))
            .unwrap();
        let r1 = library.run_spending("r1").unwrap();
        assert_eq!(r1.counted_since.as_deref(), Some(&since[..10]));
        assert_eq!(r1.total.price_micros, 5, "the call of no run is not guessed into the run's");
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
