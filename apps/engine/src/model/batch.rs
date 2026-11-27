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
//!    [`Outcome::NotStarted`] with why. A stop cuts short a rate-limit wait already begun (within [`SPACING`]),
//!    and the request that waited is not sent. Items with the same slot are asked once and all given that outcome
//!    (and counted as done together).
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

use super::chosen::Chosen;
use super::language::{
    Answered, ApiKey, Ask, AskError, Budget, Client, Counting, Endpoint, Estimate, KeyHeader, NoAnswer, OverBudget,
    Planned, Recorder, Reply, Transport, TransportError, Unavailable,
};
use crate::jobs::{Progress, Tracker};
use crate::library::asking::Intent;
use crate::library::{AnswerKey, Feature, Kept, Library, LibraryError, Slot};
use crate::usage::Call;

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

/// What a batch would cost, before anything is asked: what the interface shows as "about N dollars" (`BatchCost`
/// in the contracts: the items, and what asking those not kept would cost, flattened beside them).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BatchEstimate {
    pub items: usize,
    /// Kept already: they cost nothing.
    pub kept: usize,
    pub to_ask: usize,
    /// What asking the rest would cost, at the chosen provider's prices ([`Estimate::counting`] says how tokens
    /// were reckoned: it is an estimate).
    #[serde(flatten)]
    pub cost: Estimate,
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
    /// What was about to be asked could not be written down first ([`crate::library::asking`]): nothing is sent
    /// whose cost could then be lost unseen.
    #[error("the library could not be written, so nothing more was asked")]
    Unrecorded,
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

/// What asking the items not kept yet would cost, asked of `chosen` (whose model every item asks), at its
/// provider's prices and as it caches and answers in JSON ([`Chosen::estimate`]), prompt tokens reckoned from
/// characters ([`Counting::ASSUMED`]). Nothing is sent.
pub fn estimate_batch<T: DeserializeOwned>(
    library: &Library,
    items: &[Item<'_>],
    chosen: &Chosen,
) -> Result<BatchEstimate, LibraryError> {
    let Sorted { to_ask, repeats, .. } = sorted::<T>(library, items)?;
    let pricing = chosen.pricing();
    let mut plans: Vec<Planned> = Vec::new();
    for lane in lanes(items, &to_ask) {
        let asks: Vec<&Item<'_>> = lane.iter().map(|index| &items[*index]).collect();
        let first = &asks[0].ask;
        debug_assert_eq!(first.model, chosen.model(), "a batch asks the chosen model");
        plans.push(Planned {
            shared_chars: characters(first.system) + first.shared.map_or(0, characters),
            // as sent there: with the schema in it where the provider cannot be held to one
            question_chars: asks.iter().map(|item| pricing.question_chars(&item.ask)).max().unwrap_or(0),
            questions: u32::try_from(asks.len()).unwrap_or(u32::MAX),
            answer_tokens: asks.iter().map(|item| item.likely_tokens).max().unwrap_or(0),
            max_tokens: asks.iter().map(|item| item.ask.max_tokens).max().unwrap_or(0),
        });
    }
    Ok(BatchEstimate {
        items: items.len(),
        kept: items.len() - to_ask.len() - repeats.len(),
        to_ask: to_ask.len(),
        cost: chosen.estimate(&plans, Counting::ASSUMED),
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
    // an item asked in the place of repeats of it is done for them too
    let mut with_repeats: HashMap<usize, u64> = HashMap::new();
    for (_, original) in &repeats {
        *with_repeats.entry(*original).or_default() += 1;
    }

    let cancelled = || tracker.cancelled();
    let paced = Paced { inner: asking.transport, gate: Mutex::new(Gate::default()), cancelled: &cancelled };
    // the calls each thread's item took are held until its answer is kept with them, in one transaction
    let pending = Pending::default();
    let client = Client::new(&paced, asking.endpoint.clone(), &pending);
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
                        let call = || ask_one(&client, &pending, &asking, &items[index], budget);
                        let Some(outcome) = flight.ask(&stopped, call) else { return };
                        if let Some(why) = halts(&outcome) {
                            locked(&stop).get_or_insert(why);
                        }
                        locked(&answered).push((index, outcome));
                        let mut done = locked(&done);
                        *done += 1 + with_repeats.get(&index).copied().unwrap_or(0);
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

/// The calls made on each thread and not yet written: a thread asks its items one after the other, so what it holds
/// is what its item under way took (every attempt of it).
#[derive(Default)]
struct Pending {
    calls: Mutex<HashMap<thread::ThreadId, Vec<Call>>>,
}

impl Pending {
    /// What this thread's item took, taken out.
    fn take(&self) -> Vec<Call> {
        locked(&self.calls).remove(&thread::current().id()).unwrap_or_default()
    }
}

impl Recorder for Pending {
    fn record(&self, call: Call) {
        locked(&self.calls).entry(thread::current().id()).or_default().push(call);
    }
}

/// What is known of whether a question that gave no answer was billed, from how it ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Billed {
    /// Its calls were paid for and are recorded now.
    Recorded,
    /// Nothing of it was billed: refused, not sent, or answered with an error (which the APIs do not bill).
    No,
    /// It may have been: sent and then lost, or a panic part way. Its intent stays, told as unknown.
    Unknown,
}

/// How a question that gave no answer ended, as far as its cost goes. The answers' paths that are paid for all
/// record a call (`Client::ask` records every reply it was given), so a question with none recorded was not
/// billed, unless the request was sent and then lost, or the asking panicked: those are unknown.
fn billed(asked: &std::thread::Result<Result<Answered<impl Sized>, AskError>>, calls: &[Call]) -> Billed {
    match asked {
        Err(_) | Ok(Err(AskError::NoAnswer(NoAnswer::Transport(TransportError::Lost)))) => Billed::Unknown,
        _ if !calls.is_empty() => Billed::Recorded,
        _ => Billed::No,
    }
}

fn ask_one<T: DeserializeOwned + Serialize>(
    client: &Client<'_>,
    pending: &Pending,
    asking: &Asking<'_>,
    item: &Item<'_>,
    budget: &Budget,
) -> Outcome<T> {
    // written before anything is sent: a crash once the provider has billed and before the answer is kept leaves it
    let (most_tokens, most_usd) = client.most(&item.ask);
    let intent = Intent {
        slot: &item.slot,
        provider: asking.endpoint.provider().id(),
        model: item.ask.model,
        most_tokens,
        most_micros: most_usd.map(|usd| usd.picodollars().div_ceil(1_000_000)),
    };
    if let Err(error) = asking.library.intend(&intent) {
        eprintln!("library: what was about to be asked could not be written, so it was not asked: {error}");
        return Outcome::NotStarted(Stopped::Unrecorded);
    }
    let asked = catch_unwind(AssertUnwindSafe(|| client.ask_json::<T>(&item.ask, budget)));
    let calls = pending.take();
    let billed = billed(&asked, &calls);
    let outcome = match asked {
        Ok(Ok(answered)) => {
            // the answer and what it cost, kept together (and the intent cleared with them): never a call paid for
            // whose answer is lost
            let kept = match serde_json::to_string(&answered.answer) {
                Ok(content) => {
                    asking.recorder.keep_paid(asking.library, &item.slot, &answered.model, item.prompt, &content, calls)
                }
                // nothing to keep, but the calls were paid for
                Err(error) => asking.recorder.record_paid(asking.library, &item.slot, calls).and(Err(error.into())),
            };
            if let Err(error) = kept {
                // the answer is still handed back; it is asked for again next time
                eprintln!("library: an answer for {} could not be kept: {error}", item.slot.feature.name());
            }
            return Outcome::Answered(answered);
        }
        // stopped while it waited for its turn: nothing more of it was sent
        Ok(Err(AskError::NoAnswer(NoAnswer::Transport(TransportError::Cancelled)))) => {
            Outcome::NotStarted(Stopped::Cancelled)
        }
        Ok(Err(AskError::NoAnswer(why))) => Outcome::Failed(Failure::NoAnswer(why)),
        Ok(Err(AskError::Unavailable(why))) => Outcome::Failed(Failure::Unavailable(why)),
        Ok(Err(AskError::OverBudget(why))) => Outcome::NotStarted(Stopped::OverBudget(why)),
        Err(_) => Outcome::Failed(Failure::Panicked),
    };
    // no usable answer: what was paid for is recorded all the same, and the intent goes unless it is unknown
    let settled = match billed {
        Billed::Recorded => asking.recorder.record_paid(asking.library, &item.slot, calls),
        Billed::No => asking.library.not_billed(&item.slot).map(drop),
        Billed::Unknown => {
            for call in calls {
                asking.recorder.record(call);
            }
            Ok(())
        }
    };
    if let Err(error) = settled {
        eprintln!("library: what a question for {} cost could not be settled: {error}", item.slot.feature.name());
    }
    outcome
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
///
/// Every request goes through [`Transport::post_keyed`], which must reach the caller's transport as it is, with the
/// header the key is sent in: left to its default, it would become a `post`, whose key goes as `x-api-key`, the
/// Claude API's header, which no other provider reads (OpenAI's would answer that no key was sent).
struct Paced<'a> {
    inner: &'a dyn Transport,
    gate: Mutex<Gate>,
    /// Whether the batch was asked to stop: a wait for a turn (up to a minute and a half after a rate limit) is cut
    /// short by it, and the request is then not sent.
    cancelled: &'a (dyn Fn() -> bool + Sync),
}

impl Paced<'_> {
    /// Wait for this request's turn, when the batch is held back, in steps of [`SPACING`] so that a stop is
    /// heard within one.
    fn turn(&self) -> Result<(), TransportError> {
        let mut left = locked(&self.gate).turn(Instant::now()).unwrap_or(Duration::ZERO);
        while !left.is_zero() {
            if (self.cancelled)() {
                return Err(TransportError::Cancelled);
            }
            let step = left.min(SPACING);
            self.inner.wait(step);
            left -= step;
        }
        Ok(())
    }
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
        self.turn()?;
        self.inner.post(url, key, headers, body, patience)
    }

    fn post_keyed(
        &self,
        url: &str,
        key: &ApiKey,
        sent_as: KeyHeader,
        headers: &[(&str, &str)],
        body: &[u8],
        patience: Duration,
    ) -> Result<Reply, TransportError> {
        self.turn()?;
        self.inner.post_keyed(url, key, sent_as, headers, body, patience)
    }

    fn get(&self, url: &str, key: &ApiKey, sent_as: KeyHeader, patience: Duration) -> Result<Reply, TransportError> {
        self.turn()?;
        self.inner.get(url, key, sent_as, patience)
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

    use super::super::language::fake::{self, Fake, error, message, ok, response};
    use super::super::language::{Effort, Form, KEY_VARIABLE, Limit, Provider};
    use super::*;
    use crate::jobs::Cancel;
    use crate::library::{AnswerKey, Feature, Subject};
    use crate::usage::{Call, Usd};

    /// The Claude API's model these items ask, as a feature would have it chosen.
    fn chosen() -> Chosen {
        Chosen::of(Provider::Anthropic, "claude-opus-5-5", Effort::Low, Ok(endpoint()))
    }

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
                "panic" => panic!("the transport broke"),
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
                    key: AnswerKey::new(
                        Feature::Answers,
                        Provider::Anthropic,
                        "claude-opus-5-5",
                        1,
                        &[shared.as_str()],
                        question,
                    ),
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

        let estimate = estimate_batch::<Said>(&library, &items, &chosen()).unwrap();
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
        // each call holds about 2,030 tokens at worst (the model asked, then a fallback) and takes 2,000: three fit
        // in 7,000, a fourth never does
        let echo = Echo { slow: Some(Duration::from_millis(5)), took: (1_000, 1_000), ..Echo::default() };
        let budget = Budget::new(Limit::Tokens(7_000));
        let outcomes = Run::new().ask(&library, &echo, &items, &budget, 8);
        assert!(budget.spent().0 <= 7_000, "spent {}", budget.spent().0);
        assert_eq!(budget.held(), (0, Usd::ZERO), "nothing is held once the batch has ended");
        let answered = outcomes.iter().filter(|outcome| matches!(outcome, Outcome::Answered(_))).count();
        assert_eq!(answered, 3);
        assert!(outcomes.iter().all(|outcome| matches!(
            outcome,
            Outcome::Answered(_) | Outcome::NotStarted(Stopped::OverBudget(OverBudget::Tokens { .. }))
        )));
        assert_eq!(echo.posts.load(Ordering::SeqCst), 3, "nothing was sent that the budget refused");
    }

    #[test]
    fn a_panicking_transport_gives_back_what_its_call_held_and_the_rest_of_the_batch_runs_within_the_budget() {
        let (_folder, library) = library();
        let (mut questions, papers) = numbered(5);
        questions[0] = "panic".to_owned();
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        // one call's worst case (about 2,030 tokens) fits, two do not: had the panicking call kept what it held,
        // every call after it would be refused
        let echo = Echo { took: (10, 10), ..Echo::default() };
        let budget = Budget::new(Limit::Tokens(3_000));
        let outcomes = Run::new().ask(&library, &echo, &items, &budget, 1);
        assert_eq!(outcomes[0], Outcome::Failed(Failure::Panicked));
        assert!(outcomes[1..].iter().all(|outcome| matches!(outcome, Outcome::Answered(_))), "{outcomes:?}");
        assert_eq!(budget.held(), (0, Usd::ZERO), "nothing is left held");
        assert_eq!(budget.spent().0, 4 * 20);
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
        assert_eq!(estimate_batch::<Said>(&library, &items, &chosen()).unwrap().cost.calls, 1);
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
        let estimate = estimate_batch::<Said>(&library, &items, &chosen()).unwrap();
        assert_eq!((estimate.to_ask, estimate.cost.calls), (6, 6));
        let tokens = &estimate.cost.tokens;
        // a: written once, read twice; b: written once, read once; c: written once
        assert!(tokens.cache_write_5m > 0 && tokens.cache_read == tokens.cache_write_5m, "{tokens:?}");
        assert!(estimate.cost.usd.unwrap() < estimate.cost.most_usd.unwrap());
        assert_eq!(
            lanes(&items, &[0, 1, 2, 3, 4, 5]),
            [vec![0, 1, 2], vec![3, 4], vec![5]],
            "one paper's questions on one thread, in order"
        );
        let unpriced: Vec<Item<'_>> =
            items.into_iter().map(|item| Item { ask: Ask { model: "claude-unknown-1", ..item.ask }, ..item }).collect();
        let unknown = Chosen::of(Provider::Anthropic, "claude-unknown-1", Effort::Low, Ok(endpoint()));
        let estimate = estimate_batch::<Said>(&library, &unpriced, &unknown).unwrap();
        assert_eq!((estimate.cost.usd, estimate.cost.most_usd), (None, None));
        assert!(estimate.cost.most_tokens > estimate.cost.tokens.all(), "the worst case is still told, in tokens");
    }

    #[test]
    fn a_batch_is_estimated_at_the_chosen_providers_prices_and_as_it_caches() {
        let (_folder, library) = library();
        let questions: Vec<String> = (0..3).map(|n| format!("question {n}")).collect();
        let papers = vec!["a".repeat(3_000); 3];
        let schema = schema();
        let at = |provider: Provider, model: &'static str| {
            let items: Vec<Item<'_>> = items(&questions, &papers, &schema)
                .into_iter()
                .map(|item| Item { ask: Ask { model, ..item.ask }, ..item })
                .collect();
            let chosen = Chosen::of(provider, model, Effort::Low, Ok(fake::endpoint(provider)));
            estimate_batch::<Said>(&library, &items, &chosen).unwrap().cost
        };
        let openai = at(Provider::OpenAi, "gpt-6.2-sol");
        assert_eq!((openai.provider, openai.model.as_str()), (Provider::OpenAi, "gpt-6.2-sol"));
        assert!(openai.usd.is_some(), "priced from OpenAI's prices, not reported as unknown");
        // a server on this computer: no API fee, no caching reported (each question pays for the prefix in full)
        let local = at(Provider::Compatible, "qwen3:32b");
        assert_eq!((local.usd, local.on_this_computer), (None, true));
        assert_eq!(local.tokens.cache_read, 0);
        assert!(local.tokens.input >= 3 * 1_000, "{:?}", local.tokens);
        // a provider that takes no schema: it is written into every question, and counted there
        let prompted = at(Provider::Meta, "muse-spark-1.3");
        let held = at(Provider::Groq, "openai/gpt-oss-120b");
        assert!(prompted.tokens.input > held.tokens.input, "the schema written into the question is counted");
    }

    #[test]
    fn every_answer_is_kept_with_what_it_cost_and_a_failed_call_is_recorded_alone() {
        let (_folder, library) = library();
        let (mut questions, papers) = numbered(4);
        questions[3] = "fail".to_owned();
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        let echo = Echo { took: (40, 20), ..Echo::default() };
        let cancel = Cancel::new();
        let (log, progress) = (|_: &str| {}, |_: Progress| {});
        let tracker = Tracker::new(&log, &progress, &cancel);
        let asking =
            Asking { library: &library, transport: &echo, endpoint: endpoint(), recorder: &library, in_flight: 2 };
        let outcomes = ask_all::<Said>(asking, &items, &plenty(), &tracker).unwrap();
        assert_eq!(outcomes.iter().filter(|outcome| matches!(outcome, Outcome::Answered(_))).count(), 3);
        for item in &items[..3] {
            assert!(library.kept::<Said>(Feature::Answers, &item.slot.key).unwrap().is_some());
        }
        // three answers and their calls; the failing one was tried three times, each answered with an error the
        // API does not bill: nothing of it reaches the ledger as tokens
        let spending = library.spending().unwrap();
        assert_eq!(spending.total.calls, 3, "one row per answered call, written with its answer");
    }

    #[test]
    fn an_answer_that_cannot_be_written_down_still_has_its_call_recorded() {
        /// Read from the answer like any other, but refuses to be written back as JSON.
        #[derive(Clone, Debug, PartialEq, Deserialize)]
        struct Unwritable {
            #[allow(dead_code)]
            said: String,
        }
        impl Serialize for Unwritable {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("not this one"))
            }
        }
        let (_folder, library) = library();
        let (questions, papers) = numbered(1);
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        let run = Run::new();
        let (log, progress) = (|_: &str| {}, |_: Progress| {});
        let tracker = Tracker::new(&log, &progress, &run.cancel);
        let asking = Asking {
            library: &library,
            transport: &Echo { took: (40, 20), ..Echo::default() },
            endpoint: endpoint(),
            recorder: &run.calls,
            in_flight: 1,
        };
        let outcomes = ask_all::<Unwritable>(asking, &items, &plenty(), &tracker).unwrap();
        assert!(matches!(outcomes[0], Outcome::Answered(_)));
        assert_eq!(run.calls.lock().unwrap().len(), 1, "paid for, so recorded");
    }

    #[test]
    fn a_batch_to_another_provider_sends_the_key_as_that_provider_reads_it() {
        let (_folder, library) = library();
        let (questions, papers) = numbered(1);
        let schema = schema();
        let items: Vec<Item<'_>> = items(&questions, &papers, &schema)
            .into_iter()
            .map(|item| Item { ask: Ask { model: "gpt-6.2-sol", ..item.ask }, ..item })
            .collect();
        let answer = json!({"said": "question 0"}).to_string();
        let fake =
            Fake::new(vec![Ok(ok(&response("gpt-6.2-sol", &answer, json!({"input_tokens": 9, "output_tokens": 3}))))]);
        let run = Run::new();
        let log = |_: &str| {};
        let progress = |_: Progress| {};
        let tracker = Tracker::new(&log, &progress, &run.cancel);
        let asking = Asking {
            library: &library,
            transport: &fake,
            endpoint: fake::endpoint(Provider::OpenAi),
            recorder: &run.calls,
            in_flight: 1,
        };
        let outcomes = ask_all::<Said>(asking, &items, &plenty(), &tracker).unwrap();
        assert!(matches!(outcomes[0], Outcome::Answered(_)), "{outcomes:?}");
        let sent = fake.sent();
        assert_eq!((sent[0].url.as_str(), sent[0].key_as), ("https://api.openai.com/v1/responses", KeyHeader::Bearer));
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
        assert!(waited.iter().all(|step| *step <= SPACING), "waited in steps a stop can cut short: {waited:?}");
        let all: Duration = waited.iter().sum();
        assert!(all > Duration::from_millis(6_900) && all <= Duration::from_secs(7), "{all:?}");
    }

    #[test]
    fn a_stop_cuts_a_rate_limit_wait_short_and_the_request_that_waited_is_not_sent() {
        let (_folder, library) = library();
        let questions = vec!["limited".to_owned(), "question 1".to_owned()];
        let papers = vec!["paper".to_owned(); 2];
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        let run = Run::new();
        // the stop is asked for while the client waits out the rate limit: Echo's wait does not sleep, so it is
        // asked for at the first step of the wait
        struct Stopping<'a> {
            echo: Echo,
            cancel: &'a Cancel,
        }
        impl Transport for Stopping<'_> {
            fn post(
                &self,
                url: &str,
                key: &ApiKey,
                headers: &[(&str, &str)],
                body: &[u8],
                patience: Duration,
            ) -> Result<Reply, TransportError> {
                self.echo.post(url, key, headers, body, patience)
            }
            fn wait(&self, duration: Duration) {
                self.echo.wait(duration);
                self.cancel.cancel();
            }
        }
        let stopping = Stopping { echo: Echo::default(), cancel: &run.cancel };
        let log = |_: &str| {};
        let progress = |_: Progress| {};
        let tracker = Tracker::new(&log, &progress, &run.cancel);
        let asking = Asking {
            library: &library,
            transport: &stopping,
            endpoint: endpoint(),
            recorder: &run.calls,
            in_flight: 1,
        };
        let outcomes = ask_all::<Said>(asking, &items, &plenty(), &tracker).unwrap();
        assert_eq!(outcomes, [Outcome::NotStarted(Stopped::Cancelled), Outcome::NotStarted(Stopped::Cancelled)]);
        assert_eq!(stopping.echo.posts.load(Ordering::SeqCst), 1, "only the request that was limited was sent");
        assert_eq!(stopping.echo.waited.lock().unwrap().len(), 1, "one step of the seven seconds, then stopped");
    }

    #[test]
    fn the_same_question_twice_counts_both_as_done_when_it_is_answered() {
        let (_folder, library) = library();
        let questions = vec!["question".to_owned(), "question".to_owned(), "other".to_owned()];
        let papers = vec!["paper".to_owned(); 3];
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        let run = Run::new();
        run.ask(&library, &Echo::default(), &items, &plenty(), 1);
        let progress = run.progress.lock().unwrap();
        assert_eq!(progress.last().map(|progress| (progress.done, progress.of)), Some((3, 3)));
    }

    /// A recorder that dies where a crash would: after the provider answered, before the answer is kept.
    struct Crashing;

    impl Recorder for Crashing {
        fn record(&self, _call: Call) {}

        fn keep_paid(
            &self,
            _library: &Library,
            _slot: &Slot,
            _model: &str,
            _prompt: u32,
            _content: &str,
            _calls: Vec<Call>,
        ) -> Result<(), LibraryError> {
            panic!("the app was closed");
        }
    }

    /// The Claude API answering every question, that looks at the library's intents while each request is under way.
    struct Watching<'a> {
        echo: Echo,
        library: &'a Library,
        seen: Mutex<Vec<i64>>,
    }

    impl Transport for Watching<'_> {
        fn post(
            &self,
            url: &str,
            key: &ApiKey,
            headers: &[(&str, &str)],
            body: &[u8],
            patience: Duration,
        ) -> Result<Reply, TransportError> {
            self.seen.lock().unwrap().push(self.library.unsettled().unwrap().calls);
            self.echo.post(url, key, headers, body, patience)
        }
    }

    fn asking<'a>(library: &'a Library, transport: &'a dyn Transport, recorder: &'a dyn Recorder) -> Asking<'a> {
        Asking { library, transport, endpoint: endpoint(), recorder, in_flight: 1 }
    }

    #[test]
    fn a_crash_after_the_provider_answered_leaves_an_intent_that_is_told_on_reopening_and_replaced_when_asked_again() {
        let folder = tempfile::tempdir().unwrap();
        let (questions, papers) = numbered(1);
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        let cancel = Cancel::new();
        let (log, progress) = (|_: &str| {}, |_: Progress| {});
        {
            let library = Library::open(folder.path()).unwrap();
            let echo = Echo { took: (40, 20), ..Echo::default() };
            let crashed = catch_unwind(AssertUnwindSafe(|| {
                let tracker = Tracker::new(&log, &progress, &cancel);
                ask_all::<Said>(asking(&library, &echo, &Crashing), &items, &plenty(), &tracker)
            }));
            assert!(crashed.is_err(), "the crash came between the answer and keeping it");
            assert_eq!(echo.posts.load(Ordering::SeqCst), 1, "the provider was asked, and billed");
        }
        // the next start: the call is told as asked with its outcome unknown, at most what it held
        let library = Library::open(folder.path()).unwrap();
        let unsettled = library.unsettled().unwrap();
        assert_eq!(unsettled.calls, 1);
        assert!(unsettled.most_micros > 0, "{unsettled:?}");
        assert!(unsettled.said().unwrap().contains("may have been billed"));
        assert!(library.kept::<Said>(Feature::Answers, &items[0].slot.key).unwrap().is_none());
        // asked again when the user starts that work again: the intent is replaced, then settled with the answer
        let watching = Watching { echo: Echo::default(), library: &library, seen: Mutex::new(Vec::new()) };
        let tracker = Tracker::new(&log, &progress, &cancel);
        let outcomes = ask_all::<Said>(asking(&library, &watching, &library), &items, &plenty(), &tracker).unwrap();
        assert!(matches!(outcomes[0], Outcome::Answered(_)));
        assert_eq!(*watching.seen.lock().unwrap(), [1], "replaced while it was asked, not added to");
        assert_eq!(library.unsettled().unwrap().calls, 0);
    }

    #[test]
    fn every_question_asked_has_its_intent_written_before_it_is_sent_and_none_is_left_after_many_threads() {
        let (_folder, library) = library();
        let (mut questions, papers) = numbered(24);
        questions[5] = "fail".to_owned();
        questions[9] = "unavailable".to_owned();
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        let watching = Watching {
            echo: Echo { slow: Some(Duration::from_millis(2)), ..Echo::default() },
            library: &library,
            seen: Mutex::new(Vec::new()),
        };
        let cancel = Cancel::new();
        let (log, progress) = (|_: &str| {}, |_: Progress| {});
        let tracker = Tracker::new(&log, &progress, &cancel);
        let asking = Asking { in_flight: 8, ..asking(&library, &watching, &library) };
        ask_all::<Said>(asking, &items, &plenty(), &tracker).unwrap();
        assert!(watching.seen.lock().unwrap().iter().all(|under_way| *under_way >= 1), "written before sending");
        assert_eq!(library.unsettled().unwrap().calls, 0, "kept, recorded, or known not billed: none is left");
    }

    #[test]
    fn a_request_lost_after_it_was_sent_is_not_sent_again_and_its_intent_stays_as_unknown() {
        /// Sends nothing for the first question (unreachable), and loses the answer to the second.
        struct Losing(AtomicUsize);
        impl Transport for Losing {
            fn post(
                &self,
                _url: &str,
                _key: &ApiKey,
                _headers: &[(&str, &str)],
                body: &[u8],
                _patience: Duration,
            ) -> Result<Reply, TransportError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                if String::from_utf8_lossy(body).contains("question 0") {
                    Err(TransportError::Unreachable)
                } else {
                    Err(TransportError::Lost)
                }
            }
            fn wait(&self, _duration: Duration) {}
        }
        let (_folder, library) = library();
        let (questions, papers) = numbered(2);
        let schema = schema();
        let items = items(&questions, &papers, &schema);
        let losing = Losing(AtomicUsize::new(0));
        let cancel = Cancel::new();
        let (log, progress) = (|_: &str| {}, |_: Progress| {});
        let tracker = Tracker::new(&log, &progress, &cancel);
        let outcomes = ask_all::<Said>(asking(&library, &losing, &library), &items, &plenty(), &tracker).unwrap();
        assert_eq!(outcomes[1], Outcome::Failed(Failure::NoAnswer(NoAnswer::Transport(TransportError::Lost))));
        assert_eq!(losing.0.load(Ordering::SeqCst), 3 + 1, "the unreachable one tried three times, the lost one once");
        assert_eq!(library.unsettled().unwrap().calls, 1, "only the lost one may have been billed");
    }
}
