//! The detector's states, a run's scores, and a paper's verdict (the Python's `test_detector_availability.py` and
//! the detector's part of `test_review.py`, ported).

use std::sync::Mutex;

use super::*;
use crate::jobs::{Cancel, Progress as JobProgress};
use crate::library::detector::{Generator, Human, Kind, Mirror, POOL};
use crate::library::{Known, papers::tests::paper as record};
use crate::model::language::Provider;

pub(crate) const HUMAN: &str =
    "we study a model and show results on data with a simple method and report error rates for several tasks ";
pub(crate) const MACHINE: &str =
    "this paper delves into an intricate landscape showcasing pivotal insights that underscore robust novel advances ";

/// The Python test's texts: one style six times over, and a word of its own.
pub(crate) fn text(base: &str, n: usize) -> String {
    let mut words: Vec<String> = base.repeat(6).split_whitespace().map(str::to_owned).collect();
    words.push(format!("topic{n}"));
    words.join(" ")
}

/// A library with a built training set of the two styles (as a build, or the Python's import, would leave it).
pub(crate) fn trained_library() -> (tempfile::TempDir, Library) {
    let folder = tempfile::tempdir().unwrap();
    let library = Library::open(folder.path()).unwrap();
    build_in(&library);
    (folder, library)
}

/// Keep that training set in `library`.
pub(crate) fn build_in(library: &Library) {
    let pool: Vec<Human> = (0..400)
        .map(|n| Human { id: format!("1906.{n:05}"), title: format!("Title {n}"), r#abstract: text(HUMAN, n) })
        .collect();
    library.keep_detector_day(POOL, "imported", &pool).unwrap();
    let generator = Generator { provider: Some(Provider::OpenAi), model: "gpt-6.2-sol".to_owned() };
    let mirrors: Vec<Mirror> = pool
        .iter()
        .enumerate()
        .flat_map(|(n, human)| {
            Kind::ALL.map(|kind| Mirror {
                id: human.id.clone(),
                kind,
                generator: generator.clone(),
                // a topic word of their own, so that no word but the style's tells the two apart
                text: text(MACHINE, n + 1_000),
            })
        })
        .collect();
    library.keep_detector_mirrors(&mirrors).unwrap();
    let train: Vec<String> =
        pool.iter().filter(|human| !classifier::held_out(&human.id)).take(60).map(|human| human.id.clone()).collect();
    library.keep_detector_built(&train, &[40, 60]).unwrap();
}

struct Quiet {
    lines: Mutex<Vec<String>>,
    cancel: Cancel,
}

impl Quiet {
    fn new() -> Self {
        Quiet { lines: Mutex::new(Vec::new()), cancel: Cancel::new() }
    }

    fn score(&self, library: &Library, settings: &Settings, ids: &[String]) -> RunScores {
        let log = |line: &str| self.lines.lock().unwrap().push(line.to_owned());
        let progress = |_: JobProgress| {};
        score_run(library, settings, ids, &Tracker::new(&log, &progress, &self.cancel))
    }
}

fn on() -> Settings {
    Settings::default()
}

fn off() -> Settings {
    Settings { detect_ai: false, ..Settings::default() }
}

fn papers(library: &Library) -> Vec<String> {
    let mut records = vec![record("h"), record("m"), record("short")];
    records[0].r#abstract = text(HUMAN, 1);
    records[1].r#abstract = text(MACHINE, 1);
    records[2].r#abstract = "too short to judge".to_owned();
    library.add_papers(&records, None, Known::Keep).unwrap();
    vec!["h".to_owned(), "m".to_owned(), "short".to_owned(), "not-in-library".to_owned()]
}

#[test]
fn exactly_one_state_holds_and_each_says_why() {
    let (_folder, library) = trained_library();
    let trained = model(&library).unwrap().unwrap();
    assert_eq!(availability(false, None, None), Availability::Off);
    assert_eq!(availability(true, None, None), Availability::Untrained);
    assert!(Availability::Untrained.why(None).contains("does not need a graphics card"));
    assert!(Availability::Untrained.why(Some("about USD 3.10 with gpt-6.2-sol of OpenAI")).contains("USD 3.10"));
    assert!(!Availability::Untrained.why(None).contains("USD"), "a cost that cannot be estimated is not named");
    assert_eq!(availability(true, Some((3, 2000)), None), Availability::Building { done: 3, of: 2000 });
    assert!(matches!(availability(true, None, Some(&trained)), Availability::Ready { .. }));
    assert_eq!(availability(false, None, Some(&trained)), Availability::Off, "switched off wins over trained");
    let older = Trained { classifier: Classifier { features: 0, ..trained.classifier.clone() }, ..(*trained).clone() };
    let stale = availability(true, None, Some(&older));
    assert!(matches!(stale, Availability::Stale { .. }) && stale.scores());
    for state in [Availability::Off, Availability::Untrained, Availability::Building { done: 0, of: 1 }, stale] {
        assert!(!state.why(None).is_empty(), "{state:?}");
    }
    assert!(!Availability::Untrained.scores() && !Availability::Off.scores());
}

#[test]
fn a_kept_classifier_from_a_newer_version_or_one_that_does_not_read_is_not_used_nor_overwritten() {
    for (features, content) in [(features::FEATURES + 1, "{}"), (features::FEATURES, "not a classifier")] {
        let (_folder, library) = trained_library();
        library.keep_detector_model(features, logistic::TRAINING, content).unwrap();
        assert_eq!(model(&library).unwrap(), None, "untrained rather than an error");
        assert_eq!(library.detector_model().unwrap().unwrap().content, content, "left as it was");
        let ids = papers(&library);
        let scores = Quiet::new().score(&library, &on(), &ids);
        assert_eq!(scores.outcome, Scoring::Skipped { why: Skipped::Untrained });
    }
}

#[test]
fn the_pool_trained_on_is_its_first_abstracts_however_many_days_kept() {
    let (_folder, library) = trained_library();
    let small = training::Sizes { pool: 300, ..training::Sizes::RECIPE };
    assert_eq!(training::pool(&library, small).unwrap().len(), 300);
    assert_eq!(training::pool(&library, training::Sizes::RECIPE).unwrap().len(), 400);
}

#[test]
fn a_run_tags_nothing_when_the_detector_is_off_or_untrained() {
    let folder = tempfile::tempdir().unwrap();
    let library = Library::open(folder.path()).unwrap();
    let ids = papers(&library);
    for (settings, why, said) in
        [(off(), Skipped::Off, "Switched off"), (on(), Skipped::Untrained, "no AI-text detector")]
    {
        let quiet = Quiet::new();
        let scores = quiet.score(&library, &settings, &ids);
        assert_eq!(scores.outcome, Scoring::Skipped { why });
        assert_eq!(scores.flags(), [false; 4]);
        assert!(quiet.lines.lock().unwrap()[0].contains(said));
    }
}

#[test]
fn a_run_scores_abstracts_once_per_classifier_and_text_and_flags_those_above_the_threshold() {
    let (_folder, library) = trained_library();
    let ids = papers(&library);
    let first = Quiet::new().score(&library, &on(), &ids);
    assert_eq!(first.outcome, Scoring::Scored { scored: 3, kept: 0, flagged: 1 });
    assert_eq!(first.flags(), [false, true, false, false], "no verdict, not tagged");
    assert!(first.papers[0].score.is_some() && first.papers[2].score.is_none() && first.papers[3].score.is_none());
    let again = Quiet::new().score(&library, &on(), &ids);
    assert_eq!(again.outcome, Scoring::Kept { flagged: 1 }, "nothing scored twice");
    assert_eq!(again.papers, first.papers);

    // a revised abstract is scored again, and only it
    let mut revised = record("h");
    revised.r#abstract = text(MACHINE, 7);
    library.add_papers(&[revised], None, Known::Replace).unwrap();
    let third = Quiet::new().score(&library, &on(), &ids);
    assert_eq!(third.outcome, Scoring::Scored { scored: 1, kept: 2, flagged: 2 });
}

#[test]
fn scores_are_the_same_on_one_thread_and_on_many() {
    let (_folder, library) = trained_library();
    let records: Vec<_> = (0..300)
        .map(|n| {
            let mut paper = record(&format!("p{n}"));
            paper.r#abstract = text(if n % 3 == 0 { MACHINE } else { HUMAN }, n);
            paper
        })
        .collect();
    library.add_papers(&records, None, Known::Keep).unwrap();
    let ids: Vec<String> = records.iter().map(|paper| paper.id.clone()).collect();
    let trained = model(&library).unwrap().unwrap();
    let (one, many) = crate::threads::on_one_and_many(|| {
        // scored afresh each time: nothing kept between the two
        let progress = Progress::new();
        threads::each(&records, &progress, |paper| trained.classifier.score(&paper.r#abstract).map(f64::to_bits))
    });
    assert_eq!(one, many);
    let scored = Quiet::new().score(&library, &on(), &ids);
    assert_eq!(scored.flags().iter().filter(|flag| **flag).count(), 100);
}

#[test]
fn a_cancelled_run_keeps_what_it_scored_and_flags_nothing() {
    let (_folder, library) = trained_library();
    let ids = papers(&library);
    let quiet = Quiet::new();
    quiet.cancel.cancel();
    let scores = quiet.score(&library, &on(), &ids);
    assert!(matches!(scores.outcome, Scoring::Failed { .. }));
    assert_eq!(scores.flags(), [false; 4]);
}

#[test]
fn a_papers_verdict_names_what_raised_it_and_only_for_the_text_that_was_scored() {
    let (_folder, library) = trained_library();
    let ids = papers(&library);
    let machine = library.paper("m").unwrap().unwrap();
    assert_eq!(verdict(&library, &machine).unwrap(), None, "never scored");
    Quiet::new().score(&library, &on(), &ids);
    let told = verdict(&library, &machine).unwrap().unwrap();
    assert!(told.flagged && told.score > told.threshold && told.false_alarms == FALSE_ALARMS);
    assert!(told.markers.iter().any(|marker| ["delves", "intricate", "pivotal"].contains(&marker.as_str())));
    assert_eq!(told.score, (told.score * 1000.0).round() / 1000.0, "three decimals, as the Python showed it");
    let human = library.paper("h").unwrap().unwrap();
    assert!(!verdict(&library, &human).unwrap().unwrap().flagged);
    assert_eq!(verdict(&library, &library.paper("short").unwrap().unwrap()).unwrap(), None, "too short");
    let mut revised = machine.clone();
    revised.r#abstract = format!("{} revised", machine.r#abstract);
    assert_eq!(verdict(&library, &revised).unwrap(), None, "not the text that was scored");
}

#[test]
fn the_validation_names_the_models_that_wrote_what_it_learnt() {
    let (_folder, library) = trained_library();
    let trained = model(&library).unwrap().unwrap();
    let told = serde_json::to_value(Validation::of(&trained, None)).unwrap();
    assert_eq!(told["generators"], serde_json::json!(["gpt-6.2-sol (OpenAI)"]));
    assert_eq!(told["false_alarms"], 0.01);
    assert!(told["written_caught"].as_f64().unwrap() > 0.9);
}
