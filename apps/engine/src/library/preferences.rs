//! The user's settings and the exchange rates last asked for.
//!
//! Settings are kept as given, one JSON value per name; which names exist, their defaults and what each may be
//! are the settings module's business, not the storage's. Changing several settings is one transaction, so a
//! change the settings module refused in part is never kept in part.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Library, LibraryError};

/// What one US dollar buys in a currency, as the bank gave it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rate {
    /// ISO 4217 code: "EUR".
    pub code: String,
    pub rate: f64,
    /// The day the bank's rate is from.
    pub date: String,
    /// The day it was asked for: a rate is asked for once a day.
    pub asked: String,
}

pub(super) fn put_setting(
    connection: &Connection,
    name: &str,
    value: &Value,
    replace: bool,
) -> Result<usize, LibraryError> {
    let sql = if replace {
        "INSERT INTO settings (name, value) VALUES (?1, ?2) ON CONFLICT (name) DO UPDATE SET value = excluded.value"
    } else {
        "INSERT INTO settings (name, value) VALUES (?1, ?2) ON CONFLICT (name) DO NOTHING"
    };
    Ok(connection.execute(sql, params![name, serde_json::to_string(value)?])?)
}

pub(super) fn put_rate(connection: &Connection, rate: &Rate, replace: bool) -> rusqlite::Result<usize> {
    let conflict = if replace {
        "DO UPDATE SET rate = excluded.rate, rate_date = excluded.rate_date, asked = excluded.asked"
    } else {
        "DO NOTHING"
    };
    connection.execute(
        &format!("INSERT INTO exchange_rates (code, rate, rate_date, asked) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (code) {conflict}"),
        params![rate.code, rate.rate, rate.date, rate.asked],
    )
}

impl Library {
    /// Every setting that was ever changed, by name.
    pub fn settings(&self) -> Result<Map<String, Value>, LibraryError> {
        self.read(|connection| {
            let mut statement = connection.prepare("SELECT name, value FROM settings ORDER BY name")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, super::json_column(row, 1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Change these settings, all or none.
    pub fn change_settings(&self, changes: &Map<String, Value>) -> Result<(), LibraryError> {
        self.write(|transaction| {
            for (name, value) in changes {
                put_setting(transaction, name, value, true)?;
            }
            Ok(())
        })
    }

    pub fn rate(&self, code: &str) -> Result<Option<Rate>, LibraryError> {
        self.read(|connection| {
            Ok(connection
                .query_row("SELECT code, rate, rate_date, asked FROM exchange_rates WHERE code = ?1", [code], |row| {
                    Ok(Rate { code: row.get(0)?, rate: row.get(1)?, date: row.get(2)?, asked: row.get(3)? })
                })
                .optional()?)
        })
    }

    pub fn keep_rate(&self, rate: &Rate) -> Result<(), LibraryError> {
        self.write(|transaction| Ok(put_rate(transaction, rate, true).map(drop)?))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_are_read_back_as_they_were_changed() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        assert!(library.settings().unwrap().is_empty());
        let changes = json!({"max_links": 8, "exclude_flags": ["llm_declared"], "describe_model": "claude-opus-5-5"});
        library.change_settings(changes.as_object().unwrap()).unwrap();
        library.change_settings(json!({"max_links": 12}).as_object().unwrap()).unwrap();
        let settings = library.settings().unwrap();
        assert_eq!(
            Value::Object(settings),
            json!({"max_links": 12, "exclude_flags": ["llm_declared"], "describe_model": "claude-opus-5-5"})
        );
    }

    #[test]
    fn a_rate_is_kept_per_currency_and_replaced_when_asked_again() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        assert_eq!(library.rate("CAD").unwrap(), None);
        let rate = Rate { code: "CAD".into(), rate: 1.42, date: "2026-11-25".into(), asked: "2026-11-26".into() };
        library.keep_rate(&rate).unwrap();
        assert_eq!(library.rate("CAD").unwrap(), Some(rate.clone()));
        let today = Rate { rate: 1.43, asked: "2026-11-27".into(), ..rate };
        library.keep_rate(&today).unwrap();
        assert_eq!(library.rate("CAD").unwrap(), Some(today));
        assert!(
            library.keep_rate(&Rate { code: "XXX".into(), rate: 0.0, date: "d".into(), asked: "d".into() }).is_err()
        );
    }
}
