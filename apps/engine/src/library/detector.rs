//! What the AI-text detector ([`crate::detector`]) keeps: its training set and the model fitted on it.
//!
//! | table               | what                                                                          |
//! |---------------------|-------------------------------------------------------------------------------|
//! | `detector_humans`   | human abstracts from arXiv: the 2019 pool, and the samples of the year check   |
//! | `detector_days`     | which days of arXiv each sample was fetched for, so none is fetched twice      |
//! | `detector_mirrors`  | what a model wrote for a pool paper ("written" from its title, or "polished"), with the provider and model that wrote it |
//! | `detector_training` | the training set as built: the pool papers trained on, in the order added, and the rounds of mining |
//! | `detector_model`    | the classifier fitted on it, and what was measured about it                   |
//!
//! Nothing here is about a paper of the library: a pool paper is a 2019 abstract kept only to learn from, so
//! these tables have no reference to `papers`.
//!
//! The Python backend kept the same training set in `ai_detection/training.json` (`{"pool", "machine", "train",
//! "rounds"}`) and its year check in `ai_detection/validation.json`; [`Library::import_detector`] moves them in,
//! so that a training set someone already paid for is used as it is. The Python kept no model: it fitted one from
//! `training.json` whenever the file changed. Nor does anything need importing from its `scores.json` here: those
//! scores are moved by the library's import into `ai_scores`, under the Python's classifier version, and are
//! computed again by the engine's own classifier (locally, in seconds).

use std::fs;
use std::path::Path;

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::import::ImportError;
use super::store::StoreError;
use super::{Library, LibraryError, now};
use crate::model::language::Provider;

/// The sample of human abstracts the classifier is trained and measured on.
pub const POOL: &str = "pool";

/// A human abstract kept for the detector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Human {
    pub id: String,
    pub title: String,
    #[serde(rename = "abstract")]
    pub r#abstract: String,
}

/// Which mirror of a human abstract a model wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// An abstract written from the paper's title alone.
    Written,
    /// The human abstract, polished by a model.
    Polished,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::Written, Kind::Polished];

    /// As stored, and as the Python named it.
    pub const fn name(self) -> &'static str {
        match self {
            Kind::Written => "written",
            Kind::Polished => "polished",
        }
    }

    fn from_name(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// The model that wrote a mirror. The detector learns "written the way these models write", so which models they
/// were is part of what it means.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct Generator {
    /// None for a mirror kept before the provider was recorded (the Python wrote with the Claude API only, and
    /// recorded the model; an entry without even that has neither).
    pub provider: Option<Provider>,
    /// As the provider named it; "unknown" when it was not recorded.
    pub model: String,
}

/// A model's mirror of one pool paper.
#[derive(Clone, Debug, PartialEq)]
pub struct Mirror {
    pub id: String,
    pub kind: Kind,
    pub generator: Generator,
    pub text: String,
}

/// The training set as it was built: which pool papers were trained on, in the order they were added, and how many
/// were in after each round of mining (the first: before any).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Built {
    pub train: Vec<String>,
    pub rounds: Vec<usize>,
    pub built_at: String,
}

/// The share of abstracts flagged in one week of arXiv submissions of a year.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct YearFlagged {
    pub abstracts: usize,
    pub flagged: f64,
}

/// The share flagged of each year's week, by year.
pub type ByYear = std::collections::BTreeMap<String, YearFlagged>;

/// The classifier as kept: its content is the detector's own (JSON), with the versions of the code that fitted it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeptModel {
    /// The version of the features (words and word pairs) and of the training it was fitted with.
    pub features: u32,
    pub training: u32,
    pub content: String,
    pub trained_at: String,
}

/// What was moved in from the Python's files.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ImportedDetector {
    pub humans: usize,
    pub mirrors: usize,
    /// Whether the training set as built (`train`, `rounds`) was moved in.
    pub built: bool,
    /// Entries that did not read as what they should be.
    pub skipped: usize,
}

fn count(value: i64) -> usize {
    usize::try_from(value).unwrap_or(0)
}

fn signed(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn put_human(connection: &rusqlite::Connection, sample: &str, position: i64, human: &Human) -> rusqlite::Result<usize> {
    connection
        .prepare_cached(
            "INSERT INTO detector_humans (sample, paper_id, position, title, abstract) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (sample, paper_id) DO NOTHING",
        )?
        .execute(params![sample, human.id, position, human.title, human.r#abstract])
}

fn next_position(connection: &rusqlite::Connection, table: &str) -> rusqlite::Result<i64> {
    // `table` is one of this module's own names, never a caller's text
    connection.query_row(&format!("SELECT COALESCE(MAX(position) + 1, 0) FROM {table}"), [], |row| row.get(0))
}

fn put_mirror(connection: &rusqlite::Connection, position: i64, mirror: &Mirror) -> rusqlite::Result<usize> {
    connection
        .prepare_cached(
            "INSERT INTO detector_mirrors (paper_id, kind, provider, model, text, position, kept_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (paper_id, kind) DO NOTHING",
        )?
        .execute(params![
            mirror.id,
            mirror.kind.name(),
            mirror.generator.provider.map(Provider::id),
            mirror.generator.model,
            mirror.text,
            position,
            now()
        ])
}

impl Library {
    /// The human abstracts of `sample`, in the order they were kept.
    pub fn detector_humans(&self, sample: &str) -> Result<Vec<Human>, LibraryError> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT paper_id, title, abstract FROM detector_humans WHERE sample = ?1 ORDER BY position",
            )?;
            let rows = statement.query_map([sample], |row| {
                Ok(Human { id: row.get(0)?, title: row.get(1)?, r#abstract: row.get(2)? })
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    /// Keep what one day of arXiv gave `sample` (the papers it had that are kept, in order), and that the day was
    /// fetched, in one transaction: a day is either fetched and kept, or neither.
    pub fn keep_detector_day(&self, sample: &str, day: &str, humans: &[Human]) -> Result<(), LibraryError> {
        self.write(|transaction| {
            let first = next_position(transaction, "detector_humans")?;
            for (offset, human) in humans.iter().enumerate() {
                put_human(transaction, sample, first + signed(offset), human)?;
            }
            transaction
                .prepare_cached(
                    "INSERT INTO detector_days (sample, day, papers, fetched_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (sample, day) DO UPDATE SET papers = excluded.papers, fetched_at = excluded.fetched_at",
                )?
                .execute(params![sample, day, signed(humans.len()), now()])?;
            Ok(())
        })
    }

    /// The days of arXiv already fetched for `sample`.
    pub fn detector_days(&self, sample: &str) -> Result<Vec<String>, LibraryError> {
        self.read(|connection| {
            let mut statement =
                connection.prepare_cached("SELECT day FROM detector_days WHERE sample = ?1 ORDER BY day")?;
            let rows = statement.query_map([sample], |row| row.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    /// Every mirror kept, in the order they were kept.
    pub fn detector_mirrors(&self) -> Result<Vec<Mirror>, LibraryError> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT paper_id, kind, provider, model, text FROM detector_mirrors ORDER BY position",
            )?;
            let rows = statement.query_map([], |row| {
                let kind: String = row.get(1)?;
                let provider: Option<String> = row.get(2)?;
                Ok((row.get::<_, String>(0)?, kind, provider, row.get::<_, String>(3)?, row.get::<_, String>(4)?))
            })?;
            let mut mirrors = Vec::new();
            for row in rows {
                let (id, kind, provider, model, text) = row?;
                // the CHECK constraint keeps any other kind out
                let Some(kind) = Kind::from_name(&kind) else { continue };
                let provider = provider.as_deref().and_then(Provider::from_id);
                mirrors.push(Mirror { id, kind, generator: Generator { provider, model }, text });
            }
            Ok(mirrors)
        })
    }

    /// Keep mirrors just written, after those kept before; one already kept for its paper and kind stays as it is.
    pub fn keep_detector_mirrors(&self, mirrors: &[Mirror]) -> Result<usize, LibraryError> {
        self.write(|transaction| {
            let first = next_position(transaction, "detector_mirrors")?;
            let mut kept = 0;
            for (offset, mirror) in mirrors.iter().enumerate() {
                kept += put_mirror(transaction, first + signed(offset), mirror)?;
            }
            Ok(kept)
        })
    }

    /// The training set as built, if it was; and what the year check found, if it was run.
    pub fn detector_built(&self) -> Result<(Option<Built>, Option<ByYear>), LibraryError> {
        self.read(|connection| {
            let row: Option<(String, String, Option<String>, String)> = connection
                .prepare_cached("SELECT train, rounds, by_year, built_at FROM detector_training WHERE id = 1")?
                .query_row([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
                .optional()?;
            let Some((train, rounds, by_year, built_at)) = row else { return Ok((None, None)) };
            let built =
                Built { train: serde_json::from_str(&train)?, rounds: serde_json::from_str(&rounds)?, built_at };
            let by_year = by_year.map(|years| serde_json::from_str(&years)).transpose()?;
            Ok((Some(built), by_year))
        })
    }

    /// Keep the training set as built (replacing what was kept: a build that resumed and finished).
    pub fn keep_detector_built(&self, train: &[String], rounds: &[usize]) -> Result<(), LibraryError> {
        let (train, rounds) = (serde_json::to_string(train)?, serde_json::to_string(rounds)?);
        self.write(|transaction| {
            transaction
                .prepare_cached(
                    "INSERT INTO detector_training (id, train, rounds, by_year, built_at) VALUES (1, ?1, ?2, NULL, ?3)
                     ON CONFLICT (id) DO UPDATE SET train = excluded.train, rounds = excluded.rounds,
                         built_at = excluded.built_at",
                )?
                .execute(params![train, rounds, now()])?;
            Ok(())
        })
    }

    /// Keep what the year check found.
    pub fn keep_detector_years(&self, by_year: &ByYear) -> Result<bool, LibraryError> {
        let by_year = serde_json::to_string(by_year)?;
        self.write(|transaction| {
            Ok(transaction
                .prepare_cached("UPDATE detector_training SET by_year = ?1 WHERE id = 1")?
                .execute([by_year])?
                > 0)
        })
    }

    /// The classifier kept, if one was fitted.
    pub fn detector_model(&self) -> Result<Option<KeptModel>, LibraryError> {
        self.read(|connection| {
            Ok(connection
                .prepare_cached("SELECT features, training, content, trained_at FROM detector_model WHERE id = 1")?
                .query_row([], |row| {
                    let features: i64 = row.get(0)?;
                    let training: i64 = row.get(1)?;
                    Ok(KeptModel {
                        features: u32::try_from(features).unwrap_or(0),
                        training: u32::try_from(training).unwrap_or(0),
                        content: row.get(2)?,
                        trained_at: row.get(3)?,
                    })
                })
                .optional()?)
        })
    }

    /// Keep a fitted classifier, replacing the one kept.
    pub fn keep_detector_model(&self, features: u32, training: u32, content: &str) -> Result<(), LibraryError> {
        self.write(|transaction| {
            transaction
                .prepare_cached(
                    "INSERT INTO detector_model (id, features, training, content, trained_at) VALUES (1, ?1, ?2, ?3, ?4)
                     ON CONFLICT (id) DO UPDATE SET features = excluded.features, training = excluded.training,
                         content = excluded.content, trained_at = excluded.trained_at",
                )?
                .execute(params![features, training, content, now()])?;
            Ok(())
        })
    }

    /// How many human abstracts `sample` has and how many mirrors are kept, without reading them.
    pub fn detector_counts(&self, sample: &str) -> Result<(usize, usize), LibraryError> {
        self.read(|connection| {
            let humans: i64 =
                connection
                    .query_row("SELECT COUNT(*) FROM detector_humans WHERE sample = ?1", [sample], |row| row.get(0))?;
            let mirrors: i64 = connection.query_row("SELECT COUNT(*) FROM detector_mirrors", [], |row| row.get(0))?;
            Ok((count(humans), count(mirrors)))
        })
    }

    /// Move the Python's detector files in `folder` (`ai_detection/training.json`, and the year check of
    /// `ai_detection/validation.json`) into these tables. What the tables already have wins, as in the rest of the
    /// import: importing twice is importing once. Missing files are nothing to import.
    pub fn import_detector(&self, folder: &Path) -> Result<ImportedDetector, ImportError> {
        let folder = folder.join("ai_detection");
        let Some(training) = read_json(&folder.join("training.json"))? else { return Ok(ImportedDetector::default()) };
        let by_year = read_json(&folder.join("validation.json"))?.and_then(|report| years(&report));
        let old = OldTraining::read(&training);
        let mut imported = ImportedDetector { skipped: old.skipped, ..ImportedDetector::default() };
        self.write(|transaction| {
            let first = next_position(transaction, "detector_humans")?;
            for (offset, human) in old.pool.iter().enumerate() {
                imported.humans += put_human(transaction, POOL, first + signed(offset), human)?;
            }
            let first = next_position(transaction, "detector_mirrors")?;
            for (offset, mirror) in old.mirrors.iter().enumerate() {
                imported.mirrors += put_mirror(transaction, first + signed(offset), mirror)?;
            }
            if let Some((train, rounds)) = &old.built {
                let years = by_year.as_ref().map(serde_json::to_string).transpose()?;
                imported.built = transaction
                    .prepare_cached(
                        "INSERT INTO detector_training (id, train, rounds, by_year, built_at) VALUES (1, ?1, ?2, ?3, ?4)
                         ON CONFLICT (id) DO NOTHING",
                    )?
                    .execute(params![serde_json::to_string(train)?, serde_json::to_string(rounds)?, years, now()])?
                    > 0;
            }
            Ok(())
        })?;
        Ok(imported)
    }
}

fn read_json(path: &Path) -> Result<Option<Value>, ImportError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(StoreError::Io { path: path.to_path_buf(), source }.into()),
    };
    // a file that is not JSON at all is not the Python's; nothing is guessed from it
    match serde_json::from_str(&text) {
        Ok(value) => Ok(Some(value)),
        Err(error) => {
            eprintln!("library: {} is not JSON and was not imported: {error}", path.display());
            Ok(None)
        }
    }
}

/// The year check of the Python's `validation.json`: `{"by_year": {"2019": {"abstracts", "flagged"}, ...}}`.
fn years(report: &Value) -> Option<ByYear> {
    serde_json::from_value(report.get("by_year")?.clone()).ok()
}

/// `training.json` as the Python wrote it, read entry by entry: one that does not read is skipped, not fatal.
struct OldTraining {
    pool: Vec<Human>,
    mirrors: Vec<Mirror>,
    /// `train` and `rounds`, when the Python finished building (it wrote them only at the end).
    built: Option<(Vec<String>, Vec<usize>)>,
    skipped: usize,
}

impl OldTraining {
    fn read(value: &Value) -> Self {
        let mut skipped = 0;
        let list = |key: &str| value.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
        let mut pool = Vec::new();
        for entry in list("pool") {
            match serde_json::from_value::<Human>(entry) {
                Ok(human) => pool.push(human),
                Err(_) => skipped += 1,
            }
        }
        let mut mirrors = Vec::new();
        for entry in list("machine") {
            match mirror(&entry) {
                Some(mirror) => mirrors.push(mirror),
                None => skipped += 1,
            }
        }
        let train: Vec<String> = list("train").iter().filter_map(|id| id.as_str().map(str::to_owned)).collect();
        // a round was once a number of papers, and later a record of what it measured with that number in it
        let rounds: Vec<usize> = list("rounds")
            .iter()
            .filter_map(|round| round.as_u64().or_else(|| round.get("humans").and_then(Value::as_u64)))
            .filter_map(|humans| usize::try_from(humans).ok())
            .collect();
        let built = (!train.is_empty() && !rounds.is_empty() && rounds.iter().all(|size| *size <= train.len()))
            .then_some((train, rounds));
        OldTraining { pool, mirrors, built, skipped }
    }
}

/// One `machine` entry: `{"id", "kind", "generator"?, "text"}`. The Python wrote with the Claude API only.
fn mirror(entry: &Value) -> Option<Mirror> {
    let text = |key: &str| entry.get(key).and_then(Value::as_str);
    let kind = Kind::from_name(text("kind")?)?;
    let generator = match text("generator") {
        Some(model) if model.starts_with("claude-") => {
            Generator { provider: Some(Provider::Anthropic), model: model.to_owned() }
        }
        Some(model) => Generator { provider: None, model: model.to_owned() },
        None => Generator { provider: None, model: "unknown".to_owned() },
    };
    Some(Mirror { id: text("id")?.to_owned(), kind, generator, text: text("text")?.to_owned() })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn library() -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        (folder, library)
    }

    fn human(id: &str) -> Human {
        Human { id: id.to_owned(), title: format!("Title {id}"), r#abstract: format!("Abstract of {id}.") }
    }

    /// The Python's files as it laid them out, with synthetic content: a pool, mirrors (one from before the model
    /// was recorded, one of a kind it never wrote, one cut short), the training set as built (rounds in both forms
    /// it wrote them), and its year check.
    fn python_files(folder: &Path) {
        let detection = folder.join("ai_detection");
        fs::create_dir_all(&detection).unwrap();
        let training = json!({
            "pool": [
                {"id": "1906.00001", "title": "Title one", "abstract": "Abstract one."},
                {"id": "1906.00002", "title": "Title two", "abstract": "Abstract two."},
                {"id": "1906.00003", "title": "Title three"},
            ],
            "machine": [
                {"id": "1906.00001", "kind": "written", "generator": "claude-sonnet-5-5", "text": "Written one."},
                {"id": "1906.00001", "kind": "polished", "generator": "claude-haiku-5-5", "text": "Polished one."},
                {"id": "1906.00002", "kind": "written", "text": "Written two, model not recorded."},
                {"id": "1906.00002", "kind": "summarised", "generator": "claude-haiku-5-5", "text": "Not a kind."},
                {"id": "1906.00002", "kind": "polished"},
            ],
            "train": ["1906.00001", "1906.00002"],
            "rounds": [{"round": 0, "humans": 1, "threshold": 0.9, "written_caught": 0.8}, 2],
        });
        fs::write(detection.join("training.json"), training.to_string()).unwrap();
        let validation = json!({"threshold": 0.3, "by_year": {"2019": {"abstracts": 368, "flagged": 0.0027}}});
        fs::write(detection.join("validation.json"), validation.to_string()).unwrap();
    }

    #[test]
    fn the_pythons_training_set_is_moved_in_as_it_was_and_importing_twice_is_importing_once() {
        let (folder, library) = library();
        python_files(folder.path());
        let imported = library.import_detector(folder.path()).unwrap();
        assert_eq!(imported, ImportedDetector { humans: 2, mirrors: 3, built: true, skipped: 3 });
        let pool = library.detector_humans(POOL).unwrap();
        assert_eq!(pool.iter().map(|human| human.id.as_str()).collect::<Vec<_>>(), ["1906.00001", "1906.00002"]);
        assert_eq!(pool[1].r#abstract, "Abstract two.");
        let mirrors = library.detector_mirrors().unwrap();
        assert_eq!(mirrors.len(), 3);
        assert_eq!(
            mirrors[0].generator,
            Generator { provider: Some(Provider::Anthropic), model: "claude-sonnet-5-5".to_owned() }
        );
        assert_eq!((mirrors[1].kind, mirrors[1].text.as_str()), (Kind::Polished, "Polished one."));
        assert_eq!(mirrors[2].generator, Generator { provider: None, model: "unknown".to_owned() });
        let (built, by_year) = library.detector_built().unwrap();
        let built = built.unwrap();
        assert_eq!((built.train.len(), built.rounds), (2, vec![1, 2]));
        assert_eq!(by_year.unwrap()["2019"], YearFlagged { abstracts: 368, flagged: 0.0027 });

        let again = library.import_detector(folder.path()).unwrap();
        assert_eq!(again, ImportedDetector { humans: 0, mirrors: 0, built: false, skipped: 3 });
        assert_eq!(library.detector_counts(POOL).unwrap(), (2, 3));
    }

    #[test]
    fn a_training_set_the_python_never_finished_is_moved_in_without_being_called_built() {
        let (folder, library) = library();
        let detection = folder.path().join("ai_detection");
        fs::create_dir_all(&detection).unwrap();
        let partial =
            json!({"pool": [{"id": "a", "title": "t", "abstract": "x"}], "machine": [], "train": [], "rounds": []});
        fs::write(detection.join("training.json"), partial.to_string()).unwrap();
        let imported = library.import_detector(folder.path()).unwrap();
        assert_eq!((imported.humans, imported.built), (1, false));
        assert_eq!(library.detector_built().unwrap().0, None);
    }

    #[test]
    fn no_files_and_a_file_that_is_not_json_import_nothing() {
        let (folder, library) = library();
        assert_eq!(library.import_detector(folder.path()).unwrap(), ImportedDetector::default());
        let detection = folder.path().join("ai_detection");
        fs::create_dir_all(&detection).unwrap();
        fs::write(detection.join("training.json"), "\u{80}not json").unwrap();
        assert_eq!(library.import_detector(folder.path()).unwrap(), ImportedDetector::default());
    }

    #[test]
    fn a_day_is_kept_with_its_papers_and_mirrors_after_those_before_and_never_twice() {
        let (_folder, library) = library();
        library.keep_detector_day(POOL, "2019-05-01", &[human("b"), human("a")]).unwrap();
        library.keep_detector_day(POOL, "2019-05-02", &[human("c")]).unwrap();
        library.keep_detector_day("2021", "2021-10-04", &[human("a")]).unwrap();
        let ids: Vec<String> = library.detector_humans(POOL).unwrap().into_iter().map(|human| human.id).collect();
        assert_eq!(ids, ["b", "a", "c"], "in the order kept, not by id");
        assert_eq!(library.detector_days(POOL).unwrap(), ["2019-05-01", "2019-05-02"]);
        assert_eq!(library.detector_humans("2021").unwrap().len(), 1);

        let generator = Generator { provider: Some(Provider::OpenAi), model: "gpt-6.2-sol".to_owned() };
        let mirror = |id: &str, kind, text: &str| Mirror {
            id: id.to_owned(),
            kind,
            generator: generator.clone(),
            text: text.to_owned(),
        };
        assert_eq!(library.keep_detector_mirrors(&[mirror("b", Kind::Written, "one")]).unwrap(), 1);
        let again = [mirror("b", Kind::Written, "another"), mirror("a", Kind::Polished, "two")];
        assert_eq!(library.keep_detector_mirrors(&again).unwrap(), 1, "the one kept stays");
        let kept = library.detector_mirrors().unwrap();
        assert_eq!(kept.iter().map(|mirror| mirror.text.as_str()).collect::<Vec<_>>(), ["one", "two"]);
        assert_eq!(kept[1].generator, generator);
    }

    #[test]
    fn the_model_and_the_training_set_are_replaced_whole() {
        let (_folder, library) = library();
        assert_eq!(library.detector_model().unwrap(), None);
        library.keep_detector_model(1, 1, "{\"a\":1}").unwrap();
        library.keep_detector_model(2, 1, "{\"a\":2}").unwrap();
        let kept = library.detector_model().unwrap().unwrap();
        assert_eq!((kept.features, kept.training, kept.content.as_str()), (2, 1, "{\"a\":2}"));
        assert!(!library.keep_detector_years(&Default::default()).unwrap(), "nothing built yet to add it to");
        library.keep_detector_built(&["a".to_owned()], &[1]).unwrap();
        library.keep_detector_built(&["a".to_owned(), "b".to_owned()], &[1, 2]).unwrap();
        let years = std::collections::BTreeMap::from([("2025".to_owned(), YearFlagged { abstracts: 3, flagged: 0.5 })]);
        assert!(library.keep_detector_years(&years).unwrap());
        let (built, by_year) = library.detector_built().unwrap();
        assert_eq!(built.unwrap().rounds, [1, 2]);
        assert_eq!(by_year.unwrap(), years);
    }
}
