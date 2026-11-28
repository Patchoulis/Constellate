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

use crate::library::core::store::{self, StoreError};
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
    pub describe_provider: Provider,
    /// One of that provider's models: for the Claude API one of [`Choices::describe_models`]; for a provider whose
    /// ids may be typed, any plain id it accepts.
    pub describe_model: String,
    /// How hard the model thinks: sent as the nearest effort the model has, or not at all to one without.
    pub describe_effort: Effort,
    /// The provider of the model that reads papers (tables, agree / disagree, questions, claims, weaknesses,
    /// explanations, citation kinds). None: that model is the one above, which names communities and writes
    /// overviews ([`Settings::model_for`]). Set, it is set with its model and effort, checked together.
    pub reading_provider: Option<Provider>,
    /// One of that provider's models, checked as [`Settings::describe_model`] is.
    pub reading_model: Option<String>,
    /// How hard the model that reads papers thinks.
    pub reading_effort: Option<Effort>,
}

/// Which of the two choices of language model: the one that names communities and writes their overviews, or the
/// one that reads papers. The second is the first until it is chosen on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelRole {
    Naming,
    Reading,
}

/// A provider, one of its models and an effort: what a [`ModelRole`] comes to in the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelPick<'a> {
    pub provider: Provider,
    pub model: &'a str,
    pub effort: Effort,
}

impl Settings {
    /// The provider, model and effort chosen for `role`. The model that reads papers is the naming one while it has
    /// not been chosen on its own, so that nothing changes for anyone who never chooses it.
    pub fn model_for(&self, role: ModelRole) -> ModelPick<'_> {
        let naming =
            ModelPick { provider: self.describe_provider, model: &self.describe_model, effort: self.describe_effort };
        match (role, self.reading_provider, self.reading_model.as_deref()) {
            (ModelRole::Reading, Some(provider), Some(model)) => {
                ModelPick { provider, model, effort: self.reading_effort.unwrap_or(self.describe_effort) }
            }
            _ => naming,
        }
    }

    /// Whether the model that reads papers is the naming one, by not having been chosen on its own.
    pub fn reads_with_naming_model(&self) -> bool {
        self.reading_provider.is_none()
    }
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
            describe_provider: Provider::Anthropic,
            describe_model: "claude-opus-5-5".to_owned(),
            describe_effort: Effort::Medium,
            reading_provider: None,
            reading_model: None,
            reading_effort: None,
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
    /// The model that reads papers set back to the naming one (`reading_provider: null`) and chosen in one change.
    #[error("{key} cannot be set while reading_provider is null (the model that reads papers is then the naming one)")]
    WithoutProvider { key: &'static str },
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
                // a name typed or pasted keeps no spaces around it, as the Python backend kept it
                "embedding_model" => {
                    next.embedding_model = choice("embedding_model", &trimmed(value), &choices.embedding_models)?
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
                    let name = value.as_str().map(str::trim).filter(|name| !name.is_empty());
                    next.describe_model = name.ok_or(Invalid::NotAName { key: "describe_model" })?.to_owned();
                }
                "describe_effort" => {
                    next.describe_effort = serde_json::from_value(value.clone())
                        .map_err(|_| Invalid::NotAChoice { key: "describe_effort", known: efforts() })?;
                }
                // the model that reads papers: below, once the naming one it starts from is known
                "reading_provider" | "reading_model" | "reading_effort" => {}
                _ => {} // a setting this engine does not have: ignored, as an older or newer interface may send it
            }
        }
        if changes.contains_key("describe_provider") || changes.contains_key("describe_model") {
            let switched = next.describe_provider != self.describe_provider;
            let first = switched && !changes.contains_key("describe_model");
            next.describe_model =
                model("describe_model", next.describe_provider, &next.describe_model, first, choices)?;
        }
        if READING_KEYS.iter().any(|key| changes.contains_key(*key)) {
            next = next.with_reading(changes, choices)?;
        }
        Ok(next)
    }

    /// These settings with the model that reads papers changed as `changes` say: set back to the naming one by
    /// `reading_provider: null`, else chosen from what it is now (the naming one, while it has not been chosen on
    /// its own) with the provider, model and effort sent, the model checked against its provider as the naming
    /// one is.
    fn with_reading(mut self, changes: &Map<String, Value>, choices: &Choices) -> Result<Settings, Invalid> {
        if changes.get("reading_provider").is_some_and(Value::is_null) {
            if let Some(key) = ["reading_model", "reading_effort"]
                .into_iter()
                .find(|key| changes.get(*key).is_some_and(|value| !value.is_null()))
            {
                return Err(Invalid::WithoutProvider { key });
            }
            (self.reading_provider, self.reading_model, self.reading_effort) = (None, None, None);
            return Ok(self);
        }
        let now = self.model_for(ModelRole::Reading);
        let provider = match changes.get("reading_provider") {
            Some(value) => provider("reading_provider", value)?,
            None => now.provider,
        };
        let typed = match changes.get("reading_model") {
            Some(value) => {
                let name = value.as_str().map(str::trim).filter(|name| !name.is_empty());
                name.ok_or(Invalid::NotAName { key: "reading_model" })?.to_owned()
            }
            None => now.model.to_owned(),
        };
        let effort = match changes.get("reading_effort") {
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|_| Invalid::NotAChoice { key: "reading_effort", known: efforts() })?,
            None => now.effort,
        };
        let first = provider != now.provider && !changes.contains_key("reading_model");
        let checked = model("reading_model", provider, &typed, first, choices)?;
        (self.reading_provider, self.reading_model, self.reading_effort) =
            (Some(provider), Some(checked), Some(effort));
        Ok(self)
    }

    /// The settings a stored object describes. A key that is missing, or whose value is no longer allowed (a
    /// model that was removed, a file edited by hand), takes its default: the stored file is never a reason for
    /// the app not to start.
    pub fn from_stored(stored: &Map<String, Value>, choices: &Choices) -> Settings {
        let mut settings = Settings::default();
        // a model is checked against its provider, so the settings that choose it are applied together first: the
        // naming model's, then the reading model's, which starts from the naming one (a file written before there
        // was a reading model has none of its keys, and reads papers with the naming model)
        let only = |keys: &[&str]| -> Map<String, Value> {
            stored.iter().filter(|(key, _)| keys.contains(&key.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        for [provider, model, effort] in [MODEL_KEYS, READING_KEYS] {
            if provider == "reading_provider" && stored.get(provider).is_none_or(Value::is_null) {
                continue; // reads with the naming model, whatever else a hand-edited file says of it
            }
            let chosen = only(&[provider, model, effort]);
            // when they do not go together (a model since removed, an effort a newer engine wrote), as much of them
            // is kept as can be: the provider with its model, else the provider with its first model, and the
            // effort alone; the provider is never dropped for a bad model, since it decides where papers' text is
            // sent. A reading provider that cannot be kept at all leaves the reading model unset (and said), never
            // made the naming provider by its effort alone
            let part = |keys: &[&str]| -> Map<String, Value> {
                let kept = chosen.iter().filter(|(key, _)| keys.contains(&key.as_str()));
                kept.map(|(k, v)| (k.clone(), v.clone())).collect()
            };
            match settings.apply(&chosen, choices) {
                Ok(next) => settings = next,
                Err(_) => {
                    let with_model = settings.apply(&part(&[provider, model]), choices);
                    if let Ok(next) = with_model.or_else(|_| settings.apply(&part(&[provider]), choices)) {
                        settings = next;
                    }
                    let unset = provider == "reading_provider" && settings.reading_provider.is_none();
                    let with_effort = (!unset).then(|| settings.apply(&part(&[effort]), choices).ok()).flatten();
                    if let Some(next) = with_effort {
                        settings = next;
                    }
                }
            }
        }
        let grouped = |key: &str| MODEL_KEYS.contains(&key) || READING_KEYS.contains(&key);
        for (key, value) in stored.iter().filter(|(key, _)| !grouped(key)) {
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
    let (settings, mismatch) = Settings::from_stored_checked(&stored, choices);
    if let Some(mismatch) = mismatch {
        // told where the engine tells what is wrong with its files: the model used is the one the settings show
        eprintln!("settings: {mismatch}");
    }
    Ok(settings)
}

/// Change some settings in `file` and answer them all. A bad value changes nothing at all.
///
/// The change is checked against the settings as stored when the file is locked, and written in the same step:
/// two windows changing the provider and the model at once cannot store a model of one provider with the other
/// (each change is checked against what the other wrote). The settings that choose the model are always written
/// together, so that what is stored is a pair that was checked together.
pub fn save(file: &Path, changes: &Map<String, Value>, choices: &Choices) -> Result<Settings, SettingsError> {
    let saved = store::try_update_json(file, |stored| {
        let next = Settings::from_stored(stored, choices).apply(changes, choices)?;
        let changed = serde_json::to_value(&next).unwrap_or_default();
        let touches = |group: [&'static str; 3]| {
            let touched = changes.keys().any(|key| group.contains(&key.as_str()));
            group.into_iter().filter(move |_| touched).map(str::to_owned)
        };
        for key in changes.keys().cloned().chain(touches(MODEL_KEYS)).chain(touches(READING_KEYS)) {
            if let Some(value) = changed.get(&key) {
                stored.insert(key, value.clone());
            }
        }
        Ok::<Settings, Invalid>(next)
    })?;
    saved?;
    Ok(load(file, choices)?)
}

/// Settings stored that do not go together: what was used instead is said, never chosen silently.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the stored model {stored} is not one of {provider}'s ({why}): {used} is used instead until another is chosen")]
pub struct Mismatch {
    pub provider: &'static str,
    pub stored: String,
    pub used: String,
    pub why: String,
}

impl Settings {
    /// [`Settings::from_stored`], and what of a stored model did not belong with its stored provider, if any (the
    /// naming model's first, else the reading model's).
    pub fn from_stored_checked(stored: &Map<String, Value>, choices: &Choices) -> (Settings, Option<Mismatch>) {
        let settings = Settings::from_stored(stored, choices);
        let together: Map<String, Value> = stored
            .iter()
            .filter(|(key, _)| ["describe_provider", "describe_model"].contains(&key.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let naming = match Settings::default().apply(&together, choices) {
            Err(why) if stored.contains_key("describe_model") => Some(Mismatch {
                provider: settings.describe_provider.about().label,
                stored: stored.get("describe_model").and_then(Value::as_str).unwrap_or("").to_owned(),
                used: settings.describe_model.clone(),
                why: why.to_string(),
            }),
            _ => None,
        };
        // the model that reads papers, when it is chosen on its own: checked on the naming model read
        let reading: Map<String, Value> = stored
            .iter()
            .filter(|(key, _)| ["reading_provider", "reading_model"].contains(&key.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let chosen_alone = stored.get("reading_provider").is_some_and(|value| !value.is_null());
        let reading = match settings.apply(&reading, choices) {
            Err(why) if chosen_alone && stored.get("reading_model").is_some_and(|value| !value.is_null()) => {
                let used = settings.model_for(ModelRole::Reading);
                Some(Mismatch {
                    provider: used.provider.about().label,
                    stored: stored.get("reading_model").and_then(Value::as_str).unwrap_or("").to_owned(),
                    used: used.model.to_owned(),
                    why: why.to_string(),
                })
            }
            _ => None,
        };
        (settings, naming.or(reading))
    }
}

/// The settings that choose the language model, checked together.
const MODEL_KEYS: [&str; 3] = ["describe_provider", "describe_model", "describe_effort"];
/// The settings that choose the model that reads papers, checked together (and against the naming model while
/// they are not set).
const READING_KEYS: [&str; 3] = ["reading_provider", "reading_model", "reading_effort"];

fn efforts() -> Vec<String> {
    ["low", "medium", "high", "xhigh", "max"].map(str::to_owned).to_vec()
}

fn provider(key: &'static str, value: &Value) -> Result<Provider, Invalid> {
    let known = || Invalid::NotAChoice { key, known: Provider::ALL.iter().map(|one| one.id().to_owned()).collect() };
    value.as_str().and_then(Provider::from_id).ok_or_else(known)
}

/// The model kept for `provider` (the setting `key`): `model` when the provider has it; the provider's `first` model
/// when it was switched without a model, rather than another provider's (and for one that lists none, a refusal:
/// its model must be given).
fn model(
    key: &'static str,
    provider: Provider,
    model: &str,
    first: bool,
    choices: &Choices,
) -> Result<String, Invalid> {
    let about = provider.about();
    let model = match (first, about.models.first()) {
        (true, Some(first)) => first.id,
        (true, None) => return Err(Invalid::NotAModel { key, provider: about.label }),
        (false, _) => model,
    };
    if provider == Provider::Anthropic {
        // the Claude API's models are the engine's own list, as they always were
        return choice(key, &Value::from(model), &choices.describe_models);
    }
    match about.ids {
        ModelIds::Listed if !about.accepts(model) => {
            Err(Invalid::NotAChoice { key, known: about.models.iter().map(|one| one.id.to_owned()).collect() })
        }
        _ if !about.accepts(model) => Err(Invalid::NotAModel { key, provider: about.label }),
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

/// A name without the spaces around it; anything else as it is, for [`choice`] to refuse. Only for the names a
/// person may type (the models): the clustering method and the acceptance filter are picked from a list, and the
/// Python backend compared those as sent.
fn trimmed(value: &Value) -> Value {
    match value {
        Value::String(name) => Value::from(name.trim()),
        other => other.clone(),
    }
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
                "reading_provider": null, "reading_model": null, "reading_effort": null,
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

    /// `test_settings_keep_their_types_and_ignore_unknown_keys` of the Python backend: through the file this time,
    /// and what is read back afterwards is what the change answered.
    #[test]
    fn saved_settings_keep_their_types_ignore_unknown_keys_and_are_what_is_read_back() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("settings.json");
        let asked = changes(json!({"max_communities": "12", "clustering": "kmeans", "nonsense": 1}));
        let saved = save(&file, &asked, &choices()).unwrap();
        assert_eq!((saved.max_communities, saved.clustering.as_str()), (12, "kmeans"));
        let told = serde_json::to_value(&saved).unwrap();
        assert_eq!(told["max_communities"], json!(12)); // a number, not the string that was sent
        assert!(told.get("nonsense").is_none());
        assert_eq!(load(&file, &choices()).unwrap(), saved);
        let stored: Map<String, Value> = store::read_json(&file).unwrap().unwrap();
        assert!(!stored.contains_key("nonsense"));
    }

    /// `test_the_setting_is_a_switch_and_the_interface_is_told_the_state` of the Python backend, its settings part.
    #[test]
    fn the_ai_text_detector_is_a_switch_saved_as_sent() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("settings.json");
        assert!(!save(&file, &changes(json!({"detect_ai": false})), &choices()).unwrap().detect_ai);
        assert!(!load(&file, &choices()).unwrap().detect_ai);
        assert!(save(&file, &changes(json!({"detect_ai": true})), &choices()).unwrap().detect_ai);
        let refused = save(&file, &changes(json!({"detect_ai": 0})), &choices()).unwrap_err();
        assert_eq!(refused.to_string(), "detect_ai must be true or false");
    }

    #[test]
    fn the_bounds_themselves_are_allowed_and_one_past_them_is_not() {
        let allowed = changes(json!({
            "max_communities": 500, "min_community_size": 2, "sub_levels": 6, "max_links": 1,
            "min_split_modularity": 0,
        }));
        let saved = Settings::default().apply(&allowed, &choices()).unwrap();
        assert_eq!(
            (saved.max_communities, saved.min_community_size, saved.sub_levels, saved.max_links),
            (500, 2, 6, 1)
        );
        assert_eq!(saved.min_split_modularity, 0.0);
        let cases = [
            (json!({"max_communities": 501}), "max_communities must be a number between 2 and 500"),
            (json!({"max_communities": 1}), "max_communities must be a number between 2 and 500"),
            (json!({"min_community_size": 100_001}), "min_community_size must be a number between 2 and 100000"),
            (json!({"sub_levels": -1}), "sub_levels must be a number between 0 and 6"),
            (json!({"sub_levels": 7}), "sub_levels must be a number between 0 and 6"),
            (json!({"max_links": 51}), "max_links must be a number between 1 and 50"),
            (json!({"max_links": null}), "max_links must be a number between 1 and 50"),
            (json!({"min_split_modularity": -0.01}), "min_split_modularity must be a number between 0.0 and 1.0"),
            (json!({"min_split_modularity": "half"}), "min_split_modularity must be a number between 0.0 and 1.0"),
            (json!({"describe": "yes"}), "describe must be true or false"),
            (json!({"include_ai_papers": null}), "include_ai_papers must be true or false"),
        ];
        for (bad, said) in cases {
            assert_eq!(Settings::default().apply(&changes(bad), &choices()).unwrap_err().to_string(), said);
        }
    }

    /// The Python backend kept a typed name without the spaces around it; a name picked from a list (the clustering
    /// method, the acceptance filter) it compared as sent, and so does this one.
    #[test]
    fn a_typed_model_name_is_kept_without_the_spaces_around_it() {
        let asked =
            changes(json!({"embedding_model": " BAAI/bge-large-en-v1.5 ", "describe_model": " claude-sonnet-5\n"}));
        let saved = Settings::default().apply(&asked, &choices()).unwrap();
        assert_eq!(saved.embedding_model, "BAAI/bge-large-en-v1.5");
        assert_eq!(saved.describe_model, "claude-sonnet-5");
        let typed = changes(json!({"describe_provider": "compatible", "describe_model": "  qwen3:32b "}));
        assert_eq!(Settings::default().apply(&typed, &choices()).unwrap().describe_model, "qwen3:32b");
        let unknown = Settings::default().apply(&changes(json!({"embedding_model": "my/own-model"})), &choices());
        assert!(
            unknown.unwrap_err().to_string().starts_with("embedding_model must be one of Qwen/Qwen3-Embedding-0.6B")
        );
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
        assert_eq!((openai.describe_provider.id(), openai.describe_model.as_str()), ("openai", "gpt-6.2-sol"));
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
        // the same provider sent again keeps its model; one that lists none must be given its model
        let astra = before
            .apply(&changes(json!({"describe_provider": "openai", "describe_model": "gpt-6.2-astra"})), &choices());
        let again = astra.unwrap().apply(&changes(json!({"describe_provider": "openai"})), &choices()).unwrap();
        assert_eq!(again.describe_model, "gpt-6.2-astra");
        assert!(before.apply(&changes(json!({"describe_provider": "compatible"})), &choices()).is_err());
        // a stored model since removed keeps its provider (where papers go), with that provider's first model
        let stored =
            changes(json!({"describe_provider": "openai", "describe_model": "gpt-gone", "describe_effort": "extreme"}));
        let stored = Settings::from_stored(&stored, &Choices { describe_models: Vec::new(), ..choices() });
        assert_eq!((stored.describe_provider.id(), stored.describe_model.as_str()), ("openai", "gpt-gone"));
        let removed = changes(
            json!({"describe_provider": "anthropic", "describe_model": "claude-gone", "describe_effort": "low"}),
        );
        let removed = Settings::from_stored(&removed, &choices());
        assert_eq!((removed.describe_model.as_str(), removed.describe_effort), ("claude-opus-5-5", Effort::Low));
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("settings.json");
        save(&file, &changes(json!({"describe_provider": "gemini"})), &choices()).unwrap();
        save(&file, &changes(json!({"describe_effort": "low"})), &choices()).unwrap();
        let loaded = load(&file, &choices()).unwrap();
        assert_eq!((loaded.describe_provider.id(), loaded.describe_model.as_str()), ("gemini", "gemini-4-carbon"));
        assert_eq!(loaded.describe_effort, Effort::Low);
    }

    #[test]
    fn the_provider_can_only_be_one_of_the_providers_and_is_told_by_its_id() {
        let chosen = Settings::default().apply(&changes(json!({"describe_provider": "openai"})), &choices()).unwrap();
        assert_eq!(chosen.describe_provider, Provider::OpenAi);
        let told = serde_json::to_value(&chosen).unwrap();
        assert_eq!(told["describe_provider"], "openai");
        assert_eq!(serde_json::from_value::<Settings>(told.clone()).unwrap(), chosen);
        // a name that is not a provider's id is no provider: not in a change, not read back, not in another case
        for not_one in ["OpenAI", "claude", "", "openai "] {
            let refused = Settings::default().apply(&changes(json!({"describe_provider": not_one})), &choices());
            assert!(refused.is_err(), "{not_one:?}");
            let mut stored = told.clone();
            stored["describe_provider"] = json!(not_one);
            assert!(serde_json::from_value::<Settings>(stored).is_err(), "{not_one:?}");
        }
        // a stored file that names no provider keeps the default one, the Claude API
        let stored = Settings::from_stored(&changes(json!({"describe_provider": "OpenAI"})), &choices());
        assert_eq!(stored.describe_provider, Provider::Anthropic);
    }

    #[test]
    fn two_windows_changing_the_provider_and_the_model_at_once_never_store_a_pair_that_does_not_go_together() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("settings.json");
        let file = &file;
        std::thread::scope(|scope| {
            // one window switches the provider alone, back and forth; the other picks an OpenAI model alone,
            // which is a choice only while OpenAI is the provider
            scope.spawn(move || {
                for round in 0..150 {
                    let provider = if round % 2 == 0 { "anthropic" } else { "openai" };
                    save(file, &changes(json!({"describe_provider": provider})), &choices()).unwrap();
                }
            });
            scope.spawn(move || {
                for _ in 0..150 {
                    // refused when the provider is the Claude API's, as checked under the lock
                    let _ = save(file, &changes(json!({"describe_model": "gpt-6.2-astra"})), &choices());
                }
            });
            scope.spawn(move || {
                for _ in 0..300 {
                    let stored: Map<String, Value> = store::read_json(file).unwrap().unwrap_or_default();
                    let (_, mismatch) = Settings::from_stored_checked(&stored, &choices());
                    assert_eq!(mismatch, None, "{stored:?}");
                }
            });
        });
    }

    #[test]
    fn a_stored_model_that_is_not_its_providers_is_said_not_swapped_silently() {
        let stored = changes(json!({"describe_provider": "anthropic", "describe_model": "gpt-6.2-astra"}));
        let (settings, mismatch) = Settings::from_stored_checked(&stored, &choices());
        assert_eq!(settings.describe_model, "claude-opus-5-5");
        let mismatch = mismatch.unwrap();
        assert_eq!((mismatch.stored.as_str(), mismatch.used.as_str()), ("gpt-6.2-astra", "claude-opus-5-5"));
        assert!(mismatch.to_string().contains("is used instead"), "{mismatch}");
        let fine = changes(json!({"describe_provider": "openai", "describe_model": "gpt-6.2-astra"}));
        assert_eq!(Settings::from_stored_checked(&fine, &choices()).1, None);
    }

    fn reading(settings: &Settings) -> (&'static str, &str, Effort) {
        let pick = settings.model_for(ModelRole::Reading);
        (pick.provider.id(), pick.model, pick.effort)
    }

    #[test]
    fn the_model_that_reads_papers_is_the_naming_one_until_it_is_chosen_on_its_own() {
        let before = Settings::default();
        assert!(before.reads_with_naming_model());
        assert_eq!(reading(&before), ("anthropic", "claude-opus-5-5", Effort::Medium));
        // the naming model changed: the reading one follows it while it is not chosen
        let naming = changes(json!({"describe_provider": "openai", "describe_model": "gpt-6.2-astra"}));
        let follows = before.apply(&naming, &choices()).unwrap();
        assert_eq!(reading(&follows), ("openai", "gpt-6.2-astra", Effort::Medium));
        // chosen on its own, with every check the naming model has
        let cheaper = changes(json!({"reading_provider": "gemini", "reading_effort": "low"}));
        let chosen = follows.apply(&cheaper, &choices()).unwrap();
        assert_eq!(reading(&chosen), ("gemini", "gemini-4-carbon", Effort::Low), "a provider alone: its first model");
        let naming_pick = chosen.model_for(ModelRole::Naming);
        assert_eq!((naming_pick.provider, naming_pick.model), (Provider::OpenAi, "gpt-6.2-astra"), "naming unchanged");
        let typed = changes(json!({"reading_provider": "compatible", "reading_model": " qwen3:32b "}));
        assert_eq!(reading(&before.apply(&typed, &choices()).unwrap()).1, "qwen3:32b");
        // a model alone is of the provider it reads with now, and an effort alone keeps the model
        let model_only = before.apply(&changes(json!({"reading_model": "claude-sonnet-5"})), &choices()).unwrap();
        assert_eq!(reading(&model_only), ("anthropic", "claude-sonnet-5", Effort::Medium));
        let effort_only = model_only.apply(&changes(json!({"reading_effort": "high"})), &choices()).unwrap();
        assert_eq!(reading(&effort_only), ("anthropic", "claude-sonnet-5", Effort::High));
        // once chosen, changing the naming model leaves it as it is
        let renamed = effort_only.apply(&changes(json!({"describe_provider": "gemini"})), &choices()).unwrap();
        assert_eq!(reading(&renamed), ("anthropic", "claude-sonnet-5", Effort::High));
        // set back: the naming model again
        let back = renamed.apply(&changes(json!({"reading_provider": null})), &choices()).unwrap();
        assert!(back.reads_with_naming_model());
        assert_eq!(reading(&back), ("gemini", "gemini-4-carbon", Effort::Medium));
        let all_null = changes(json!({"reading_provider": null, "reading_model": null, "reading_effort": null}));
        assert!(renamed.apply(&all_null, &choices()).unwrap().reads_with_naming_model());
        let cases = [
            (json!({"reading_provider": "claude"}), "reading_provider must be one of"),
            (json!({"reading_model": "gpt-6.2-sol"}), "reading_model must be one of claude-opus-5-5"),
            (
                json!({"reading_provider": "openai", "reading_model": "../etc"}),
                "reading_model must be a model id OpenAI accepts",
            ),
            (json!({"reading_provider": "compatible"}), "reading_model must be a model id"),
            (json!({"reading_effort": "extreme"}), "reading_effort must be one of low, medium, high, xhigh, max"),
            (json!({"reading_model": null}), "reading_model must be a name"),
            (json!({"reading_provider": null, "reading_model": "claude-sonnet-5"}), "reading_model cannot be set"),
        ];
        for (bad, said) in cases {
            let mut asked = changes(bad);
            asked.insert("sub_levels".into(), json!(1)); // the good change must not go through either
            let refused = before.apply(&asked, &choices()).unwrap_err().to_string();
            assert!(refused.starts_with(said), "{refused}");
        }
    }

    #[test]
    fn a_settings_file_written_before_the_reading_model_reads_papers_with_the_naming_model() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("settings.json");
        let older =
            json!({"describe_provider": "openai", "describe_model": "gpt-6.2-astra", "describe_effort": "high"});
        std::fs::write(&file, older.to_string()).unwrap();
        let loaded = load(&file, &choices()).unwrap();
        assert!(loaded.reads_with_naming_model());
        assert_eq!(reading(&loaded), ("openai", "gpt-6.2-astra", Effort::High));
        // a change of the reading model writes its three keys together, and nothing else of the file changes
        let saved = save(&file, &changes(json!({"reading_provider": "gemini"})), &choices()).unwrap();
        let stored: Map<String, Value> = store::read_json(&file).unwrap().unwrap();
        assert_eq!(
            (&stored["reading_provider"], &stored["reading_model"], &stored["reading_effort"]),
            (&json!("gemini"), &json!("gemini-4-carbon"), &json!("high"))
        );
        assert_eq!(stored["describe_model"], json!("gpt-6.2-astra"));
        assert_eq!(load(&file, &choices()).unwrap(), saved);
        save(&file, &changes(json!({"reading_provider": null})), &choices()).unwrap();
        let stored: Map<String, Value> = store::read_json(&file).unwrap().unwrap();
        assert_eq!(stored["reading_model"], Value::Null);
        assert!(load(&file, &choices()).unwrap().reads_with_naming_model());
    }

    #[test]
    fn a_stored_reading_model_that_is_not_its_providers_keeps_the_provider_and_is_said() {
        // a model since removed: the provider is kept (it decides where papers' text is sent), with its first model
        let stored = changes(json!({
            "describe_provider": "openai", "describe_model": "gpt-6.2-astra",
            "reading_provider": "anthropic", "reading_model": "gpt-6.2-astra", "reading_effort": "extreme",
        }));
        let (settings, mismatch) = Settings::from_stored_checked(&stored, &choices());
        assert_eq!(reading(&settings), ("anthropic", "claude-opus-5-5", Effort::Medium));
        let mismatch = mismatch.unwrap();
        assert_eq!((mismatch.stored.as_str(), mismatch.used.as_str()), ("gpt-6.2-astra", "claude-opus-5-5"));
        // a provider whose stored model cannot be kept and that has no first model: left unset and said, never
        // turned into a choice of the naming provider by its effort alone
        let lost =
            changes(json!({"reading_provider": "compatible", "reading_model": "../bad", "reading_effort": "low"}));
        let (settings, mismatch) = Settings::from_stored_checked(&lost, &choices());
        assert!(settings.reads_with_naming_model(), "{settings:?}");
        assert!(mismatch.is_some());
        // a hand-edited file with a model but no provider reads with the naming model
        let half = changes(json!({"reading_provider": null, "reading_model": "claude-sonnet-5"}));
        let (settings, mismatch) = Settings::from_stored_checked(&half, &choices());
        assert!(settings.reads_with_naming_model());
        assert_eq!(mismatch, None);
        let fine = changes(json!({"reading_provider": "openai", "reading_model": "gpt-6.2-sol"}));
        assert_eq!(Settings::from_stored_checked(&fine, &choices()).1, None);
    }

    #[test]
    fn a_stored_value_that_is_no_longer_allowed_takes_its_default_and_the_others_are_kept() {
        let stored = changes(json!({"clustering": "removed-method", "max_links": "lots", "sub_levels": 4, "old": 1}));
        let settings = Settings::from_stored(&stored, &choices());
        assert_eq!((settings.clustering.as_str(), settings.max_links, settings.sub_levels), ("spectral", 8, 4));
    }
}
