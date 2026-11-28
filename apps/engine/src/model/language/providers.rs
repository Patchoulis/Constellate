//! Every provider as the engine was started with it: an [`Endpoint`] for each one whose variables are set, and,
//! for the interface, what it is told of each ([`Listed`]): whether a key is set (never the key), the host requests
//! go to, why one cannot be used, what it cannot do, and its models with their prices.

use std::fmt;

use serde::Serialize;
use url::Url;

use super::provider::{Address, Capabilities, Key, ModelIds, Offered, Provider};
use super::{Effort, Endpoint, Price, Setup, Unavailable, listed_price, on_loopback};

/// The providers, read once when the engine starts (the variables do not change while it runs).
#[derive(Clone)]
pub struct Providers {
    rows: Vec<Row>,
}

#[derive(Clone)]
struct Row {
    provider: Provider,
    endpoint: Result<Endpoint, Unavailable>,
    key_set: bool,
    /// The address the user set, when one is: only its host is ever told.
    set_base: Option<String>,
}

impl fmt::Debug for Providers {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // which providers can be used, and nothing of how they are reached
        let ready: Vec<&str> =
            self.rows.iter().filter(|row| row.endpoint.is_ok()).map(|row| row.provider.id()).collect();
        formatter.debug_struct("Providers").field("ready", &ready).finish()
    }
}

impl Default for Providers {
    /// None set up: what the tests' engines have.
    fn default() -> Self {
        Self::from_lookup(|_| None)
    }
}

impl Providers {
    /// From whatever `lookup` says the variables are: the engine's own reading of the environment and `.env`.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let given = |name: &str| lookup(name).map(|value| value.trim().to_owned()).filter(|value| !value.is_empty());
        let rows = Provider::ALL
            .into_iter()
            .map(|provider| {
                let about = provider.about();
                Row {
                    provider,
                    endpoint: Endpoint::for_provider(provider, &lookup),
                    key_set: given(about.key.variable()).is_some(),
                    set_base: about.address.variable().and_then(&given),
                }
            })
            .collect();
        Self { rows }
    }

    /// From the process's environment.
    pub fn from_environment() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// The endpoint of `provider`, or why it cannot be used.
    pub fn endpoint(&self, provider: Provider) -> Result<Endpoint, Unavailable> {
        match self.rows.iter().find(|row| row.provider == provider) {
            Some(row) => row.endpoint.clone(),
            None => Err(Setup::KeyNotSet(provider.about().key.variable()).into()),
        }
    }

    /// What the interface is told of every provider, in the order it lists them.
    pub fn listing(&self) -> Vec<Listed> {
        self.rows.iter().map(Row::listed).collect()
    }
}

impl Row {
    fn listed(&self) -> Listed {
        let about = self.provider.about();
        let set_host = self.set_base.as_deref().and_then(|base| Url::parse(base).ok());
        let place = match (&self.endpoint, &set_host) {
            (Ok(endpoint), _) if endpoint.on_this_computer() => Place::ThisComputer,
            (_, Some(url)) if self.provider == Provider::Compatible && on_loopback(url) => Place::ThisComputer,
            (_, Some(_)) => Place::Set,
            (_, None) if matches!(about.address, Address::Required(_)) => Place::NotSet,
            (_, None) => Place::Provider,
        };
        let host = match (&set_host, about.address) {
            (Some(url), _) => url.host_str().map(|host| match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_owned(),
            }),
            (None, Address::Fixed(default) | Address::Overridable { default, .. }) => {
                Url::parse(default).ok().and_then(|url| url.host_str().map(str::to_owned))
            }
            (None, Address::Required(_)) => None,
        };
        let capabilities = match &self.endpoint {
            Ok(endpoint) => endpoint.capabilities(),
            Err(_) => about.capabilities,
        };
        let here = place == Place::ThisComputer;
        Listed {
            id: self.provider,
            label: about.label,
            key: KeyState {
                variable: about.key.variable(),
                needed: matches!(about.key, Key::Required(_)),
                set: self.key_set,
            },
            address: AddressState { variable: about.address.variable(), host, place },
            available: match &self.endpoint {
                Ok(_) => Availability::Ready,
                Err(why) => Availability::Unavailable { why: instruction(why) },
            },
            ids: about.ids,
            capabilities,
            limits: capabilities.limits(),
            models: about
                .models
                .iter()
                .map(|offered| ListedModel { offered: *offered, price: listed_price(self.provider, offered.id, here) })
                .collect(),
            price_of_typed: if here { Price::ThisComputer } else { Price::NotKnown },
            docs: about.docs,
            reading_default: about
                .reading_default()
                .map(|offered| ReadingDefault { model: offered.id, effort: super::READING_DEFAULT_EFFORT }),
        }
    }
}

/// What to do about `why`, in a few words that name a variable and never its value.
fn instruction(why: &Unavailable) -> String {
    match why {
        Unavailable::NoKey => "set ANTHROPIC_API_KEY".to_owned(),
        Unavailable::BadEndpoint => "ANTHROPIC_BASE_URL must be an https address".to_owned(),
        Unavailable::Setup(Setup::KeyNotSet(variable) | Setup::AddressNotSet(variable)) => format!("set {variable}"),
        Unavailable::Setup(Setup::NotHttps(variable)) => {
            format!("{variable} must be an https address, or http to this computer")
        }
        other => other.to_string(),
    }
}

/// One provider as the interface is told it (`ModelProvider` in the contracts).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Listed {
    pub id: Provider,
    pub label: &'static str,
    pub key: KeyState,
    pub address: AddressState,
    pub available: Availability,
    pub ids: ModelIds,
    pub capabilities: Capabilities,
    /// What it cannot do, in plain words.
    pub limits: Vec<&'static str>,
    pub models: Vec<ListedModel>,
    /// The price of a model id typed rather than listed: not known, or no fee on this computer.
    pub price_of_typed: Price,
    pub docs: &'static str,
    /// The model that reads papers by default while this provider names communities; none: the naming model.
    pub reading_default: Option<ReadingDefault>,
}

/// One of a provider's listed models, and the effort it reads papers at by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ReadingDefault {
    pub model: &'static str,
    pub effort: Effort,
}

/// Whether the key is set: the variable's name, never its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct KeyState {
    pub variable: &'static str,
    /// A server on this computer usually needs none.
    pub needed: bool,
    pub set: bool,
}

/// Where requests go: the variable that changes it, and the host (never the rest of the address).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AddressState {
    pub variable: Option<&'static str>,
    pub host: Option<String>,
    pub place: Place,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    /// The provider's own.
    Provider,
    /// Another the user set (a proxy, a gateway, a server elsewhere).
    Set,
    /// A server on this computer.
    ThisComputer,
    /// None is set, and there is no default.
    NotSet,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Availability {
    Ready,
    /// `why` is what to do: "set OPENAI_API_KEY".
    Unavailable {
        why: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ListedModel {
    #[serde(flatten)]
    pub offered: Offered,
    pub price: Price,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const SECRET: &str = "sk-test-0123456789abcdef-not-real";

    fn lookup(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned())
    }

    #[test]
    fn a_provider_without_its_key_is_listed_with_what_to_set() {
        let providers = Providers::from_lookup(lookup(&[]));
        let listing = providers.listing();
        let openai = listing.iter().find(|row| row.id == Provider::OpenAi).unwrap();
        assert_eq!(openai.available, Availability::Unavailable { why: "set OPENAI_API_KEY".to_owned() });
        assert_eq!(openai.address.host.as_deref(), Some("api.openai.com"));
        let anthropic = listing.iter().find(|row| row.id == Provider::Anthropic).unwrap();
        assert_eq!(anthropic.available, Availability::Unavailable { why: "set ANTHROPIC_API_KEY".to_owned() });
        let compatible = listing.iter().find(|row| row.id == Provider::Compatible).unwrap();
        assert_eq!(
            compatible.available,
            Availability::Unavailable { why: "set CONSTELLATE_MODEL_BASE_URL".to_owned() }
        );
        assert_eq!(compatible.address.place, Place::NotSet);
    }

    #[test]
    fn a_server_on_this_computer_needs_no_key_and_costs_no_fee() {
        let providers = Providers::from_lookup(lookup(&[("CONSTELLATE_MODEL_BASE_URL", "http://localhost:11434/v1")]));
        let compatible = providers.listing().into_iter().find(|row| row.id == Provider::Compatible).unwrap();
        assert_eq!(compatible.available, Availability::Ready);
        assert_eq!(
            (compatible.address.place, compatible.address.host.as_deref()),
            (Place::ThisComputer, Some("localhost:11434"))
        );
        assert_eq!(compatible.price_of_typed, Price::ThisComputer);
        assert!(providers.endpoint(Provider::Compatible).unwrap().on_this_computer());
        let elsewhere =
            Providers::from_lookup(lookup(&[("CONSTELLATE_MODEL_BASE_URL", "http://192.168.1.20:11434/v1")]));
        let refused = elsewhere.listing().into_iter().find(|row| row.id == Provider::Compatible).unwrap();
        assert!(matches!(refused.available, Availability::Unavailable { ref why } if why.contains("https")));
        assert_eq!(refused.address.place, Place::Set);
    }

    #[test]
    fn no_key_is_ever_in_what_is_told() {
        let providers = Providers::from_lookup(lookup(&[
            ("ANTHROPIC_API_KEY", SECRET),
            ("OPENAI_API_KEY", SECRET),
            ("GEMINI_API_KEY", SECRET),
            ("OPENROUTER_API_KEY", SECRET),
            ("CONSTELLATE_MODEL_API_KEY", SECRET),
            ("CONSTELLATE_MODEL_BASE_URL", "https://user:password-not-real@gateway.example.org/v1"),
        ]));
        let told = serde_json::to_string(&providers.listing()).unwrap();
        assert!(!told.contains(SECRET), "a key is told");
        assert!(!told.contains("password-not-real") && !told.contains("/v1"), "only the host of an address is told");
        assert!(told.contains("gateway.example.org"));
        let debug = format!("{providers:?} {:?}", providers.endpoint(Provider::OpenAi));
        assert!(!debug.contains(SECRET) && !debug.contains("password-not-real"), "{debug}");
        let listing = providers.listing();
        let openai = listing.iter().find(|row| row.id == Provider::OpenAi).unwrap();
        assert_eq!((openai.key.set, openai.available.clone()), (true, Availability::Ready));
    }

    #[test]
    fn models_are_listed_with_their_prices_or_that_they_are_not_known() {
        let listing = Providers::default().listing();
        let model = |provider: Provider, id: &str| {
            let row = listing.iter().find(|row| row.id == provider).unwrap();
            row.models.iter().find(|model| model.offered.id == id).unwrap().price
        };
        assert_eq!(
            model(Provider::Anthropic, "claude-opus-5-5"),
            Price::Known { input: 4.0, output: 20.0, cache_read: Some(0.2) }
        );
        assert_eq!(
            model(Provider::OpenAi, "gpt-6.2-sol"),
            Price::Known { input: 2.0, output: 10.0, cache_read: Some(0.1) }
        );
        assert_eq!(model(Provider::Moonshot, "kimi-k3.1-pro"), Price::NotKnown, "its price table could not be read");
    }
}
