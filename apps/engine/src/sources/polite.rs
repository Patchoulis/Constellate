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
//! The rules of a schedule hold within one call (the engine runs one download batch at a time: its one background
//! job does). What holds across every call is arXiv's pace, below.
//!
//! # arXiv's one pace
//!
//! Everything the engine sends to arXiv (its API for listings and lookups, its PDFs, a publisher's redirect to an
//! arXiv PDF) waits for a turn at one [`ArxivPace`], [`ARXIV_PACE`]: one request at a time (arXiv asks programs
//! for a single connection), the next one [`super::citations::PAUSE`] after the last ended (it asks for no more
//! than one request every three seconds), and after a refusal (429, 503) as long as the refusal asks, else
//! [`FIRST_BACK_OFF`] doubling with each refusal in a row. Turns are given in the order they were asked for:
//! a reader opening a paper while a run reads its PDFs waits behind the requests asked before it, never behind
//! those asked after. The network ([`super::Http`]) takes the turn for every address on arXiv, so no caller can
//! forget to: [`Paced`] does it for any [`Fetch`]. A request that would wait longer than its own time limit is
//! told so at once instead of holding its caller.

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

/// arXiv within a batch: one request at a time, so that a batch holds at most one turn at [`ARXIV_PACE`]; the
/// pause between requests is the pace's, kept across everything the engine sends to arXiv.
pub const ARXIV: Rule = Rule { at_once: 1, pause: Duration::ZERO };
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
/// attempt on one thread, and never shared. A redirect to arXiv needs nothing of it: every request to arXiv waits
/// for its turn at [`ARXIV_PACE`] in the network itself ([`Paced`]).
struct Watched<'a> {
    fetch: &'a (dyn Fetch + Sync),
    refusal: Cell<Option<Option<Duration>>>,
}

impl Fetch for Watched<'_> {
    fn get(&self, url: &str, limits: Limits) -> Result<Response, FetchError> {
        let answer = self.fetch.get(url, limits);
        // the last answer of the attempt decides: a refusal followed by an answer is not a refusal
        if let Ok(response) = &answer {
            self.refusal.set(refused(response));
        }
        answer
    }
}

/// A refusal to be asked so often, and how long it asks to be left alone when it says.
fn refused(response: &Response) -> Option<Option<Duration>> {
    matches!(response.status, 429 | 503).then_some(response.retry_after)
}

/// Whose turn it is at arXiv.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Turn {
    /// Nobody's: the next in line may begin at `next` (at once when `None`).
    Free { next: Option<Instant> },
    /// A request is under way.
    Taken,
}

/// The line of requests waiting for arXiv, and whose turn it is.
#[derive(Debug)]
struct Line {
    /// Tickets of those waiting, in the order they asked.
    waiting: VecDeque<u64>,
    issued: u64,
    turn: Turn,
    /// Refusals in a row, for the back-off.
    refusals: u32,
}

/// arXiv's one pace: see the module's "arXiv's one pace". [`ARXIV_PACE`] is the engine's; a test makes its own
/// with a short pause.
#[derive(Debug)]
pub struct ArxivPace {
    pause: Duration,
    line: Mutex<Line>,
    changed: Condvar,
}

/// The pace everything the engine sends to arXiv keeps.
pub static ARXIV_PACE: ArxivPace = ArxivPace::new(super::citations::PAUSE);

/// A turn at arXiv, held while one request is under way; given back when dropped (on a panic too), with the
/// pause, or the back-off a refusal asked for, before the next turn.
pub struct Granted<'a> {
    pace: &'a ArxivPace,
    refusal: Option<Option<Duration>>,
}

impl Granted<'_> {
    /// What the request was answered: a refusal (429, 503) keeps arXiv waiting for as long as it asks.
    pub fn answered(&mut self, response: &Response) {
        self.refusal = refused(response);
    }
}

impl Drop for Granted<'_> {
    fn drop(&mut self) {
        let mut line = self.pace.lock();
        let wait = match self.refusal {
            Some(asked) => {
                line.refusals = line.refusals.saturating_add(1);
                let doubled = FIRST_BACK_OFF.saturating_mul(2u32.saturating_pow(line.refusals - 1));
                asked.unwrap_or(doubled).min(LONGEST_WAIT).max(self.pace.pause)
            }
            None => {
                line.refusals = 0;
                self.pace.pause
            }
        };
        line.turn = Turn::Free { next: Some(Instant::now() + wait) };
        drop(line);
        self.pace.changed.notify_all();
    }
}

/// Leaves the line when the one waiting gives up (or panics), so that whoever is behind is not held up.
struct InLine<'a> {
    pace: &'a ArxivPace,
    ticket: u64,
    served: bool,
}

impl Drop for InLine<'_> {
    fn drop(&mut self) {
        if !self.served {
            self.pace.lock().waiting.retain(|ticket| *ticket != self.ticket);
            self.pace.changed.notify_all();
        }
    }
}

impl ArxivPace {
    /// A pace of `pause` between the end of one request and the beginning of the next.
    pub const fn new(pause: Duration) -> Self {
        Self {
            pause,
            line: Mutex::new(Line {
                waiting: VecDeque::new(),
                issued: 0,
                turn: Turn::Free { next: None },
                refusals: 0,
            }),
            changed: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Line> {
        // every change under the lock is whole: a panic elsewhere leaves the line as it was
        self.line.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Wait in line for a turn, at most `patience`. Turns are given in the order they were asked for. When the
    /// turn cannot come within `patience` (arXiv asked to be left alone for longer, or the line is too long), the
    /// answer is how long until arXiv may next be asked, and the line is left at once.
    pub fn turn(&self, patience: Duration) -> Result<Granted<'_>, Duration> {
        let deadline = Instant::now() + patience;
        let mut line = self.lock();
        let ticket = line.issued;
        line.issued += 1;
        line.waiting.push_back(ticket);
        let mut in_line = InLine { pace: self, ticket, served: false };
        loop {
            let now = Instant::now();
            let first = line.waiting.front() == Some(&ticket);
            let free_at = match line.turn {
                Turn::Free { next } => Some(next.unwrap_or(now)),
                Turn::Taken => None,
            };
            match free_at {
                Some(at) if first && at <= now => {
                    line.waiting.pop_front();
                    line.turn = Turn::Taken;
                    in_line.served = true;
                    return Ok(Granted { pace: self, refusal: None });
                }
                // arXiv asked to be left alone past the deadline: no use waiting for it
                Some(at) if at > deadline => {
                    drop(line);
                    return Err(at - now);
                }
                _ if now >= deadline => {
                    drop(line);
                    return Err(free_at.map_or(self.pause, |at| at.saturating_duration_since(now).max(self.pause)));
                }
                _ => {}
            }
            let until = match free_at {
                Some(at) if first => at.min(deadline),
                _ => deadline,
            };
            line = self
                .changed
                .wait_timeout(line, until.saturating_duration_since(now))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// A [`Fetch`] whose every request to arXiv waits for its turn at an [`ArxivPace`]; requests to other hosts go as
/// they come. The network ([`super::Http`]) is one, at [`ARXIV_PACE`].
pub struct Paced<'p, F> {
    pub inner: F,
    pub pace: &'p ArxivPace,
}

impl<F: Fetch> Fetch for Paced<'_, F> {
    fn get(&self, url: &str, limits: Limits) -> Result<Response, FetchError> {
        if site(url) != "arxiv.org" {
            return self.inner.get(url, limits);
        }
        let mut turn = self.pace.turn(limits.time).map_err(|left| {
            FetchError::Unreachable(
                url.to_owned(),
                format!("arXiv is to be left alone for {} more seconds", left.as_secs().max(1)),
            )
        })?;
        let answer = self.inner.get(url, limits);
        if let Ok(response) = &answer {
            turn.answered(response);
        }
        answer
    }
}

impl<F: Fetch + ?Sized> Fetch for std::sync::Arc<F> {
    fn get(&self, url: &str, limits: Limits) -> Result<Response, FetchError> {
        (**self).get(url, limits)
    }
}

impl<F: Fetch + ?Sized> Fetch for &F {
    fn get(&self, url: &str, limits: Limits) -> Result<Response, FetchError> {
        (**self).get(url, limits)
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
        assert_eq!(ARXIV, Rule { at_once: 1, pause: Duration::ZERO }, "the pause is the one pace's");
        assert_eq!(ARXIV_PACE.pause, Duration::from_secs(3));
    }

    /// Requests to arXiv through `pace`, each taking `lasting`, from `threads` threads that ask one after another
    /// (thread 0 first); answers the order they began in.
    fn line_up(pace: &ArxivPace, threads: usize, lasting: Duration) -> Vec<usize> {
        let began = Mutex::new(Vec::new());
        let busy = Mutex::new(0usize);
        std::thread::scope(|scope| {
            for thread in 0..threads {
                let (began, busy) = (&began, &busy);
                scope.spawn(move || {
                    let turn = pace.turn(Duration::from_secs(10)).unwrap();
                    {
                        let mut now = busy.lock().unwrap();
                        *now += 1;
                        assert_eq!(*now, 1, "two requests at arXiv at once");
                    }
                    began.lock().unwrap().push(thread);
                    std::thread::sleep(lasting);
                    *busy.lock().unwrap() -= 1;
                    drop(turn);
                });
                // one after another: each is in line before the next asks
                while pace.lock().issued <= thread as u64 {
                    std::thread::yield_now();
                }
            }
        });
        began.into_inner().unwrap()
    }

    #[test]
    fn arxiv_is_asked_once_at_a_time_in_the_order_turns_were_asked_for_with_its_pause_between() {
        let pace = ArxivPace::new(Duration::from_millis(40));
        let started = Instant::now();
        let began = line_up(&pace, 6, Duration::from_millis(5));
        assert_eq!(began, (0..6).collect::<Vec<_>>(), "first come, first served");
        // six requests of 5 ms, with five pauses of 40 ms between them
        assert!(started.elapsed() >= Duration::from_millis(5 * 40 + 6 * 5), "{:?}", started.elapsed());
        assert!(pace.lock().waiting.is_empty());
    }

    #[test]
    fn a_refusal_keeps_arxiv_waiting_as_long_as_it_asks_and_a_turn_that_cannot_come_in_time_is_refused_at_once() {
        let pace = ArxivPace::new(Duration::from_millis(10));
        {
            let mut turn = pace.turn(Duration::from_secs(1)).unwrap();
            turn.answered(&Response { retry_after: Some(Duration::from_secs(120)), ..status(429) });
        }
        let started = Instant::now();
        let left = pace.turn(Duration::from_secs(5)).err().unwrap();
        assert!(started.elapsed() < Duration::from_millis(500), "told at once, not after waiting");
        assert!(left > Duration::from_secs(100), "{left:?}");
        assert!(pace.lock().waiting.is_empty(), "a request that gives up leaves the line");
        assert_eq!(pace.lock().refusals, 1);

        // a refusal that does not say how long: the back-off, then answered again, the pause only
        let pace = ArxivPace::new(Duration::from_millis(10));
        let mut turn = pace.turn(Duration::from_secs(1)).unwrap();
        turn.answered(&status(503));
        drop(turn);
        assert!(pace.turn(FIRST_BACK_OFF - Duration::from_secs(5)).is_err());
        let Turn::Free { next: Some(next) } = pace.lock().turn else { panic!() };
        assert!(next > Instant::now() + FIRST_BACK_OFF - Duration::from_secs(2));
    }

    #[test]
    fn one_who_gives_up_waiting_does_not_hold_up_those_behind() {
        let pace = ArxivPace::new(Duration::ZERO);
        let held = pace.turn(Duration::from_secs(1)).unwrap();
        std::thread::scope(|scope| {
            let impatient = scope.spawn(|| pace.turn(Duration::from_millis(30)).is_err());
            while pace.lock().issued < 2 {
                std::thread::yield_now();
            }
            let patient = scope.spawn(|| pace.turn(Duration::from_secs(5)).map(drop).is_ok());
            assert!(impatient.join().unwrap(), "gave up");
            drop(held);
            assert!(patient.join().unwrap(), "served once the turn was free, though one ahead had left");
        });
        assert!(pace.lock().waiting.is_empty());
    }

    #[test]
    fn a_paced_fetch_waits_its_turn_only_for_arxiv() {
        static PACE: ArxivPace = ArxivPace::new(Duration::from_millis(60));
        let fake = Fake::new()
            .with("https://arxiv.org/pdf/1", ok("application/pdf", "one"))
            .with("https://export.arxiv.org/api/query?x", ok("text/xml", "two"))
            .with("https://proceedings.mlr.press/a.pdf", ok("application/pdf", "three"));
        let paced = Paced { inner: &fake, pace: &PACE };
        for address in
            ["https://arxiv.org/pdf/1", "https://proceedings.mlr.press/a.pdf", "https://export.arxiv.org/api/query?x"]
        {
            assert_eq!(paced.get(address, LIMITS).unwrap().status, 200);
        }
        let began = fake.began("*");
        assert!(began[1] - began[0] < Duration::from_millis(50), "another host does not wait");
        assert!(began[2] - began[0] >= Duration::from_millis(60), "the API and the PDFs keep one pace");
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
    fn a_publishers_redirect_to_arxiv_waits_for_its_turn_at_arxiv() {
        // four publisher addresses that each redirect to an arXiv PDF, beside four arXiv addresses of its own
        let mut fake = Fake::new().slow(Duration::from_millis(10));
        let mut addresses = Vec::new();
        for paper in 0..4 {
            let publisher = format!("https://proceedings.mlr.press/{paper}.pdf");
            let arxiv = format!("https://arxiv.org/pdf/{paper}");
            let redirected = format!("https://arxiv.org/pdf/moved-{paper}");
            fake = fake
                .with(&publisher, super::super::fake::redirect(302, &redirected))
                .with(&redirected, ok("application/pdf", &redirected))
                .with(&arxiv, ok("application/pdf", &arxiv));
            addresses.push(publisher);
            addresses.push(arxiv);
        }
        let follow = |fetch: &dyn Fetch, address: &str| -> Result<String, String> {
            let (_, response) = super::super::get_following(fetch, address, LIMITS, &super::super::any_https)
                .map_err(|e| e.to_string())?;
            Ok(response.text())
        };
        // as through the network: every request to arXiv waits for its turn at the one pace
        static PACE: ArxivPace = ArxivPace::new(Duration::from_millis(40));
        let paced = Paced { inner: &fake, pace: &PACE };
        let outcomes = fetch_all(&paced, &addresses, &quick(), &Progress::new(), follow);
        assert!(outcomes.iter().all(|outcome| matches!(outcome, Outcome::Done(Ok(_)))), "{outcomes:?}");
        assert_eq!(fake.most_at_once("arxiv.org"), 1, "arXiv was asked more than once at a time");
        let began = fake.began("arxiv.org");
        assert_eq!(began.len(), 8);
        for pair in began.windows(2) {
            assert!(pair[1] - pair[0] >= Duration::from_millis(40), "{:?} apart", pair[1] - pair[0]);
        }
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
