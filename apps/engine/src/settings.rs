//! The person's settings, kept in `settings.json` in the data folder.
//!
//! The interface sends any subset of the settings to change ([`Settings::apply`]); every value is checked, and a
//! single bad one changes nothing at all, so the person is told what is wrong and nothing half-applied stays
//! behind. Unknown keys are ignored (an older or newer interface), a missing key takes its default.
//!
//! Some settings are a choice among things other parts of the engine own: the clustering methods, the
//! acceptance filters, the declaration flags, the models. Those lists are given by whoever asks ([`Choices`]),
//! so that this module knows how to check a choice without knowing what the choices are.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::library::store::{self, StoreError};
use crate::model::language::{Effort, ModelIds, Provider};

/// Every setting, with the names and the JSON shape the interface uses (`Settings` in
/// `packages/contracts/src/index.ts`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub embedding_model: String,
    /// Remove the mean embedding before cosine similarity.
    pub center_embeddings: bool,
    /// How communities are cut: one of [`Choices::clustering`].
    pub clustering: String,
    pub max_communities: u32,
    pub min_community_size: u32,
    /// A split into two communities is kept only if its halves are this distinct.
    pub min_split_modularity: f64,
    /// How many times a community is split again into sub-communities.
    pub sub_levels: u32,
    /// Most links drawn from one paper to its closest papers.
    pub max_links: u32,
    /// Score every abstract for wording of the kind language models produce, and tag those above the threshold.
    pub detect_ai: bool,
    /// Papers the AI-text detector flags are always tagged; false leaves them out of a run altogether.
    pub include_ai_papers: bool,
    /// Which conference papers go into a run: one of [`Choices::acceptance`].
    pub acceptance_filter: String,
    /// Papers whose authors' declarations raise one of these flags are left out, in [`Choices::flags`] order.
    pub exclude_flags: Vec<String>,
    /// Ask the language model to describe every community.
    pub describe: bool,
    /// The provider of the language model (a [`Provider`]'s id): `anthropic`, `openai`, `compatible`, ...
    pub describe_provider: String,
    /// One of that provider's models: for the Claude API one of [`Choices::describe_models`]; for a provider whose
    /// ids may be typed, any plain id it accepts.
    pub describe_model: String,
    /// How hard the model thinks: sent as the nearest effort the model has, or not at all to one without.
    pub describe_effort: Effort,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            embedding_model: "Qwen/Qwen3-Embedding-0.6B".to_owned(),
            center_embeddings: true,
            clustering: "spectral".to_owned(),
            max_communities: 24,
            min_community_size: 15,
            min_split_modularity: 0.05,
            sub_levels: 2,
            max_links: 8,
            detect_ai: true,
            include_ai_papers: true,
            acceptance_filter: "all".to_owned(),
            exclude_flags: Vec::new(),
            describe: true,
            describe_provider: "anthropic".to_owned(),
            describe_model: "claude-opus-5-5".to_owned(),
            describe_effort: Effort::Medium,
        }
    }
}

/// What a choice may be, as the modules that own each list say. Every list is in the order it is shown in;
/// [`Settings::exclude_flags`] is kept in the order of [`Choices::flags`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choices {
    pub embedding_models: Vec<String>,
    pub clustering: Vec<String>,
    pub acceptance: Vec<String>,
    pub flags: Vec<String>,
    pub describe_models: Vec<String>,
}

/// What each whole number may be, smallest and largest.
const MAX_COMMUNITIES: (u32, u32) = (2, 500);
const MIN_COMMUNITY_SIZE: (u32, u32) = (2, 100_000);
const SUB_LEVELS: (u32, u32) = (0, 6);
const MAX_LINKS: (u32, u32) = (1, 50);
/// What [`Settings::min_split_modularity`] may be.
const MIN_SPLIT_MODULARITY: (f64, f64) = (0.0, 1.0);

/// Why a value cannot be a setting, worded for the person who tried to set it.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum Invalid {
    #[error("{key} must be true or false")]
    NotASwitch { key: &'static str },
    /// The bounds are written as the person reads them: `2` for a whole number, `0.0` for a fraction.
    #[error("{key} must be a number between {low} and {high}")]
    OutOfRange { key: &'static str, low: String, high: String },
    #[error("{key} must be a whole number")]
    NotWhole { key: &'static str },
    #[error("{key} must be a name")]
    NotAName { key: &'static str },
    #[error("{key} must be one of {}", known.join(", "))]
    NotAChoice { key: &'static str, known: Vec<String> },
    #[error("{key} must be a list of: {}", known.join(", "))]
    NotFlags { key: &'static str, known: Vec<String> },
    #[error("{key} must be a model id {provider} accepts (letters, digits and . _ - / :)")]
    NotAModel { key: &'static str, provider: &'static str },
}

/// Why settings could not be changed: a value the person gave, or the file they are kept in.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error(transparent)]
    Invalid(#[from] Invalid),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl Settings {
    /// These settings with `changes` made, or why one of them cannot be made (and then none is).
    pub fn apply(&self, changes: &Map<String, Value>, choices: &Choices) -> Result<Settings, Invalid> {
        let mut next = self.clone();
        for (key, value) in changes {
            match key.as_str() {
                "embedding_model" => {
                    next.embedding_model = choice("embedding_model", value, &choices.embedding_models)?
                }
                "center_embeddings" => next.center_embeddings = switch("center_embeddings", value)?,
                "clustering" => next.clustering = choice("clustering", value, &choices.clustering)?,
                "max_communities" => next.max_communities = whole("max_communities", value, MAX_COMMUNITIES)?,
                "min_community_size" => {
                    next.min_community_size = whole("min_community_size", value, MIN_COMMUNITY_SIZE)?;
                }
                "min_split_modularity" => {
                    next.min_split_modularity = fraction("min_split_modularity", value, MIN_SPLIT_MODULARITY)?;
                }
                "sub_levels" => next.sub_levels = whole("sub_levels", value, SUB_LEVELS)?,
                "max_links" => next.max_links = whole("max_links", value, MAX_LINKS)?,
                "detect_ai" => next.detect_ai = switch("detect_ai", value)?,
                "include_ai_papers" => next.include_ai_papers = switch("include_ai_papers", value)?,
                "acceptance_filter" => {
                    next.acceptance_filter = choice("acceptance_filter", value, &choices.acceptance)?
                }
                "exclude_flags" => next.exclude_flags = flags("exclude_flags", value, &choices.flags)?,
                "describe" => next.describe = switch("describe", value)?,
                "describe_provider" => next.describe_provider = provider("describe_provider", value)?,
                // checked below, against the provider it is chosen with
                "describe_model" => {
                    let name = value.as_str().filter(|name| !name.trim().is_empty());
                    next.describe_model = name.ok_or(Invalid::NotAName { key: "describe_model" })?.to_owned();
                }
                "describe_effort" => {
                    next.describe_effort = serde_json::from_value(value.clone())
                        .map_err(|_| Invalid::NotAChoice { key: "describe_effort", known: efforts() })?;
                }
                _ => {} // a setting this engine does not have: ignored, as an older or newer interface may send it
            }
        }
        if changes.contains_key("describe_provider") || changes.contains_key("describe_model") {
            next.describe_model = model(&next.describe_provider, &next.describe_model, changes, choices)?;
        }
        Ok(next)
    }

    /// The settings a stored object describes. A key that is missing, or whose value is no longer allowed (a
    /// model that was removed, a file edited by hand), takes its default: the stored file is never a reason for
    /// the app not to start.
    pub fn from_stored(stored: &Map<String, Value>, choices: &Choices) -> Settings {
        let mut settings = Settings::default();
        // a model is checked against its provider, so the settings that choose it are applied together first
        let chosen: Map<String, Value> = stored
            .iter()
            .filter(|(key, _)| MODEL_KEYS.contains(&key.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if let Ok(next) = settings.apply(&chosen, choices) {
            settings = next;
        }
        for (key, value) in stored.iter().filter(|(key, _)| !chosen.contains_key(*key)) {
            let one = Map::from_iter([(key.clone(), value.clone())]);
            if let Ok(next) = settings.apply(&one, choices) {
                settings = next;
            }
        }
        settings
    }
}

/// The settings kept in `file`; the defaults when there is no such file.
pub fn load(file: &Path, choices: &Choices) -> Result<Settings, StoreError> {
    let stored: Map<String, Value> = store::read_json(file)?.unwrap_or_default();
    Ok(Settings::from_stored(&stored, choices))
}

/// Change some settings in `file` and answer them all. A bad value changes nothing at all.
pub fn save(file: &Path, changes: &Map<String, Value>, choices: &Choices) -> Result<Settings, SettingsError> {
    // Checked before the file is locked, so that a bad value is refused without touching it; each value is
    // checked on its own, so the check does not depend on what another writer changes meanwhile.
    let changed = serde_json::to_value(load(file, choices)?.apply(changes, choices)?).unwrap_or_default();
    // a provider changed alone changes its model too (to the provider's first): that is kept with it
    let model = changes.contains_key("describe_provider").then(|| "describe_model".to_owned());
    store::update_json(file, |stored| {
        for key in changes.keys().chain(model.as_ref()) {
            if let Some(value) = changed.get(key) {
                stored.insert(key.clone(), value.clone());
            }
        }
    })?;
    Ok(load(file, choices)?)
}

/// The settings that choose the language model, checked together.
const MODEL_KEYS: [&str; 3] = ["describe_provider", "describe_model", "describe_effort"];

fn efforts() -> Vec<String> {
    ["low", "medium", "high", "xhigh", "max"].map(str::to_owned).to_vec()
}

fn provider(key: &'static str, value: &Value) -> Result<String, Invalid> {
    let known = || Invalid::NotAChoice { key, known: Provider::ALL.iter().map(|one| one.id().to_owned()).collect() };
    let provider = value.as_str().and_then(Provider::from_id).ok_or_else(known)?;
    Ok(provider.id().to_owned())
}

/// The model kept for `provider`: `model` when the provider has it; when only the provider was changed, that
/// provider's first model rather than another provider's.
fn model(provider: &str, model: &str, changes: &Map<String, Value>, choices: &Choices) -> Result<String, Invalid> {
    let provider = Provider::from_id(provider).unwrap_or(Provider::Anthropic);
    let about = provider.about();
    let model = match about.models.first() {
        Some(first) if !changes.contains_key("describe_model") => first.id,
        _ => model,
    };
    if provider == Provider::Anthropic {
        // the Claude API's models are the engine's own list, as they always were
        return choice("describe_model", &Value::from(model), &choices.describe_models);
    }
    match about.ids {
        ModelIds::Listed if !about.accepts(model) => Err(Invalid::NotAChoice {
            key: "describe_model",
            known: about.models.iter().map(|one| one.id.to_owned()).collect(),
        }),
        _ if !about.accepts(model) => Err(Invalid::NotAModel { key: "describe_model", provider: about.label }),
        _ => Ok(model.to_owned()),
    }
}

fn switch(key: &'static str, value: &Value) -> Result<bool, Invalid> {
    value.as_bool().ok_or(Invalid::NotASwitch { key })
}

/// A number, or a string that is one (`"12"`, as a text field sends it). `true` is not a number.
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn whole(key: &'static str, value: &Value, (low, high): (u32, u32)) -> Result<u32, Invalid> {
    // NaN is in no range, and infinity in none of these
    let number = number(value).filter(|number| (f64::from(low)..=f64::from(high)).contains(number));
    let Some(number) = number else {
        return Err(Invalid::OutOfRange { key, low: low.to_string(), high: high.to_string() });
    };
    if number.fract() != 0.0 {
        return Err(Invalid::NotWhole { key });
    }
    // within `low..=high`, so it fits; the conversion cannot lose anything
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(number as u32)
}

fn fraction(key: &'static str, value: &Value, (low, high): (f64, f64)) -> Result<f64, Invalid> {
    number(value).filter(|number| (low..=high).contains(number)).ok_or_else(|| Invalid::OutOfRange {
        key,
        low: format!("{low:?}"),
        high: format!("{high:?}"),
    })
}

fn choice(key: &'static str, value: &Value, known: &[String]) -> Result<String, Invalid> {
    let name = value.as_str().filter(|name| !name.trim().is_empty()).ok_or(Invalid::NotAName { key })?;
    // as sent, as the Python backend compares it: " kmeans " is not one of the choices
    if !known.iter().any(|one| one == name) {
        return Err(Invalid::NotAChoice { key, known: known.to_vec() });
    }
    Ok(name.to_owned())
}

/// The flags asked for, in the order of `known`, each once.
fn flags(key: &'static str, value: &Value, known: &[String]) -> Result<Vec<String>, Invalid> {
    let refused = || Invalid::NotFlags { key, known: known.to_vec() };
    let asked = value.as_array().ok_or_else(refused)?;
    let mut names = Vec::with_capacity(asked.len());
    for flag in asked {
        match flag.as_str() {
            Some(name) if known.iter().any(|one| one == name) => names.push(name),
            _ => return Err(refused()),
        }
    }
    Ok(known.iter().filter(|one| names.contains(&one.as_str())).cloned().collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn choices() -> Choices {
        let list = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
        Choices {
            embedding_models: list(&["Qwen/Qwen3-Embedding-0.6B", "BAAI/bge-large-en-v1.5"]),
            clustering: list(&["spectral", "spectral-knn", "kmeans", "fiedler"]),
            acceptance: list(&["all", "highlights", "oral"]),
            flags: list(&["llm_declared", "closed_code", "no_limitations", "human_subjects"]),
            describe_models: list(&["claude-opus-5-5", "claude-sonnet-5"]),
        }
    }

    fn changes(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn the_defaults_are_told_to_the_interface_with_its_names_and_types() {
        let told = serde_json::to_value(Settings::default()).unwrap();
        assert_eq!(
            told,
            json!({
                "embedding_model": "Qwen/Qwen3-Embedding-0.6B", "center_embeddings": true, "clustering": "spectral",
                "max_communities": 24, "min_community_size": 15, "min_split_modularity": 0.05, "sub_levels": 2,
                "max_links": 8, "detect_ai": true, "include_ai_papers": true, "acceptance_filter": "all",
                "exclude_flags": [], "describe": true, "describe_provider": "anthropic",
                "describe_model": "claude-opus-5-5", "describe_effort": "medium",
            })
        );
    }

    #[test]
    fn a_bad_setting_changes_nothing_and_says_what_is_wrong() {
        let cases = [
            (json!({"center_embeddings": "false"}), "center_embeddings must be true or false"), // a string is no switch
            (json!({"max_communities": 0}), "max_communities must be a number between 2 and 500"),
            (json!({"max_communities": "many"}), "max_communities must be a number between 2 and 500"),
            (json!({"max_communities": true}), "max_communities must be a number between 2 and 500"),
            (json!({"max_communities": 12.5}), "max_communities must be a whole number"),
            (json!({"max_communities": "NaN"}), "max_communities must be a number between 2 and 500"),
            (json!({"max_communities": "inf"}), "max_communities must be a number between 2 and 500"),
            (json!({"min_community_size": 0}), "min_community_size must be a number between 2 and 100000"),
            (json!({"clustering": "magic"}), "clustering must be one of spectral, spectral-knn, kmeans, fiedler"),
            (json!({"clustering": " kmeans "}), "clustering must be one of spectral, spectral-knn, kmeans, fiedler"),
            (json!({"clustering": "  "}), "clustering must be a name"),
            (json!({"embedding_model": ""}), "embedding_model must be a name"),
            (json!({"embedding_model": 3}), "embedding_model must be a name"),
            (json!({"describe_model": "gpt"}), "describe_model must be one of claude-opus-5-5, claude-sonnet-5"),
            (json!({"min_split_modularity": 7}), "min_split_modularity must be a number between 0.0 and 1.0"),
            (json!({"acceptance_filter": "best"}), "acceptance_filter must be one of all, highlights, oral"),
            (
                json!({"exclude_flags": ["nonsense"]}),
                "exclude_flags must be a list of: llm_declared, closed_code, no_limitations, human_subjects",
            ),
            (
                json!({"exclude_flags": "llm_declared"}),
                "exclude_flags must be a list of: llm_declared, closed_code, no_limitations, human_subjects",
            ),
        ];
        let before = Settings::default();
        for (bad, said) in cases {
            let mut asked = changes(bad);
            asked.insert("sub_levels".into(), json!(1)); // the good change must not go through either
            let refused = before.apply(&asked, &choices()).unwrap_err();
            assert_eq!(refused.to_string(), said);
        }
    }

    #[test]
    fn good_settings_are_kept_with_their_types_and_unknown_keys_are_ignored() {
        let asked = changes(json!({
            "max_communities": "12", "center_embeddings": false, "clustering": "kmeans", "junk": 1,
            "min_split_modularity": 1, "sub_levels": 0.0,
        }));
        let saved = Settings::default().apply(&asked, &choices()).unwrap();
        assert_eq!((saved.max_communities, saved.center_embeddings, saved.clustering.as_str()), (12, false, "kmeans"));
        assert_eq!((saved.min_split_modularity, saved.sub_levels), (1.0, 0));
        assert!(serde_json::to_value(&saved).unwrap().get("junk").is_none());
    }

    #[test]
    fn excluded_flags_are_kept_once_each_in_the_order_they_are_listed() {
        let asked = changes(json!({"exclude_flags": ["closed_code", "llm_declared", "closed_code"]}));
        let saved = Settings::default().apply(&asked, &choices()).unwrap();
        assert_eq!(saved.exclude_flags, ["llm_declared", "closed_code"]);
    }

    #[test]
    fn saved_settings_are_read_back_and_a_bad_change_leaves_the_file_as_it_was() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("settings.json");
        assert_eq!(load(&file, &choices()).unwrap(), Settings::default()); // no file yet
        let saved = save(&file, &changes(json!({"detect_ai": false, "max_links": 3})), &choices()).unwrap();
        assert_eq!((saved.detect_ai, saved.max_links), (false, 3));
        let before = std::fs::read(&file).unwrap();
        let refused = save(&file, &changes(json!({"detect_ai": true, "max_links": 0})), &choices()).unwrap_err();
        assert!(matches!(refused, SettingsError::Invalid(Invalid::OutOfRange { key: "max_links", .. })));
        assert_eq!(std::fs::read(&file).unwrap(), before);
        assert_eq!(load(&file, &choices()).unwrap(), saved);
    }

    #[test]
    fn the_language_model_is_a_provider_and_one_of_its_models_checked_together() {
        let before = Settings::default();
        let openai = before
            .apply(
                &changes(
                    json!({"describe_provider": "openai", "describe_model": "gpt-6.2-sol", "describe_effort": "high"}),
                ),
                &choices(),
            )
            .unwrap();
        assert_eq!((openai.describe_provider.as_str(), openai.describe_model.as_str()), ("openai", "gpt-6.2-sol"));
        assert_eq!(openai.describe_effort, Effort::High);
        // a provider whose ids may be typed takes an id it does not list; the Claude API only its own
        let typed = before
            .apply(&changes(json!({"describe_provider": "compatible", "describe_model": "qwen3:32b"})), &choices());
        assert_eq!(typed.unwrap().describe_model, "qwen3:32b");
        let cases = [
            (json!({"describe_provider": "claude"}), "describe_provider must be one of"),
            (
                json!({"describe_provider": "openai", "describe_model": "../etc"}),
                "describe_model must be a model id OpenAI accepts",
            ),
            (
                json!({"describe_provider": "anthropic", "describe_model": "gpt-6.2-sol"}),
                "describe_model must be one of claude-opus-5-5",
            ),
            (json!({"describe_effort": "extreme"}), "describe_effort must be one of low, medium, high, xhigh, max"),
        ];
        for (bad, said) in cases {
            let refused = before.apply(&changes(bad), &choices()).unwrap_err().to_string();
            assert!(refused.starts_with(said), "{refused}");
        }
        // the provider changed alone: its first model, not the other provider's
        let switched = before.apply(&changes(json!({"describe_provider": "gemini"})), &choices()).unwrap();
        assert_eq!(switched.describe_model, "gemini-4-carbon");
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("settings.json");
        save(&file, &changes(json!({"describe_provider": "gemini"})), &choices()).unwrap();
        save(&file, &changes(json!({"describe_effort": "low"})), &choices()).unwrap();
        let loaded = load(&file, &choices()).unwrap();
        assert_eq!((loaded.describe_provider.as_str(), loaded.describe_model.as_str()), ("gemini", "gemini-4-carbon"));
        assert_eq!(loaded.describe_effort, Effort::Low);
    }

    #[test]
    fn a_stored_value_that_is_no_longer_allowed_takes_its_default_and_the_others_are_kept() {
        let stored = changes(json!({"clustering": "removed-method", "max_links": "lots", "sub_levels": 4, "old": 1}));
        let settings = Settings::from_stored(&stored, &choices());
        assert_eq!((settings.clustering.as_str(), settings.max_links, settings.sub_levels), ("spectral", 8, 4));
    }
}
