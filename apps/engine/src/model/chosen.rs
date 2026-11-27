//! The language model a feature asks, as one value ([`Chosen`]): the provider, that provider's model and the effort,
//! read together with the endpoint they are asked at, so that no feature can price one provider's model, key its
//! answer under another, and send it to a third. And what a job may spend once the user has seen what it would cost
//! ([`budget`]), the same way for every feature.
//!
//! Every feature was first written for the Claude API alone. What changes with the provider, and is decided here
//! rather than in each feature:
//!
//! | what                        | how                                                                              |
//! |-----------------------------|----------------------------------------------------------------------------------|
//! | the model                   | the one the settings choose for that provider, else the provider's first; never another provider's id |
//! | the effort                  | the setting, as near as the model has it; a feature may only ask for less        |
//! | the estimate                | at that provider's prices, caching and way of answering in JSON ([`Pricing`])  |
//! | where an answer is kept     | under the provider and the model ([`AnswerKey::new`])                          |
//! | the budget                  | in dollars when the model has a price, in tokens when it has none             |

use std::fmt;

use super::language::provider::{EffortControl, effort_for};
use super::language::{Budget, Counting, Effort, Endpoint, Estimate, Limit, Planned, Pricing, Provider, Unavailable};
use crate::library::{AnswerKey, Feature};
use crate::settings::Settings;
use crate::usage::Usd;

/// The provider, model and effort the features ask with, and where.
#[derive(Clone)]
pub struct Chosen {
    provider: Provider,
    model: String,
    effort: Effort,
    /// Where it is asked, or why it cannot be: the provider's endpoint, which is [`Chosen::provider`]'s.
    endpoint: Result<Endpoint, Unavailable>,
}

impl fmt::Debug for Chosen {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // the endpoint says nothing of itself either; only whether it can be used
        formatter
            .debug_struct("Chosen")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("effort", &self.effort)
            .field("ready", &self.endpoint.is_ok())
            .finish()
    }
}

/// The same choice: provider, model and effort, and whether it can be asked (an endpoint has no equality of its
/// own, and two of one provider read from the same variables are the same).
impl PartialEq for Chosen {
    fn eq(&self, other: &Self) -> bool {
        (self.provider, &self.model, self.effort, self.endpoint.is_ok())
            == (other.provider, &other.model, other.effort, other.endpoint.is_ok())
    }
}

/// No model can be chosen: the provider lists none, and the settings name none of it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("choose a model of {0} in the settings: it has no model of its own to fall back on")]
pub struct NoModel(pub &'static str);

/// The model shown with an estimate is no longer the one that would be asked.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the model chosen in the settings is now {now}: look at its estimate before asking")]
pub struct Changed {
    pub now: String,
}

impl Chosen {
    /// The model the features ask: `settings`' provider (`wanted`, as read from them) unless `endpoint` is another
    /// provider's (a test's fixed endpoint), the settings' model when it is that provider's, else the provider's first
    /// model, and the settings' effort.
    pub fn new(
        endpoint: Result<Endpoint, Unavailable>,
        wanted: Provider,
        settings: &Settings,
    ) -> Result<Self, NoModel> {
        let provider = endpoint.as_ref().map_or(wanted, Endpoint::provider);
        let about = provider.about();
        let set = settings.describe_model.trim();
        let model = if settings.describe_provider == provider && !set.is_empty() {
            set.to_owned()
        } else {
            // the settings' model is another provider's: never sent here
            about.models.first().map(|first| first.id.to_owned()).ok_or(NoModel(about.label))?
        };
        Ok(Self::of(provider, &model, settings.describe_effort, endpoint))
    }

    /// `model` of `provider` at `effort` (as near as the model has it), asked at `endpoint`.
    pub fn of(provider: Provider, model: &str, effort: Effort, endpoint: Result<Endpoint, Unavailable>) -> Self {
        Self { provider, effort: nearest(provider, model, effort), model: model.to_owned(), endpoint }
    }

    /// The Claude API's `model` at medium effort, with no endpoint: what the features' tests plan with.
    #[cfg(test)]
    pub fn claude(model: &str) -> Self {
        Self::of(Provider::Anthropic, model, Effort::Medium, Err(Unavailable::NoKey))
    }

    pub fn provider(&self) -> Provider {
        self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// The effort chosen, as near as the model has it.
    pub fn effort(&self) -> Effort {
        self.effort
    }

    /// The effort chosen, but no more than `cap`: for a feature with a reason to ask for less (an answer that is a
    /// lookup, not an argument).
    pub fn effort_at_most(&self, cap: Effort) -> Effort {
        nearest(self.provider, &self.model, self.effort.min(cap))
    }

    /// Where it is asked, or why it cannot be (its key is not set).
    pub fn endpoint(&self) -> Result<Endpoint, Unavailable> {
        self.endpoint.clone()
    }

    pub fn can_ask(&self) -> bool {
        self.endpoint.is_ok()
    }

    /// How its calls are priced: at the provider's prices, nothing on this computer.
    pub fn pricing(&self) -> Pricing {
        match &self.endpoint {
            Ok(endpoint) => Pricing::of(endpoint),
            Err(_) => Pricing::listed(self.provider),
        }
    }

    /// What `planned` calls would cost ([`Pricing::estimate`]).
    pub fn estimate(&self, planned: &[Planned], counting: Counting) -> Estimate {
        self.pricing().estimate(&self.model, planned, counting)
    }

    /// The key an answer of `feature` by this model is kept under ([`AnswerKey::new`]).
    pub fn key(&self, feature: Feature, prompt: u32, papers: &[&str], question: &str) -> AnswerKey {
        AnswerKey::new(feature, self.provider, &self.model, prompt, papers, question)
    }

    /// Is this the model whose estimate the user was shown (`provider`, `model`)? A request that names no provider
    /// is from an interface that knew the Claude API only.
    pub fn as_shown(&self, provider: Option<Provider>, model: &str) -> Result<(), Changed> {
        if provider.unwrap_or(Provider::Anthropic) == self.provider && model == self.model {
            Ok(())
        } else {
            Err(Changed { now: self.to_string() })
        }
    }
}

/// "gpt-6.2-sol of OpenAI": what a message names the model by.
impl fmt::Display for Chosen {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} of {}", self.model, self.provider.about().label)
    }
}

/// `effort` as near as `model` of `provider` has it: as set for a provider that is not sent one, or a model whose
/// efforts are not listed (the client then sends none).
fn nearest(provider: Provider, model: &str, effort: Effort) -> Effort {
    let about = provider.about();
    match about.capabilities.effort {
        EffortControl::NotSent => effort,
        EffortControl::ByModel => {
            about.model(model).and_then(|listed| effort_for(effort, listed.efforts)).unwrap_or(effort)
        }
    }
}

/// What the user accepted to spend when they clicked, as the request carries it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Accepted {
    /// Nothing named: the worst case shown.
    Shown,
    /// At most this many US dollars.
    Dollars(f64),
    /// At most this many tokens (of every kind): what is shown for a model with no price.
    Tokens(u64),
}

impl Accepted {
    /// From a request's two optional fields: dollars, tokens, or neither.
    pub fn of(usd: Option<f64>, tokens: Option<u64>) -> Result<Self, Refused> {
        match (usd, tokens) {
            (Some(_), Some(_)) => Err(Refused::Both),
            (Some(usd), None) if usd.is_finite() && usd >= 0.0 => Ok(Self::Dollars(usd)),
            (None, Some(tokens)) => Ok(Self::Tokens(tokens)),
            (Some(_), None) => Err(Refused::Nothing),
            (None, None) => Ok(Self::Shown),
        }
    }
}

/// What a feature lets the user accept.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Terms {
    /// The worst case shown, and no more: refused when it has grown since it was shown (the papers, the passages or
    /// the model changed). The budget is that worst case.
    AsShown,
    /// A budget of the user's own, up to these.
    AtMost { usd: f64, tokens: u64 },
}

/// Why a job is not started on what the user accepted.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum Refused {
    #[error("what this would cost has changed since it was shown: look at the new estimate before asking")]
    Changed,
    #[error("the budget must be more than nothing")]
    Nothing,
    #[error("the budget is too large: at most {0}")]
    TooLarge(String),
    #[error("a budget is in US dollars or in tokens, not both")]
    Both,
}

/// A hundredth of a cent, in dollars: the rounding of an amount as it travelled to the interface and back.
const SLACK_USD: f64 = 1e-4;

/// The budget a job runs under, given what asking would cost (`cost`, as the user was shown it and as it is now)
/// and what they accepted: in dollars for a model with a price, in tokens for one without (a typed model id, a
/// server on this computer), so that no feature refuses a model only because it has no price. Nothing to ask is a
/// budget of nothing.
pub fn budget(cost: &Estimate, accepted: Accepted, terms: Terms) -> Result<Budget, Refused> {
    if cost.calls == 0 {
        return Ok(Budget::new(Limit::Tokens(0)));
    }
    let limit = match (cost.most_usd, accepted, terms) {
        // a dollar figure that was shown must be accepted as shown; a budget of one's own may default to it
        (Some(most), Accepted::Shown, Terms::AtMost { .. }) => Limit::Dollars(most),
        (Some(_), Accepted::Shown, Terms::AsShown) => return Err(Refused::Changed),
        (Some(most), Accepted::Dollars(usd), Terms::AsShown) if most.dollars() <= usd + SLACK_USD => {
            Limit::Dollars(most)
        }
        (Some(worst), Accepted::Dollars(usd), Terms::AtMost { usd: most, .. }) => {
            if usd > most {
                return Err(Refused::TooLarge(format!("{most} US dollars")));
            }
            if usd <= 0.0 {
                return Err(Refused::Nothing);
            }
            // the worst case shown, sent back as it was shown, is that worst case: not a picodollar less for the
            // rounding of a figure that travelled as a decimal
            let own = dollars(usd);
            let shown = worst.dollars() <= usd + SLACK_USD && worst.picodollars() > own.picodollars();
            Limit::Dollars(if shown { worst } else { own })
        }
        (None, Accepted::Shown, _) => Limit::Tokens(cost.most_tokens),
        (None, Accepted::Tokens(tokens), Terms::AsShown) if cost.most_tokens <= tokens => {
            Limit::Tokens(cost.most_tokens)
        }
        (None, Accepted::Tokens(tokens), Terms::AtMost { tokens: most, .. }) => {
            if tokens > most {
                return Err(Refused::TooLarge(format!("{most} tokens")));
            }
            if tokens == 0 {
                return Err(Refused::Nothing);
            }
            Limit::Tokens(tokens)
        }
        // grown since it was shown, or shown with a price it no longer has (or the other way round)
        (Some(_), Accepted::Dollars(_) | Accepted::Tokens(_), _)
        | (None, Accepted::Dollars(_) | Accepted::Tokens(_), _) => {
            return Err(Refused::Changed);
        }
    };
    Ok(Budget::new(limit))
}

/// Dollars as a budget holds them: positive and finite, as [`Accepted::of`] checked.
fn dollars(usd: f64) -> Usd {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let picodollars = (usd * 1e12).round() as u64;
    Usd::from_picodollars(picodollars)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::model::language::Caching;
    use crate::model::language::fake::endpoint;
    use crate::usage::Tokens;

    fn settings(provider: &str, model: &str, effort: Effort) -> Settings {
        Settings {
            describe_provider: Provider::from_id(provider).unwrap(),
            describe_model: model.to_owned(),
            describe_effort: effort,
            ..Settings::default()
        }
    }

    #[test]
    fn the_model_is_the_settings_one_for_its_provider_and_never_another_providers() {
        let openai = settings("openai", "gpt-6.2-astra", Effort::High);
        let chosen = Chosen::new(Ok(endpoint(Provider::OpenAi)), Provider::OpenAi, &openai).unwrap();
        assert_eq!(
            (chosen.provider(), chosen.model(), chosen.effort()),
            (Provider::OpenAi, "gpt-6.2-astra", Effort::High)
        );
        // the settings name a Claude model, but OpenAI is where it would be asked: its own first model instead
        let claude = settings("anthropic", "claude-opus-5-5", Effort::Medium);
        let fixed = Chosen::new(Ok(endpoint(Provider::OpenAi)), Provider::Anthropic, &claude).unwrap();
        assert_eq!((fixed.provider(), fixed.model()), (Provider::OpenAi, "gpt-6.2-sol"));
        // no endpoint yet: the provider the settings want, priced as it is listed
        let unset = Chosen::new(Err(Unavailable::NoKey), Provider::OpenAi, &openai).unwrap();
        assert_eq!((unset.provider(), unset.can_ask()), (Provider::OpenAi, false));
        // a provider that lists no model, with another provider's model in the settings: no model at all
        let refused = Chosen::new(Ok(endpoint(Provider::Compatible)), Provider::Anthropic, &claude);
        assert_eq!(refused.unwrap_err(), NoModel("OpenAI-compatible endpoint"));
        let typed = settings("compatible", "qwen3:32b", Effort::Max);
        let local = Chosen::new(Ok(endpoint(Provider::Compatible)), Provider::Compatible, &typed).unwrap();
        assert_eq!((local.model(), local.pricing().on_this_computer), ("qwen3:32b", true));
    }

    #[test]
    fn the_effort_is_the_setting_as_near_as_the_model_has_it_and_a_feature_may_only_lower_it() {
        let chosen = Chosen::of(Provider::Gemini, "gemini-4-carbon", Effort::Max, Err(Unavailable::NoKey));
        assert_eq!(chosen.effort(), Effort::High, "Gemini 4 Carbon goes up to high");
        assert_eq!(chosen.effort_at_most(Effort::Low), Effort::Low);
        assert_eq!(chosen.effort_at_most(Effort::Xhigh), Effort::High);
        let low = Chosen::of(Provider::Anthropic, "claude-opus-5-5", Effort::Low, Err(Unavailable::NoKey));
        assert_eq!(low.effort_at_most(Effort::Medium), Effort::Low, "a cap never raises the setting");
    }

    #[test]
    fn the_model_shown_is_compared_with_its_provider() {
        let chosen = Chosen::of(Provider::OpenRouter, "x-ai/grok-4.7", Effort::Medium, Err(Unavailable::NoKey));
        assert!(chosen.as_shown(Some(Provider::OpenRouter), "x-ai/grok-4.7").is_ok());
        let elsewhere = chosen.as_shown(Some(Provider::Xai), "x-ai/grok-4.7").unwrap_err();
        assert_eq!(
            elsewhere.to_string(),
            "the model chosen in the settings is now x-ai/grok-4.7 of OpenRouter: look at its estimate before asking"
        );
        assert!(chosen.as_shown(None, "x-ai/grok-4.7").is_err(), "no provider named: the Claude API's");
    }

    fn cost(most_usd: Option<f64>, most_tokens: u64) -> Estimate {
        Estimate {
            provider: Provider::OpenAi,
            model: "gpt-6.2-sol".to_owned(),
            calls: 1,
            tokens: Tokens::default(),
            usd: most_usd.map(|_| Usd::ZERO),
            most_usd: most_usd.map(dollars),
            most_tokens,
            counting: Counting::ASSUMED,
            caching: Caching::AutomaticWritesBilled,
            on_this_computer: false,
        }
    }

    #[test]
    fn a_priced_model_is_budgeted_in_dollars_and_an_unpriced_one_in_tokens_as_shown() {
        let priced = cost(Some(0.25), 9_000);
        let unpriced = cost(None, 9_000);
        let limit = |budget: Budget| budget.limit();
        assert_eq!(
            budget(&priced, Accepted::Shown, Terms::AsShown).unwrap_err(),
            Refused::Changed,
            "a price shown is accepted as shown"
        );
        let own = Terms::AtMost { usd: 5.0, tokens: 1_000_000 };
        assert_eq!(
            limit(budget(&priced, Accepted::Shown, own).unwrap()),
            Limit::Dollars(dollars(0.25)),
            "a budget of one's own defaults to the worst case"
        );
        assert_eq!(
            limit(budget(&priced, Accepted::Dollars(0.25), Terms::AsShown).unwrap()),
            Limit::Dollars(dollars(0.25))
        );
        assert_eq!(budget(&priced, Accepted::Dollars(0.2), Terms::AsShown).unwrap_err(), Refused::Changed);
        assert_eq!(limit(budget(&unpriced, Accepted::Shown, Terms::AsShown).unwrap()), Limit::Tokens(9_000));
        assert_eq!(limit(budget(&unpriced, Accepted::Tokens(9_000), Terms::AsShown).unwrap()), Limit::Tokens(9_000));
        assert_eq!(budget(&unpriced, Accepted::Tokens(8_999), Terms::AsShown).unwrap_err(), Refused::Changed);
        // shown with a price it no longer has, or the other way round: the estimate must be looked at again
        assert_eq!(budget(&unpriced, Accepted::Dollars(1.0), Terms::AsShown).unwrap_err(), Refused::Changed);
        assert_eq!(budget(&priced, Accepted::Tokens(9_000), Terms::AsShown).unwrap_err(), Refused::Changed);
        let own = Terms::AtMost { usd: 5.0, tokens: 1_000_000 };
        assert_eq!(limit(budget(&priced, Accepted::Dollars(2.0), own).unwrap()), Limit::Dollars(dollars(2.0)));
        assert!(matches!(budget(&priced, Accepted::Dollars(6.0), own), Err(Refused::TooLarge(_))));
        assert_eq!(limit(budget(&unpriced, Accepted::Tokens(50_000), own).unwrap()), Limit::Tokens(50_000));
        assert!(matches!(budget(&unpriced, Accepted::Tokens(2_000_000), own), Err(Refused::TooLarge(_))));
        let nothing = Estimate { calls: 0, ..priced };
        assert_eq!(limit(budget(&nothing, Accepted::Dollars(0.1), Terms::AsShown).unwrap()), Limit::Tokens(0));
    }

    #[test]
    fn an_accepted_budget_is_dollars_or_tokens_and_more_than_nothing() {
        assert_eq!(Accepted::of(None, None), Ok(Accepted::Shown));
        assert_eq!(Accepted::of(Some(0.5), None), Ok(Accepted::Dollars(0.5)));
        assert_eq!(Accepted::of(None, Some(40)), Ok(Accepted::Tokens(40)));
        assert_eq!(Accepted::of(Some(0.5), Some(40)), Err(Refused::Both));
        for bad in [-1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(Accepted::of(Some(bad), None), Err(Refused::Nothing), "{bad}");
        }
        assert_eq!(
            Accepted::of(Some(0.0), None),
            Ok(Accepted::Dollars(0.0)),
            "nothing, for what costs nothing (all kept)"
        );
    }
}
