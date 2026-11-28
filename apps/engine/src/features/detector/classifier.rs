//! The classifier: a logistic regression on word and word-pair weights ([`super::features`],
//! [`super::logistic`]), its threshold, and what is measured about it on papers it was never trained on.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use super::features::{FEATURES, Vectorizer};
use super::logistic::{self, TRAINING};
use super::{FALSE_ALARMS, MIN_WORDS};
use crate::features::detector::store::{Built, Generator, Human, Kind, Mirror, YearFlagged};

/// Which part of the pool a paper belongs to, decided by its id so that a paper and its mirrors always go together,
/// as the Python decided it (`int(sha1(id).hexdigest(), 16) % 10`: 0 validation, 5 test, the rest train).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// 80%: may be trained on and mined.
    Train,
    /// 10%: never trained on; decides whether a round of mining helped.
    Validation,
    /// 10%: never trained on nor used for any decision; everything reported comes from here.
    Test,
}

pub fn side(paper_id: &str) -> Side {
    // the hash as one big number, taken modulo 10 a byte at a time
    let digest = Sha1::digest(paper_id.as_bytes());
    let bucket = digest.iter().fold(0u32, |rest, byte| (rest * 256 + u32::from(*byte)) % 10);
    match bucket {
        0 => Side::Validation,
        5 => Side::Test,
        _ => Side::Train,
    }
}

pub fn held_out(paper_id: &str) -> bool {
    side(paper_id) != Side::Train
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What the Python called the classifier's version: the first ten hex digits of the SHA-1 of the sorted SHA-1s of
/// its training texts joined by `|`. It says which texts it learnt from, whatever their order.
pub fn corpus_version(texts: &[&str]) -> String {
    let mut hashes: Vec<String> = texts.iter().map(|text| hex(&Sha1::digest(text.as_bytes()))).collect();
    hashes.sort_unstable();
    hex(&Sha1::digest(hashes.join("|").as_bytes()))[..10].to_owned()
}

/// Numbers kept exactly: as their bits. JSON's decimal form is read back to the nearest double only when the parser
/// rounds correctly, which serde_json does not promise; a classifier read back a bit off would score a paper at its
/// threshold on the wrong side of it.
pub(super) mod exact {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(values: &[f64], serializer: S) -> Result<S::Ok, S::Error> {
        values.iter().map(|value| value.to_bits()).collect::<Vec<u64>>().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<f64>, D::Error> {
        Ok(Vec::<u64>::deserialize(deserializer)?.into_iter().map(f64::from_bits).collect())
    }

    pub mod one {
        use serde::{Deserialize, Deserializer, Serialize, Serializer};

        pub fn serialize<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
            value.to_bits().serialize(serializer)
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
            Ok(f64::from_bits(u64::deserialize(deserializer)?))
        }
    }
}

/// A fitted classifier: what scores a text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Classifier {
    pub vectorizer: Vectorizer,
    #[serde(with = "exact")]
    pub weights: Vec<f64>,
    #[serde(with = "exact::one")]
    pub intercept: f64,
    /// Scores above it are flagged: where [`FALSE_ALARMS`] of held-out human abstracts score higher. Infinite
    /// until measured.
    #[serde(with = "exact::one")]
    pub threshold: f64,
    /// Which texts it learnt from ([`corpus_version`]).
    pub corpus: String,
    pub features: u32,
    pub training: u32,
}

impl Classifier {
    /// Fitted on every one of `humans` and `machine` (texts).
    pub fn fit(humans: &[&str], machine: &[&str]) -> Self {
        let texts: Vec<&str> = humans.iter().chain(machine).copied().collect();
        let (vectorizer, rows) = Vectorizer::fit(&texts);
        let labels: Vec<bool> = (0..texts.len()).map(|index| index >= humans.len()).collect();
        let fitted = logistic::fit(&rows, &labels, vectorizer.len());
        Self {
            vectorizer,
            weights: fitted.weights,
            intercept: fitted.intercept,
            threshold: f64::INFINITY,
            corpus: corpus_version(&texts),
            features: FEATURES,
            training: TRAINING,
        }
    }

    /// Ready to score after being read back.
    pub fn ready(self) -> Self {
        Self { vectorizer: self.vectorizer.ready(), ..self }
    }

    /// The key a paper's score is kept under: this classifier (its texts and the code that fitted it) and the
    /// features code that scores with it now. A score kept under another key is computed again.
    pub fn key(&self) -> String {
        format!("{}-f{}t{}-s{}", self.corpus, self.features, self.training, FEATURES)
    }

    /// Log-odds of "machine" (higher: more machine-like); None for a text of fewer than [`MIN_WORDS`] words.
    pub fn score(&self, text: &str) -> Option<f64> {
        if text.split_whitespace().count() < MIN_WORDS {
            return None;
        }
        Some(self.raw(text))
    }

    fn raw(&self, text: &str) -> f64 {
        let row = self.vectorizer.transform(text);
        self.intercept + row.iter().map(|&(column, weight)| self.weights[column as usize] * weight).sum::<f64>()
    }

    /// The words and word pairs of `text` that raised its score the most (at most `top`, only those that raised
    /// it).
    pub fn markers(&self, text: &str, top: usize) -> Vec<String> {
        let mut raised: Vec<(f64, &str)> = self
            .vectorizer
            .transform(text)
            .into_iter()
            .map(|(column, weight)| {
                (weight * self.weights[column as usize], self.vectorizer.terms[column as usize].as_str())
            })
            .filter(|(raise, _)| *raise > 0.0)
            .collect();
        raised.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        raised.into_iter().take(top).map(|(_, term)| term.to_owned()).collect()
    }

    /// The `count` features that point most to a model (`machine`) or to a person.
    pub fn telling(&self, count: usize, machine: bool) -> Vec<String> {
        let mut order: Vec<usize> = (0..self.weights.len()).collect();
        let sign = if machine { -1.0 } else { 1.0 };
        order.sort_by(|a, b| (sign * self.weights[*a]).total_cmp(&(sign * self.weights[*b])).then(a.cmp(b)));
        order.into_iter().take(count).map(|column| self.vectorizer.terms[column].clone()).collect()
    }
}

/// What a classifier does on held-out texts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Measured {
    /// Where [`FALSE_ALARMS`] of the human abstracts score higher.
    pub threshold: f64,
    /// Shares of held-out mirrors above it (a mirror too short to judge counts as not caught); None with none.
    pub written_caught: Option<f64>,
    pub polished_caught: Option<f64>,
    /// The area under the ROC curve, human against every mirror; None when one side is empty.
    pub auc: Option<f64>,
}

/// numpy's `quantile` with its default ("linear") method, of values sorted ascending.
fn quantile(sorted: &[f64], q: f64) -> f64 {
    let at = q * (sorted.len() - 1) as f64;
    let below = at.floor();
    let lower = below as usize;
    let upper = (lower + 1).min(sorted.len() - 1);
    let t = at - below;
    let (a, b) = (sorted[lower], sorted[upper]);
    // numpy's lerp, which takes the nearer end
    if t >= 0.5 { b - (b - a) * (1.0 - t) } else { a + (b - a) * t }
}

/// The area under the ROC curve: the chance that a positive scores above a negative, ties counting half.
fn auc(negatives: &[f64], positives: &[f64]) -> Option<f64> {
    if negatives.is_empty() || positives.is_empty() {
        return None;
    }
    let mut all: Vec<(f64, bool)> =
        negatives.iter().map(|&score| (score, false)).chain(positives.iter().map(|&score| (score, true))).collect();
    all.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut ranks = 0.0;
    let mut start = 0;
    while start < all.len() {
        let mut end = start;
        while end < all.len() && all[end].0 == all[start].0 {
            end += 1;
        }
        let average = (start + end + 1) as f64 / 2.0; // ranks from 1
        ranks += average * all[start..end].iter().filter(|(_, positive)| *positive).count() as f64;
        start = end;
    }
    let (p, n) = (positives.len() as f64, negatives.len() as f64);
    Some((ranks - p * (p + 1.0) / 2.0) / (p * n))
}

/// The held-out texts of one side: human abstracts, and the mirrors of their papers.
pub struct Held<'a> {
    pub humans: Vec<&'a str>,
    pub mirrors: Vec<(Kind, &'a str)>,
}

pub fn held<'a>(pool: &'a [Human], mirrors: &'a [Mirror], wanted: Side) -> Held<'a> {
    let ids: HashSet<&str> =
        pool.iter().filter(|human| side(&human.id) == wanted).map(|human| human.id.as_str()).collect();
    Held {
        humans: pool
            .iter()
            .filter(|human| ids.contains(human.id.as_str()))
            .map(|human| human.r#abstract.as_str())
            .collect(),
        mirrors: mirrors
            .iter()
            .filter(|mirror| ids.contains(mirror.id.as_str()))
            .map(|mirror| (mirror.kind, mirror.text.as_str()))
            .collect(),
    }
}

/// What `model` does on `held`, as the Python's `_measure`. None when no held-out human abstract can be judged.
pub fn measure(model: &Classifier, held: &Held<'_>) -> Option<Measured> {
    let mut human: Vec<f64> = held.humans.iter().filter_map(|text| model.score(text)).collect();
    if human.is_empty() {
        return None;
    }
    human.sort_by(f64::total_cmp);
    let threshold = quantile(&human, 1.0 - FALSE_ALARMS);
    let of = |kind: Kind| -> Vec<Option<f64>> {
        held.mirrors.iter().filter(|(of, _)| *of == kind).map(|(_, text)| model.score(text)).collect()
    };
    let caught = |scores: &[Option<f64>]| {
        (!scores.is_empty()).then(|| {
            scores.iter().filter(|score| score.is_some_and(|score| score > threshold)).count() as f64
                / scores.len() as f64
        })
    };
    let (written, polished) = (of(Kind::Written), of(Kind::Polished));
    // a mirror too short to judge counts as the lowest human score, as the Python had it
    let lowest = human[0];
    let everything: Vec<f64> = written.iter().chain(&polished).map(|score| score.unwrap_or(lowest)).collect();
    Some(Measured {
        threshold,
        written_caught: caught(&written),
        polished_caught: caught(&polished),
        auc: auc(&human, &everything),
    })
}

/// The texts of the training set restricted to `train` (pool paper ids): the human abstracts in the pool's order,
/// and the mirrors of those papers in the order they were kept.
pub fn training_texts<'a>(pool: &'a [Human], mirrors: &'a [Mirror], train: &[String]) -> (Vec<&'a str>, Vec<&'a str>) {
    let wanted: HashSet<&str> = train.iter().map(String::as_str).collect();
    let humans = pool.iter().filter(|human| wanted.contains(human.id.as_str())).map(|human| human.r#abstract.as_str());
    let machine =
        mirrors.iter().filter(|mirror| wanted.contains(mirror.id.as_str())).map(|mirror| mirror.text.as_str());
    (humans.collect(), machine.collect())
}

pub fn fit_on(pool: &[Human], mirrors: &[Mirror], train: &[String]) -> Classifier {
    let (humans, machine) = training_texts(pool, mirrors, train);
    Classifier::fit(&humans, &machine)
}

/// What one round of mining bought: the catch rates with only the papers that were in by then.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Round {
    pub round: usize,
    pub humans: usize,
    pub written_caught: Option<f64>,
    pub polished_caught: Option<f64>,
}

/// How many of the mirrors trained on one model wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Written {
    /// A provider's id; none when it was not recorded.
    pub provider: Option<String>,
    pub model: String,
    pub mirrors: usize,
}

/// What was measured about the classifier on held-out papers (all of it from the "test" side).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// The models whose text it learnt, with how many of its mirrors each wrote.
    pub written_by: Vec<Written>,
    /// Human abstracts trained on (each with its mirrors).
    pub trained_on: usize,
    /// Held-out human abstracts it was measured on.
    pub held_out: usize,
    pub false_alarms: f64,
    pub measured: Measured,
    pub rounds: Vec<Round>,
    pub machine_words: Vec<String>,
    pub human_words: Vec<String>,
    /// The share flagged in one week of arXiv each year, when the year check was run.
    pub by_year: Option<BTreeMap<String, YearFlagged>>,
}

/// The classifier of a built training set and its report, as the Python's `classifier()` made them: fitted on the
/// training papers, its threshold set on the test side, each round measured again with the papers in by then.
pub fn trained(pool: &[Human], mirrors: &[Mirror], built: &Built) -> Option<(Classifier, Report)> {
    let test = held(pool, mirrors, Side::Test);
    let mut model = fit_on(pool, mirrors, &built.train);
    let measured = measure(&model, &test)?;
    model.threshold = measured.threshold;
    let rounds = built
        .rounds
        .iter()
        .enumerate()
        .map(|(round, &humans)| {
            let earlier = fit_on(pool, mirrors, &built.train[..humans.min(built.train.len())]);
            let measured = measure(&earlier, &test);
            Round {
                round,
                humans,
                written_caught: measured.as_ref().and_then(|measured| measured.written_caught),
                polished_caught: measured.as_ref().and_then(|measured| measured.polished_caught),
            }
        })
        .collect();
    let wanted: HashSet<&str> = built.train.iter().map(String::as_str).collect();
    let mut by: BTreeMap<&Generator, usize> = BTreeMap::new();
    for mirror in mirrors.iter().filter(|mirror| wanted.contains(mirror.id.as_str())) {
        *by.entry(&mirror.generator).or_default() += 1;
    }
    let mut written_by: Vec<Written> = by
        .into_iter()
        .map(|(generator, mirrors)| Written {
            provider: generator.provider.map(|provider| provider.id().to_owned()),
            model: generator.model.clone(),
            mirrors,
        })
        .collect();
    written_by.sort_by(|a, b| b.mirrors.cmp(&a.mirrors).then_with(|| a.model.cmp(&b.model)));
    let report = Report {
        written_by,
        trained_on: built.train.len(),
        held_out: test.humans.len(),
        false_alarms: FALSE_ALARMS,
        measured,
        rounds,
        machine_words: model.telling(25, true),
        human_words: model.telling(25, false),
        by_year: None,
    };
    Some((model, report))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const HUMAN: &str =
        "we study a model and show results on data with a simple method and report error rates for several tasks ";
    const MACHINE: &str = "this paper delves into an intricate landscape showcasing pivotal insights that underscore robust novel advances ";

    /// The Python test's texts: one style six times over, and a word of its own.
    fn text(base: &str, n: usize) -> String {
        let mut words: Vec<String> = base.repeat(6).split_whitespace().map(str::to_owned).collect();
        words.push(format!("topic{n}"));
        words.join(" ")
    }

    fn as_str(texts: &[String]) -> Vec<&str> {
        texts.iter().map(String::as_str).collect()
    }

    #[test]
    fn the_classifier_separates_wording_and_names_the_words() {
        let humans: Vec<String> = (0..30).map(|n| text(HUMAN, n)).collect();
        let machine: Vec<String> = (0..30).map(|n| text(MACHINE, n)).collect();
        let model = Classifier::fit(&as_str(&humans), &as_str(&machine));
        let (human, machine_score) = (model.score(&text(HUMAN, 99)).unwrap(), model.score(&text(MACHINE, 99)).unwrap());
        assert!(machine_score > human);
        assert_eq!(model.score("too short to judge"), None);
        let markers = model.markers(&text(MACHINE, 99), 6);
        assert!(
            markers.iter().any(|marker| ["delves", "intricate", "pivotal"].contains(&marker.as_str())),
            "{markers:?}"
        );
        assert!(!model.markers(&text(HUMAN, 99), 6).iter().any(|marker| marker == "delves" || marker == "pivotal"));
        assert_eq!(
            model.corpus,
            Classifier::fit(&as_str(&humans), &as_str(&machine)).corpus,
            "the same texts, the same version"
        );
    }

    #[test]
    fn held_out_papers_are_chosen_by_id_and_are_about_a_fifth_as_the_python_chose_them() {
        let ids: Vec<String> = (0..2000).map(|n| format!("19{:02}.{n:05}", n % 100)).collect();
        let share = ids.iter().filter(|id| held_out(id)).count() as f64 / 2000.0;
        assert!(0.15 < share && share < 0.25, "{share}");
        // int(hashlib.sha1(id.encode()).hexdigest(), 16) % 10, worked out by the Python: 4, 8, 2, 6, 5
        let python = [
            ("1906.00001", Side::Train),
            ("1906.10000", Side::Train),
            ("a", Side::Train),
            ("1906.10035", Side::Train),
            ("1906.10014", Side::Test),
        ];
        for (id, expected) in python {
            assert_eq!(side(id), expected, "{id}");
        }
        assert_eq!(side("1906.00001"), side("1906.00001"));
    }

    #[test]
    fn the_corpus_version_is_the_pythons_sha1_of_sorted_sha1s_whatever_the_order() {
        // the first ten hex digits of sha1("|".join(sorted(sha1(text) for text in texts))), worked out by hashlib
        assert_eq!(corpus_version(&["abc"]), "9ef2bdeea2");
        assert_eq!(corpus_version(&["a human abstract", "a machine abstract"]), "fba6b2c787");
        assert_eq!(corpus_version(&["a machine abstract", "a human abstract"]), "fba6b2c787");
        // UTF-8 bytes are hashed, not characters
        assert_eq!(corpus_version(&["café", "naïve"]), "b892f4faec");
        // no texts: the SHA-1 of nothing
        assert_eq!(corpus_version(&[]), "da39a3ee5e");
    }

    #[test]
    fn the_quantile_and_the_auc_are_numpys_and_scikit_learns() {
        let sorted = [1.0, 2.0, 3.0, 4.0, 10.0];
        assert_eq!(quantile(&sorted, 0.5), 3.0);
        assert!((quantile(&sorted, 0.99) - 9.76).abs() < 1e-12); // numpy.quantile([1, 2, 3, 4, 10], 0.99)
        assert_eq!(quantile(&[7.0], 0.99), 7.0);
        assert_eq!(auc(&[0.0, 1.0], &[2.0, 3.0]), Some(1.0));
        assert_eq!(auc(&[1.0, 1.0], &[1.0]), Some(0.5), "ties count half");
        assert_eq!(auc(&[0.0, 2.0], &[1.0]), Some(0.5));
        assert_eq!(auc(&[], &[1.0]), None);
    }
}
