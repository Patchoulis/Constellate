//! Asking the model about many papers at once: what every feature that asks one question per paper (or per
//! citation, per claim) goes through, so that none of them pays twice, overspends, or floods the API.
//!
//! A feature hands over its questions as [`Item`]s, each one [`Ask`] and the [`Slot`] its answer is kept in.
//!
//! 1. **What is kept is not asked again.** An item whose answer is kept ([`Library::kept`]) is
//!    [`Outcome::Kept`] and costs nothing.
//! 2. **The cost is told first.** [`estimate_batch`] says what the rest would cost ("about USD 12 for these 739
//!    papers") without calling anything, for the interface to show before the user starts.
//! 3. **Calls run on a few threads of their own**, at most `in_flight` at once ([`Asking::in_flight`]): the client
//!    blocks on the network, so these are plain scoped threads, never the processor's pool (rayon), which reading
//!    PDFs keeps busy. Items with the same prefix (model, system prompt, shared text: one paper's text asked
//!    several questions) are asked one after another on one thread, so that the first writes the prompt cache and
//!    the others read it, as the estimate counts them.
//! 4. **Nothing new is started once it cannot be paid for or answered**: when a call is refused by the
//!    [`Budget`] with no other call under way (each call holds its worst case before it is sent and settles at
//!    what it took, so racing calls can never pass it together; one refused while others hold theirs waits for
//!    one of them to settle and asks again), when the API says it cannot be used at all ([`Unavailable`]; an
//!    unknown model fails only its own items), or when the job is asked to stop. What was not started is
//!    [`Outcome::NotStarted`] with why. A stop does not cut short a rate-limit wait already begun.
//!    Items with the same slot are asked once and all given that outcome.
//! 5. **A rate limit pauses everyone, then lets them go one by one.** The client waits out a `retry-after`; here
//!    that wait holds back every thread's next request too, and when it is over they go [`SPACING`] apart rather
//!    than all at once into the same limit.
//! 6. **Every answer is kept as it arrives**, so stopping half way loses nothing, and every call is recorded in
//!    the ledger by the client ([`Recorder`]), usable or not.
//! 7. **Outcomes come back in the order of the items**, whatever order the threads finished in.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;

use super::language::{
    Answered, ApiKey, Ask, AskError, Budget, Client, Counting, Endpoint, Estimate, NoAnswer, OverBudget, Planned,
    Recorder, Reply, Transport, TransportError, Unavailable, estimate,
};
use crate::jobs::{Progress, Tracker};
use crate::library::{AnswerKey, Feature, Kept, Library, LibraryError, Slot};
use crate::usage::Usd;

/// Calls under way at once when a feature has no reason to choose otherwise: enough to finish 739 papers in
/// minutes, few enough to stay under the API's rate limits for most accounts.
pub const IN_FLIGHT: usize = 4;
/// How far apart the requests held back by a rate limit are sent once it has passed.
pub const SPACING: Duration = Duration::from_millis(250);

/// One question of a batch.
pub struct Item<'a> {
    /// Where its answer is kept, and looked for before it is asked.
    pub slot: Slot,
    /// The version of the feature's prompt (the key has it too); kept with the answer.
    pub prompt: u32,
    pub ask: Ask<'a>,
    /// What its answer is likely to take, thinking included: for the estimate ([`Planned::answer_tokens`]).
    pub likely_tokens: u32,
}

/// What a batch would cost, before anything is asked: what the interface shows as "about N dollars".
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BatchEstimate {
    pub items: usize,
    /// Kept already: they cost nothing.
    pub kept: usize,
    pub to_ask: usize,
    /// One estimate per model asked ([`Estimate::counting`] says how tokens were reckoned: it is an estimate).
    pub by_model: Vec<Estimate>,
    /// Likely cost of all of them; `None` when a model's price is not known.
    pub usd: Option<Usd>,
    /// If every answer took its `max_tokens` and nothing was read from the cache.
    pub most_usd: Option<Usd>,
}

/// What came of one item.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome<T> {
    /// Its answer was kept from before: not asked.
    Kept(Kept<T>),
    /// Asked and answered now (and kept).
    Answered(Answered<T>),
    /// Asked, with no usable answer.
    Failed(Failure),
    /// Not asked.
    NotStarted(Stopped),
}

/// Why an item that was asked has no answer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    /// This answer only: the next item may well be answered.
    #[error(transparent)]
    NoAnswer(NoAnswer),
    /// The API cannot be used: the batch stopped here.
    #[error(transparent)]
    Unavailable(Unavailable),
    #[error("asking stopped unexpectedly")]
    Panicked,
}

/// Why a batch stopped starting new calls.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Stopped {
    #[error(transparent)]
    OverBudget(OverBudget),
    #[error("asked to stop")]
    Cancelled,
    #[error(transparent)]
    Unavailable(Unavailable),
}

/// What a batch is asked with.
pub struct Asking<'a> {
    /// Where answers are looked for and kept.
    pub library: &'a Library,
    pub transport: &'a dyn Transport,
    pub endpoint: Endpoint,
    /// Where every call is recorded: the library's ledger, as a rule.
    pub recorder: &'a dyn Recorder,
    /// The most calls under way at once ([`IN_FLIGHT`]); at least one.
    pub in_flight: usize,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The items of a batch told apart: those to ask, the answers kept for others, and the repeats.
struct Sorted<T> {
    /// Indices of the items not kept yet, in order, each slot once.
    to_ask: Vec<usize>,
    /// By item: its kept answer, if it has one.
    kept: Vec<Option<Kept<T>>>,
    /// (a repeat, the item asked in its place): an item whose slot an earlier item has is not paid for twice.
    repeats: Vec<(usize, usize)>,
}

fn sorted<T: DeserializeOwned>(library: &Library, items: &[Item<'_>]) -> Result<Sorted<T>, LibraryError> {
    let mut to_ask = Vec::new();
    let mut kept = Vec::with_capacity(items.len());
    let mut repeats = Vec::new();
    let mut first: HashMap<(Feature, &AnswerKey), usize> = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        let found = library.kept::<T>(item.slot.feature, &item.slot.key)?;
        if found.is_none() {
            match first.get(&(item.slot.feature, &item.slot.key)) {
                Some(&original) => repeats.push((index, original)),
                None => {
                    first.insert((item.slot.feature, &item.slot.key), index);
                    to_ask.push(index);
                }
            }
        }
        kept.push(found);
    }
    Ok(Sorted { to_ask, kept, repeats })
}

/// `indices` in lanes: the items with one prefix (model, system prompt, shared part), in their order, lanes in
/// the order their first item comes.
fn lanes(items: &[Item<'_>], indices: &[usize]) -> Vec<Vec<usize>> {
    let mut lanes: Vec<Vec<usize>> = Vec::new();
    let mut by_prefix: HashMap<(&str, &str, Option<&str>), usize> = HashMap::new();
    for &index in indices {
        let ask = &items[index].ask;
        let lane = *by_prefix.entry((ask.model, ask.system, ask.shared)).or_insert_with(|| {
            lanes.push(Vec::new());
            lanes.len() - 1
        });
        lanes[lane].push(index);
    }
    lanes
}

fn characters(text: &str) -> u64 {
    u64::try_from(text.chars().count()).unwrap_or(u64::MAX)
}

/// What asking the items not kept yet would cost, with prompt tokens reckoned as `counting` says (the client's
/// [`Client::counting`] once it has been calibrated, else [`Counting::ASSUMED`]). Nothing is sent.
pub fn estimate_batch<T: DeserializeOwned>(
    library: &Library,
    items: &[Item<'_>],
    counting: Counting,
) -> Result<BatchEstimate, LibraryError> {
    let Sorted { to_ask, repeats, .. } = sorted::<T>(library, items)?;
    let mut by_model: Vec<(&str, Vec<Planned>)> = Vec::new();
    for lane in lanes(items, &to_ask) {
        let asks: Vec<&Item<'_>> = lane.iter().map(|index| &items[*index]).collect();
        let first = &asks[0].ask;
        let plan = Planned {
            shared_chars: characters(first.system) + first.shared.map_or(0, characters),
            question_chars: asks.iter().map(|item| characters(item.ask.question)).max().unwrap_or(0),
            questions: u32::try_from(asks.len()).unwrap_or(u32::MAX),
            answer_tokens: asks.iter().map(|item| item.likely_tokens).max().unwrap_or(0),
            max_tokens: asks.iter().map(|item| item.ask.max_tokens).max().unwrap_or(0),
        };
        match by_model.iter_mut().find(|(model, _)| *model == first.model) {
            Some((_, plans)) => plans.push(plan),
            None => by_model.push((first.model, vec![plan])),
        }
    }
    let by_model: Vec<Estimate> = by_model.iter().map(|(model, plans)| estimate(model, plans, counting)).collect();
    let total = |cost: fn(&Estimate) -> Option<Usd>| by_model.iter().map(cost).sum::<Option<Usd>>();
    Ok(BatchEstimate {
        items: items.len(),
        kept: items.len() - to_ask.len() - repeats.len(),
        to_ask: to_ask.len(),
        usd: total(|estimate| estimate.usd),
        most_usd: total(|estimate| estimate.most_usd),
        by_model,
    })
}

/// Ask every item not kept yet, as the module says, and keep each answer as it arrives. Progress (items done of
/// all, dollars spent so far from `budget`) and whether to stop go through `tracker`. One outcome per item, in
/// the items' order. Fails only when the library cannot be read to see what is kept.
pub fn ask_all<T>(
    asking: Asking<'_>,
    items: &[Item<'_>],
    budget: &Budget,
    tracker: &Tracker<'_>,
) -> Result<Vec<Outcome<T>>, LibraryError>
where
    T: DeserializeOwned + Serialize + Send + Clone,
{
    let Sorted { to_ask, kept, repeats } = sorted::<T>(asking.library, items)?;
    let of = u64::try_from(items.len()).unwrap_or(u64::MAX);
    // counted under a lock, so that the progress told never goes backwards
    let asked = u64::try_from(to_ask.len() + repeats.len()).unwrap_or(of);
    let done = Mutex::new(of - asked);
    let report = |done: u64| tracker.progress(Progress { done, of, usd: budget.spent().1 });
    report(*locked(&done));

    let paced = Paced { inner: asking.transport, gate: Mutex::new(Gate::default()) };
    let client = Client::new(&paced, asking.endpoint, asking.recorder);
    // the first reason to stop is the one told
    let stop: Mutex<Option<Stopped>> = Mutex::new(None);
    let stopped = || {
        let mut stop = locked(&stop);
        if stop.is_none() && tracker.cancelled() {
            *stop = Some(Stopped::Cancelled);
        }
        stop.clone()
    };
    let lanes = lanes(items, &to_ask);
    let next = AtomicUsize::new(0);
    let flight = Flight::default();
    let answered: Mutex<Vec<(usize, Outcome<T>)>> = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..asking.in_flight.max(1).min(lanes.len()) {
            scope.spawn(|| {
                while stopped().is_none() {
                    let Some(lane) = lanes.get(next.fetch_add(1, Ordering::SeqCst)) else { return };
                    for &index in lane {
                        if stopped().is_some() {
                            return; // the rest are told as not started
                        }
                        let call = || ask_one(&client, asking.library, &items[index], budget);
                        let Some(outcome) = flight.ask(&stopped, call) else { return };
                        if let Some(why) = halts(&outcome) {
                            locked(&stop).get_or_insert(why);
                        }
                        locked(&answered).push((index, outcome));
                        let mut done = locked(&done);
                        *done += 1;
                        report(*done);
                    }
                }
            });
        }
    });

    let mut outcomes: Vec<Option<Outcome<T>>> = kept.into_iter().map(|kept| kept.map(Outcome::Kept)).collect();
    for (index, outcome) in answered.into_inner().unwrap_or_else(PoisonError::into_inner) {
        outcomes[index] = Some(outcome);
    }
    for (repeat, original) in repeats {
        outcomes[repeat] = outcomes[original].clone();
    }
    let why = stopped().unwrap_or(Stopped::Cancelled);
    Ok(outcomes.into_iter().map(|outcome| outcome.unwrap_or_else(|| Outcome::NotStarted(why.clone()))).collect())
}

/// Why an outcome stops the batch: the API cannot be used, or the budget refused a call with nothing else under
/// way (so nothing can settle below what it held). One model the account cannot use stops only its own items.
fn halts<T>(outcome: &Outcome<T>) -> Option<Stopped> {
    match outcome {
        Outcome::Failed(Failure::Unavailable(Unavailable::UnknownModel(_))) => None,
        Outcome::Failed(Failure::Unavailable(why)) => Some(Stopped::Unavailable(why.clone())),
        Outcome::NotStarted(why) => Some(why.clone()),
        Outcome::Kept(_) | Outcome::Answered(_) | Outcome::Failed(_) => None,
    }
}

/// How many of a batch's calls are under way, and how many have settled against the budget (refused ones do
/// not: they change nothing, so they must not wake one another).
#[derive(Default)]
struct Counts {
    under_way: usize,
    settled: u64,
}

/// The calls of a batch under way. Each holds its worst case against the budget and settles at what it took, so a
/// call refused while others are under way may well fit once one of them ends: it waits for that and asks again.
/// Only a call refused with nothing else under way stops the batch.
#[derive(Default)]
struct Flight {
    counts: Mutex<Counts>,
    ended: Condvar,
}

impl Flight {
    /// The outcome of `call`; None when the batch was stopped while it waited to be asked again.
    fn ask<T>(&self, stopped: &dyn Fn() -> Option<Stopped>, call: impl Fn() -> Outcome<T>) -> Option<Outcome<T>> {
        loop {
            let before = {
                let mut counts = locked(&self.counts);
                counts.under_way += 1;
                counts.settled
            };
            let outcome = call();
            let refused = matches!(outcome, Outcome::NotStarted(Stopped::OverBudget(_)));
            let mut counts = locked(&self.counts);
            counts.under_way -= 1;
            if !refused {
                counts.settled += 1;
            }
            self.ended.notify_all();
            // refused with nothing under way and nothing settled since: no room will come
            if !refused || (counts.under_way == 0 && counts.settled == before) {
                return Some(outcome);
            }
            while counts.under_way > 0 && counts.settled == before {
                counts = self.ended.wait(counts).unwrap_or_else(PoisonError::into_inner);
            }
            drop(counts);
            if stopped().is_some() {
                return None;
            }
        }
    }
}

fn ask_one<T: DeserializeOwned + Serialize>(
    client: &Client<'_>,
    library: &Library,
    item: &Item<'_>,
    budget: &Budget,
) -> Outcome<T> {
    match catch_unwind(AssertUnwindSafe(|| client.ask_json::<T>(&item.ask, budget))) {
        Ok(Ok(answered)) => {
            if let Err(error) = library.keep(&item.slot, &answered.model, item.prompt, &answered.answer) {
                // the answer is still handed back; it is asked for again next time
                eprintln!("library: an answer for {} could not be kept: {error}", item.slot.feature.name());
            }
            Outcome::Answered(answered)
        }
        Ok(Err(AskError::NoAnswer(why))) => Outcome::Failed(Failure::NoAnswer(why)),
        Ok(Err(AskError::Unavailable(why))) => Outcome::Failed(Failure::Unavailable(why)),
        Ok(Err(AskError::OverBudget(why))) => Outcome::NotStarted(Stopped::OverBudget(why)),
        Err(_) => Outcome::Failed(Failure::Panicked),
    }
}

/// When the requests of a batch may go: held back by a wait one of them was told to make, then let go
/// [`SPACING`] apart.
#[derive(Debug, Default)]
struct Gate {
    until: Option<Instant>,
    /// When the last request held back was let go.
    last: Option<Instant>,
}

impl Gate {
    fn pause(&mut self, now: Instant, wait: Duration) {
        let until = now + wait;
        self.until = Some(self.until.map_or(until, |before| before.max(until)));
    }

    /// How long a request about to go at `now` must wait first, if at all.
    fn turn(&mut self, now: Instant) -> Option<Duration> {
        let until = self.until.filter(|until| *until > now)?;
        let slot = self.last.map_or(until, |last| until.max(last + SPACING));
        self.last = Some(slot);
        Some(slot.saturating_duration_since(now))
    }
}

/// The caller's transport, with the batch's [`Gate`] in front: a wait the client makes after a rate limit (or an
/// overloaded server) holds back every thread's next request, not only its own.
struct Paced<'a> {
    inner: &'a dyn Transport,
    gate: Mutex<Gate>,
}

impl Transport for Paced<'_> {
    fn post(
        &self,
        url: &str,
        key: &ApiKey,
        headers: &[(&str, &str)],
        body: &[u8],
        patience: Duration,
    ) -> Result<Reply, TransportError> {
        let wait = locked(&self.gate).turn(Instant::now());
        if let Some(wait) = wait {
            self.inner.wait(wait);
        }
        self.inner.post(url, key, headers, body, patience)
    }

    fn wait(&self, duration: Duration) {
        // waited at the next request, in turn
        locked(&self.gate).pause(Instant::now(), duration);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::super::language::fake::{error, message, ok};
    use super::super::language::{Effort, Form, KEY_VARIABLE, Limit};
    use super::*;
    use crate::jobs::Cancel;
    use crate::library::{AnswerKey, Feature, Subject};
    use crate::usage::Call;

    /// The Claude API as these tests need it: it answers each question with the question itself, counts how many
    /// requests are under way at once, and fails for the questions that say so. Never the network.
    #[derive(Default)]
    struct Echo {
        under_way: AtomicUsize,
        most: AtomicUsize,
        posts: AtomicUsize,
        slow: Option<Duration>,
        /// Asked to stop after this many requests.
        cancel_after: Option<(usize, Cancel)>,
        /// Tokens each answer says it took (input, output).
        took: (u64, u64),
        waited: Mutex<Vec<Duration>>,
        rate_limited_once: AtomicBool,
    }

    impl Transport for Echo {
        fn post(
            &self,
            _url: &str,
            _key: &ApiKey,
            _headers: &[(&str, &str)],
            body: &[u8],
            _patience: Duration,
        ) -> Result<Reply, TransportError> {
            let now = self.under_way.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(now, Ordering::SeqCst);
            let posted = self.posts.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some((after, cancel)) = &self.cancel_after {
                if posted >= *after {
                    cancel.cancel();
                }
            }
            if let Some(slow) = self.slow {
                thread::sleep(slow);
            }
            let body: Value = serde_json::from_slice(body).unwrap();
            let question =
                body["messages"][0]["content"].as_array().unwrap().last().unwrap()["text"].as_str().unwrap().to_owned();
            self.under_way.fetch_sub(1, Ordering::SeqCst);
            let usage = json!({"input_tokens": self.took.0, "output_tokens": self.took.1});
            Ok(match question.as_str() {
                "fail" => error(500, "api_error", "Internal server error"),
                "unavailable" => error(401, "authentication_error", "invalid x-api-key"),
                "unknown model" => error(404, "not_found_error", "model: claude-opus-9"),
                "limited" if !self.rate_limited_once.swap(true, Ordering::SeqCst) => {
                    let mut limited = error(429, "rate_limit_error", "slow down");
                    limited.retry_after = Some("7".to_owned());
                    limited
                }
                _ => ok(&message("claude-opus-5-5", &json!({"said": question}).to_string(), "end_turn", usage)),
            })
        }

        fn wait(&self, duration: Duration) {
            self.waited.lock().unwrap().push(duration);
        }
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct Said {
        said: String,
    }

    fn schema() -> Value {
        json!({"type": "object", "additionalProperties": false, "required": ["said"], "properties": {"said": {"type": "string"}}})
    }

    fn endpoint() -> Endpoint {
        Endpoint::from_lookup(|name| (name == KEY_VARIABLE).then(|| "test-key-not-real".to_owned())).unwrap()
    }

    /// One item per question, each about its own paper unless `shared` says otherwise.
    fn items<'a>(questions: &'a [String], shared: &'a [String], schema: &'a Value) -> Vec<Item<'a>> {
        questions
            .iter()
            .zip(shared)
            .map(|(question, shared)| Item {
                slot: Slot {
                    feature: Feature::Answers,
                    key: AnswerKey::new(Feature::Answers, "claude-opus-5-5", 1, &[shared.as_str()], question),
                    subject: Subject::Paper(shared.clone()),
                },
                prompt: 1,
                ask: Ask {
                    purpose: "questions about papers",
                    model: "claude-opus-5-5",
                    system: "Answer from the paper only.",
                    shared: Some(shared),
                    question,
                    form: Form::Json(schema),
                    effort: Effort::Low,
                    max_tokens: 1_000,
                },
                likely_tokens: 300,
            })
            .collect()
    }

    fn numbered(count: usize) -> (Vec<String>, Vec<String>) {
        ((0..count).map(|n| format!("question {n}")).collect(), (0..count).map(|n| format!("paper {n}")).collect())
    }

    fn plenty() -> Budget {
        Budget::new(Limit::Dollars(Usd::from_picodollars(10_000_000_000_000)))
    }

    struct Run {
        progress: Mutex<Vec<Progress>>,
        cancel: Cancel,
        calls: Mutex<Vec<Call>>,
    }

    impl Run {
        fn new() -> Self {
            Run { progress: Mutex::new(Vec::new()), cancel: Cancel::new(), calls: Mutex::new(Vec::new()) }
        }

        fn ask(
            &self,
            library: &Library,
            transport: &Echo,
            items: &[Item<'_>],
            budget: &Budget,
            in_flight: usize,
        ) -> Vec<Outcome<Said>> {
            let log = |_: &str| {};
            let progress = |progress: Progress| self.progress.lock().unwrap().push(progress);
            let tracker = Tracker::new(&log, &progress, &self.cancel);
            let asking = Asking { library, transport, endpoint: endpoint(), recorder: &self.calls, in_flight };
            ask_all(asking, items, budget, &tracker).unwrap()
        }
    }

    fn said(outcome: &Outcome<Said>) -> Option<&str> {
        match outcome {
            Outcome::Answered(answered) => Some(&answered.answer.said),
            Outcome::Kept(kept) => Some(&kept.answer.said),
            Outcome::Failed(_) | Outcome::NotStarted(_) => None,
        }
    }

    fn library() -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        (folder, library)
    }

    #[test]
    fn every_item_is_answered_in_its_own_place_and_never_more_than_the_cap_at_once() {
        let (_folder, library) = library();
        let (questions, papers) = numbered(24);
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        let echo = Echo { slow: Some(Duration::from_millis(10)), took: (40, 20), ..Echo::default() };
        let run = Run::new();
        let outcomes = run.ask(&library, &echo, &items, &plenty(), 3);
        let answers: Vec<Option<&str>> = outcomes.iter().map(said).collect();
        assert_eq!(answers, questions.iter().map(|question| Some(question.as_str())).collect::<Vec<_>>());
        assert!(echo.most.load(Ordering::SeqCst) <= 3, "{} at once", echo.most.load(Ordering::SeqCst));
        assert!(echo.most.load(Ordering::SeqCst) >= 2, "the calls did run side by side");
        assert_eq!(run.calls.lock().unwrap().len(), 24, "every call is recorded");
        let progress = run.progress.lock().unwrap();
        assert_eq!((progress[0].done, progress.last().unwrap().done, progress[0].of), (0, 24, 24));
        assert!(progress.windows(2).all(|pair| pair[0].done <= pair[1].done), "progress never goes back");
        assert!(progress.last().unwrap().usd > Usd::ZERO);
    }

    #[test]
    fn a_second_run_asks_nothing_for_what_is_kept_and_only_again_for_what_failed() {
        let (_folder, library) = library();
        let (mut questions, papers) = numbered(6);
        questions[2] = "fail".to_owned();
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        let echo = Echo::default();
        let first = Run::new().ask(&library, &echo, &items, &plenty(), 2);
        assert_eq!(first[2], Outcome::Failed(Failure::NoAnswer(NoAnswer::ServerError(500))));
        assert_eq!(first.iter().filter(|outcome| matches!(outcome, Outcome::Answered(_))).count(), 5);
        let posted = echo.posts.load(Ordering::SeqCst);

        let estimate = estimate_batch::<Said>(&library, &items, Counting::ASSUMED).unwrap();
        assert_eq!((estimate.items, estimate.kept, estimate.to_ask), (6, 5, 1));
        let again = Run::new().ask(&library, &echo, &items, &plenty(), 2);
        assert!(again.iter().enumerate().all(|(index, outcome)| (index == 2) != matches!(outcome, Outcome::Kept(_))));
        assert_eq!(echo.posts.load(Ordering::SeqCst) - posted, 3, "only the failed one, tried and tried again twice");
        assert_eq!(said(&again[4]), Some("question 4"));
    }

    #[test]
    fn the_budget_is_never_passed_even_by_calls_racing_for_it() {
        let (_folder, library) = library();
        let (questions, papers) = numbered(16);
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        // each call holds about 1,020 tokens at worst and takes 1,000: three fit in 3,500, a fourth never does
        let echo = Echo { slow: Some(Duration::from_millis(5)), took: (600, 400), ..Echo::default() };
        let budget = Budget::new(Limit::Tokens(3_500));
        let outcomes = Run::new().ask(&library, &echo, &items, &budget, 8);
        assert!(budget.spent().0 <= 3_500, "spent {}", budget.spent().0);
        let answered = outcomes.iter().filter(|outcome| matches!(outcome, Outcome::Answered(_))).count();
        assert_eq!(answered, 3);
        assert!(outcomes.iter().all(|outcome| matches!(
            outcome,
            Outcome::Answered(_) | Outcome::NotStarted(Stopped::OverBudget(OverBudget::Tokens { .. }))
        )));
        assert_eq!(echo.posts.load(Ordering::SeqCst), 3, "nothing was sent that the budget refused");
    }

    #[test]
    fn a_budget_held_by_calls_under_way_is_waited_for_not_given_up_on() {
        let (_folder, library) = library();
        let (questions, papers) = numbered(16);
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        // each call holds about 1,020 tokens at worst but takes 20: four are held at once in 5,000, all fit in time
        let echo = Echo { slow: Some(Duration::from_millis(5)), took: (10, 10), ..Echo::default() };
        let budget = Budget::new(Limit::Tokens(5_000));
        let outcomes = Run::new().ask(&library, &echo, &items, &budget, 8);
        assert!(outcomes.iter().all(|outcome| matches!(outcome, Outcome::Answered(_))), "{outcomes:?}");
        assert_eq!(budget.spent().0, 16 * 20);
    }

    #[test]
    fn the_same_question_twice_in_a_batch_is_paid_for_once() {
        let (_folder, library) = library();
        let questions = vec!["question".to_owned(); 3];
        let papers = vec!["paper".to_owned(); 3];
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        assert_eq!(estimate_batch::<Said>(&library, &items, Counting::ASSUMED).unwrap().by_model[0].calls, 1);
        let echo = Echo::default();
        let outcomes = Run::new().ask(&library, &echo, &items, &plenty(), 2);
        assert_eq!(echo.posts.load(Ordering::SeqCst), 1);
        assert!(outcomes.iter().all(|outcome| said(outcome) == Some("question")));
    }

    #[test]
    fn a_cancelled_batch_starts_nothing_new_and_keeps_what_was_answered() {
        let (_folder, library) = library();
        let (questions, papers) = numbered(40);
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        let run = Run::new();
        let echo = Echo {
            slow: Some(Duration::from_millis(2)),
            cancel_after: Some((5, run.cancel.clone())),
            ..Echo::default()
        };
        let outcomes = run.ask(&library, &echo, &items, &plenty(), 2);
        let posted = echo.posts.load(Ordering::SeqCst);
        assert!(posted <= 5 + 2, "{posted} requests: at most those under way when it was asked to stop");
        let answered: Vec<usize> = (0..40).filter(|index| matches!(outcomes[*index], Outcome::Answered(_))).collect();
        assert_eq!(answered.len(), posted);
        assert!(outcomes.iter().filter(|outcome| **outcome == Outcome::NotStarted(Stopped::Cancelled)).count() >= 33);
        for index in answered {
            let kept = library.kept::<Said>(Feature::Answers, &items[index].slot.key).unwrap();
            assert_eq!(kept.unwrap().answer.said, questions[index], "kept as it arrived");
        }
    }

    #[test]
    fn an_api_that_cannot_be_used_stops_the_batch_and_says_why() {
        let (_folder, library) = library();
        let (mut questions, papers) = numbered(5);
        questions[1] = "unavailable".to_owned();
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        let echo = Echo::default();
        let outcomes = Run::new().ask(&library, &echo, &items, &plenty(), 1);
        assert_eq!(said(&outcomes[0]), Some("question 0"));
        assert_eq!(outcomes[1], Outcome::Failed(Failure::Unavailable(Unavailable::KeyRefused)));
        for outcome in &outcomes[2..] {
            assert_eq!(*outcome, Outcome::NotStarted(Stopped::Unavailable(Unavailable::KeyRefused)));
        }
        assert_eq!(echo.posts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_model_the_account_cannot_use_fails_only_its_own_items() {
        let (_folder, library) = library();
        let (mut questions, papers) = numbered(4);
        questions[1] = "unknown model".to_owned();
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        let outcomes = Run::new().ask(&library, &Echo::default(), &items, &plenty(), 1);
        let unknown = Unavailable::UnknownModel("claude-opus-5-5".to_owned());
        assert_eq!(outcomes[1], Outcome::Failed(Failure::Unavailable(unknown)));
        assert_eq!([0, 2, 3].map(|index| said(&outcomes[index]).is_some()), [true; 3]);
    }

    #[test]
    fn questions_over_one_papers_text_are_asked_in_turn_and_estimated_as_cache_reads_without_a_call() {
        let (_folder, library) = library();
        let questions: Vec<String> = (0..6).map(|n| format!("question {n}")).collect();
        let papers: Vec<String> =
            ["a", "a", "a", "b", "b", "c"].iter().map(|paper| format!("{paper}{}", "x".repeat(3_000))).collect();
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        let estimate = estimate_batch::<Said>(&library, &items, Counting::ASSUMED).unwrap();
        assert_eq!((estimate.to_ask, estimate.by_model.len(), estimate.by_model[0].calls), (6, 1, 6));
        let tokens = &estimate.by_model[0].tokens;
        // a: written once, read twice; b: written once, read once; c: written once
        assert!(tokens.cache_write_5m > 0 && tokens.cache_read == tokens.cache_write_5m, "{tokens:?}");
        assert!(estimate.usd.unwrap() < estimate.most_usd.unwrap());
        assert_eq!(
            lanes(&items, &[0, 1, 2, 3, 4, 5]),
            [vec![0, 1, 2], vec![3, 4], vec![5]],
            "one paper's questions on one thread, in order"
        );
        let unpriced: Vec<Item<'_>> =
            items.into_iter().map(|item| Item { ask: Ask { model: "claude-unknown-1", ..item.ask }, ..item }).collect();
        assert_eq!(estimate_batch::<Said>(&library, &unpriced, Counting::ASSUMED).unwrap().usd, None);
    }

    #[test]
    fn a_rate_limit_holds_every_request_back_and_lets_them_go_one_by_one() {
        let mut gate = Gate::default();
        let now = Instant::now();
        assert_eq!(gate.turn(now), None, "nothing to wait for");
        gate.pause(now, Duration::from_secs(7));
        gate.pause(now, Duration::from_secs(2)); // a shorter wait does not cut a longer one short
        let turns: Vec<Duration> = (0..3).map(|_| gate.turn(now).unwrap()).collect();
        assert_eq!(turns, [Duration::from_secs(7), Duration::from_millis(7_250), Duration::from_millis(7_500)]);
        assert_eq!(gate.turn(now + Duration::from_secs(8)), None, "once it has passed, requests go at once");

        // through the client: the wait it is told is made before the next request, by the batch's gate
        let (_folder, library) = library();
        let questions = vec!["limited".to_owned()];
        let papers = vec!["paper".to_owned()];
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        let echo = Echo::default();
        let outcomes = Run::new().ask(&library, &echo, &items, &plenty(), 1);
        assert_eq!(said(&outcomes[0]), Some("limited"));
        let waited = echo.waited.lock().unwrap().clone();
        assert_eq!(waited.len(), 1);
        assert!(waited[0] > Duration::from_millis(6_900) && waited[0] <= Duration::from_secs(7), "{waited:?}");
    }
}
