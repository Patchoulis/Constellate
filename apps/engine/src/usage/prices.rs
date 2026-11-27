//! What one call to a model cost, from its tokens and the model's published prices.
//!
//! Amounts are whole numbers, never floating point: a call to a small model costs a fraction of a micro-dollar
//! (seven input tokens to Haiku 5.5 are 0.7 of one), and adding up thousands of such calls in floating point, or
//! in whole micro-dollars, would drift. A price of `p` dollars per million tokens is exactly `p` micro-dollars per
//! token, that is `p * 10^6` picodollars per token, a whole number for every price written to the micro-dollar.
//! So [`Usd`] counts picodollars, and every call and every sum of calls is exact.

use std::iter::Sum;
use std::ops::{Add, AddAssign};

use serde::{Deserialize, Serialize, Serializer};

use super::{Invalid, bundled};

/// Picodollars in a dollar.
const PICODOLLARS: u64 = 1_000_000_000_000;

/// An amount of US dollars, held exactly as a whole number of picodollars (millionths of a micro-dollar).
///
/// Up to about 18 million dollars; past that an amount stays at the largest one rather than wrapping round to a
/// small one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Usd(u64);

impl Usd {
    pub const ZERO: Self = Self(0);

    pub const fn from_picodollars(picodollars: u64) -> Self {
        Self(picodollars)
    }

    pub const fn picodollars(self) -> u64 {
        self.0
    }

    /// In dollars, for showing: the one place an amount becomes floating point.
    pub fn dollars(self) -> f64 {
        self.0 as f64 / PICODOLLARS as f64
    }
}

impl Add for Usd {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl AddAssign for Usd {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sum for Usd {
    fn sum<I: Iterator<Item = Self>>(amounts: I) -> Self {
        amounts.fold(Self::ZERO, Add::add)
    }
}

/// The interface is told dollars (`usd: number` in the contracts).
impl Serialize for Usd {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.dollars())
    }
}

/// What a model's tokens cost, in picodollars per token (the same number as micro-dollars per million tokens).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PerToken {
    pub input: u64,
    pub output: u64,
}

impl PerToken {
    fn of(self, input_tokens: u64, output_tokens: u64) -> Usd {
        let input = input_tokens.saturating_mul(self.input);
        Usd(input.saturating_add(output_tokens.saturating_mul(self.output)))
    }
}

/// A model's prices: one rate, and for some models a second one for long prompts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelPrice {
    pub rate: PerToken,
    pub long_prompt: Option<LongPrompt>,
}

/// The rate of a prompt longer than `above` input tokens, for the whole call (input and output).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LongPrompt {
    pub above: u64,
    pub rate: PerToken,
}

impl ModelPrice {
    pub fn of(&self, input_tokens: u64, output_tokens: u64) -> Usd {
        let rate = match self.long_prompt {
            Some(long) if input_tokens > long.above => long.rate,
            _ => self.rate,
        };
        rate.of(input_tokens, output_tokens)
    }
}

/// What a call cost, or `None` for a model whose price is not known here: unpriced, which is not the same as
/// free. The API may answer with a dated id (`claude-opus-5-5-20261101`); it has the price of the model it names.
pub fn price(model: &str, input_tokens: u64, output_tokens: u64) -> Option<Usd> {
    let models = &bundled().ok()?.models;
    Some(models.get(undated(model))?.of(input_tokens, output_tokens))
}

/// `claude-opus-5-5-20261101` -> `claude-opus-5-5`; any other id as it is.
fn undated(model: &str) -> &str {
    match model.rsplit_once('-') {
        Some((name, date)) if date.len() == 8 && date.bytes().all(|byte| byte.is_ascii_digit()) => name,
        _ => model,
    }
}

/// A model's prices as written in the file: US dollars per million tokens.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Written {
    input: f64,
    output: f64,
    long_prompt: Option<WrittenLongPrompt>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenLongPrompt {
    above: u64,
    input: f64,
    output: f64,
}

/// Dollars per million tokens -> picodollars per token, refusing what is not a price or cannot be held exactly.
fn per_token(dollars_per_million: f64, model: &str) -> Result<u64, Invalid> {
    // a million dollars per million tokens is far past any real price, and keeps every product well inside u64
    if !(dollars_per_million.is_finite() && (0.0..=1e6).contains(&dollars_per_million)) {
        return Err(Invalid::Price(model.to_owned()));
    }
    let micro = dollars_per_million * 1e6;
    let whole = micro.round();
    // 0.1 * 1e6 is 100000.00000000001 in floating point: closer than this is the whole number that was meant
    if (micro - whole).abs() > 1e-3 {
        return Err(Invalid::TooFine(model.to_owned()));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // whole, 0 or more, at most 10^12: checked
    Ok(whole as u64)
}

impl Written {
    pub(super) fn checked(self, model: &str) -> Result<ModelPrice, Invalid> {
        let rate = PerToken { input: per_token(self.input, model)?, output: per_token(self.output, model)? };
        let long_prompt = match self.long_prompt {
            None => None,
            Some(long) if long.above == 0 => return Err(Invalid::LongPrompt(model.to_owned())),
            Some(long) => Some(LongPrompt {
                above: long.above,
                rate: PerToken { input: per_token(long.input, model)?, output: per_token(long.output, model)? },
            }),
        };
        Ok(ModelPrice { rate, long_prompt })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// An exact amount from a whole number of micro-dollars and a fraction of one, for readable expectations.
    fn micro(micro_dollars: f64) -> Usd {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Usd::from_picodollars((micro_dollars * 1e6).round() as u64)
    }

    fn dollars(dollars: f64) -> Usd {
        micro(dollars * 1e6)
    }

    #[test]
    fn a_call_is_priced_by_its_model_and_its_length() {
        assert_eq!(price("claude-opus-5-5", 1_000_000, 100_000), Some(dollars(4.0 + 2.0)));
        assert_eq!(price("claude-haiku-5-5", 50_000, 10_000), Some(dollars(0.005 + 0.005)));
        // the long-prompt rate, for the whole call
        assert_eq!(price("claude-haiku-5-5", 200_000, 10_000), Some(dollars(0.10 + 0.025)));
        assert_eq!(price("some-other-model", 10, 10), None);
    }

    #[test]
    fn the_long_prompt_rate_starts_after_its_limit_not_at_it() {
        assert_eq!(price("claude-haiku-5-5", 100_000, 0), Some(dollars(0.01)));
        assert_eq!(price("claude-haiku-5-5", 100_001, 0), Some(micro(100_001.0 * 0.5)));
    }

    #[test]
    fn the_model_that_answered_finds_its_price_whatever_the_api_calls_it() {
        assert_eq!(price("claude-opus-5-5-20261101", 1_000_000, 0), Some(dollars(4.0)));
        assert_eq!(price("claude-sonnet-5", 1_000_000, 0), Some(dollars(2.0))); // what Sonnet 5.5 falls back to
        assert_eq!(price("claude-opus-5", 0, 1_000_000), Some(dollars(25.0)));
    }

    #[test]
    fn an_unknown_model_is_unpriced_not_free() {
        for model in ["", "claude-opus-5-5-latest", "claude-opus-5-5-2026110", "CLAUDE-OPUS-5-5", "gpt-5"] {
            assert_eq!(price(model, 1_000, 1_000), None, "{model}");
        }
        assert_eq!(price("claude-opus-5-5", 0, 0), Some(Usd::ZERO)); // a known model with no tokens is free
    }

    #[test]
    fn a_tiny_call_is_priced_exactly() {
        // 7 * 0.1 + 3 * 0.5 micro-dollars: less than a micro-dollar's worth of input is still counted
        assert_eq!(price("claude-haiku-5-5", 7, 3), Some(Usd::from_picodollars(2_200_000)));
    }

    #[test]
    fn the_prices_are_the_published_ones() {
        let models = &bundled().unwrap().models;
        let rate = |model: &str| models[model].rate;
        assert_eq!(rate("claude-opus-5-5"), PerToken { input: 4_000_000, output: 20_000_000 });
        assert_eq!(rate("claude-sonnet-5-5"), PerToken { input: 2_000_000, output: 10_000_000 });
        assert_eq!(rate("claude-haiku-5-5"), PerToken { input: 100_000, output: 500_000 });
        assert_eq!(rate("claude-fable-5-2"), PerToken { input: 8_000_000, output: 40_000_000 });
        assert_eq!(rate("claude-opus-5"), PerToken { input: 5_000_000, output: 25_000_000 });
        assert_eq!(rate("claude-opus-4-8"), PerToken { input: 5_000_000, output: 25_000_000 });
        assert_eq!(rate("claude-sonnet-5"), PerToken { input: 2_000_000, output: 10_000_000 });
        assert_eq!(
            models["claude-haiku-5-5"].long_prompt,
            Some(LongPrompt { above: 100_000, rate: PerToken { input: 500_000, output: 2_500_000 } })
        );
        assert_eq!(models.values().filter(|model| model.long_prompt.is_some()).count(), 1);
    }

    #[test]
    fn an_amount_too_large_to_hold_stays_at_the_largest_rather_than_wrapping() {
        let most = price("claude-fable-5-2", u64::MAX, u64::MAX).unwrap();
        assert_eq!(most, Usd::from_picodollars(u64::MAX));
        assert_eq!(most + dollars(1.0), most);
    }

    #[test]
    fn amounts_are_shown_in_dollars() {
        assert_eq!(serde_json::to_value(dollars(6.0)).unwrap(), serde_json::json!(6.0));
        assert!((Usd::from_picodollars(2_200_000).dollars() - 2.2e-6).abs() < 1e-18);
    }
}
