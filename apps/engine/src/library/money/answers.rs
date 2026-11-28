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

use crate::library::{Library, LibraryError, now};
use crate::model::language::{Provider, price_key};
use crate::text::pdf::passages::READER;

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
    /// A community's name and description, written when a run is made ([`crate::features::writing::describe`]).
    Describing,
    /// A community's overview, with every point's quotes ([`crate::features::writing::overview`]).
    Overview,
    /// A community's papers closest in meaning to each statement of its description, found with the embedding
    /// model on this computer (no language model, no cost: [`crate::features::writing::closest`]).
    Closest,
    /// What a model wrote for the AI-text detector's training set (a 2019 abstract written from its title, or
    /// polished): kept here as it arrives, so that none is paid for twice (`crate::features::detector`).
    Detector,
}

impl Feature {
    pub const ALL: [Feature; 11] = [
        Feature::Extraction,
        Feature::CitationKinds,
        Feature::Stances,
        Feature::Answers,
        Feature::Critique,
        Feature::Explanation,
        Feature::ClaimTree,
        Feature::Describing,
        Feature::Overview,
        Feature::Closest,
        Feature::Detector,
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
            Feature::Describing => "describing",
            Feature::Overview => "overview",
            Feature::Closest => "closest_papers",
            Feature::Detector => "ai_detector",
        }
    }

    /// May an answer kept while papers were read by an older passages reader ([`READER`]) be used as it is?
    ///
    /// Yes for every feature whose answer is checked against the papers' text whenever it is loaded (its quotes
    /// located again by [`crate::evidence::check`], those no longer found dropped), or that reads no paper's passages
    /// at all (titles and abstracts, embeddings): the words it is shown against are the paper's words now. No for a
    /// feature keyed by a place in the paper's text whose answer cannot show it was written about the words at that
    /// place now: an explanation is kept by its passage's address and does not keep the passage's words, and the
    /// reader that changed may have changed them. A citation's reading keeps its sentence, so it is used only when
    /// that sentence is the one at its place now (`features::citation_contexts::kinds::kept_reading`, through
    /// [`Library::kept_older`]).
    pub const fn survives_a_new_reader(self) -> bool {
        match self {
            Feature::Extraction
            | Feature::Stances
            | Feature::Answers
            | Feature::Critique
            | Feature::ClaimTree
            | Feature::Describing
            | Feature::Overview
            | Feature::Closest
            | Feature::Detector => true,
            Feature::Explanation | Feature::CitationKinds => false,
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
    pub(super) fn stored(&self) -> String {
        match self {
            Subject::Paper(id) => format!("paper:{id}"),
            Subject::Run(run) => format!("run:{run}"),
            Subject::Community { run, community } => format!("community:{run}/{community}"),
        }
    }
}

/// The key an answer is kept under. Made only by [`AnswerKey::new`], so every feature makes it the same way.
///
/// It also knows the keys the same answer was kept under while papers were read by each older passages reader
/// (newest first), so that an answer paid for then is still found ([`Library::kept`]). Two keys are the same key
/// when their current keys are.
#[derive(Clone, Debug)]
pub struct AnswerKey {
    hex: String,
    older: Vec<String>,
}

impl PartialEq for AnswerKey {
    fn eq(&self, other: &Self) -> bool {
        self.hex == other.hex
    }
}

impl Eq for AnswerKey {}

impl std::hash::Hash for AnswerKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.hex.hash(state);
    }
}

impl PartialOrd for AnswerKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AnswerKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.hex.cmp(&other.hex)
    }
}

impl AnswerKey {
    /// The key of an answer of `feature`, written by `model` of `provider` with version `prompt` of the feature's
    /// prompt, about `papers`, to `question`, read with this version of the passages reader ([`READER`]). It knows
    /// the keys the same answer had under each older reader too, so that an answer paid for then is still found
    /// where it may be ([`Library::kept`], [`Feature::survives_a_new_reader`]).
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
        Self::with_older(feature, &price_key(provider, model), prompt, papers, question)
    }

    /// The key of what a feature keeps of its own, written by no model (the list of a community's questions):
    /// the key of [`AnswerKey::new`] with an empty model, as such keys always were.
    pub fn of_feature(feature: Feature, version: u32, papers: &[&str], question: &str) -> Self {
        Self::with_older(feature, "", version, papers, question)
    }

    /// The key made with this reader ([`READER`]), knowing those made with each older one.
    fn with_older(feature: Feature, model: &str, prompt: u32, papers: &[&str], question: &str) -> Self {
        Self {
            hex: Self::hashed(feature, model, prompt, READER, papers, question),
            older: (1..READER).rev().map(|reader| Self::hashed(feature, model, prompt, reader, papers, question)).collect(),
        }
    }

    fn hashed(feature: Feature, model: &str, prompt: u32, reader: u32, papers: &[&str], question: &str) -> String {
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
        field(&reader.to_string());
        field(&papers.len().to_string());
        for paper in &papers {
            field(paper);
        }
        field(question);
        hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// A key as it was kept (its hex), as another table holds it beside what it is the key of: for forgetting the
    /// answer kept under it, whatever the key would be made as now.
    pub fn from_stored(hex: String) -> Self {
        Self { hex, older: Vec::new() }
    }

    pub fn as_str(&self) -> &str {
        &self.hex
    }

    /// The keys of older passages readers, newest first: for tests of features that look under them.
    #[cfg(test)]
    pub fn older_for_tests(key: &AnswerKey) -> &[String] {
        &key.older
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
    ///
    /// When nothing is kept under the key of this passages reader, the answer kept under the key of the newest
    /// older reader is used, for a feature whose answers survive a new reader ([`Feature::survives_a_new_reader`]).
    /// It is read where it is, not moved: going back a version finds it still.
    pub fn kept<T: DeserializeOwned>(
        &self,
        feature: Feature,
        key: &AnswerKey,
    ) -> Result<Option<Kept<T>>, LibraryError> {
        if let Some(kept) = self.kept_under(feature, &key.hex)? {
            return Ok(Some(kept));
        }
        if feature.survives_a_new_reader() {
            for older in &key.older {
                if let Some(kept) = self.kept_under(feature, older)? {
                    return Ok(Some(kept));
                }
            }
        }
        Ok(None)
    }

    /// The answers kept under the keys of older passages readers, newest first, for a feature that checks for
    /// itself that one was written about the words it would be shown against.
    pub fn kept_older<T: DeserializeOwned>(
        &self,
        feature: Feature,
        key: &AnswerKey,
    ) -> Result<Vec<Kept<T>>, LibraryError> {
        let mut found = Vec::new();
        for older in &key.older {
            found.extend(self.kept_under(feature, older)?);
        }
        Ok(found)
    }

    fn kept_under<T: DeserializeOwned>(&self, feature: Feature, hex: &str) -> Result<Option<Kept<T>>, LibraryError> {
        let row: Option<(String, String, String, i64, i64)> = self.read(|connection| {
            Ok(connection
                .prepare_cached(
                    "SELECT content, kept_at, model, prompt, reader FROM kept_answers
                     WHERE feature = ?1 AND answer_key = ?2",
                )?
                .query_row(params![feature.name(), hex], |row| {
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
        self.write(|transaction| put_answer(transaction, slot, model, prompt, &content))
    }

    /// Keep an answer the model was just paid for (`content`, its JSON) and record in the ledger the `calls` that
    /// paid for it, in one transaction: a crash can leave neither a call paid for whose answer is lost (and paid
    /// for again) nor an answer kept whose cost is missing from the ledger. The intent written before the call was
    /// sent ([`super::asking`]) goes in the same transaction: a crash before it leaves the intent, which says the
    /// call's outcome is unknown.
    pub fn keep_paid(
        &self,
        slot: &Slot,
        model: &str,
        prompt: u32,
        content: &str,
        calls: &[super::ledger::Call<'_>],
    ) -> Result<(), LibraryError> {
        self.write(|transaction| {
            put_answer(transaction, slot, model, prompt, content)?;
            let at = now();
            for call in calls {
                super::ledger::put_calls(transaction, &at, call, 1)?;
            }
            super::asking::settle(transaction, slot)?;
            Ok(())
        })
    }

    /// Forget the one answer `feature` kept under `key`: a redo of one answer, where its subject has others that
    /// stay (one question of a run's many, one passage's explanation of a paper's). It is asked for again. False
    /// when none was kept.
    pub fn forget_key(&self, feature: Feature, key: &AnswerKey) -> Result<bool, LibraryError> {
        self.write(|transaction| Ok(forget_all_versions(transaction, feature, key)? > 0))
    }

    /// Forget the answers `feature` kept under any of `keys`, in one transaction: how many there were.
    pub fn forget_keys(&self, feature: Feature, keys: &[AnswerKey]) -> Result<usize, LibraryError> {
        self.write(|transaction| {
            let mut forgotten = 0;
            for key in keys {
                forgotten += forget_all_versions(transaction, feature, key)?;
            }
            Ok(forgotten)
        })
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

/// An answer kept in `slot`, replacing what was kept there, in a transaction of the caller's.
fn put_answer(
    connection: &rusqlite::Connection,
    slot: &Slot,
    model: &str,
    prompt: u32,
    content: &str,
) -> Result<(), LibraryError> {
    connection
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
}

/// The answer kept under `key` and under its keys of older passages readers: what a redo asked for by key
/// forgets, since otherwise the older answer would be found again in its place ([`Library::kept`]).
fn forget_all_versions(connection: &rusqlite::Connection, feature: Feature, key: &AnswerKey) -> Result<usize, LibraryError> {
    let mut forgotten = forget_key(connection, feature, key.as_str())?;
    for older in &key.older {
        forgotten += forget_key(connection, feature, older)?;
    }
    Ok(forgotten)
}

/// [`Library::forget_key`] in a transaction of the caller's, the key as stored (hex): how many rows went.
pub(crate) fn forget_key(
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
        key_with_reader(feature, model, prompt, READER, papers, question)
    }

    /// [`key_before_providers`] with the passages reader's version given.
    fn key_with_reader(feature: Feature, model: &str, prompt: u32, reader: u32, papers: &[&str], question: &str) -> String {
        let mut papers = papers.to_vec();
        papers.sort_unstable();
        papers.dedup();
        let mut hash = Sha256::new();
        let count = papers.len().to_string();
        let (prompt, reader) = (prompt.to_string(), reader.to_string());
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
        // as kept in libraries already, by the first passages reader: the key of one answer, written down, and
        // still among those a key of this reader looks under
        let kept = AnswerKey::hashed(Feature::Answers, "claude-opus-5-5", 1, 1, &["a", "b"], "What is new?");
        assert_eq!(kept, "b03f1011ed6dc1dfbbaa131184f88fa329802546c1b0a7e5d502c54e28878644");
        let now = AnswerKey::new(Feature::Answers, Provider::Anthropic, "claude-opus-5-5", 1, &["a", "b"], "What is new?");
        assert_eq!(now.older.last(), Some(&kept));
        for reader in 1..READER {
            let older = key_with_reader(Feature::Answers, "claude-opus-5-5", 1, reader, &["a", "b"], "What is new?");
            assert!(now.older.contains(&older), "reader {reader}");
        }
        // what a feature keeps of its own (no model) is found where it was kept too
        let list = AnswerKey::of_feature(Feature::Stances, 1, &[], "questions");
        assert_eq!(list.as_str(), key_before_providers(Feature::Stances, "", 1, &[], "questions"));
    }

    /// Keep `answer` for `feature` under the key the same question had with passages reader `reader`.
    fn keep_under_reader(library: &Library, feature: Feature, reader: u32, answer: &str) -> AnswerKey {
        let now = AnswerKey::new(feature, Provider::Anthropic, "claude-opus-5-5", 1, &["p"], "q");
        let key = if reader == READER {
            now.clone()
        } else {
            AnswerKey::from_stored(AnswerKey::hashed(feature, "claude-opus-5-5", 1, reader, &["p"], "q"))
        };
        library.keep(&Slot { feature, key, subject: Subject::Paper("p".into()) }, "claude-opus-5-5", 1, &answer).unwrap();
        now
    }

    #[test]
    fn an_answer_kept_under_an_older_reader_is_still_found_where_its_quotes_are_checked_again() {
        for feature in Feature::ALL.into_iter().filter(|feature| feature.survives_a_new_reader()) {
            for reader in 1..READER {
                let folder = tempfile::tempdir().unwrap();
                let library = Library::open(folder.path()).unwrap();
                let key = keep_under_reader(&library, feature, reader, &format!("paid for under reader {reader}"));
                let kept = library.kept::<String>(feature, &key).unwrap().unwrap();
                assert_eq!(kept.answer, format!("paid for under reader {reader}"), "{feature:?}");
            }
            // the newest older answer first, and this reader's over every older one
            let folder = tempfile::tempdir().unwrap();
            let library = Library::open(folder.path()).unwrap();
            for reader in 1..READER {
                keep_under_reader(&library, feature, reader, &format!("reader {reader}"));
            }
            let key = AnswerKey::new(feature, Provider::Anthropic, "claude-opus-5-5", 1, &["p"], "q");
            let newest_older = format!("reader {}", READER - 1);
            assert_eq!(library.kept::<String>(feature, &key).unwrap().unwrap().answer, newest_older);
            keep_under_reader(&library, feature, READER, "this reader's");
            assert_eq!(library.kept::<String>(feature, &key).unwrap().unwrap().answer, "this reader's");
            // read where it is, not moved: the older rows are all still there
            for reader in 1..READER {
                let older = AnswerKey::from_stored(AnswerKey::hashed(feature, "claude-opus-5-5", 1, reader, &["p"], "q"));
                assert!(library.kept::<String>(feature, &older).unwrap().is_some());
            }
        }
    }

    #[test]
    fn an_answer_kept_by_a_place_in_the_text_under_an_older_reader_is_not_taken_for_the_words_there_now() {
        for feature in [Feature::Explanation, Feature::CitationKinds] {
            let folder = tempfile::tempdir().unwrap();
            let library = Library::open(folder.path()).unwrap();
            let key = keep_under_reader(&library, feature, READER - 1, "about the words read before");
            assert_eq!(library.kept::<String>(feature, &key).unwrap(), None, "{feature:?}");
            // it is there for a feature that checks the words itself, and kept
            assert_eq!(library.kept_older::<String>(feature, &key).unwrap().len(), 1);
        }
    }

    #[test]
    fn a_redo_by_key_forgets_the_answer_under_every_reader() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let key = keep_under_reader(&library, Feature::Answers, 1, "old");
        assert!(library.forget_key(Feature::Answers, &key).unwrap());
        assert_eq!(library.kept::<String>(Feature::Answers, &key).unwrap(), None, "not found again in its place");
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
    fn an_answer_paid_for_is_kept_with_its_cost_or_neither_is_written() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let first = slot(Feature::Critique, Subject::Paper("p".to_owned()), "q");
        let call = |model| super::super::ledger::Call {
            run: None,
            model,
            purpose: "a paper's weaknesses",
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            price_micros: Some(30),
        };
        let content = serde_json::to_string(&claims("x")).unwrap();
        library
            .keep_paid(&first, "claude-opus-5-5", 1, &content, &[call("claude-opus-5-5"), call("claude-sonnet-5-5")])
            .unwrap();
        assert!(library.kept::<Vec<Claim<Cell>>>(Feature::Critique, &first.key).unwrap().is_some());
        assert_eq!(library.spending().unwrap().total.calls, 2, "both attempts are in the ledger");
        // the ledger cannot be written: the answer is not kept either, so no answer stands without its cost
        library.write(|transaction| Ok(transaction.execute_batch("DROP TABLE spending")?)).unwrap();
        let other = slot(Feature::Critique, Subject::Paper("p".to_owned()), "another question");
        assert!(library.keep_paid(&other, "claude-opus-5-5", 1, &content, &[call("claude-opus-5-5")]).is_err());
        assert_eq!(library.kept::<Vec<Claim<Cell>>>(Feature::Critique, &other.key).unwrap(), None);
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
