//! What the language model answered for a feature, kept so that no answer is paid for twice.
//!
//! Each feature keeps the model's raw answer (its [`crate::evidence::Claim`]s, never `Cited`: evidence is not
//! read back from JSON, so a kept answer is checked again when it is loaded, which is cheap once the papers'
//! passages are kept). An answer is kept under its feature and a key ([`AnswerKey::new`]) made from everything it
//! depends on, so that asking the same thing again finds it and asking something different does not. With it are
//! kept when it was written, the model that wrote it, the feature's prompt version and the passages reader's
//! version.
//!
//! A redo button clears one feature's answers about one thing ([`Library::forget`]): a paper, a run, a community.
//! That is the answer's [`Subject`], given when it is kept; another feature's answers about the same thing stay.

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use super::{Library, LibraryError, now};
use crate::model::language::{Provider, price_key};
use crate::text::passages::READER;

/// The features that keep answers. A variant's name is what is stored ([`Feature::name`]): it never changes once
/// released, or the answers kept under it are lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Feature {
    /// A table of what each of a community's papers says (dataset, method, result, ...), one row per paper.
    Extraction,
    /// What each citation in a paper does: builds on, compares with, disputes, ...
    CitationKinds,
    /// Which papers agree and which disagree about a claim.
    Stances,
    /// Questions about papers, answered with citations.
    Answers,
    /// A critique of a paper beside its reviewers' comments.
    Critique,
    /// "Explain this passage".
    Explanation,
    /// A paper's claims as a tree, with its reviews mapped onto them.
    ClaimTree,
}

impl Feature {
    pub const ALL: [Feature; 7] = [
        Feature::Extraction,
        Feature::CitationKinds,
        Feature::Stances,
        Feature::Answers,
        Feature::Critique,
        Feature::Explanation,
        Feature::ClaimTree,
    ];

    /// As stored.
    pub const fn name(self) -> &'static str {
        match self {
            Feature::Extraction => "extraction",
            Feature::CitationKinds => "citation_kinds",
            Feature::Stances => "stances",
            Feature::Answers => "answers",
            Feature::Critique => "critique",
            Feature::Explanation => "explanation",
            Feature::ClaimTree => "claim_tree",
        }
    }
}

/// A feature as the interface is told it: by its stored name ("citation_kinds").
impl Serialize for Feature {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

/// What an answer is about, which is what a redo clears it by. Told to the interface as `{"paper": id}`,
/// `{"run": name}` or `{"community": {"run", "community"}}` (a finished job's subject).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    Paper(String),
    Run(String),
    /// A community of a run, by its tree path ("c0.2").
    Community {
        run: String,
        community: String,
    },
}

impl Subject {
    /// As stored: `paper:<id>`, `run:<name>`, `community:<run>/<tree path>`. A run's name never has a `/`
    /// (`api::routes::one_name`), so these cannot be mistaken for one another.
    fn stored(&self) -> String {
        match self {
            Subject::Paper(id) => format!("paper:{id}"),
            Subject::Run(run) => format!("run:{run}"),
            Subject::Community { run, community } => format!("community:{run}/{community}"),
        }
    }
}

/// The key an answer is kept under. Made only by [`AnswerKey::new`], so every feature makes it the same way.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AnswerKey(String);

impl AnswerKey {
    /// The key of an answer of `feature`, written by `model` of `provider` with version `prompt` of the feature's
    /// prompt, about `papers`, to `question`, read with this version of the passages reader ([`READER`]).
    ///
    /// `question` is everything else the answer depends on, as the feature writes it down: the question asked,
    /// the columns of a table (as JSON, in their order), the passage to explain (its address). Whatever it puts in
    /// the prompt that is not the papers' text belongs here. The papers are a set: their order and repeats do not
    /// change the key.
    ///
    /// The key is the SHA-256, in lower-case hex, of these fields in this order, each written as its length in
    /// bytes (eight bytes, little-endian) and then its UTF-8 bytes: `constellate answer key 1`, the feature's
    /// name, the model as its price is kept ([`price_key`]: a Claude API model by its own id, any other provider's
    /// as `provider:id`), the prompt version and the reader version (in decimal), the number of distinct papers
    /// (in decimal), each paper id in byte order, and the question. The lengths keep fields from running into one
    /// another ("ab" + "c" is not "a" + "bc"). Changing any of this changes every key: answers kept before would
    /// no longer be found (and would be asked for again), so the first field's number is raised with it.
    ///
    /// The provider is part of the model's field rather than a field of its own so that the keys of the Claude
    /// API's answers are byte for byte those made before there were other providers: nothing kept is orphaned.
    /// Two providers serving one model id (OpenRouter and a gateway) keep their answers apart, since the models
    /// behind the id may differ.
    ///
    /// A paper's PDF is not in the key: a paper whose PDF is replaced keeps its answers, whose quotes are checked
    /// again against the new text when they are loaded (those no longer found are dropped). A feature that must
    /// ask again then forgets the paper's answers ([`Library::forget`]).
    pub fn new(
        feature: Feature,
        provider: Provider,
        model: &str,
        prompt: u32,
        papers: &[&str],
        question: &str,
    ) -> Self {
        Self::hashed(feature, &price_key(provider, model), prompt, papers, question)
    }

    /// The key of what a feature keeps of its own, written by no model (the list of a community's questions):
    /// the key of [`AnswerKey::new`] with an empty model, as such keys always were.
    pub fn of_feature(feature: Feature, version: u32, papers: &[&str], question: &str) -> Self {
        Self::hashed(feature, "", version, papers, question)
    }

    fn hashed(feature: Feature, model: &str, prompt: u32, papers: &[&str], question: &str) -> Self {
        let mut papers = papers.to_vec();
        papers.sort_unstable();
        papers.dedup();
        let mut hash = Sha256::new();
        let mut field = |text: &str| {
            hash.update((text.len() as u64).to_le_bytes());
            hash.update(text.as_bytes());
        };
        field("constellate answer key 1");
        field(feature.name());
        field(model);
        field(&prompt.to_string());
        field(&READER.to_string());
        field(&papers.len().to_string());
        for paper in &papers {
            field(paper);
        }
        field(question);
        Self(hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where an answer is kept: its feature, its key, and what it is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub feature: Feature,
    pub key: AnswerKey,
    pub subject: Subject,
}

/// A kept answer, with what made it.
#[derive(Clone, Debug, PartialEq)]
pub struct Kept<T> {
    pub answer: T,
    /// When it was kept, UTC.
    pub kept_at: String,
    /// The model that wrote it, as the API named it (a fallback's name when one answered).
    pub model: String,
    /// The feature's prompt version it was asked with.
    pub prompt: u32,
    /// The passages reader's version when it was asked.
    pub reader: u32,
}

impl Library {
    /// The answer kept for `feature` under `key`. One that no longer reads as `T` (the feature changed its answer's
    /// shape without raising its prompt version) counts as not kept: it is asked for again and replaced.
    pub fn kept<T: DeserializeOwned>(
        &self,
        feature: Feature,
        key: &AnswerKey,
    ) -> Result<Option<Kept<T>>, LibraryError> {
        let row: Option<(String, String, String, i64, i64)> = self.read(|connection| {
            Ok(connection
                .prepare_cached(
                    "SELECT content, kept_at, model, prompt, reader FROM kept_answers
                     WHERE feature = ?1 AND answer_key = ?2",
                )?
                .query_row(params![feature.name(), key.as_str()], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
                })
                .optional()?)
        })?;
        Ok(row.and_then(|(content, kept_at, model, prompt, reader)| {
            Some(Kept {
                answer: serde_json::from_str(&content).ok()?,
                kept_at,
                model,
                prompt: u32::try_from(prompt).ok()?,
                reader: u32::try_from(reader).ok()?,
            })
        }))
    }

    /// Keep `answer`, written by `model` with version `prompt` of the feature's prompt, in `slot` (replacing what
    /// was kept there).
    pub fn keep<T: Serialize>(&self, slot: &Slot, model: &str, prompt: u32, answer: &T) -> Result<(), LibraryError> {
        let content = serde_json::to_string(answer)?;
        self.write(|transaction| {
            transaction
                .prepare_cached(
                    "INSERT INTO kept_answers (feature, answer_key, subject, model, prompt, reader, content, kept_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT (feature, answer_key) DO UPDATE SET
                         subject = excluded.subject, model = excluded.model, prompt = excluded.prompt,
                         reader = excluded.reader, content = excluded.content, kept_at = excluded.kept_at",
                )?
                .execute(params![
                    slot.feature.name(),
                    slot.key.as_str(),
                    slot.subject.stored(),
                    model,
                    prompt,
                    READER,
                    content,
                    now()
                ])?;
            Ok(())
        })
    }

    /// Forget the one answer `feature` kept under `key`: a redo of one answer, where its subject has others that
    /// stay (one question of a run's many, one passage's explanation of a paper's). It is asked for again. False
    /// when none was kept.
    pub fn forget_key(&self, feature: Feature, key: &AnswerKey) -> Result<bool, LibraryError> {
        self.write(|transaction| Ok(forget_key(transaction, feature, key.as_str())? > 0))
    }

    /// Forget every answer `feature` kept about any of `subjects` (a redo): they are asked for again. Answers of
    /// other features, and about other subjects, stay. Returns how many were forgotten.
    pub fn forget(&self, feature: Feature, subjects: &[Subject]) -> Result<usize, LibraryError> {
        self.write(|transaction| {
            let mut forget =
                transaction.prepare_cached("DELETE FROM kept_answers WHERE feature = ?1 AND subject = ?2")?;
            let mut forgotten = 0;
            for subject in subjects {
                forgotten += forget.execute(params![feature.name(), subject.stored()])?;
            }
            Ok(forgotten)
        })
    }
}

/// [`Library::forget_key`] in a transaction of the caller's, the key as stored (hex): how many rows went.
pub(super) fn forget_key(
    connection: &rusqlite::Connection,
    feature: Feature,
    key: &str,
) -> Result<usize, LibraryError> {
    Ok(connection
        .prepare_cached("DELETE FROM kept_answers WHERE feature = ?1 AND answer_key = ?2")?
        .execute(params![feature.name(), key])?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::evidence::{Claim, Quote};

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct Cell {
        column: String,
        value: String,
    }

    fn claims(value: &str) -> Vec<Claim<Cell>> {
        vec![Claim {
            statement: Cell { column: "dataset".to_owned(), value: value.to_owned() },
            quotes: vec![Quote { paper: "p".to_owned(), quote: "We evaluate on ImageNet and on CIFAR-10.".to_owned() }],
        }]
    }

    fn slot(feature: Feature, subject: Subject, question: &str) -> Slot {
        Slot {
            feature,
            key: AnswerKey::new(feature, Provider::Anthropic, "claude-opus-5-5", 1, &["p"], question),
            subject,
        }
    }

    #[test]
    fn a_kept_answer_is_handed_back_with_what_made_it_and_replaced_when_kept_again() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let slot = slot(Feature::Extraction, Subject::Paper("p".to_owned()), r#"["dataset"]"#);
        assert_eq!(library.kept::<Vec<Claim<Cell>>>(Feature::Extraction, &slot.key).unwrap(), None);
        library.keep(&slot, "claude-opus-5-5", 3, &claims("ImageNet")).unwrap();
        let kept = library.kept::<Vec<Claim<Cell>>>(Feature::Extraction, &slot.key).unwrap().unwrap();
        assert_eq!(kept.answer, claims("ImageNet"));
        assert_eq!((kept.model.as_str(), kept.prompt, kept.reader), ("claude-opus-5-5", 3, READER));
        assert!(kept.kept_at.ends_with('Z'));
        library.keep(&slot, "claude-opus-5-5", 3, &claims("CIFAR-10")).unwrap();
        let again = library.kept::<Vec<Claim<Cell>>>(Feature::Extraction, &slot.key).unwrap().unwrap();
        assert_eq!(again.answer, claims("CIFAR-10"));
        // the same key under another feature is another answer
        assert_eq!(library.kept::<Vec<Claim<Cell>>>(Feature::Answers, &slot.key).unwrap(), None);
        // an answer that no longer reads as what the feature expects is not kept, as far as it is concerned
        assert_eq!(library.kept::<Vec<u32>>(Feature::Extraction, &slot.key).unwrap(), None);
    }

    /// The key as it was made before there were providers other than the Claude API: written out again here, apart
    /// from the code that makes keys now, so that a change there that would orphan the answers kept for Claude fails.
    fn key_before_providers(feature: Feature, model: &str, prompt: u32, papers: &[&str], question: &str) -> String {
        let mut papers = papers.to_vec();
        papers.sort_unstable();
        papers.dedup();
        let mut hash = Sha256::new();
        let count = papers.len().to_string();
        let (prompt, reader) = (prompt.to_string(), READER.to_string());
        let fields = ["constellate answer key 1", feature.name(), model, &prompt, &reader, &count];
        for text in fields.into_iter().chain(papers).chain([question]) {
            hash.update((text.len() as u64).to_le_bytes());
            hash.update(text.as_bytes());
        }
        hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn the_claude_apis_keys_are_byte_for_byte_those_made_before_there_were_other_providers() {
        for feature in Feature::ALL {
            for model in ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-2"] {
                let now = AnswerKey::new(feature, Provider::Anthropic, model, 3, &["b", "a"], "a question");
                assert_eq!(now.as_str(), key_before_providers(feature, model, 3, &["a", "b"], "a question"));
            }
        }
        if READER == 1 {
            // as kept in libraries already: the key of one answer, written down
            let kept = AnswerKey::new(
                Feature::Answers,
                Provider::Anthropic,
                "claude-opus-5-5",
                1,
                &["a", "b"],
                "What is new?",
            );
            assert_eq!(kept.as_str(), "b03f1011ed6dc1dfbbaa131184f88fa329802546c1b0a7e5d502c54e28878644");
        }
        // what a feature keeps of its own (no model) is found where it was kept too
        let list = AnswerKey::of_feature(Feature::Stances, 1, &[], "questions");
        assert_eq!(list.as_str(), key_before_providers(Feature::Stances, "", 1, &[], "questions"));
    }

    #[test]
    fn one_model_id_served_by_two_providers_keeps_two_answers() {
        let by = |provider| AnswerKey::new(Feature::Critique, provider, "deepseek-v4-pro", 1, &["p"], "q");
        let keys =
            [by(Provider::DeepSeek), by(Provider::OpenRouter), by(Provider::Compatible), by(Provider::Anthropic)];
        for (index, key) in keys.iter().enumerate() {
            assert!(keys[index + 1..].iter().all(|other| other != key), "{index}");
        }
        assert_eq!(
            keys[0].as_str(),
            key_before_providers(Feature::Critique, "deepseek:deepseek-v4-pro", 1, &["p"], "q")
        );
    }

    #[test]
    fn a_redo_by_key_forgets_that_one_answer_only() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let paper = Subject::Paper("p".to_owned());
        let (first, second) =
            (slot(Feature::Explanation, paper.clone(), "p. 1"), slot(Feature::Explanation, paper, "p. 2"));
        for slot in [&first, &second] {
            library.keep(slot, "claude-opus-5-5", 1, &claims("x")).unwrap();
        }
        assert!(library.forget_key(Feature::Explanation, &first.key).unwrap());
        assert!(!library.forget_key(Feature::Explanation, &first.key).unwrap(), "nothing left to forget");
        assert!(
            !library.forget_key(Feature::Critique, &second.key).unwrap(),
            "another feature's key is another answer"
        );
        assert_eq!(library.kept::<Vec<Claim<Cell>>>(Feature::Explanation, &first.key).unwrap(), None);
        assert!(library.kept::<Vec<Claim<Cell>>>(Feature::Explanation, &second.key).unwrap().is_some());
    }

    #[test]
    fn a_key_changes_with_everything_the_answer_depends_on_and_not_with_the_order_of_the_papers() {
        let key = |feature, model, prompt, papers: &[&str], question| {
            AnswerKey::new(feature, Provider::Anthropic, model, prompt, papers, question)
        };
        let base = key(Feature::Answers, "claude-opus-5-5", 1, &["a", "b"], "What is new?");
        assert_eq!(base, key(Feature::Answers, "claude-opus-5-5", 1, &["b", "a", "b"], "What is new?"));
        for other in [
            key(Feature::Critique, "claude-opus-5-5", 1, &["a", "b"], "What is new?"),
            key(Feature::Answers, "claude-sonnet-5-5", 1, &["a", "b"], "What is new?"),
            key(Feature::Answers, "claude-opus-5-5", 2, &["a", "b"], "What is new?"),
            key(Feature::Answers, "claude-opus-5-5", 1, &["a"], "What is new?"),
            key(Feature::Answers, "claude-opus-5-5", 1, &["a", "b"], "What is old?"),
            key(Feature::Answers, "claude-opus-5-5", 1, &["ab"], ""),
            key(Feature::Answers, "claude-opus-5-5", 1, &["a"], "b"),
        ] {
            assert_ne!(base, other);
        }
        assert_ne!(
            key(Feature::Answers, "m", 1, &["ab", "c"], "q"),
            key(Feature::Answers, "m", 1, &["a", "bc"], "q"),
            "fields cannot run into one another"
        );
        // fixed, so that a key made today finds an answer kept yesterday
        assert_eq!(base.as_str().len(), 64);
        assert_eq!(
            base,
            AnswerKey::new(Feature::Answers, Provider::Anthropic, "claude-opus-5-5", 1, &["a", "b"], "What is new?")
        );
        assert_ne!(
            base,
            AnswerKey::new(Feature::Answers, Provider::OpenRouter, "claude-opus-5-5", 1, &["a", "b"], "What is new?")
        );
    }

    #[test]
    fn a_redo_forgets_one_features_answers_about_one_subject_and_nothing_else() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let paper = Subject::Paper("p".to_owned());
        let run = Subject::Run("r1".to_owned());
        let community = Subject::Community { run: "r1".to_owned(), community: "c0".to_owned() };
        let slots = [
            slot(Feature::Extraction, paper.clone(), "paper 1"),
            slot(Feature::Extraction, paper.clone(), "paper 2"),
            slot(Feature::Critique, paper.clone(), "paper 1"),
            slot(Feature::Extraction, run.clone(), "run"),
            slot(Feature::Extraction, community.clone(), "community"),
        ];
        for slot in &slots {
            library.keep(slot, "claude-opus-5-5", 1, &claims("x")).unwrap();
        }
        assert_eq!(library.forget(Feature::Extraction, std::slice::from_ref(&paper)).unwrap(), 2);
        let still = |slot: &Slot| library.kept::<Vec<Claim<Cell>>>(slot.feature, &slot.key).unwrap().is_some();
        assert_eq!(slots.iter().map(still).collect::<Vec<_>>(), [false, false, true, true, true]);
        assert_eq!(library.forget(Feature::Extraction, &[run, community]).unwrap(), 2);
        assert_eq!(library.forget(Feature::Extraction, &[paper]).unwrap(), 0);
        assert!(still(&slots[2]));
    }

    #[test]
    fn every_feature_has_a_name_of_its_own() {
        let mut names: Vec<&str> = Feature::ALL.iter().map(|feature| feature.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Feature::ALL.len());
    }
}
