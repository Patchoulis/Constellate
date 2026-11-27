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

/// The tokens of one call, by how they are billed. `input` is the prompt read at the full rate: the API's
/// `input_tokens`, which leaves out what was read from or written to the prompt cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    /// Read from the prompt cache (`cache_read_input_tokens`).
    pub cache_read: u64,
    /// Written to the prompt cache to live five minutes, or an hour (`cache_creation`, by lifetime).
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
}

impl Tokens {
    /// The whole prompt, however it was billed: what a long-prompt rate is chosen by.
    pub fn prompt(&self) -> u64 {
        self.input.saturating_add(self.cache_read).saturating_add(self.cache_written())
    }

    /// Tokens written to the cache, either lifetime.
    pub fn cache_written(&self) -> u64 {
        self.cache_write_5m.saturating_add(self.cache_write_1h)
    }

    /// Every token of the call, prompt and answer: what a budget in tokens counts.
    pub fn all(&self) -> u64 {
        self.prompt().saturating_add(self.output)
    }

    fn cached(&self) -> bool {
        self.cache_read > 0 || self.cache_written() > 0
    }
}

impl Add for Tokens {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            cache_read: self.cache_read.saturating_add(other.cache_read),
            cache_write_5m: self.cache_write_5m.saturating_add(other.cache_write_5m),
            cache_write_1h: self.cache_write_1h.saturating_add(other.cache_write_1h),
        }
    }
}

/// What the prompt cache costs per token, in picodollars, as published beside a model's other prices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheRates {
    pub read: u64,
    pub write_5m: u64,
    pub write_1h: u64,
}

/// What a model's tokens cost, in picodollars per token (the same number as micro-dollars per million tokens).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PerToken {
    pub input: u64,
    pub output: u64,
    /// `None` for a model whose cache prices are not known here: a call that used the cache is then unpriced.
    pub cache: Option<CacheRates>,
}

impl PerToken {
    fn of(self, tokens: &Tokens) -> Option<Usd> {
        let times = |count: u64, rate: u64| count.saturating_mul(rate);
        let plain = times(tokens.input, self.input).saturating_add(times(tokens.output, self.output));
        let cache = match self.cache {
            Some(cache) => times(tokens.cache_read, cache.read)
                .saturating_add(times(tokens.cache_write_5m, cache.write_5m))
                .saturating_add(times(tokens.cache_write_1h, cache.write_1h)),
            None if tokens.cached() => return None,
            None => 0,
        };
        Some(Usd(plain.saturating_add(cache)))
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
    /// The rate a call is billed at: the long-prompt one when its whole prompt, cached or not, is past the limit.
    pub fn rate_for(&self, tokens: &Tokens) -> PerToken {
        match self.long_prompt {
            Some(long) if tokens.prompt() > long.above => long.rate,
            _ => self.rate,
        }
    }

    /// What `tokens` cost; `None` when they used the prompt cache and its prices are not known.
    pub fn cost(&self, tokens: &Tokens) -> Option<Usd> {
        self.rate_for(tokens).of(tokens)
    }

    /// What a call that did not use the prompt cache cost.
    pub fn of(&self, input_tokens: u64, output_tokens: u64) -> Usd {
        let tokens = Tokens { input: input_tokens, output: output_tokens, ..Tokens::default() };
        self.cost(&tokens).unwrap_or(Usd::ZERO) // no cache tokens, so always priced
    }
}

/// What a call cost, or `None` for a model whose price is not known here: unpriced, which is not the same as
/// free. The API may answer with a dated id (`claude-opus-5-5-20261101`); it has the price of the model it names.
pub fn price(model: &str, input_tokens: u64, output_tokens: u64) -> Option<Usd> {
    price_of(model, &Tokens { input: input_tokens, output: output_tokens, ..Tokens::default() })
}

/// What a call cost, cache reads and writes each at their own price; `None` for a model whose price, or whose
/// cache prices when the call used the cache, are not known here.
pub fn price_of(model: &str, tokens: &Tokens) -> Option<Usd> {
    model_price(model)?.cost(tokens)
}

/// A model's prices, whatever date its id carries.
pub fn model_price(model: &str) -> Option<&'static ModelPrice> {
    bundled().ok()?.models.get(undated(model))
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
    /// Who sells the model at this price: a provider id of `model::language::Provider`, with an entry in the
    /// file's `sources`.
    pub(super) provider: String,
    input: f64,
    output: f64,
    cache: Option<WrittenCache>,
    long_prompt: Option<WrittenLongPrompt>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenLongPrompt {
    above: u64,
    input: f64,
    output: f64,
    cache: Option<WrittenCache>,
}

/// The prompt cache's prices as written: reading from it, and writing to it for five minutes or for an hour (for
/// a provider whose lifetimes differ, its short one and its long one, as its source in the file says). A provider
/// that caches by itself and publishes no price for writing has only `read`: what it writes is billed as input.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenCache {
    read: f64,
    write_5m: Option<f64>,
    write_1h: Option<f64>,
}

impl WrittenCache {
    fn checked(&self, model: &str, input: u64) -> Result<CacheRates, Invalid> {
        // both lifetimes have a price, or neither has: one without the other is a price left out by mistake
        if self.write_5m.is_some() != self.write_1h.is_some() {
            return Err(Invalid::Price(model.to_owned()));
        }
        let write = |price: Option<f64>| price.map_or(Ok(input), |price| per_token(price, model));
        Ok(CacheRates {
            read: per_token(self.read, model)?,
            write_5m: write(self.write_5m)?,
            write_1h: write(self.write_1h)?,
        })
    }
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
        let cache = |written: &Option<WrittenCache>, input: u64| {
            written.as_ref().map(|cache| cache.checked(model, input)).transpose()
        };
        let input = per_token(self.input, model)?;
        let rate = PerToken { input, output: per_token(self.output, model)?, cache: cache(&self.cache, input)? };
        let long_prompt = match self.long_prompt {
            None => None,
            Some(long) if long.above == 0 => return Err(Invalid::LongPrompt(model.to_owned())),
            Some(long) => {
                let input = per_token(long.input, model)?;
                Some(LongPrompt {
                    above: long.above,
                    rate: PerToken { input, output: per_token(long.output, model)?, cache: cache(&long.cache, input)? },
                })
            }
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
        let rate = |model: &str| (models[model].rate.input, models[model].rate.output);
        assert_eq!(rate("claude-opus-5-5"), (4_000_000, 20_000_000));
        assert_eq!(rate("claude-sonnet-5-5"), (2_000_000, 10_000_000));
        assert_eq!(rate("claude-haiku-5-5"), (100_000, 500_000));
        assert_eq!(rate("claude-fable-5-2"), (8_000_000, 40_000_000));
        assert_eq!(rate("claude-opus-5"), (5_000_000, 25_000_000));
        assert_eq!(rate("claude-opus-4-8"), (5_000_000, 25_000_000));
        assert_eq!(rate("claude-sonnet-5"), (2_000_000, 10_000_000));
        let long = models["claude-haiku-5-5"].long_prompt.unwrap();
        assert_eq!((long.above, long.rate.input, long.rate.output), (100_000, 500_000, 2_500_000));
        let claude = models.iter().filter(|(id, _)| id.starts_with("claude-"));
        assert_eq!(claude.filter(|(_, model)| model.long_prompt.is_some()).count(), 1);
    }

    #[test]
    fn the_cache_prices_are_the_published_ones() {
        let cache = |model: &str| model_price(model).unwrap().rate.cache.unwrap();
        // Claude Opus 5.5: reads $0.20, five-minute writes $5, one-hour writes $8 per million tokens
        assert_eq!(cache("claude-opus-5-5"), CacheRates { read: 200_000, write_5m: 5_000_000, write_1h: 8_000_000 });
        assert_eq!(cache("claude-sonnet-5-5"), CacheRates { read: 100_000, write_5m: 2_500_000, write_1h: 4_000_000 });
        assert_eq!(cache("claude-fable-5-2"), CacheRates { read: 200_000, write_5m: 10_000_000, write_1h: 16_000_000 });
        // Claude Haiku 5.5: 0.1x, 1.25x and 2x the input rate, on each of its two rate cards
        assert_eq!(cache("claude-haiku-5-5"), CacheRates { read: 10_000, write_5m: 125_000, write_1h: 200_000 });
        let long = model_price("claude-haiku-5-5").unwrap().long_prompt.unwrap().rate.cache.unwrap();
        assert_eq!(long, CacheRates { read: 50_000, write_5m: 625_000, write_1h: 1_000_000 });
        for model in ["claude-opus-5", "claude-opus-4-8", "claude-sonnet-5"] {
            assert!(model_price(model).unwrap().rate.cache.is_some(), "{model}");
        }
    }

    #[test]
    fn cache_reads_and_writes_are_priced_at_their_own_rates() {
        let tokens =
            Tokens { input: 1_000, output: 500, cache_read: 100_000, cache_write_5m: 20_000, cache_write_1h: 0 };
        // 1000 * 4 + 500 * 20 + 100000 * 0.2 + 20000 * 5 micro-dollars
        assert_eq!(price_of("claude-opus-5-5", &tokens), Some(micro(4_000.0 + 10_000.0 + 20_000.0 + 100_000.0)));
        let hour = Tokens { cache_write_1h: 1_000, ..Tokens::default() };
        assert_eq!(price_of("claude-opus-5-5", &hour), Some(micro(8_000.0)));
        // the long-prompt rate is chosen by the whole prompt, cached or not
        let long = Tokens { input: 10, cache_read: 200_000, ..Tokens::default() };
        assert_eq!(price_of("claude-haiku-5-5", &long), Some(micro(10.0 * 0.5 + 200_000.0 * 0.05)));
        assert_eq!((long.prompt(), long.all()), (200_010, 200_010));
    }

    #[test]
    fn a_call_that_used_the_cache_of_a_model_without_cache_prices_is_unpriced() {
        let mut model = *model_price("claude-opus-5-5").unwrap();
        model.rate.cache = None;
        assert_eq!(model.cost(&Tokens { input: 10, cache_read: 10, ..Tokens::default() }), None);
        assert_eq!(model.cost(&Tokens { input: 10, ..Tokens::default() }), Some(micro(40.0)));
    }

    #[test]
    fn other_providers_models_are_priced_under_their_provider_with_the_published_prices() {
        // GPT-6.2 Sol: $2 in, $10 out, $0.10 cached, $2.50 for a 30-minute write; past 272K prompt tokens input and
        // cache double and output is 1.5 times
        let sol = model_price("openai:gpt-6.2-sol").unwrap();
        assert_eq!((sol.rate.input, sol.rate.output), (2_000_000, 10_000_000));
        assert_eq!(sol.rate.cache.unwrap(), CacheRates { read: 100_000, write_5m: 2_500_000, write_1h: 5_000_000 });
        let long = sol.long_prompt.unwrap();
        assert_eq!((long.above, long.rate.input, long.rate.output), (272_000, 4_000_000, 15_000_000));
        // a provider that publishes no price for writing the cache: writes are billed as input
        let carbon = model_price("gemini:gemini-4-carbon").unwrap().rate;
        assert_eq!(carbon.cache.unwrap(), CacheRates { read: 200_000, write_5m: carbon.input, write_1h: carbon.input });
        // the same model sold by two providers is two prices, and an id alone finds neither
        assert_ne!(model_price("groq:openai/gpt-oss-120b"), model_price("baseten:openai/gpt-oss-120b"));
        assert_eq!(model_price("openai/gpt-oss-120b"), None);
        assert_eq!(model_price("compatible:qwen3:32b"), None, "a model of the user's own server has no price");
        let bundled = bundled().unwrap();
        for (id, provider) in &bundled.providers {
            assert!(bundled.sources.contains_key(provider), "{id}: {provider} has a source");
        }
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
