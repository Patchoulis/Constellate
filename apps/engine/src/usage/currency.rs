//! What a US dollar is worth in another currency, and on which day it was worth that.
//!
//! The Claude API bills in US dollars; the interface shows what was spent in the currency of the user's region.
//! The rates are the European Central Bank's reference rates of the day written in the bundled file: never asked
//! for at run time, so nothing about the user, not even their currency, leaves the computer for this. A currency
//! the file has no rate for is shown as what was billed, in dollars.

use chrono::NaiveDate;
use serde::{Serialize, Serializer};

use super::{Usd, bundled};

/// A currency and what a US dollar buys of it, told to the interface as `{code, rate, date}`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Rate {
    /// ISO 4217: `CAD`.
    pub code: String,
    /// How many of it one US dollar buys.
    #[serde(rename = "rate")]
    pub per_dollar: f64,
    /// The day the bank published the rate, for "at the rate of <date>"; none for the dollar itself.
    #[serde(serialize_with = "day")]
    pub date: Option<NaiveDate>,
}

fn day<S: Serializer>(date: &Option<NaiveDate>, serializer: S) -> Result<S::Ok, S::Error> {
    match date {
        Some(date) => serializer.collect_str(date),
        None => serializer.serialize_none(),
    }
}

impl Rate {
    /// The currency the API bills in, which needs no rate.
    pub fn dollar() -> Self {
        Self { code: "USD".to_owned(), per_dollar: 1.0, date: None }
    }

    /// An amount in this currency, for showing.
    pub fn of(&self, amount: Usd) -> f64 {
        amount.dollars() * self.per_dollar
    }
}

/// Three letters, upper case: the shape of a currency code.
pub(super) fn is_code(code: &str) -> bool {
    code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_uppercase())
}

/// The rate of a currency, by its code in any case (`"cad"`, `"CAD"`). The dollar itself for `USD`, for anything
/// that is not a currency code, and for a currency the bundled file has no rate for.
pub fn rate(code: &str) -> Rate {
    let code = code.trim().to_ascii_uppercase();
    if !is_code(&code) || code == "USD" {
        return Rate::dollar();
    }
    let Ok(bundled) = bundled() else { return Rate::dollar() };
    match bundled.per_dollar.get(&code) {
        Some(&per_dollar) => Rate { code, per_dollar, date: Some(bundled.rates_date) },
        None => Rate::dollar(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn the_dollar_is_itself_and_has_no_date() {
        assert_eq!(rate("USD"), Rate { code: "USD".to_owned(), per_dollar: 1.0, date: None });
        assert_eq!(rate("usd"), Rate::dollar());
    }

    #[test]
    fn a_currency_has_the_bundled_rate_and_the_day_it_is_from() {
        let euro = rate("EUR");
        let bundled = bundled().unwrap();
        assert_eq!(euro.code, "EUR");
        assert_eq!(euro.per_dollar, bundled.per_dollar["EUR"]);
        assert_eq!(euro.date, Some(bundled.rates_date));
    }

    #[test]
    fn a_code_is_read_whatever_its_case_or_spaces() {
        assert_eq!(rate(" cad "), rate("CAD"));
        assert_eq!(rate("CAD").code, "CAD");
    }

    #[test]
    fn what_is_not_a_known_currency_is_the_dollar() {
        // the Python's nonsense codes too: none is ever looked up, nor could name a file
        for code in ["", "EURO", "E1R", "XYZ", "€", "dollars", "C$", "../x", "XXX"] {
            assert_eq!(rate(code), Rate::dollar(), "{code:?}");
        }
    }

    #[test]
    fn every_currency_the_interface_can_show_has_a_rate() {
        // units.ts maps a region to its currency; a currency added there and missing here would be shown in dollars
        let units = Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/lib/units.ts");
        let units = std::fs::read_to_string(units).unwrap();
        let table = &units[units.find("const CURRENCY_OF").unwrap()..];
        let table = &table[..table.find("};").unwrap()];
        let codes: Vec<&str> = table.split('"').skip(1).step_by(2).filter(|text| is_code(text)).collect();
        assert!(codes.len() > 10, "units.ts was not read: {codes:?}");
        assert!(units.contains("const EURO"));
        for code in codes.into_iter().chain(["EUR"]).filter(|code| *code != "USD") {
            assert_ne!(rate(code), Rate::dollar(), "{code} has no rate in apps/engine/data/prices.json");
        }
    }

    #[test]
    fn a_rate_is_told_to_the_interface_as_code_rate_and_date() {
        let told = serde_json::to_value(Rate {
            code: "CAD".to_owned(),
            per_dollar: 1.5,
            date: NaiveDate::from_ymd_opt(2026, 11, 26),
        });
        assert_eq!(told.unwrap(), json!({"code": "CAD", "rate": 1.5, "date": "2026-11-26"}));
        assert_eq!(serde_json::to_value(Rate::dollar()).unwrap(), json!({"code": "USD", "rate": 1.0, "date": null}));
    }

    #[test]
    fn an_amount_is_converted_at_the_rate() {
        let rate = Rate { code: "CAD".to_owned(), per_dollar: 1.5, date: None };
        assert!((rate.of(Usd::from_picodollars(2_000_000_000_000)) - 3.0).abs() < 1e-12);
    }
}
