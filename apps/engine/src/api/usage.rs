//! What the language model has cost, for the line under the model in the sidebar.
//!
//! ```text
//! GET /api/usage?currency=CAD   everything spent (`Usage` in the contracts): in all, by model and by purpose, with
//!                               the tokens by how they are billed; what this engine has spent since it started; who
//!                               answers (the chosen provider, and whether its key is set, never the key) and with
//!                               which model; and what a US dollar is worth in the currency asked for, and on which day
//! ```
//!
//! Every amount is in US dollars, the currency the providers bill in; the interface multiplies by the rate. The
//! rates are compiled into the engine ([`crate::usage::currency`]), never asked for at run time. A currency that is
//! not three letters, or that the engine has no rate for, is the dollar itself, as the Python had it.
//!
//! A call to a model whose price is not known is counted (in calls, tokens and `unpriced_calls`) and never priced
//! at nothing: a model or a purpose whose every call is unpriced has no `usd` at all (`null`), so that it cannot
//! read as having cost 0. One with some priced calls has the cost of those, and its `unpriced_calls` says it is
//! short. The same holds of the totals, in all and this session: `usd` and `session_usd` are `null` when calls
//! were made and none had a price, and the cost of the priced ones when some had.
//!
//! This session's totals are the ledger's rows stamped at or after the moment the engine built its routes (its
//! start; [`started`]): every row carries the instant it was recorded at, so nothing is counted twice or kept in
//! memory beside the ledger, and an engine that restarts starts its session again at nothing. The rows moved in from
//! the Python's ledger are stamped with the day it started, long before.
//!
//! The Python's ledger (`usage.json` in the library folder) is moved into the database when the library is first
//! opened, as a part of moving the earlier version's data in ([`crate::library::Library::import_old`]): once, recorded as moved,
//! and on its own, so that a damaged file elsewhere does not keep it out. A failure to move it is told in
//! `/api/library` and `/api/status`; the answer is given without it, and the move is tried again at the next start.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::SystemTime;

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::routing::{MethodRouter, get};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::routes::{ApiError, Shared, on_the_side};
use crate::library::{Spending, Totals};
use crate::model::chosen::Chosen;
use crate::usage::{Rate, rate};

/// The longest `?currency=` read: a code is three letters, and anything longer is the dollar without being read.
const LONGEST_CODE: usize = 8;

/// When the engine started: set when its routes are built, before the first request.
static STARTED: OnceLock<DateTime<Utc>> = OnceLock::new();

/// The routes of this module, for [`super::routes::table`]. Building them is the engine starting: the session's
/// totals count from here.
pub fn table() -> Vec<(&'static str, MethodRouter<Shared>)> {
    started();
    vec![("/api/usage", get(usage))]
}

/// When this engine started: from then on is "this session".
pub fn started() -> DateTime<Utc> {
    *STARTED.get_or_init(|| DateTime::<Utc>::from(SystemTime::now()))
}

/// The query: the currency to show the amounts in.
#[derive(Deserialize)]
struct Asked {
    currency: Option<String>,
}

/// What `GET /api/usage` answers: `Usage` in the contracts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Told {
    /// Who answers and with which model, for naming communities and writing overviews.
    pub provider: Answerer,
    pub model: ModelTold,
    /// The same, for reading papers: the naming model's while it has not been chosen on its own.
    pub reading: Reader,
    /// The day of the first call in the ledger; none when nothing has been spent.
    pub since: Option<String>,
    pub currency: Rate,
    /// US dollars, of the priced calls only; none when calls were made and none of them had a price (a cost that
    /// cannot be known is not shown, never as 0).
    pub usd: Option<f64>,
    pub calls: i64,
    pub unpriced_calls: i64,
    /// US dollars spent since the engine started; none as `usd` is.
    pub session_usd: Option<f64>,
    pub session_calls: i64,
    pub models: BTreeMap<String, Entry>,
    pub purposes: BTreeMap<String, Entry>,
}

/// Who answers: the chosen provider, or the host its requests go to when that is not the provider's own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Answerer {
    pub name: String,
    /// Whether it can be asked: its key is set. Only that, never the key.
    pub ready: bool,
}

/// The model the features ask with, by id and as shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModelTold {
    pub name: String,
    pub label: String,
}

/// Who answers the features that read papers, and with which model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Reader {
    pub provider: Answerer,
    pub model: ModelTold,
}

impl Reader {
    fn of(chosen: &Chosen) -> Self {
        Reader { provider: answerer(chosen), model: model_told(chosen) }
    }
}

/// `UsageEntry` in the contracts: one model's or one purpose's calls.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Entry {
    pub calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// US dollars of its priced calls; none when no call of it had a price, which is not a cost of 0.
    pub usd: Option<f64>,
    pub unpriced_calls: i64,
}

/// What some calls cost in US dollars, when any of them had a price.
fn known_usd(totals: &Totals) -> Option<f64> {
    let priced = totals.calls - totals.unpriced_calls;
    (totals.calls == 0 || priced > 0).then(|| totals.usd())
}

impl From<&Totals> for Entry {
    fn from(totals: &Totals) -> Self {
        Entry {
            calls: totals.calls,
            input_tokens: totals.input_tokens,
            output_tokens: totals.output_tokens,
            cache_read_tokens: totals.cache_read_tokens,
            cache_write_tokens: totals.cache_write_tokens,
            usd: known_usd(totals),
            unpriced_calls: totals.unpriced_calls,
        }
    }
}

/// Who answers, as the interface shows it: the provider's name, or the host when its address was set to another
/// (a gateway, a server of the user's own); whether its key is set.
fn answerer(chosen: &Chosen) -> Answerer {
    let label = chosen.provider().about().label;
    let name = match chosen.endpoint() {
        Ok(endpoint) if !endpoint.is_default() => endpoint.host().unwrap_or_else(|| label.to_owned()),
        Ok(_) | Err(_) => label.to_owned(),
    };
    Answerer { name, ready: chosen.can_ask() }
}

/// The model by its id and as its provider lists it; a model typed rather than listed is shown by its id.
fn model_told(chosen: &Chosen) -> ModelTold {
    let name = chosen.model().to_owned();
    let label = chosen.provider().about().model(&name).map_or_else(|| name.clone(), |offered| offered.label.to_owned());
    ModelTold { name, label }
}

/// The answer, from the ledger's totals, this session's, the chosen model (which reads papers too, until
/// [`Told::read_by`] says another does) and the currency.
pub fn told(spending: &Spending, session: &Totals, chosen: &Chosen, currency: Rate) -> Told {
    let entries =
        |table: &BTreeMap<String, Totals>| table.iter().map(|(key, totals)| (key.clone(), totals.into())).collect();
    Told {
        provider: answerer(chosen),
        model: model_told(chosen),
        reading: Reader::of(chosen),
        since: spending.since.clone(),
        currency,
        usd: known_usd(&spending.total),
        calls: spending.total.calls,
        unpriced_calls: spending.total.unpriced_calls,
        session_usd: known_usd(session),
        session_calls: session.calls,
        models: entries(&spending.models),
        purposes: entries(&spending.purposes),
    }
}

impl Told {
    /// The same answer, with `reading` as the model that reads papers.
    pub fn read_by(self, reading: &Chosen) -> Told {
        Told { reading: Reader::of(reading), ..self }
    }
}

/// The currency asked for, read as the Python read it: three letters in any case, else the dollar.
fn currency_of(asked: Option<&str>) -> Rate {
    match asked {
        Some(code) if code.len() <= LONGEST_CODE => rate(code),
        Some(_) | None => Rate::dollar(),
    }
}

async fn usage(
    State(engine): State<Shared>,
    asked: Result<Query<Asked>, QueryRejection>,
) -> Result<Json<Told>, ApiError> {
    // a query that is not one currency (two of them, say) is the dollar, as the Python had it
    let currency = currency_of(asked.ok().and_then(|Query(asked)| asked.currency).as_deref());
    let answer = on_the_side(move || {
        let chosen = engine.chosen()?;
        let reading = engine.reading()?;
        let library = engine.library()?;
        let spending = library.spending()?;
        let session = library.spending_since(started())?;
        Ok(told(&spending, &session, &chosen, currency).read_by(&reading))
    })
    .await?;
    Ok(Json(answer))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::super::routes::Engine;
    use super::super::tests::served;
    use super::*;
    use crate::api::guard::config::tests::STRONG;
    use crate::api::guard::config::{Environment, Listen};
    use crate::library::{Call, Library};
    use crate::model::language::{Effort, Provider, Unavailable};
    use crate::settings::Choices;
    use crate::usage::bundled;

    const AT: &str = "127.0.0.1:8765";

    fn choices() -> Choices {
        let one = |name: &str| vec![name.to_owned()];
        Choices {
            embedding_models: one("Qwen/Qwen3-Embedding-0.6B"),
            clustering: one("spectral"),
            acceptance: one("all"),
            flags: one("llm_declared"),
            describe_models: one("claude-opus-5-5"),
        }
    }

    /// A call as the features record it (`model::language`), priced (or not) at the bundled prices, in millionths
    /// of a dollar rounded up.
    fn call<'a>(model: &'a str, purpose: &'a str, input_tokens: i64, output_tokens: i64) -> Call<'a> {
        let price = crate::usage::price(model, input_tokens as u64, output_tokens as u64);
        let price_micros = price.map(|usd| usd.picodollars().div_ceil(1_000_000) as i64);
        Call { model, purpose, input_tokens, output_tokens, cache_read_tokens: 0, cache_write_tokens: 0, price_micros }
    }

    fn totals(calls: i64, price_micros: i64, unpriced_calls: i64) -> Totals {
        Totals { calls, price_micros, unpriced_calls, ..Totals::default() }
    }

    /// The engine as served on this computer, with a data folder of its own; with `key`, the Claude API's endpoint
    /// with that key (never reached: nothing here asks a model).
    fn engine(data: &Path, key: Option<&str>) -> axum::Router {
        let listen = Listen::from_environment(&Environment::from_pairs([("CONSTELLATE_TOKEN", STRONG)])).unwrap();
        let mut engine = Engine::new(data.to_path_buf(), choices());
        if let Some(key) = key {
            engine.set_claude_key(key, None);
        }
        served(&listen, AT, engine).0
    }

    async fn ask(router: &axum::Router, path: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .uri(path)
            .header(header::HOST, AT)
            .header(header::AUTHORIZATION, format!("Bearer {STRONG}"))
            .body(Body::empty())
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// The field names of `interface <name>` in the contracts, as written there (the nested objects' own fields
    /// are not included).
    fn contract_fields(name: &str) -> BTreeSet<String> {
        let contracts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/contracts/src/index.ts");
        let contracts = std::fs::read_to_string(contracts).unwrap();
        let start = contracts.find(&format!("export interface {name} {{")).unwrap();
        let body = &contracts[start..];
        let body = &body[body.find('\n').unwrap()..body.find("\n}").unwrap()];
        body.lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("/*") && !line.starts_with('*'))
            .filter_map(|line| line.split_once(':').map(|(field, _)| field.trim_end_matches('?').to_owned()))
            .filter(|field| !field.is_empty() && field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .collect()
    }

    fn keys(value: &Value) -> BTreeSet<String> {
        value.as_object().unwrap().keys().cloned().collect()
    }

    #[test]
    fn the_answer_has_the_fields_of_the_contracts_usage_and_usage_entry_and_no_others() {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(&data.path().join("library")).unwrap();
        library.record_call(&call("claude-opus-5-5", "naming communities", 10_000, 1_000)).unwrap();
        let spending = library.spending().unwrap();
        let told = told(&spending, &Totals::default(), &Chosen::claude("claude-opus-5-5"), rate("EUR"));
        let told = serde_json::to_value(told).unwrap();
        assert_eq!(keys(&told), contract_fields("Usage"));
        assert_eq!(keys(&told["models"]["claude-opus-5-5"]), contract_fields("UsageEntry"));
        assert_eq!(keys(&told["purposes"]["naming communities"]), contract_fields("UsageEntry"));
        let names = |fields: &[&str]| fields.iter().map(|field| (*field).to_owned()).collect::<BTreeSet<_>>();
        assert_eq!(keys(&told["provider"]), names(&["name", "ready"]));
        assert_eq!(keys(&told["model"]), names(&["name", "label"]));
        assert_eq!(keys(&told["reading"]), names(&["provider", "model"]));
        assert_eq!(told["reading"]["model"], told["model"], "the naming model reads papers until another is chosen");
        assert_eq!(keys(&told["currency"]), names(&["code", "rate", "date"]));
        assert_eq!(told["currency"]["date"], json!(bundled().unwrap().rates_date.to_string()));
    }

    // test_calls_add_up_by_model_and_by_purpose
    #[test]
    fn calls_add_up_by_model_and_by_purpose_and_unpriced_ones_are_counted_apart() {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(&data.path().join("library")).unwrap();
        library.record_call(&call("claude-opus-5-5", "naming communities", 10_000, 1_000)).unwrap();
        library.record_call(&call("claude-opus-5-5", "community overviews", 100_000, 5_000)).unwrap();
        library.record_call(&call("some-other-model", "naming communities", 500, 500)).unwrap();
        let session = library.spending_since(started()).unwrap();
        let told = told(&library.spending().unwrap(), &session, &Chosen::claude("claude-opus-5-5"), Rate::dollar());
        assert_eq!((told.calls, told.session_calls, told.unpriced_calls), (3, 3, 1));
        assert!((told.usd.unwrap() - (0.06 + 0.5)).abs() < 1e-12 && told.usd == told.session_usd);
        assert_eq!(told.models["claude-opus-5-5"].input_tokens, 110_000);
        assert_eq!(told.purposes["naming communities"].calls, 2);
        assert_eq!(told.purposes["naming communities"].unpriced_calls, 1);
        assert_eq!(told.since.unwrap().len(), "2026-11-27".len());
    }

    #[test]
    fn a_model_or_purpose_with_no_priced_call_has_no_cost_rather_than_a_cost_of_nothing() {
        let unpriced = Entry::from(&totals(2, 0, 2));
        assert_eq!((unpriced.usd, unpriced.unpriced_calls), (None, 2));
        assert_eq!(serde_json::to_value(unpriced).unwrap()["usd"], Value::Null);
        // some priced: the cost of those, short by the others
        assert_eq!(Entry::from(&totals(3, 1_500, 1)).usd, Some(0.0015));
        // a known model with no tokens did cost nothing
        assert_eq!(Entry::from(&totals(1, 0, 0)).usd, Some(0.0));
    }

    #[test]
    fn the_totals_of_someone_who_used_only_unpriced_models_have_no_cost_and_priced_calls_still_total() {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(&data.path().join("library")).unwrap();
        let chosen = Chosen::claude("claude-opus-5-5");
        let nothing =
            told(&library.spending().unwrap(), &library.spending_since(started()).unwrap(), &chosen, Rate::dollar());
        assert_eq!((nothing.usd, nothing.session_usd), (Some(0.0), Some(0.0)), "nothing spent is a known 0");
        library.record_call(&call("some-other-model", "naming communities", 500, 500)).unwrap();
        let unpriced =
            told(&library.spending().unwrap(), &library.spending_since(started()).unwrap(), &chosen, Rate::dollar());
        assert_eq!((unpriced.usd, unpriced.session_usd), (None, None));
        let json = serde_json::to_value(&unpriced).unwrap();
        assert_eq!((json["usd"].clone(), json["session_usd"].clone()), (Value::Null, Value::Null));
        library.record_call(&call("claude-opus-5-5", "naming communities", 10_000, 1_000)).unwrap();
        let both =
            told(&library.spending().unwrap(), &library.spending_since(started()).unwrap(), &chosen, Rate::dollar());
        assert!((both.usd.unwrap() - 0.06).abs() < 1e-12, "{:?}", both.usd);
        assert_eq!((both.session_usd, both.unpriced_calls), (both.usd, 1));
    }

    // test_many_tiny_calls_do_not_drift
    #[test]
    fn many_tiny_calls_do_not_drift() {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(&data.path().join("library")).unwrap();
        for _ in 0..320 {
            library.record_call(&call("claude-haiku-5-5", "naming communities", 70_000, 30_000)).unwrap();
        }
        let session = library.spending_since(started()).unwrap();
        let told = told(&library.spending().unwrap(), &session, &Chosen::claude("claude-haiku-5-5"), Rate::dollar());
        // 320 * (70000 * 0.10 + 30000 * 0.50) dollars per million tokens: summed in whole millionths, exactly
        assert_eq!(told.models["claude-haiku-5-5"].usd, Some(7_040_000.0 / 1e6));
        assert_eq!(told.usd, told.session_usd);
        assert!(told.usd.is_some());
    }

    // test_the_interface_is_told_who_provides_the_model
    #[test]
    fn the_provider_is_named_with_whether_its_key_is_set_and_the_model_with_its_label() {
        let without =
            told(&Spending::default(), &Totals::default(), &Chosen::claude("claude-opus-5-5"), Rate::dollar());
        assert_eq!(without.provider, Answerer { name: "Anthropic".to_owned(), ready: false });
        assert_eq!(
            without.model,
            ModelTold { name: "claude-opus-5-5".to_owned(), label: "Claude Opus 5.5".to_owned() }
        );
        let typed = Chosen::of(Provider::Anthropic, "claude-next", Effort::Medium, Err(Unavailable::NoKey));
        assert_eq!(model_told(&typed).label, "claude-next");
        // another model reads papers: told apart, the naming one as it was
        let local = Chosen::of(Provider::Compatible, "qwen3:32b", Effort::Medium, Err(Unavailable::NoKey));
        let apart = without.clone().read_by(&local);
        assert_eq!((apart.model.clone(), apart.provider.clone()), (without.model.clone(), without.provider.clone()));
        assert_eq!(apart.reading.model, ModelTold { name: "qwen3:32b".to_owned(), label: "qwen3:32b".to_owned() });
        assert_eq!(apart.reading.provider.name, "OpenAI-compatible endpoint");
        // a gateway set in place of the provider's own address is named by its host
        let gateway = Environment::from_pairs([
            ("ANTHROPIC_API_KEY", "test-key-not-real-0123456789"),
            ("ANTHROPIC_BASE_URL", "https://gateway.example.org/v1"),
        ])
        .language_endpoint();
        let chosen = Chosen::of(Provider::Anthropic, "claude-opus-5-5", Effort::Medium, gateway);
        assert_eq!(answerer(&chosen), Answerer { name: "gateway.example.org".to_owned(), ready: true });
    }

    // test_the_dollar_and_nonsense_need_no_asking, test_the_interface_is_told_the_rate_with_what_was_spent
    #[test]
    fn a_currency_is_three_letters_with_a_bundled_rate_or_else_the_dollar() {
        for code in [None, Some("USD"), Some("usd"), Some(""), Some("dollars"), Some("C$"), Some("../x"), Some("XYZ")] {
            assert_eq!(currency_of(code), Rate::dollar(), "{code:?}");
        }
        assert_eq!(currency_of(Some(&"E".repeat(10_000))), Rate::dollar());
        let canadian = currency_of(Some("cad"));
        assert_eq!((canadian.code.as_str(), canadian.date), ("CAD", Some(bundled().unwrap().rates_date)));
    }

    #[tokio::test]
    async fn the_route_answers_with_the_python_ledger_moved_in_once_and_this_sessions_calls_apart() {
        let data = tempfile::tempdir().unwrap();
        let folder = data.path().join("library");
        std::fs::create_dir_all(&folder).unwrap();
        // the Python's ledger, as `usage.record` wrote it
        let ledger = json!({"since": "2026-10-01",
            "models": {
                "claude-opus-5-5": {"calls": 3, "input_tokens": 100, "output_tokens": 50, "usd": 0.0123, "unpriced_calls": 0},
                "mystery": {"calls": 2, "input_tokens": 10, "output_tokens": 5, "usd": 0.0, "unpriced_calls": 2}},
            "purposes": {"naming communities": {"calls": 5, "input_tokens": 110, "output_tokens": 55, "usd": 0.0123,
                "unpriced_calls": 2}}});
        std::fs::write(folder.join("usage.json"), ledger.to_string()).unwrap();
        let key = "test-key-not-real-0123456789";
        let router = engine(data.path(), Some(key));
        Library::open(&folder)
            .unwrap()
            .record_call(&call("claude-opus-5-5", "naming communities", 1_000_000, 0))
            .unwrap();

        let (status, told) = ask(&router, "/api/usage?currency=cad").await;
        assert_eq!(status, StatusCode::OK);
        assert!(!told.to_string().contains(key), "a key is told");
        assert_eq!(told["provider"], json!({"name": "Anthropic", "ready": true}));
        assert_eq!(told["model"], json!({"name": "claude-opus-5-5", "label": "Claude Opus 5.5"}));
        assert_eq!(told["since"], "2026-10-01");
        assert_eq!(told["currency"]["code"], "CAD");
        assert_eq!((told["calls"].clone(), told["unpriced_calls"].clone()), (json!(6), json!(2)));
        assert!((told["usd"].as_f64().unwrap() - 4.0123).abs() < 1e-9);
        // this session: the call made since the engine started, not the Python's
        assert_eq!((told["session_calls"].clone(), told["session_usd"].clone()), (json!(1), json!(4.0)));
        assert_eq!(told["models"]["mystery"]["usd"], Value::Null);
        assert_eq!(told["models"]["mystery"]["unpriced_calls"], 2);
        assert_eq!(told["purposes"]["imported"]["calls"], 5);

        // asked again: the ledger is not moved twice
        let (_, again) = ask(&router, "/api/usage?currency=XX&currency=YY").await;
        assert_eq!(again["calls"], 6);
        assert_eq!(again["currency"], json!({"code": "USD", "rate": 1.0, "date": null}));
    }

    #[tokio::test]
    async fn a_ledger_that_cannot_be_moved_does_not_stop_the_answer() {
        let data = tempfile::tempdir().unwrap();
        let folder = data.path().join("library");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("usage.json"), "{ not json").unwrap();
        let (status, told) = ask(&engine(data.path(), None), "/api/usage").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (told["calls"].clone(), told["usd"].clone(), told["since"].clone()),
            (json!(0), json!(0.0), Value::Null)
        );
        assert_eq!(told["provider"]["ready"], false);
    }
}
