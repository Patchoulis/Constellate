//! The Rust detector against the Python's own numbers.
//!
//! `fixtures/python.json` is what `apps/backend/rp/detect.py` (scikit-learn and numpy, versions in its `about`)
//! computed once, run by hand, on a synthetic corpus: two made-up styles of text with different word
//! distributions, made by the generator below from splitmix64 seed 20190601, written as the Python's
//! `training.json` (400 pool papers, mirrors of 30 held-out and 150 training papers, three rounds). Nothing in it
//! is a real abstract. The script that ran the Python is not kept in the repository (no Python is); it did exactly
//! what [`corpus`] does, then `detect.classifier()` and `detect.report()`, and fitted the same problem again with
//! `tol=1e-12` to give its exact minimum.
//!
//! The test writes the same corpus as `training.json`, moves it in through the library's import as a user's
//! files would be, lets the detector fit its classifier, and compares, within these tolerances:
//!
//! | what                                         | against the Python as shipped | against the exact minimum |
//! |----------------------------------------------|-------------------------------|---------------------------|
//! | vocabulary (size, terms), idf, version       | equal (idf to 1e-12)          |                           |
//! | coefficients, intercept, scores              | 3e-3                          | 1e-6                      |
//! | threshold                                    | 1e-3 (it showed 3 decimals)   | 1e-6                      |
//! | catch rates, AUC, rounds, telling words, markers | equal                     |                           |
//!
//! The Python's own solver stopped at a gradient of 1e-4, which leaves its scores about 1e-3 from the minimum; the
//! engine's goes to 1e-10 ([`super::logistic`]).

use std::fs;

use serde_json::{Value, json};

use super::classifier::{Side, side};
use super::model;
use crate::library::Library;

const MASK: u128 = (1 << 64) - 1;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = ((u128::from(z ^ (z >> 30)) * 0xBF58_476D_1CE4_E5B9) & MASK) as u64;
        z = ((u128::from(z ^ (z >> 27)) * 0x94D0_49BB_1331_11EB) & MASK) as u64;
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const COMMON: &str = "the of and a to in we is for that on with this by as are an be from our which it can these at model models method data results learning network performance approach show using problem based training new";
const HUMAN: &str = "propose proposed due thus various order able furthermore state-of-the-art compared towards like different have there namely i.e novel utilize we've doesn't CNN ImageNet naïve Schrödinger LSTM e-commerce well-known outperforms";
const MACHINE: &str = "across because including yet yields remains substantially suggest evaluate further each must delve delves intricate pivotal showcasing underscore robust landscape notably comprehensive leverages crucial enhancing seamlessly multifaceted fostering realm nuanced";

fn topic(n: usize) -> String {
    let mut letters = Vec::new();
    let mut n = n + 26;
    while n > 0 {
        letters.push(char::from(b'a' + (n % 26) as u8));
        n /= 26;
    }
    format!("tp{}", letters.iter().rev().collect::<String>())
}

fn tokens(rng: &mut Rng, own: &[&str], other: &[&str], subject: &str, short: bool) -> Vec<String> {
    let common: Vec<&str> = COMMON.split(' ').collect();
    let length = if short { 30 } else { 60 + rng.below(45) };
    let mut words = Vec::new();
    for _ in 0..length {
        let r = rng.below(100);
        let mut word = if r < 50 {
            let (a, b) = (rng.below(common.len()), rng.below(common.len()));
            common[a.min(b)].to_owned()
        } else if r < 85 {
            own[rng.below(own.len())].to_owned()
        } else if r < 93 {
            other[rng.below(other.len())].to_owned()
        } else {
            subject.to_owned()
        };
        if rng.below(11) == 0 {
            word.push(',');
        } else if rng.below(17) == 0 {
            word.push('.');
        }
        words.push(word);
    }
    let mut first = words[0].chars();
    let capital: String = first.next().map(|c| c.to_uppercase().chain(first).collect()).unwrap_or_default();
    words[0] = capital;
    words
}

fn polish(rng: &mut Rng, words: &[String], machine: &[&str]) -> Vec<String> {
    words
        .iter()
        .map(|word| if rng.below(100) < 30 { machine[rng.below(machine.len())].to_owned() } else { word.clone() })
        .collect()
}

/// The corpus, as the Python's `training.json`: see the module.
pub(super) fn corpus() -> Value {
    let (human, machine): (Vec<&str>, Vec<&str>) = (HUMAN.split(' ').collect(), MACHINE.split(' ').collect());
    let mut rng = Rng(20_190_601);
    let mut pool = Vec::new();
    let mut words_of = Vec::new();
    for index in 0..400 {
        let id = format!("1906.{:05}", 10_000 + 7 * index);
        let words = tokens(&mut rng, &human, &machine, &topic(index), index % 20 == 13);
        pool.push(json!({"id": id, "title": format!("Title {}", topic(index)), "abstract": words.join(" ")}));
        words_of.push(words);
    }
    let held: Vec<usize> =
        (0..400).filter(|index| side(pool[*index]["id"].as_str().unwrap_or("")) != Side::Train).collect();
    let free: Vec<usize> =
        (0..400).filter(|index| side(pool[*index]["id"].as_str().unwrap_or("")) == Side::Train).collect();
    let generators = ["claude-sonnet-5-5", "claude-haiku-5-5"];
    let mut mirrors = Vec::new();
    for (position, &index) in held.iter().take(30).chain(free.iter().take(150)).enumerate() {
        let id = pool[index]["id"].clone();
        let subject = topic(index);
        let written = tokens(&mut rng, &machine, &human, &subject, false).join(" ");
        let polished = polish(&mut rng, &words_of[index], &machine).join(" ");
        for (kind, text) in [("written", written), ("polished", polished)] {
            let mut entry = json!({"id": id, "kind": kind, "generator": generators[position % 2], "text": text});
            if position == 3 && kind == "written" {
                entry.as_object_mut().map(|entry| entry.remove("generator"));
            }
            mirrors.push(entry);
        }
    }
    let train: Vec<Value> = free.iter().take(150).map(|index| pool[*index]["id"].clone()).collect();
    let rounds =
        json!([{"round": 0, "humans": 100, "threshold": 1.0, "written_caught": 0.5, "polished_caught": 0.5}, 125, 150]);
    json!({"pool": pool, "machine": mirrors, "train": train, "rounds": rounds})
}

fn close(ours: f64, theirs: f64, tolerance: f64, what: &str) {
    assert!((ours - theirs).abs() <= tolerance, "{what}: {ours} against {theirs} (tolerance {tolerance})");
}

#[test]
#[allow(clippy::unwrap_used)]
fn the_detector_trained_on_the_pythons_files_matches_the_pythons_numbers() {
    let python: Value = serde_json::from_str(include_str!("fixtures/python.json")).unwrap();
    let folder = tempfile::tempdir().unwrap();
    let detection = folder.path().join("ai_detection");
    fs::create_dir_all(&detection).unwrap();
    fs::write(detection.join("training.json"), corpus().to_string()).unwrap();
    let library = Library::open(folder.path()).unwrap();
    library.import_detector(folder.path()).unwrap();
    let trained = model(&library).unwrap().unwrap();
    let (classifier, report) = (&trained.classifier, &trained.report);

    // the same corpus: the held-out texts the Python sampled are those generated here
    let sample: Vec<&str> = python["sample"].as_array().unwrap().iter().map(|text| text.as_str().unwrap()).collect();
    let pool = library.detector_humans(crate::library::detector::POOL).unwrap();
    let test: Vec<&str> =
        pool.iter().filter(|human| side(&human.id) == Side::Test).map(|human| human.r#abstract.as_str()).collect();
    assert_eq!(&test[..6], &sample[..6]);

    // the features
    assert_eq!(classifier.vectorizer.len(), python["vocabulary"].as_u64().unwrap() as usize);
    let terms = &classifier.vectorizer.terms;
    assert_eq!(json!(terms[..5]), python["first_terms"]);
    assert_eq!(json!(terms[terms.len() - 5..]), python["last_terms"]);
    for (term, idf) in python["idf"].as_object().unwrap() {
        let column = classifier.vectorizer.column(term).unwrap() as usize;
        close(classifier.vectorizer.idf[column], idf.as_f64().unwrap(), 1e-12, term);
    }
    assert_eq!(classifier.corpus, python["version"].as_str().unwrap());

    // the regression, against the Python as shipped and against the exact minimum
    for (expected, tolerance) in [(&python["as_shipped"], 3e-3), (&python["optimum"], 1e-6)] {
        close(classifier.intercept, expected["intercept"].as_f64().unwrap(), tolerance, "intercept");
        for (term, weight) in expected["coefficients"].as_object().unwrap() {
            let column = classifier.vectorizer.column(term).unwrap() as usize;
            close(classifier.weights[column], weight.as_f64().unwrap(), tolerance, term);
        }
        for (text, score) in sample.iter().zip(expected["scores"].as_array().unwrap()) {
            match (classifier.score(text), score.as_f64()) {
                (Some(ours), Some(theirs)) => close(ours, theirs, tolerance, "score"),
                (ours, theirs) => assert_eq!(ours, theirs, "too short to judge on both sides"),
            }
        }
    }
    close(classifier.threshold, python["optimum"]["measured"]["threshold"].as_f64().unwrap(), 1e-6, "threshold");

    // what was measured, as the Python reported it
    let theirs = &python["as_shipped"]["report"];
    close(classifier.threshold, theirs["threshold"].as_f64().unwrap(), 1e-3, "threshold as shown");
    assert_eq!(json!(report.measured.written_caught), theirs["written_caught"]);
    assert_eq!(json!(report.measured.polished_caught), theirs["polished_caught"]);
    close(report.measured.auc.unwrap(), theirs["auc"].as_f64().unwrap(), 1e-12, "auc");
    assert_eq!(
        (json!(report.trained_on), json!(report.held_out)),
        (theirs["trained_on"].clone(), theirs["held_out"].clone())
    );
    assert_eq!(json!(report.false_alarms), theirs["false_alarms"]);
    assert_eq!(serde_json::to_value(&report.rounds).unwrap(), theirs["rounds"]);
    assert_eq!(json!(report.machine_words), theirs["machine_words"]);
    assert_eq!(json!(report.human_words), theirs["human_words"]);
    let markers: Vec<Vec<String>> =
        sample[..3].iter().chain(&sample[6..8]).map(|text| classifier.markers(text, super::MARKERS)).collect();
    assert_eq!(json!(markers), python["as_shipped"]["markers"]);

    // which models wrote what it learnt: told, not assumed (the one mirror without a model is a held-out paper's)
    let written: Vec<(Option<&str>, &str, usize)> =
        report.written_by.iter().map(|by| (by.provider.as_deref(), by.model.as_str(), by.mirrors)).collect();
    assert_eq!(written, [(Some("anthropic"), "claude-haiku-5-5", 150), (Some("anthropic"), "claude-sonnet-5-5", 150)]);
    let unknown = library.detector_mirrors().unwrap().into_iter().filter(|mirror| mirror.generator.provider.is_none());
    assert_eq!(unknown.map(|mirror| mirror.generator.model).collect::<Vec<_>>(), ["unknown"]);
}

/// The same check on a user's real training set, run by hand and never in CI: `CONSTELLATE_DETECTOR_FOLDER` names
/// a library folder with the Python's `ai_detection/training.json`, and `CONSTELLATE_DETECTOR_EXPECTED` a JSON file
/// of what the Python computed from that same file at its exact minimum (`threshold`, `written_caught`,
/// `polished_caught`, `auc`: its `_measure` on the test side, fitted with `tol=1e-12`). Not `validation.json`: that
/// is written only when the Python's report is run, and may be of an older training set. Nothing of it is kept.
#[test]
#[ignore = "reads a real library folder named by CONSTELLATE_DETECTOR_FOLDER"]
#[allow(clippy::unwrap_used)]
fn the_detector_matches_the_pythons_report_on_a_real_training_set() {
    let (Ok(folder), Ok(expected)) =
        (std::env::var("CONSTELLATE_DETECTOR_FOLDER"), std::env::var("CONSTELLATE_DETECTOR_EXPECTED"))
    else {
        return;
    };
    let folder = std::path::PathBuf::from(folder);
    let validation: Value = serde_json::from_str(&fs::read_to_string(expected).unwrap()).unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let library = Library::open(scratch.path()).unwrap();
    library.import_detector(&folder).unwrap();
    let started = std::time::Instant::now();
    let trained = model(&library).unwrap().unwrap();
    eprintln!("fitted with every round in {:?}", started.elapsed());
    let report = &trained.report;
    eprintln!(
        "threshold {} (Python {}), written {:?} ({}), polished {:?} ({}), auc {:?} ({})",
        trained.classifier.threshold,
        validation["threshold"],
        report.measured.written_caught,
        validation["written_caught"],
        report.measured.polished_caught,
        validation["polished_caught"],
        report.measured.auc,
        validation["auc"]
    );
    close(trained.classifier.threshold, validation["threshold"].as_f64().unwrap(), 1e-6, "threshold");
    close(report.measured.written_caught.unwrap(), validation["written_caught"].as_f64().unwrap(), 1e-12, "written");
    close(report.measured.polished_caught.unwrap(), validation["polished_caught"].as_f64().unwrap(), 1e-12, "polished");
    close(report.measured.auc.unwrap(), validation["auc"].as_f64().unwrap(), 1e-9, "auc");

    // the time a run of realistic size takes: the folder's own papers (up to 12,000), scored and kept
    let Ok(lines) = fs::read_to_string(folder.join("papers.jsonl")) else { return };
    let papers: Vec<crate::library::Paper> = lines
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .take(12_000)
        .enumerate()
        .map(|(index, record)| {
            let mut paper = crate::library::papers::tests::paper(&format!("p{index}"));
            paper.title = record["title"].as_str().unwrap_or("").to_owned();
            paper.r#abstract = record["abstract"].as_str().unwrap_or("").to_owned();
            paper
        })
        .collect();
    library.add_papers(&papers, None, crate::library::Known::Keep).unwrap();
    let ids: Vec<String> = papers.iter().map(|paper| paper.id.clone()).collect();
    let (log, progress, cancel) = (|_: &str| {}, |_: crate::jobs::Progress| {}, crate::jobs::Cancel::new());
    let tracker = crate::jobs::Tracker::new(&log, &progress, &cancel);
    let started = std::time::Instant::now();
    let scored = super::score_run(&library, &crate::settings::Settings::default(), &ids, &tracker);
    let first = started.elapsed();
    let started = std::time::Instant::now();
    let again = super::score_run(&library, &crate::settings::Settings::default(), &ids, &tracker);
    eprintln!(
        "{} papers on {} threads: scored and kept in {first:?} ({:?}); again, all kept, in {:?} ({:?})",
        ids.len(),
        crate::threads::computing(),
        scored.outcome,
        started.elapsed(),
        again.outcome
    );
}
