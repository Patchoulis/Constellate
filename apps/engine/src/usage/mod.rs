//! What the language model has cost: each call priced in US dollars, the calls summed, and the sum shown in the
//! currency of the user's region. A model whose price is not known (a model of the user's own server, one typed
//! rather than listed) is unpriced, which is not free: its calls are counted in tokens only.
//!
//! Prices and exchange rates are not asked for at run time. Both are in `apps/engine/data/prices.json`, compiled
//! into the engine and read once: the model prices as the provider publishes them, the exchange rates as the
//! European Central Bank published them on the day written beside them. A scheduled workflow
//! (`.github/workflows/prices.yml`, through `scripts/update-prices.mjs`) refreshes the rates and releases a new
//! version when one has moved by more than 1%. The rates an app has are therefore as old as its version, which
//! is why the day they are from is always shown beside them.
//!
//! | module       | what lives there                                                       |
//! |--------------|------------------------------------------------------------------------|
//! | [`prices`]   | what one call cost, exactly, in [`Usd`]                                 |
//! | [`currency`] | what a dollar is worth in another currency, and since when             |
//! | [`ledger`]   | the totals the interface shows, worked out from a list of calls        |
//!
//! Keeping the calls is not done here: whoever stores them hands them to [`ledger::Summary::of`].

pub mod currency;
pub mod ledger;
pub mod prices;

use std::collections::BTreeMap;
use std::sync::LazyLock;

use chrono::NaiveDate;
use serde::Deserialize;

pub use currency::{Rate, rate};
pub use ledger::{Call, Summary, Tally};
pub use prices::{CacheRates, ModelPrice, Tokens, Usd, model_price, price, price_of};

/// The file as it is in the repository; `scripts/update-prices.mjs` is the only thing that rewrites it.
const BUNDLED_TEXT: &str = include_str!("../../data/prices.json");

/// Read on first use. The tests (and the pre-commit hook, and the workflow) make sure the file is valid; should it
/// not be all the same, every call is unpriced and every currency is the dollar, rather than the app failing.
static BUNDLED: LazyLock<Result<Bundled, Invalid>> = LazyLock::new(|| Bundled::parse(BUNDLED_TEXT));

/// The prices and rates compiled into the engine.
pub fn bundled() -> Result<&'static Bundled, &'static Invalid> {
    BUNDLED.as_ref()
}

/// Model prices and exchange rates, checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Bundled {
    /// The day the file was last changed.
    pub updated: NaiveDate,
    /// By model id without a date: `claude-opus-5-5`; another provider's as `provider:id` (`openai:gpt-6.2-sol`),
    /// since two providers may sell one model id at two prices.
    pub models: BTreeMap<String, ModelPrice>,
    /// Who sells each model at that price, by the same key: a provider id (`anthropic`, `openai`).
    pub providers: BTreeMap<String, String>,
    /// Where each provider's prices were read, and on which day, by provider id.
    pub sources: BTreeMap<String, Source>,
    /// The day the rates are from: the bank's publication, not the day they were fetched.
    pub rates_date: NaiveDate,
    /// Where the rates are from, in words: "European Central Bank via frankfurter.dev".
    pub rates_source: String,
    /// By currency code: how many of it one US dollar buys.
    pub per_dollar: BTreeMap<String, f64>,
}

/// Why the prices file cannot be used, naming what in it is wrong.
#[derive(Debug, thiserror::Error)]
pub enum Invalid {
    #[error("the prices file is not what it should be: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0:?} is not a day written YYYY-MM-DD")]
    Day(String),
    #[error("the price of {0} is not a number of US dollars per million tokens, 0 or more")]
    Price(String),
    #[error("the price of {0} is finer than a millionth of a dollar per million tokens")]
    TooFine(String),
    #[error("the long-prompt rate of {0} starts at 0 tokens")]
    LongPrompt(String),
    #[error("the exchange rates are against {0}, not against the US dollar")]
    Base(String),
    #[error("{0:?} is not the code of a currency other than the US dollar")]
    Code(String),
    #[error("the rate of {0} is not a number above 0")]
    Rate(String),
    #[error("the provider of {0} has no source in the file: where were its prices read?")]
    Unsourced(String),
    #[error("{0} is not written as its provider's models are: provider:id, or a Claude model's own id")]
    Key(String),
}

/// Where one provider's prices were read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// The provider's own pricing page.
    pub url: String,
    /// The day they were read there.
    pub fetched: NaiveDate,
    /// What a reader of the prices must know (a lifetime of the cache that is not five minutes, a price that
    /// changes on a known day), if anything.
    pub note: Option<String>,
}

/// The file as written. Unknown fields are refused: a field nobody reads is usually a misspelt one.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    updated: String,
    sources: BTreeMap<String, WrittenSource>,
    models: BTreeMap<String, prices::Written>,
    rates: Rates,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenSource {
    url: String,
    fetched: String,
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rates {
    base: String,
    date: String,
    source: String,
    per_dollar: BTreeMap<String, f64>,
}

fn day(text: &str) -> Result<NaiveDate, Invalid> {
    let parsed = if text.len() == 10 { NaiveDate::parse_from_str(text, "%Y-%m-%d").ok() } else { None };
    parsed.ok_or_else(|| Invalid::Day(text.to_owned()))
}

impl Bundled {
    /// The same rules as `problemsOf` in `scripts/update-prices.mjs`, which checks the file before it is committed.
    pub fn parse(text: &str) -> Result<Self, Invalid> {
        let file: File = serde_json::from_str(text)?;
        let mut sources = BTreeMap::new();
        for (provider, source) in file.sources {
            if !source.url.starts_with("https://") {
                return Err(Invalid::Unsourced(provider));
            }
            let fetched = day(&source.fetched)?;
            sources.insert(provider, Source { url: source.url, fetched, note: source.note });
        }
        let mut models = BTreeMap::new();
        let mut providers = BTreeMap::new();
        for (id, written) in file.models {
            let provider = written.provider.clone();
            if !sources.contains_key(&provider) {
                return Err(Invalid::Unsourced(id));
            }
            // a Claude model keeps its own id, as the ledger has always written it; any other is provider:id
            let keyed = match id.split_once(':') {
                Some((prefix, rest)) => prefix == provider && provider != "anthropic" && !rest.is_empty(),
                None => provider == "anthropic",
            };
            if !keyed {
                return Err(Invalid::Key(id));
            }
            models.insert(id.clone(), written.checked(&id)?);
            providers.insert(id, provider);
        }
        if file.rates.base != "USD" {
            return Err(Invalid::Base(file.rates.base));
        }
        for (code, rate) in &file.rates.per_dollar {
            if !currency::is_code(code) || code == "USD" {
                return Err(Invalid::Code(code.clone()));
            }
            if !(rate.is_finite() && *rate > 0.0) {
                return Err(Invalid::Rate(code.clone()));
            }
        }
        Ok(Self {
            updated: day(&file.updated)?,
            models,
            providers,
            sources,
            rates_date: day(&file.rates.date)?,
            rates_source: file.rates.source,
            per_dollar: file.rates.per_dollar,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// The bundled file with one change made to it.
    fn changed(change: impl FnOnce(&mut Value)) -> Result<Bundled, Invalid> {
        let mut file: Value = serde_json::from_str(BUNDLED_TEXT).unwrap();
        change(&mut file);
        Bundled::parse(&file.to_string())
    }

    #[test]
    fn the_bundled_file_is_valid() {
        let bundled = bundled().unwrap();
        assert!(bundled.models.contains_key("claude-opus-5-5"));
        assert!(bundled.per_dollar.contains_key("EUR"));
        assert!(bundled.rates_date <= bundled.updated);
    }

    #[test]
    fn a_price_that_is_not_a_price_is_refused() {
        let price = |value: Value| changed(|file| file["models"]["claude-opus-5-5"]["input"] = value);
        assert!(matches!(price(json!(-1.0)), Err(Invalid::Price(model)) if model == "claude-opus-5-5"));
        assert!(matches!(price(json!("4")), Err(Invalid::Json(_))));
        assert!(matches!(price(json!(0.000_000_1)), Err(Invalid::TooFine(_))));
        assert!(price(json!(0.0)).is_ok()); // a free model is a price too
    }

    #[test]
    fn a_misspelt_field_is_refused_rather_than_ignored() {
        let misspelt = changed(|file| file["models"]["claude-opus-5-5"]["ouptut"] = json!(20.0));
        assert!(matches!(misspelt, Err(Invalid::Json(_))));
    }

    #[test]
    fn rates_must_be_against_the_dollar_by_currency_code_and_above_zero() {
        assert!(matches!(changed(|file| file["rates"]["base"] = json!("EUR")), Err(Invalid::Base(_))));
        assert!(matches!(changed(|file| file["rates"]["per_dollar"]["eur"] = json!(0.9)), Err(Invalid::Code(_))));
        assert!(matches!(changed(|file| file["rates"]["per_dollar"]["USD"] = json!(1.0)), Err(Invalid::Code(_))));
        assert!(matches!(changed(|file| file["rates"]["per_dollar"]["EUR"] = json!(0.0)), Err(Invalid::Rate(_))));
    }

    #[test]
    fn every_price_names_its_provider_with_a_source_and_is_kept_under_its_provider() {
        let bundled = bundled().unwrap();
        assert_eq!(bundled.providers["claude-opus-5-5"], "anthropic");
        assert_eq!(bundled.providers["openai:gpt-6.2-sol"], "openai");
        assert!(bundled.sources["openai"].url.starts_with("https://developers.openai.com/"));
        let unsourced = changed(|file| file["models"]["openai:gpt-6.2-sol"]["provider"] = json!("nobody"));
        assert!(matches!(unsourced, Err(Invalid::Unsourced(_))));
        let unqualified = changed(|file| {
            let model = file["models"]["openai:gpt-6.2-sol"].clone();
            file["models"]["gpt-6.2-sol"] = model;
        });
        assert!(matches!(unqualified, Err(Invalid::Key(id)) if id == "gpt-6.2-sol"));
        let mislabelled = changed(|file| file["models"]["claude-opus-5-5"]["provider"] = json!("openai"));
        assert!(matches!(mislabelled, Err(Invalid::Key(_))));
        let undated = changed(|file| file["sources"]["openai"]["fetched"] = json!("last week"));
        assert!(matches!(undated, Err(Invalid::Day(_))));
    }

    #[test]
    fn days_are_written_in_full_and_on_the_calendar() {
        assert!(matches!(changed(|file| file["rates"]["date"] = json!("2026-02-30")), Err(Invalid::Day(_))));
        assert!(matches!(changed(|file| file["updated"] = json!("2026-1-5")), Err(Invalid::Day(_))));
    }
}
