//! Many addresses fetched at once, politely.
//!
//! A run reads thousands of PDFs, and every one is a download: time spent waiting on the network, not on a core.
//! So they are fetched by a few threads of their own, never by the computing threads ([`crate::threads`]), which
//! must not sit blocked on a socket; and under rules that keep each host's load to what it asks for:
//!
//! * at most [`Politeness::in_flight`] requests at once in all;
//! * to one host, at most its rule's `at_once` requests at once, and after each of them its `pause` before the
//!   next begins. arXiv asks programs for one request at a time and no more than one every three seconds, which
//!   is kept ([`ARXIV`], for every host of `arxiv.org`); a publisher's site gets two at once ([`PUBLISHER`]);
//! * a host that refuses (429, 503) is left alone for as long as its `Retry-After` asks, else [`FIRST_BACK_OFF`]
//!   doubling with each refusal in a row, an hour at most; the address is asked again after that, up to
//!   [`MOST_ATTEMPTS`] times in all.
//!
//! What came of each address is its own [`Outcome`], given back in the order the addresses were: one that failed,
//! or whose handling panicked, is that address's outcome, and the others are fetched as usual. A cancel
//! ([`Progress::cancel`]) begins no new request; those under way finish.
//!
//! # The schedule
//!
//! Who may fetch what next is one small state behind one lock ([`Schedule`]): for each host, the addresses
//! waiting for it, how many of its requests are under way, and when its next may begin. A thread takes the lock
//! only to choose an address and to hand back what came of it, never while it waits on the network, so the lock
//! is never held for long, and nothing that can panic runs under it (a request runs outside it, inside
//! [`threads::caught`]). Should it be poisoned all the same, it is taken again: every change under it leaves the
//! schedule whole. A schedule lives for one call, so nothing outlives a call to be bricked by it.
//!
//! Two limits of that, for whoever wires it in: the rules hold within one call, so two batches running at once
//! (or a batch beside the arXiv API's own [`super::citations::Pacer`]) each keep them separately, and the engine
//! must run one download batch at a time (its one background job does); and a host is the one the address names,
//! so a publisher's address that redirects to arXiv is paced as the publisher.

use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use url::Url;

use super::{Fetch, FetchError, Limits, Response};
use crate::threads::{self, Outcome, Progress};

/// How a host is asked: requests at once, and the pause after each before its next begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub at_once: usize,
    pub pause: Duration,
}

/// arXiv: one request at a time, three seconds apart, as it asks of programs (the API's own pace,
/// [`super::citations::PAUSE`]).
pub const ARXIV: Rule = Rule { at_once: 1, pause: super::citations::PAUSE };
/// A publisher's site (proceedings): two at a time, as a browser opening two papers would.
pub const PUBLISHER: Rule = Rule { at_once: 2, pause: Duration::ZERO };
/// Requests under way at once, over every host.
pub const IN_FLIGHT: usize = 6;
/// How long a refusal that does not say is waited out; doubled for each refusal in a row from the same host.
pub const FIRST_BACK_OFF: Duration = Duration::from_secs(30);
/// The longest a refusal is taken to ask for.
const LONGEST_WAIT: Duration = Duration::from_secs(3600);
/// Times an address is asked before its refusal is its outcome.
pub const MOST_ATTEMPTS: u32 = 3;
/// A thread waiting for a host looks at least this often whether the batch was cancelled.
const LOOK_FOR_CANCEL: Duration = Duration::from_millis(200);

/// The rules a batch keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Politeness {
    pub in_flight: usize,
    pub arxiv: Rule,
    pub others: Rule,
}

impl Default for Politeness {
    fn default() -> Self {
        Self { in_flight: IN_FLIGHT, arxiv: ARXIV, others: PUBLISHER }
    }
}

impl Politeness {
    fn rule(&self, site: &str) -> Rule {
        if site == "arxiv.org" { self.arxiv } else { self.others }
    }
}

/// The site an address is on, as its rule is kept: the host in lower case without `www.`, and `arxiv.org` for
/// every host of arXiv (`export.arxiv.org` included: it is one service, and asks to be asked as one).
fn site(address: &str) -> String {
    let host = Url::parse(address).ok().and_then(|url| url.host_str().map(str::to_ascii_lowercase)).unwrap_or_default();
    if host == "arxiv.org" || host.ends_with(".arxiv.org") {
        return "arxiv.org".to_owned();
    }
    host.strip_prefix("www.").map_or(host.clone(), str::to_owned)
}

/// One host's part of the schedule.
struct Host {
    rule: Rule,
    /// Addresses waiting for it, by their place in the batch.
    waiting: VecDeque<usize>,
    /// Its requests under way.
    busy: usize,
    /// When its next request may begin.
    next: Instant,
    /// Refusals in a row, for the back-off.
    refusals: u32,
}

/// What a thread is to do next.
enum Next {
    Fetch {
        index: usize,
        site: String,
    },
    /// Nothing can begin now; something may at the latest after this long (or when a request ends).
    Wait(Option<Duration>),
    /// Every address has its outcome, or none is left to begin.
    Finished,
}

/// Who fetches what next: see the module's "The schedule".
struct Schedule<T> {
    hosts: BTreeMap<String, Host>,
    attempts: Vec<u32>,
    outcomes: Vec<Option<Outcome<T>>>,
    /// Addresses that are waiting or under way.
    left: usize,
}

impl<T> Schedule<T> {
    fn new(addresses: &[String], politeness: &Politeness, now: Instant) -> Self {
        let mut hosts: BTreeMap<String, Host> = BTreeMap::new();
        for (index, address) in addresses.iter().enumerate() {
            let site = site(address);
            let rule = politeness.rule(&site);
            hosts
                .entry(site)
                .or_insert_with(|| Host { rule, waiting: VecDeque::new(), busy: 0, next: now, refusals: 0 })
                .waiting
                .push_back(index);
        }
        Self {
            hosts,
            attempts: vec![0; addresses.len()],
            outcomes: (0..addresses.len()).map(|_| None).collect(),
            left: addresses.len(),
        }
    }

    fn next(&mut self, now: Instant, cancelled: bool) -> Next {
        if cancelled {
            // nothing more begins: what waits keeps no outcome, and is told as cancelled at the end
            for host in self.hosts.values_mut() {
                self.left -= host.waiting.len();
                host.waiting.clear();
            }
        }
        if self.left == 0 {
            return Next::Finished;
        }
        let mut soonest: Option<Instant> = None;
        for (site, host) in &mut self.hosts {
            if host.waiting.is_empty() || host.busy >= host.rule.at_once.max(1) {
                continue;
            }
            if host.next <= now {
                if let Some(index) = host.waiting.pop_front() {
                    host.busy += 1;
                    host.next = now + host.rule.pause;
                    return Next::Fetch { index, site: site.clone() };
                }
            }
            soonest = Some(soonest.map_or(host.next, |soonest| soonest.min(host.next)));
        }
        Next::Wait(soonest.map(|at| at.saturating_duration_since(now)))
    }

    /// What came of the address at `index`, fetched from `site`. A refusal puts it back at the head of its host's
    /// queue for after the wait the refusal asked for, until it has been asked [`MOST_ATTEMPTS`] times.
    fn finished(
        &mut self,
        index: usize,
        site: &str,
        outcome: Outcome<T>,
        refusal: Option<Option<Duration>>,
        now: Instant,
        progress: &Progress,
    ) {
        self.attempts[index] += 1;
        let Some(host) = self.hosts.get_mut(site) else { return };
        host.busy = host.busy.saturating_sub(1);
        host.next = host.next.max(now + host.rule.pause);
        let again = matches!(outcome, Outcome::Done(_)) && self.attempts[index] < MOST_ATTEMPTS;
        match refusal {
            Some(asked) => {
                host.refusals += 1;
                let doubled = FIRST_BACK_OFF.saturating_mul(2u32.saturating_pow(host.refusals - 1));
                host.next = host.next.max(now + asked.unwrap_or(doubled).min(LONGEST_WAIT));
                if again {
                    host.waiting.push_front(index);
                    return;
                }
            }
            None => host.refusals = 0,
        }
        self.outcomes[index] = Some(outcome);
        self.left -= 1;
        progress.advance();
    }
}

/// A [`Fetch`] that notes whether the host refused, and how long it asked to be left alone. It is made for one
/// attempt on one thread, and never shared.
struct Watched<'a> {
    fetch: &'a (dyn Fetch + Sync),
    refusal: Cell<Option<Option<Duration>>>,
}

impl Fetch for Watched<'_> {
    fn get(&self, url: &str, limits: Limits) -> Result<Response, FetchError> {
        let answer = self.fetch.get(url, limits);
        // the last answer of the attempt decides: a refusal followed by an answer is not a refusal
        if let Ok(response) = &answer {
            self.refusal.set(matches!(response.status, 429 | 503).then_some(response.retry_after));
        }
        answer
    }
}

fn open<T>(schedule: &Mutex<Schedule<T>>) -> MutexGuard<'_, Schedule<T>> {
    schedule.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `fetch_one` for every one of `addresses`, under `politeness`; one outcome per address, in their order.
///
/// `fetch_one` is given a [`Fetch`] to fetch with and the address, and does whatever fetching the address means
/// (following redirects, checking that a PDF is a PDF: [`super::proceedings::download_pdf`]). Why the answer does
/// not depend on how the threads shared the work: each address's outcome is what `fetch_one` made of that address
/// alone, kept in the address's place.
pub fn fetch_all<T, F>(
    fetch: &(dyn Fetch + Sync),
    addresses: &[String],
    politeness: &Politeness,
    progress: &Progress,
    fetch_one: F,
) -> Vec<Outcome<T>>
where
    T: Send,
    F: Fn(&dyn Fetch, &str) -> T + Sync,
{
    progress.expect(addresses.len());
    let schedule = Mutex::new(Schedule::new(addresses, politeness, Instant::now()));
    let ready = Condvar::new();
    let worker = || fetch_while_any(fetch, addresses, &schedule, &ready, progress, &fetch_one);
    thread::scope(|scope| {
        let threads = politeness.in_flight.max(1).min(addresses.len());
        let started = (0..threads)
            .filter(|_| thread::Builder::new().name("constellate-fetch".to_owned()).spawn_scoped(scope, worker).is_ok())
            .count();
        if started == 0 && !addresses.is_empty() {
            worker(); // no thread could be had: one at a time, here
        }
    });
    let schedule = schedule.into_inner().unwrap_or_else(PoisonError::into_inner);
    schedule.outcomes.into_iter().map(|outcome| outcome.unwrap_or(Outcome::Cancelled)).collect()
}

/// One fetching thread: take the next address the schedule allows, fetch it outside the lock, hand back what came
/// of it, until none is left.
fn fetch_while_any<T, F>(
    fetch: &(dyn Fetch + Sync),
    addresses: &[String],
    schedule: &Mutex<Schedule<T>>,
    ready: &Condvar,
    progress: &Progress,
    fetch_one: &F,
) where
    F: Fn(&dyn Fetch, &str) -> T,
{
    loop {
        let (index, site) = {
            let mut state = open(schedule);
            loop {
                match state.next(Instant::now(), progress.is_cancelled()) {
                    Next::Fetch { index, site } => break (index, site),
                    Next::Finished => {
                        ready.notify_all();
                        return;
                    }
                    Next::Wait(after) => {
                        let wait = after.map_or(LOOK_FOR_CANCEL, |after| after.min(LOOK_FOR_CANCEL));
                        state = ready.wait_timeout(state, wait).unwrap_or_else(PoisonError::into_inner).0;
                    }
                }
            }
        };
        let watched = Watched { fetch, refusal: Cell::new(None) };
        let outcome = threads::caught(|| fetch_one(&watched, &addresses[index]));
        open(schedule).finished(index, &site, outcome, watched.refusal.get(), Instant::now(), progress);
        ready.notify_all();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::super::fake::{Fake, ok, status};
    use super::*;

    const LIMITS: Limits = Limits { max_bytes: 1 << 20, time: Duration::from_secs(10) };

    /// The rules, with pauses short enough for a test.
    fn quick() -> Politeness {
        Politeness {
            in_flight: 6,
            arxiv: Rule { at_once: 1, pause: Duration::from_millis(30) },
            others: Rule { at_once: 2, pause: Duration::ZERO },
        }
    }

    fn body(fetch: &dyn Fetch, address: &str) -> Result<String, String> {
        match fetch.get(address, LIMITS) {
            Ok(response) if response.status == 200 => Ok(response.text()),
            Ok(response) => Err(format!("HTTP {}", response.status)),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Ten papers on each of three publishers and ten on arXiv, all answering.
    fn many() -> (Fake, Vec<String>) {
        let mut fake = Fake::new().slow(Duration::from_millis(15));
        let mut addresses = Vec::new();
        for paper in 0..10 {
            for host in ["https://proceedings.mlr.press", "https://www.openaccess.thecvf.com", "https://arxiv.org"] {
                addresses.push(format!("{host}/paper/{paper}.pdf"));
            }
            addresses.push(format!("https://proceedings.neurips.cc/paper/{paper}.pdf"));
        }
        for address in &addresses {
            fake = fake.with(address, ok("application/pdf", address));
        }
        (fake, addresses)
    }

    #[test]
    fn no_host_ever_gets_more_requests_at_once_than_its_rule_and_arxiv_one_every_pause() {
        let (fake, addresses) = many();
        let progress = Progress::new();
        let outcomes = fetch_all(&fake, &addresses, &quick(), &progress, body);
        for (address, outcome) in addresses.iter().zip(&outcomes) {
            assert_eq!(outcome, &Outcome::Done(Ok(address.clone())), "every answer in its address's place");
        }
        for host in ["proceedings.mlr.press", "www.openaccess.thecvf.com", "proceedings.neurips.cc"] {
            let most = fake.most_at_once(host);
            assert!((1..=2).contains(&most), "{host}: {most} at once");
        }
        assert_eq!(fake.most_at_once("arxiv.org"), 1);
        assert!(fake.most_at_once("*") <= 6, "{} in all", fake.most_at_once("*"));
        assert!(fake.most_at_once("*") > 2, "hosts were asked side by side");
        let began = fake.began("arxiv.org");
        for pair in began.windows(2) {
            // the pause runs from the end of one request (15 ms) to the beginning of the next
            assert!(pair[1] - pair[0] >= Duration::from_millis(45), "{:?} apart", pair[1] - pair[0]);
        }
        assert_eq!((progress.done(), progress.total()), (40, 40));
    }

    #[test]
    fn every_host_of_arxiv_is_one_site_and_www_is_the_same_host() {
        assert_eq!(site("https://export.arxiv.org/api/query?x=1"), "arxiv.org");
        assert_eq!(site("https://ARXIV.org/pdf/1"), "arxiv.org");
        assert_eq!(site("https://www.openaccess.thecvf.com/a.pdf"), "openaccess.thecvf.com");
        assert_eq!(site("https://notarxiv.org/a.pdf"), "notarxiv.org");
        assert_eq!(site("not an address"), "");
        assert_eq!(Politeness::default().rule("arxiv.org"), ARXIV);
        assert_eq!(ARXIV, Rule { at_once: 1, pause: Duration::from_secs(3) });
    }

    #[test]
    fn a_refusal_is_waited_out_for_as_long_as_it_asks_then_asked_again() {
        let address = "https://proceedings.mlr.press/a.pdf".to_owned();
        let refused = Response { retry_after: Some(Duration::from_millis(120)), ..status(429) };
        let fake = Fake::new().first(&address, refused).with(&address, ok("application/pdf", "the paper"));
        let outcomes = fetch_all(&fake, std::slice::from_ref(&address), &quick(), &Progress::new(), body);
        assert_eq!(outcomes, [Outcome::Done(Ok("the paper".to_owned()))]);
        let began = fake.began("proceedings.mlr.press");
        assert_eq!(began.len(), 2);
        assert!(began[1] - began[0] >= Duration::from_millis(120), "asked again after {:?}", began[1] - began[0]);
    }

    #[test]
    fn a_host_that_keeps_refusing_gives_its_refusal_as_the_outcome_after_the_last_attempt() {
        let address = "https://proceedings.mlr.press/a.pdf".to_owned();
        let refused = Response { retry_after: Some(Duration::from_millis(5)), ..status(503) };
        let fake = Fake::new().with(&address, refused);
        let outcomes = fetch_all(&fake, std::slice::from_ref(&address), &quick(), &Progress::new(), body);
        assert_eq!(outcomes, [Outcome::Done(Err("HTTP 503".to_owned()))]);
        assert_eq!(fake.asked().len(), MOST_ATTEMPTS as usize);
    }

    #[test]
    fn one_address_that_fails_or_panics_loses_nothing_else_and_the_next_batch_is_fetched_as_usual() {
        let (fake, addresses) = many();
        let fake = fake.failing(&addresses[3], FetchError::TimedOut(addresses[3].clone()));
        let outcomes = fetch_all(&fake, &addresses, &quick(), &Progress::new(), |fetch, address| {
            assert!(address != addresses[5], "a reader's bug on one paper");
            body(fetch, address)
        });
        for (index, (address, outcome)) in addresses.iter().zip(&outcomes).enumerate() {
            match index {
                3 => assert!(matches!(outcome, Outcome::Done(Err(why)) if why.contains("too long"))),
                5 => assert!(matches!(outcome, Outcome::Panicked(said) if said.contains("a reader's bug"))),
                _ => assert_eq!(outcome, &Outcome::Done(Ok(address.clone()))),
            }
        }
        let again = fetch_all(&fake, &addresses[..4], &quick(), &Progress::new(), body);
        assert_eq!(again.len(), 4);
        assert_eq!(again[0], Outcome::Done(Ok(addresses[0].clone())));
    }

    #[test]
    fn a_cancelled_batch_begins_no_more_requests() {
        let (fake, addresses) = many();
        let progress = Progress::new();
        let outcomes = fetch_all(&fake, &addresses, &quick(), &progress, |fetch, address| {
            progress.cancel();
            body(fetch, address)
        });
        let fetched = outcomes.iter().filter(|outcome| matches!(outcome, Outcome::Done(_))).count();
        assert!((1..=6).contains(&fetched), "only those already begun: {fetched}");
        assert_eq!(fake.asked().len(), fetched);
        assert_eq!(outcomes.iter().filter(|outcome| **outcome == Outcome::Cancelled).count(), 40 - fetched);
    }

    #[test]
    fn many_batches_on_many_threads_at_once_each_get_their_own_answers() {
        // a stress of the schedule: 300 batches, 12 at a time, each against the same hosts
        let (fake, addresses) = many();
        let fake = fake.slow(Duration::ZERO);
        let rules = Politeness { arxiv: Rule { at_once: 1, pause: Duration::ZERO }, ..quick() };
        std::thread::scope(|scope| {
            for thread in 0..12 {
                let (fake, addresses) = (&fake, &addresses);
                scope.spawn(move || {
                    for round in 0..25 {
                        let from = (thread + round) % addresses.len();
                        let mine: Vec<String> = addresses.iter().cycle().skip(from).take(7).cloned().collect();
                        let outcomes = fetch_all(fake, &mine, &rules, &Progress::new(), body);
                        let expected: Vec<Outcome<Result<String, String>>> =
                            mine.iter().map(|address| Outcome::Done(Ok(address.clone()))).collect();
                        assert_eq!(outcomes, expected);
                    }
                });
            }
        });
        assert_eq!(fake.asked().len(), 12 * 25 * 7);
    }
}
