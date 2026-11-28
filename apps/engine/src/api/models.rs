//! The language models the interface offers, from one place.
//!
//! ```text
//! GET /api/models            every provider: whether it can be used and, when not, the variable to set; the host
//!                            requests go to; what it cannot do; its models, each with its efforts and its price
//!                            (or that the price is not known); and the model the settings choose
//! GET /api/models/endpoint   the models the OpenAI-compatible server says it has (`GET <address>/models`)
//! ```
//!
//! Nothing here tells a key: a provider's key is only "set" or "not set", and of an address only its host. The
//! keys and addresses are the engine's, read from its environment and `.env` at start; the interface cannot set
//! them. Across a network the provider is the server's ([`super::routes::server_owned`]).

use axum::Json;
use axum::extract::State;
use axum::routing::{MethodRouter, get};
use serde::Serialize;

use super::routes::{ApiError, Shared, on_the_side};
use crate::model::language::{self, Effort, Https, Listed, Provider, Providers};
use crate::settings::{self, ModelRole, Settings};

/// The routes of this module, for [`super::routes::table`].
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    vec![("/api/models", get(listing)), ("/api/models/endpoint", get(endpoint_models))]
}

/// What `GET /api/models` answers (`ModelCatalog` in the contracts).
#[derive(Serialize)]
pub struct Told {
    pub providers: Vec<Listed>,
    /// The model that names communities and writes overviews.
    pub chosen: Chosen,
    /// The model that reads papers, chosen from the same providers and models.
    pub reading: Reading,
}

/// The model the settings choose.
#[derive(Serialize)]
pub struct Chosen {
    pub provider: String,
    pub model: String,
    pub effort: Effort,
}

/// The model the settings choose to read papers, and whether that is the naming one by not being chosen on its own.
#[derive(Serialize)]
pub struct Reading {
    #[serde(flatten)]
    pub chosen: Chosen,
    pub same_as_naming: bool,
}

impl Chosen {
    fn of(settings: &Settings, role: ModelRole) -> Self {
        let picked = settings.model_for(role);
        Chosen { provider: picked.provider.id().to_owned(), model: picked.model.to_owned(), effort: picked.effort }
    }
}

/// The answer for these providers and settings.
pub fn told(providers: &Providers, settings: &Settings) -> Told {
    Told {
        providers: providers.listing(),
        chosen: Chosen::of(settings, ModelRole::Naming),
        reading: Reading {
            chosen: Chosen::of(settings, ModelRole::Reading),
            same_as_naming: settings.reads_with_naming_model(),
        },
    }
}

async fn listing(State(engine): State<Shared>) -> Result<Json<Told>, ApiError> {
    let shared = engine.clone();
    let settings = on_the_side(move || Ok(settings::load(&shared.settings_file(), &shared.choices)?)).await?;
    Ok(Json(told(&engine.providers, &settings)))
}

#[derive(Serialize)]
struct Discovered {
    models: Vec<String>,
}

async fn endpoint_models(State(engine): State<Shared>) -> Result<Json<Discovered>, ApiError> {
    // what is wrong is said in the words of its variable, never with its value
    let endpoint = engine.providers.endpoint(Provider::Compatible).map_err(|why| ApiError::Invalid(why.to_string()))?;
    let models = on_the_side(move || {
        language::discover(&Https::default(), &endpoint).map_err(|why| ApiError::Invalid(why.to_string()))
    })
    .await?;
    Ok(Json(Discovered { models }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_names_every_provider_and_the_chosen_model_and_no_key() {
        const SECRET: &str = "sk-test-zyxwvutsrqponm-not-real";
        let providers = Providers::from_lookup(|name| {
            (name.ends_with("_API_KEY") || name == "HF_TOKEN").then(|| SECRET.to_owned())
        });
        let told = serde_json::to_value(told(&providers, &Settings::default())).unwrap();
        let text = told.to_string();
        assert!(!text.contains(SECRET), "a key is told");
        assert_eq!(told["providers"].as_array().unwrap().len(), Provider::ALL.len());
        assert_eq!(
            told["chosen"],
            serde_json::json!({"provider": "anthropic", "model": "claude-opus-5-5", "effort": "medium"})
        );
        let openai = &told["providers"][1];
        assert_eq!((openai["id"].as_str(), openai["key"]["set"].as_bool()), (Some("openai"), Some(true)));
        assert_eq!(openai["available"]["state"], "ready");
        assert_eq!(openai["models"][0]["price"]["kind"], "known");
        let compatible = told["providers"].as_array().unwrap().iter().find(|row| row["id"] == "compatible").unwrap();
        assert_eq!(compatible["available"]["why"], "set CONSTELLATE_MODEL_BASE_URL");
        assert_eq!(compatible["ids"], "typed");
        assert!(compatible["limits"].as_array().unwrap().len() >= 2);
        crate::api::contract::assert_fits("ModelCatalog", &told, &["providers"]);
    }

    #[test]
    fn the_model_that_reads_papers_is_told_from_the_same_providers_and_whether_it_is_the_naming_one() {
        let providers = Providers::from_lookup(|_| None);
        let same = serde_json::to_value(told(&providers, &Settings::default())).unwrap();
        assert_eq!(
            same["reading"],
            serde_json::json!({
                "provider": "anthropic", "model": "claude-opus-5-5", "effort": "medium", "same_as_naming": true,
            })
        );
        let own = Settings {
            reading_provider: Some(Provider::Compatible),
            reading_model: Some("qwen3:32b".to_owned()),
            reading_effort: Some(Effort::Low),
            ..Settings::default()
        };
        let own = serde_json::to_value(told(&providers, &own)).unwrap();
        assert_eq!(
            own["reading"],
            serde_json::json!({"provider": "compatible", "model": "qwen3:32b", "effort": "low", "same_as_naming": false})
        );
        assert_eq!(own["chosen"]["model"], "claude-opus-5-5", "the naming model is told as it was");
        crate::api::contract::assert_fits("ModelCatalog", &own, &["providers"]);
    }
}
