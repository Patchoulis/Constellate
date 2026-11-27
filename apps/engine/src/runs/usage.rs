//! What "likely to be opened next" means: a small model of how a person comes back to their runs.
//!
//! Every time a run is opened, the open is recorded with its time. A run's **score** at time `t` is the sum, over
//! every open `i` at time `tᵢ`, of a weight that halves every [`HALF_LIFE`]:
//!
//! ```text
//! score(t) = Σᵢ 2^(−(t − tᵢ) / H)          H = HALF_LIFE (one week)
//! ```
//!
//! An open today counts 1, an open a week ago ½, a month ago about 1/20, a year ago nothing worth speaking of. The
//! score therefore rises with frequency (each open adds) and with recency (recent opens add more), in one number:
//!
//! * ten opens a month ago: 10 · 2^(−30/7) ≈ 0.51
//! * two opens today: ≈ 2, which wins
//! * one open a year ago: 2^(−52) ≈ 0.0000000000000002, which loses to the ten
//!
//! Equal scores are broken by the most recent open ([`likelier`]).
//!
//! # Why the record stays small
//!
//! The sum need not keep every open. Because every term decays by the same factor, the whole sum decays by it too:
//! if `w` is the score at the last open `r`, then at any later `t`
//!
//! ```text
//! score(t) = w · 2^(−(t − r) / H)
//! ```
//!
//! and an open at `t` makes the new record `w' = w · 2^(−(t − r) / H) + 1` at `r' = t`. So a run's [`Usage`] is the
//! decayed weight, the time of the last open and a count of opens for showing: sixteen bytes and a counter,
//! however often it is opened. An open recorded late (its time before `r`, say from a second window) is added
//! decayed to `r` instead: `w' = w + 2^(−(r − t) / H)`, which is the same sum.
//!
//! # Why a week
//!
//! A run is a piece of work a researcher returns to while it matters to them: through the days they are reading
//! a conference's papers, less after. A day would forget last week's main run over one evening spent elsewhere;
//! a month would keep a finished project on top for weeks. A week is the rhythm of most such work.
//!
//! Every function here is told the time ([`Moment`]); none reads the clock, so what they answer is exact and the
//! tests can say it in numbers. The one clock is [`SystemClock`], for whoever wires this up.

use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The time after which an open counts half: one week, in seconds.
pub const HALF_LIFE: f64 = 7.0 * 24.0 * 60.0 * 60.0;

/// A point in time: whole seconds since 1970-01-01 UTC, as stored and as told to the interface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Moment(pub i64);

impl Moment {
    /// Seconds from `self` to `later` (negative when `later` is earlier).
    pub fn until(self, later: Moment) -> i64 {
        later.0.saturating_sub(self.0)
    }

    /// This moment moved by some seconds (back, when negative).
    pub fn plus(self, seconds: i64) -> Moment {
        Moment(self.0.saturating_add(seconds))
    }
}

/// Where the time comes from. Code that decides is given a [`Moment`]; code that wires things up asks a clock.
pub trait Clock {
    fn now(&self) -> Moment;
}

/// The computer's clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Moment {
        // a clock set before 1970 reads as 1970: the scores only compare runs with each other
        let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map(|since| since.as_secs()).unwrap_or(0);
        Moment(i64::try_from(seconds).unwrap_or(i64::MAX))
    }
}

/// How much an open `seconds` ago still counts: 1 now, ½ one half-life ago. A time in the future (a clock put
/// back) counts as now, so a run never scores more than its opens.
pub fn decay(seconds: i64) -> f64 {
    // exact as a float for any span shorter than 285 million years
    let elapsed = seconds.max(0) as f64;
    (-elapsed / HALF_LIFE).exp2()
}

/// How a run has been opened: never, or some times, the last at a known moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Usage {
    #[default]
    Never,
    Opened {
        /// The last open.
        last: Moment,
        /// How many opens in all, for showing ("opened 12 times"); the score does not read it.
        times: u32,
        /// The score as it was at `last`.
        weight: f64,
    },
}

impl Usage {
    /// The record after one more open at `at`.
    #[must_use]
    pub fn opened(self, at: Moment) -> Usage {
        match self {
            Usage::Never => Usage::Opened { last: at, times: 1, weight: 1.0 },
            Usage::Opened { last, times, weight } => {
                let times = times.saturating_add(1);
                if at >= last {
                    Usage::Opened { last: at, times, weight: weight * decay(last.until(at)) + 1.0 }
                } else {
                    Usage::Opened { last, times, weight: weight + decay(at.until(last)) }
                }
            }
        }
    }

    /// How likely the run is to be opened at `now`: the decayed sum of its opens, 0 for a run never opened.
    pub fn score(&self, now: Moment) -> f64 {
        match self {
            Usage::Never => 0.0,
            Usage::Opened { last, weight, .. } => weight * decay(last.until(now)),
        }
    }

    /// When the run was last opened, if ever.
    pub fn last_opened(&self) -> Option<Moment> {
        match self {
            Usage::Never => None,
            Usage::Opened { last, .. } => Some(*last),
        }
    }

    /// How many times the run has been opened.
    pub fn times(&self) -> u32 {
        match self {
            Usage::Never => 0,
            Usage::Opened { times, .. } => *times,
        }
    }
}

/// Which of two runs is likelier to be opened at `now`: `Greater` when `a` is. The higher score wins; equal
/// scores go to the more recent open, and a run never opened is less likely than any that was.
pub fn likelier(a: &Usage, b: &Usage, now: Moment) -> Ordering {
    a.score(now).total_cmp(&b.score(now)).then_with(|| a.last_opened().cmp(&b.last_opened()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 60 * 60;
    const WEEK: i64 = 7 * DAY;
    const NOW: Moment = Moment(1_800_000_000);

    fn opened_at(times: &[Moment]) -> Usage {
        times.iter().fold(Usage::Never, |usage, at| usage.opened(*at))
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    #[test]
    fn an_open_counts_one_now_half_after_a_week_and_a_quarter_after_two() {
        let usage = Usage::Never.opened(NOW);
        assert_eq!(usage.score(NOW), 1.0);
        assert_eq!(usage.score(NOW.plus(WEEK)), 0.5);
        assert_eq!(usage.score(NOW.plus(2 * WEEK)), 0.25);
        assert_eq!(Usage::Never.score(NOW), 0.0);
    }

    #[test]
    fn the_small_record_gives_exactly_the_sum_over_every_open() {
        let opens: Vec<Moment> = [0, 3 * DAY, 3 * DAY + 60, 10 * DAY, 40 * DAY].map(Moment).to_vec();
        let usage = opened_at(&opens);
        let now = Moment(45 * DAY);
        let sum: f64 = opens.iter().map(|at| decay(at.until(now))).sum();
        assert!(close(usage.score(now), sum), "{} != {sum}", usage.score(now));
        assert_eq!(usage.times(), 5);
        assert_eq!(usage.last_opened(), Some(Moment(40 * DAY)));
    }

    #[test]
    fn an_open_recorded_late_adds_the_same_as_one_recorded_in_order() {
        let in_order = opened_at(&[Moment(0), Moment(2 * DAY), Moment(5 * DAY)]);
        let late = opened_at(&[Moment(0), Moment(5 * DAY), Moment(2 * DAY)]);
        let now = Moment(9 * DAY);
        assert!(close(in_order.score(now), late.score(now)));
        assert_eq!(late.last_opened(), Some(Moment(5 * DAY)));
        assert_eq!(late.times(), 3);
    }

    #[test]
    fn ten_opens_last_month_lose_to_two_today_but_beat_one_a_year_ago() {
        let month_ago: Vec<Moment> = (0..10).map(|hour| NOW.plus(-30 * DAY + hour * 3600)).collect();
        let heavy_last_month = opened_at(&month_ago);
        let twice_today = opened_at(&[NOW.plus(-3 * 3600), NOW.plus(-3600)]);
        let once_last_year = opened_at(&[NOW.plus(-365 * DAY)]);

        assert_eq!(likelier(&twice_today, &heavy_last_month, NOW), Ordering::Greater);
        assert_eq!(likelier(&heavy_last_month, &once_last_year, NOW), Ordering::Greater);
        assert!(heavy_last_month.score(NOW) > 0.5 && heavy_last_month.score(NOW) < 0.53);
        assert!(once_last_year.score(NOW) < 1e-15);
    }

    #[test]
    fn equal_scores_go_to_the_more_recent_open() {
        // twice a week ago is 2 · ½ = 1, exactly what once just now is
        let twice_a_week_ago = opened_at(&[NOW.plus(-WEEK), NOW.plus(-WEEK)]);
        let once_now = opened_at(&[NOW]);
        assert_eq!(twice_a_week_ago.score(NOW), once_now.score(NOW));
        assert_eq!(likelier(&once_now, &twice_a_week_ago, NOW), Ordering::Greater);
        assert_eq!(likelier(&twice_a_week_ago, &once_now, NOW), Ordering::Less);
    }

    #[test]
    fn a_run_never_opened_is_less_likely_than_one_opened_long_ago() {
        let ancient = opened_at(&[Moment(0)]);
        assert_eq!(ancient.score(Moment(i64::MAX)), 0.0); // decayed to nothing...
        assert_eq!(likelier(&ancient, &Usage::Never, Moment(i64::MAX)), Ordering::Greater); // ...but still more
        assert_eq!(likelier(&Usage::Never, &Usage::Never, NOW), Ordering::Equal);
    }

    #[test]
    fn a_clock_put_back_never_makes_a_run_count_more_than_its_opens() {
        let usage = opened_at(&[NOW, NOW]);
        assert_eq!(usage.score(NOW.plus(-WEEK)), 2.0);
    }

    #[test]
    fn the_record_is_small_and_stays_small_however_often_a_run_is_opened() {
        assert!(std::mem::size_of::<Usage>() <= 32);
        let mut usage = Usage::Never;
        for minute in 0..100_000 {
            usage = usage.opened(NOW.plus(minute * 60));
        }
        assert_eq!(usage.times(), 100_000);
        assert!(usage.score(NOW.plus(100_000 * 60)).is_finite());
    }

    #[test]
    fn a_record_is_stored_and_read_back_in_plain_words() {
        let usage = Usage::Never.opened(Moment(5));
        let told = serde_json::to_value(usage).unwrap();
        assert_eq!(told, serde_json::json!({"kind": "opened", "last": 5, "times": 1, "weight": 1.0}));
        assert_eq!(serde_json::from_value::<Usage>(told).unwrap(), usage);
        assert_eq!(serde_json::to_value(Usage::Never).unwrap(), serde_json::json!({"kind": "never"}));
    }
}
