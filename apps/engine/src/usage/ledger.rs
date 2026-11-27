//! The totals the interface shows (`Usage` in `packages/contracts`), worked out from the calls that were made.
//!
//! Only arithmetic lives here. Keeping the calls is the job of whoever records them; they hand this the list, and
//! get back what to show: totals by model and by purpose (naming communities, overviews, training the AI-text
//! detector), how many calls had no known price, and the currency with the rate and its day.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{Rate, Usd, price};

/// One call to the API, as recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// The model that answered, as the API named it (it may carry a date).
    pub model: String,
    /// What the call was for, in the interface's words: "naming communities".
    pub purpose: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Calls added up: `UsageEntry` in the contracts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Tally {
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Of the priced calls only: unpriced ones are counted in tokens and in `unpriced_calls`.
    pub usd: Usd,
    pub unpriced_calls: u64,
}

impl Tally {
    fn add(&mut self, call: &Call, cost: Option<Usd>) {
        self.calls += 1;
        self.input_tokens = self.input_tokens.saturating_add(call.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(call.output_tokens);
        match cost {
            Some(cost) => self.usd += cost,
            None => self.unpriced_calls += 1,
        }
    }
}

/// What the calls cost, every amount in US dollars (the currency the API bills in), and the currency to show it
/// in. The fields the interface also wants and that are not about the calls (the provider, the model in use, the
/// day the ledger starts) are added by whoever answers it; the session's totals are a [`Summary`] of the session's
/// calls.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Summary {
    pub currency: Rate,
    pub usd: Usd,
    pub calls: u64,
    /// Calls to a model whose price is not known: `usd` is short by them, and the interface says so.
    pub unpriced_calls: u64,
    pub models: BTreeMap<String, Tally>,
    pub purposes: BTreeMap<String, Tally>,
}

impl Summary {
    /// Each call priced once, then added to its model's and its purpose's totals. Exact: the order of the calls
    /// and their number change nothing but the totals themselves.
    pub fn of<'a>(calls: impl IntoIterator<Item = &'a Call>, currency: Rate) -> Self {
        let mut models: BTreeMap<String, Tally> = BTreeMap::new();
        let mut purposes: BTreeMap<String, Tally> = BTreeMap::new();
        for call in calls {
            let cost = price(&call.model, call.input_tokens, call.output_tokens);
            models.entry(call.model.clone()).or_default().add(call, cost);
            purposes.entry(call.purpose.clone()).or_default().add(call, cost);
        }
        let all = models.values();
        Self {
            currency,
            usd: all.clone().map(|tally| tally.usd).sum(),
            calls: all.clone().map(|tally| tally.calls).sum(),
            unpriced_calls: all.map(|tally| tally.unpriced_calls).sum(),
            models,
            purposes,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::usage::rate;
    use serde_json::json;

    fn call(model: &str, purpose: &str, input_tokens: u64, output_tokens: u64) -> Call {
        Call { model: model.to_owned(), purpose: purpose.to_owned(), input_tokens, output_tokens }
    }

    #[test]
    fn calls_add_up_by_model_and_by_purpose() {
        let calls = [
            call("claude-opus-5-5", "naming communities", 10_000, 1_000),
            call("claude-opus-5-5", "community overviews", 100_000, 5_000),
            call("some-other-model", "naming communities", 500, 500),
        ];
        let told = Summary::of(&calls, Rate::dollar());
        assert_eq!((told.calls, told.unpriced_calls), (3, 1));
        assert_eq!(told.usd, Usd::from_picodollars(560_000_000_000)); // 0.06 + 0.5 dollars
        assert_eq!(told.models["claude-opus-5-5"].input_tokens, 110_000);
        assert_eq!(told.models["some-other-model"].usd, Usd::ZERO);
        assert_eq!(told.purposes["naming communities"].calls, 2);
        assert_eq!(told.purposes["naming communities"].unpriced_calls, 1);
    }

    #[test]
    fn many_tiny_calls_do_not_drift() {
        let calls = vec![call("claude-haiku-5-5", "naming communities", 7, 3); 320];
        let told = Summary::of(&calls, Rate::dollar());
        // 320 * (7 * 0.10 + 3 * 0.50) micro-dollars, to the last picodollar
        assert_eq!(told.usd, Usd::from_picodollars(320 * 2_200_000));
        assert_eq!(told.models["claude-haiku-5-5"].usd, told.usd);
    }

    #[test]
    fn a_dated_model_id_is_priced_but_kept_as_the_api_named_it() {
        let told = Summary::of(&[call("claude-opus-5-5-20261101", "other", 1_000_000, 0)], Rate::dollar());
        assert_eq!(told.models["claude-opus-5-5-20261101"].usd, Usd::from_picodollars(4_000_000_000_000));
        assert_eq!(told.unpriced_calls, 0);
    }

    #[test]
    fn nothing_spent_is_a_summary_of_nothing() {
        let told = Summary::of(&[], Rate::dollar());
        assert_eq!((told.usd, told.calls, told.unpriced_calls), (Usd::ZERO, 0, 0));
        assert!(told.models.is_empty() && told.purposes.is_empty());
    }

    #[test]
    fn the_summary_is_told_to_the_interface_in_its_words() {
        let euro = rate("EUR");
        let told = serde_json::to_value(Summary::of(&[call("claude-opus-5-5", "other", 1_000_000, 0)], euro.clone()));
        let told = told.unwrap();
        assert_eq!(told["usd"], json!(4.0));
        assert_eq!(told["currency"]["code"], json!("EUR"));
        assert_eq!(told["currency"]["date"], json!(euro.date.unwrap().to_string()));
        assert_eq!(
            told["models"]["claude-opus-5-5"],
            json!({"calls": 1, "input_tokens": 1_000_000, "output_tokens": 0, "usd": 4.0, "unpriced_calls": 0})
        );
        assert_eq!(told["purposes"]["other"]["calls"], json!(1));
    }
}
