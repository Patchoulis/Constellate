//! The one-off move of a files-based library (the Python backend's) into the database, so that a user who
//! updates loses nothing that was downloaded, computed or paid for.
//!
//! It reads, from the library folder (`data/library`):
//!
//! | file                                  | into                                           |
//! |---------------------------------------|------------------------------------------------|
//! | `papers.jsonl`                        | papers, with their venues                      |
//! | `download_done.json`                  | download slices                                |
//! | `embeddings/<model>/vectors.npy` + `index.json` | embeddings                           |
//! | `proceedings/declarations.json`       | declarations                                   |
//! | `reviews.json`                        | ratings                                        |
//! | `ai_detection/scores.json`            | AI-text scores                                 |
//! | `ai_detection/training.json`, `validation.json` | the AI-text detector's training set ([`super::detector`]) |
//! | `descriptions.json`, `overviews.json` | descriptions, overviews                        |
//! | `usage.json`                          | the ledger                                     |
//! | `rates.json`                          | exchange rates                                 |
//! | `../settings.json`                    | settings (the Python kept them beside the library, in `data/`) |
//!
//! Every file is optional. Everything goes in as one transaction, and what the database already has wins: a row
//! already there is left as it is, so importing twice is the same as importing once, and nothing the engine has
//! written since is overwritten by an older file. The ledger, whose old totals cannot be told apart row by row,
//! is moved once and recorded as moved. The files themselves are left alone. Papers a re-import finds already
//! in the database keep the venue tags they have: the engine may have fetched a venue again since.
//!
//! The one transaction holds the write lock for the whole move, which for a large library with its vectors
//! takes longer than another process's busy timeout: the import is meant to run once, at the first start of the
//! new engine, before anything else opens the library.
//!
//! What cannot be carried over exactly, and what is done instead:
//!
//! * The Python recorded a conference as downloaded "official" even when it had fallen back to arXiv comments.
//!   Its papers tell which it was: a conference's own list has conference ids, the fallback arXiv ids. A slice
//!   with only arXiv papers is moved as [`SliceOutcome::ArxivComments`], one with none as
//!   [`SliceOutcome::Incomplete`]; both are fetched again.
//! * The old ledger kept totals by model and, separately, by purpose. The model totals are moved (purpose
//!   "imported"), so money and tokens are kept by model; the split by purpose before the move is not.
//! * An embedding folder is named after its model with characters a file name cannot have replaced. The model's
//!   real name is taken from the settings, or the default model, when one of them gives that folder name;
//!   otherwise the folder name is used as the model's name.
//! * Rows about papers that are not in `papers.jsonl` cannot be kept (every row is about a paper the library
//!   has) and are counted as skipped, as are entries that do not read as what they should be.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use super::embeddings::{put as put_embedding, width};
use super::ledger::{Call, put_calls};
use super::papers::{exists, put_paper, put_slice, tag};
use super::preferences::{Rate, put_rate, put_setting};
use super::readings::{AiScore, Declarations, Ratings, Reading, put_ai_score, put_declarations, put_ratings};
use super::store::{self, StoreError};
use super::writings::{Description, Overview, put_description, put_overview};
use super::{Known, Library, LibraryError, Origin, Paper, SliceOutcome, now};

/// The model the Python embedded with unless the settings said otherwise.
const DEFAULT_MODEL: &str = "Qwen/Qwen3-Embedding-0.6B";
/// How the ledger's move is recorded in `imported`.
const LEDGER: &str = "usage.json";

#[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
static NOT_IN_FOLDER_NAMES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w.-]+").unwrap());
#[allow(clippy::unwrap_used)]
static NPY_SHAPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"'shape':\s*\(\s*(\d+)\s*,\s*(\d+)\s*,?\s*\)").unwrap());

/// What can stop an import, always with the file.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error(transparent)]
    Library(#[from] LibraryError),
    #[error(transparent)]
    File(#[from] StoreError),
    #[error("{}, line {line}: {source}", path.display())]
    Line {
        path: PathBuf,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    /// A `.npy` file that is not a C-order little-endian float32 matrix, or is cut short.
    #[error("{}: {reason}", path.display())]
    Vectors { path: PathBuf, reason: String },
    /// A file that is not what it should be (not JSON at all).
    #[error("{}: {reason}", path.display())]
    Unreadable { path: PathBuf, reason: String },
}

impl From<rusqlite::Error> for ImportError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Library(error.into())
    }
}

/// How many rows each part of the import added; all zero for a library imported before.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Imported {
    pub papers: usize,
    pub venue_tags: usize,
    pub slices: usize,
    pub embeddings: BTreeMap<String, usize>,
    pub declarations: usize,
    pub ratings: usize,
    pub ai_scores: usize,
    /// The AI-text detector's training set ([`Library::import_detector`]), moved in its own transaction after the rest.
    pub ai_training: super::detector::ImportedDetector,
    pub descriptions: usize,
    pub overviews: usize,
    pub ledger_rows: usize,
    pub settings: usize,
    pub rates: usize,
    /// Entries left out: about a paper the library does not have, or not readable as what they should be.
    pub skipped: usize,
}

/// Open (or make) the library database in `folder` and move the files-based library in that folder into it.
pub fn import_files(folder: &Path) -> Result<Imported, ImportError> {
    Library::open(folder)?.import_files(folder)
}

impl Library {
    /// Move the files-based library in `folder` into this database; see the [module](self) for what and how.
    pub fn import_files(&self, folder: &Path) -> Result<Imported, ImportError> {
        let files = Files::read(folder)?; // everything read before the write lock is taken
        let mut connection = self.connection();
        let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut imported = files.write(&transaction)?;
        transaction.commit()?;
        drop(connection);
        imported.ai_training = self.import_detector(folder)?;
        Ok(imported)
    }

    /// Move only the old ledger (`usage.json` in `folder`) into this database, in one transaction, and say how many
    /// rows it added: what was spent before the engine is kept even when nothing else has been imported. It is
    /// recorded as moved exactly as [`Library::import_files`] records it, so moving it twice, or moving it and then
    /// importing the whole library, counts it once. No file there is nothing to move.
    pub fn import_ledger(&self, folder: &Path) -> Result<usize, ImportError> {
        let files = Files { ledger: store::read_json(&folder.join(LEDGER))?, ..Files::default() };
        if files.ledger.is_none() {
            return Ok(0);
        }
        let mut connection = self.connection();
        let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let rows = files.write_ledger(&transaction, &mut 0)?;
        transaction.commit()?;
        Ok(rows)
    }
}

/// How a finished move of the whole files-based library is recorded in `imported`.
const WHOLE_LIBRARY: &str = "files-library";

/// The files of a files-based library whose presence says there is one to move in.
const OLD_FILES: [&str; 4] = ["papers.jsonl", "download_done.json", "embeddings", "descriptions.json"];

impl Library {
    /// Move the files-based library in `folder` in, once: answers what was moved, or `None` when it was moved
    /// before (or there is none). The move and its record are one transaction, so a move that is interrupted
    /// (the app closed, the computer off) leaves nothing behind and is simply done again at the next start; a
    /// move that finished is never done again, so the old files are not read at every start.
    pub fn import_once(&self, folder: &Path) -> Result<Option<Imported>, ImportError> {
        let done = |connection: &Connection| -> rusqlite::Result<bool> {
            Ok(connection
                .query_row("SELECT 1 FROM imported WHERE source = ?1", [WHOLE_LIBRARY], |_| Ok(()))
                .optional()?
                .is_some())
        };
        if self.read(|connection| Ok(done(connection)?))? || !OLD_FILES.iter().any(|file| folder.join(file).exists()) {
            return Ok(None);
        }
        let files = Files::read(folder)?; // everything read before the write lock is taken
        let mut connection = self.connection();
        let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if done(&transaction)? {
            return Ok(None); // another process moved it in meanwhile
        }
        let imported = files.write(&transaction)?;
        transaction.execute(
            "INSERT INTO imported (source, at) VALUES (?1, ?2) ON CONFLICT (source) DO NOTHING",
            params![WHOLE_LIBRARY, now()],
        )?;
        transaction.commit()?;
        Ok(Some(imported))
    }
}

/// How the move of the AI-text detector's files is recorded in `imported`.
const DETECTOR: &str = "ai_detection";

/// What became of one part of moving the old data folder in.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum Part<T> {
    /// Moved in now, with what it added.
    Moved { added: T },
    /// Moved in at an earlier start, or nothing of it is there to move.
    Already,
    /// It could not be moved (a damaged file, the disk): nothing of this part was kept, and it is tried again at
    /// the next start. Said for the person, with the file.
    Failed { why: String },
}

impl<T> Part<T> {
    fn of<E: std::fmt::Display>(result: Result<Option<T>, E>) -> Self {
        match result {
            Ok(Some(added)) => Part::Moved { added },
            Ok(None) => Part::Already,
            Err(error) => Part::Failed { why: error.to_string() },
        }
    }

    /// The same, its reason naming files within the data folder (`data`, as displayed) rather than where that
    /// folder is on the computer: what is told goes to whoever holds the engine's token.
    fn told_in(self, data: &str) -> Self {
        match self {
            Part::Failed { why } if !data.is_empty() => Part::Failed { why: why.replace(data, "the data folder") },
            other => other,
        }
    }

    fn failed(&self) -> Option<&str> {
        match self {
            Part::Failed { why } => Some(why),
            Part::Moved { .. } | Part::Already => None,
        }
    }
}

/// The move of the Python's data folder in, part by part, in this order: the library's files (papers, slices,
/// embeddings, readings, descriptions, overviews, rates, settings, and the ledger with them), the ledger alone
/// (moved even when the files could not be), the AI-text detector's training set.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Moved {
    pub files: Part<Imported>,
    pub ledger: Part<usize>,
    pub detector: Part<super::detector::ImportedDetector>,
}

impl Default for Moved {
    /// Nothing to do: every part moved before, or none there.
    fn default() -> Self {
        Moved { files: Part::Already, ledger: Part::Already, detector: Part::Already }
    }
}

impl Moved {
    /// What could not be moved, in words for the person: empty when all went well.
    pub fn problems(&self) -> Vec<String> {
        let parts = [
            ("the library's papers, embeddings and readings", self.files.failed()),
            ("what was spent on the language model", self.ledger.failed()),
            ("the AI-text detector's training set", self.detector.failed()),
        ];
        parts
            .into_iter()
            .filter_map(|(what, why)| {
                why.map(|why| {
                    format!(
                        "The earlier version's data could not all be moved in: {what} ({why}). It is tried again at \
                         the next start; until then it is not shown."
                    )
                })
            })
            .collect()
    }
}

impl Library {
    /// Move the Python's data folder (`folder` is its `library`) in: the one move, done at the first open of the
    /// library, part by part ([`Moved`]). Each part is recorded in `imported` when it is done, so a part done is
    /// never done again; a part stopped half way (the app closed, the computer off) left nothing it wrote, or only
    /// what it writes again without doubling (the detector's texts, kept by id), and is done again at the next
    /// start. A part that fails does not stop the others.
    pub fn import_old(&self, folder: &Path) -> Moved {
        let files = Part::of(self.import_once(folder));
        let ledger = Part::of(self.import_ledger(folder).map(|rows| (rows > 0).then_some(rows)));
        let detector = Part::of(self.import_detector_once(folder));
        // what is told names the files within the data folder, never where that folder is on the computer
        let data = folder.parent().unwrap_or(folder).display().to_string();
        Moved { files: files.told_in(&data), ledger: ledger.told_in(&data), detector: detector.told_in(&data) }
    }

    /// The detector's part of [`Library::import_old`]: moved and then recorded; moving it again before the record
    /// (stopped between the two) adds nothing twice.
    fn import_detector_once(&self, folder: &Path) -> Result<Option<super::detector::ImportedDetector>, ImportError> {
        let recorded = |connection: &Connection| -> rusqlite::Result<bool> {
            Ok(connection
                .query_row("SELECT 1 FROM imported WHERE source = ?1", [DETECTOR], |_| Ok(()))
                .optional()?
                .is_some())
        };
        if self.read(|connection| Ok(recorded(connection)?))? || !folder.join(DETECTOR).exists() {
            return Ok(None);
        }
        // a training file that does not read is told, and the part tried again at the next start: never recorded
        // as moved with nothing in it, which would lose the training set for good
        let training = folder.join(DETECTOR).join("training.json");
        if training.is_file() {
            let text =
                fs::read_to_string(&training).map_err(|source| StoreError::Io { path: training.clone(), source })?;
            if let Err(error) = serde_json::from_str::<serde::de::IgnoredAny>(&text) {
                return Err(ImportError::Unreadable { path: training, reason: error.to_string() });
            }
        }
        let moved = self.import_detector(folder)?;
        self.write(|transaction| {
            transaction.execute(
                "INSERT INTO imported (source, at) VALUES (?1, ?2) ON CONFLICT (source) DO NOTHING",
                params![DETECTOR, now()],
            )?;
            Ok(())
        })?;
        Ok(Some(moved))
    }
}

/// One `embeddings/<folder>`: its index and its matrix.
struct Vectors {
    model: String,
    /// paper id -> (row, text hash)
    index: HashMap<String, (usize, String)>,
    rows: usize,
    /// The matrix's bytes after the header: `rows` rows of `width` bytes.
    bytes: Vec<u8>,
    width: usize,
    path: PathBuf,
}

/// Everything found in the folder, read and parsed.
#[derive(Default)]
struct Files {
    papers: Vec<Paper>,
    /// Records of `papers.jsonl` that are not papers.
    unreadable_papers: usize,
    slices: Vec<String>,
    vectors: Vec<Vectors>,
    declarations: Entries<KeptDeclarations>,
    ratings: Entries<Ratings>,
    /// paper id -> (text hash, classifier version, score or none)
    ai_scores: Entries<(String, String, Option<f64>)>,
    descriptions: Entries<Description>,
    overviews: Entries<Overview>,
    ledger: Option<Map<String, Value>>,
    rates: Entries<KeptRate>,
    settings: BTreeMap<String, Value>,
}

/// One entry of a `{key: ...}` file: what it should be, or anything else, to be counted as skipped. Parsed
/// straight from the file, so that objects keep their order (reviewers' fields are shown in it).
#[derive(Deserialize)]
#[serde(untagged)]
enum Entry<T> {
    Readable(T),
    Unreadable(serde::de::IgnoredAny),
}

type Entries<T> = BTreeMap<String, Entry<T>>;

fn object<T: DeserializeOwned>(path: &Path) -> Result<BTreeMap<String, T>, StoreError> {
    Ok(store::read_json(path)?.unwrap_or_default())
}

/// The papers of `papers.jsonl`, and how many of its records are JSON but not a paper (a missing title, a null
/// among the categories): those are skipped. A line that is not JSON at all means a damaged file, which stops the
/// import.
fn read_papers(path: &Path) -> Result<(Vec<Paper>, usize), ImportError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
        Err(source) => return Err(StoreError::Io { path: path.into(), source }.into()),
    };
    let (mut papers, mut unreadable) = (Vec::new(), 0);
    let last = text.lines().enumerate().filter(|(_, line)| !line.trim().is_empty()).map(|(number, _)| number).last();
    for (number, line) in text.lines().enumerate().filter(|(_, line)| !line.trim().is_empty()) {
        let record: Value = match serde_json::from_str(line) {
            Ok(record) => record,
            // the last line cut short: the Python stopped while it appended it; that one paper is skipped
            Err(_) if Some(number) == last => {
                unreadable += 1;
                continue;
            }
            Err(source) => return Err(ImportError::Line { path: path.into(), line: number + 1, source }),
        };
        match serde_json::from_value(record) {
            Ok(paper) => papers.push(paper),
            Err(_) => unreadable += 1,
        }
    }
    Ok((papers, unreadable))
}

/// A `.npy` file's matrix: (rows, columns, the data's bytes). Only what the Python wrote is read: float32,
/// little-endian, C order, two dimensions.
pub(crate) fn read_npy(path: &Path) -> Result<(usize, usize, Vec<u8>), ImportError> {
    let wrong = |reason: &str| ImportError::Vectors { path: path.into(), reason: reason.into() };
    let mut bytes = fs::read(path).map_err(|source| StoreError::Io { path: path.into(), source })?;
    if bytes.len() < 10 || !bytes.starts_with(b"\x93NUMPY") {
        return Err(wrong("not a .npy file"));
    }
    // version 1 has a two-byte header length, versions 2 and 3 a four-byte one
    let (length, start) = match bytes[6] {
        1 => (usize::from(u16::from_le_bytes([bytes[8], bytes[9]])), 10_usize),
        2 | 3 if bytes.len() >= 12 => {
            (usize::try_from(u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]])).unwrap_or(usize::MAX), 12)
        }
        _ => return Err(wrong("a .npy version this reader does not know")),
    };
    let end =
        start.checked_add(length).filter(|end| *end <= bytes.len()).ok_or_else(|| wrong("cut short in its header"))?;
    let header = String::from_utf8_lossy(&bytes[start..end]);
    if !header.contains("'descr': '<f4'") || !header.contains("'fortran_order': False") {
        return Err(wrong("not little-endian float32 in C order"));
    }
    let shape = NPY_SHAPE.captures(&header).ok_or_else(|| wrong("not a matrix"))?;
    let size = |index: usize| shape[index].parse::<usize>().map_err(|_| wrong("a shape too large"));
    let (rows, columns) = (size(1)?, size(2)?);
    let expected =
        rows.checked_mul(columns).and_then(|count| count.checked_mul(4)).ok_or_else(|| wrong("a shape too large"))?;
    if bytes.len() - end != expected {
        return Err(wrong(&format!("{} bytes of numbers for a {rows} x {columns} matrix", bytes.len() - end)));
    }
    bytes.drain(..end);
    Ok((rows, columns, bytes))
}

/// The folder the Python kept a model's embeddings in.
fn folder_name(model: &str) -> String {
    NOT_IN_FOLDER_NAMES.replace_all(model, "_").into_owned()
}

impl Files {
    fn read(folder: &Path) -> Result<Self, ImportError> {
        let settings: BTreeMap<String, Value> = object(&folder.parent().unwrap_or(folder).join("settings.json"))?;
        let mut models: Vec<String> =
            settings.get("embedding_model").and_then(Value::as_str).map(str::to_owned).into_iter().collect();
        models.push(DEFAULT_MODEL.into());
        let mut vectors = Vec::new();
        let embeddings = folder.join("embeddings");
        if let Ok(entries) = fs::read_dir(&embeddings) {
            let mut found: Vec<PathBuf> =
                entries.filter_map(Result::ok).map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
            found.sort();
            for place in found {
                let (npy, index) = (place.join("vectors.npy"), place.join("index.json"));
                if !npy.exists() || !index.exists() {
                    continue; // the Python read neither without the other
                }
                let name = place.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                let model = models.iter().find(|model| folder_name(model) == name).cloned().unwrap_or(name);
                let (rows, columns, bytes) = read_npy(&npy)?;
                let index: HashMap<String, (usize, String)> = store::read_json(&index)?.unwrap_or_default();
                vectors.push(Vectors { model, index, rows, bytes, width: columns * 4, path: npy });
            }
        }
        let (papers, unreadable_papers) = read_papers(&folder.join("papers.jsonl"))?;
        Ok(Self {
            papers,
            unreadable_papers,
            slices: store::read_json(&folder.join("download_done.json"))?.unwrap_or_default(),
            vectors,
            declarations: object(&folder.join("proceedings").join("declarations.json"))?,
            ratings: object(&folder.join("reviews.json"))?,
            ai_scores: object(&folder.join("ai_detection").join("scores.json"))?,
            descriptions: object(&folder.join("descriptions.json"))?,
            overviews: object(&folder.join("overviews.json"))?,
            // a damaged ledger keeps only itself out: the ledger's own part ([`Library::import_ledger`]) tells it
            ledger: store::read_json(&folder.join(LEDGER)).unwrap_or_else(|error| {
                eprintln!("library: the old ledger is left for its own part of the move: {error}");
                None
            }),
            rates: object(&folder.join("rates.json"))?,
            settings,
        })
    }

    /// How a slice the Python called finished is moved in: see the module's notes.
    fn outcome(&self, slice: &str) -> SliceOutcome {
        let mut parts = slice.split('|');
        let (Some("venue"), Some(venue)) = (parts.next(), parts.next()) else {
            return SliceOutcome::Complete; // a day: the Python recorded only days that could no longer grow
        };
        let mut listed =
            self.papers.iter().filter(|paper| paper.venues.iter().any(|tagged| tagged == venue)).peekable();
        if listed.peek().is_none() {
            SliceOutcome::Incomplete
        } else if listed.any(|paper| paper.origin() == Origin::Conference) {
            SliceOutcome::Complete
        } else {
            SliceOutcome::ArxivComments
        }
    }

    fn write(&self, connection: &Connection) -> Result<Imported, ImportError> {
        let mut imported = Imported::default();
        imported.skipped += self.unreadable_papers;
        for paper in &self.papers {
            // tags only with a paper this import brings in: a paper the database has keeps the tags the engine
            // gave it, which a fresh download of a venue may have taken away
            if put_paper(connection, paper, Known::Keep)? {
                imported.papers += 1;
                for venue in &paper.venues {
                    imported.venue_tags += tag(connection, &paper.id, venue)?;
                }
            }
        }
        for slice in &self.slices {
            let there = connection
                .query_row("SELECT 1 FROM download_slices WHERE slice = ?1", [slice], |_| Ok(()))
                .optional()?
                .is_some();
            if !there {
                put_slice(connection, slice, self.outcome(slice))?;
                imported.slices += 1;
            }
        }
        for vectors in &self.vectors {
            let added = self.write_vectors(connection, vectors, &mut imported.skipped)?;
            imported.embeddings.insert(vectors.model.clone(), added);
        }
        let skipped = &mut imported.skipped;
        imported.declarations =
            entries(connection, Keyed::Paper("declarations"), &self.declarations, skipped, |id, read| {
                put_declarations(connection, id, read.parser, &Reading::Read(read.declarations.clone()))
            })?;
        imported.ratings = entries(connection, Keyed::Paper("ratings"), &self.ratings, skipped, |id, ratings| {
            put_ratings(connection, id, ratings)
        })?;
        imported.ai_scores = entries(
            connection,
            Keyed::Paper("ai_scores"),
            &self.ai_scores,
            skipped,
            |id, (hash, classifier, score)| {
                let score = score.map_or(AiScore::TooShort, AiScore::Score);
                Ok(put_ai_score(connection, id, hash, classifier, score)?)
            },
        )?;
        imported.descriptions =
            entries(connection, Keyed::Community("descriptions"), &self.descriptions, skipped, |key, description| {
                Ok(put_description(connection, key, description)?)
            })?;
        imported.overviews =
            entries(connection, Keyed::Community("overviews"), &self.overviews, skipped, |key, overview| {
                put_overview(connection, key, overview)
            })?;
        imported.rates = entries(connection, Keyed::Currency, &self.rates, skipped, |code, kept| {
            if kept.rate <= 0.0 {
                return Ok(0); // not a rate: asked for again
            }
            let rate = Rate { code: code.into(), rate: kept.rate, date: kept.date.clone(), asked: kept.asked.clone() };
            Ok(put_rate(connection, &rate, false)?)
        })?;
        for (name, value) in &self.settings {
            imported.settings += put_setting(connection, name, value, false)?;
        }
        imported.ledger_rows = self.write_ledger(connection, &mut imported.skipped)?;
        Ok(imported)
    }

    fn write_vectors(
        &self,
        connection: &Connection,
        vectors: &Vectors,
        skipped: &mut usize,
    ) -> Result<usize, ImportError> {
        if let Some(kept) = width(connection, &vectors.model)?
            && kept != vectors.width
        {
            return Err(ImportError::Vectors {
                path: vectors.path.clone(),
                reason: format!("vectors of {} numbers, where the library keeps {}", vectors.width / 4, kept / 4),
            });
        }
        let mut ids: Vec<&String> = vectors.index.keys().collect();
        ids.sort();
        let mut added = 0;
        for id in ids {
            let (row, hash) = &vectors.index[id];
            let there = connection
                .query_row(
                    "SELECT 1 FROM embeddings WHERE model = ?1 AND paper_id = ?2",
                    params![vectors.model, id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if there {
                continue;
            }
            if *row >= vectors.rows || !exists(connection, id)? {
                *skipped += 1;
                continue;
            }
            let bytes = &vectors.bytes[row * vectors.width..(row + 1) * vectors.width];
            added += put_embedding(connection, &vectors.model, id, hash, bytes)?;
        }
        Ok(added)
    }

    fn write_ledger(&self, connection: &Connection, skipped: &mut usize) -> Result<usize, ImportError> {
        let Some(ledger) = &self.ledger else { return Ok(0) };
        let moved = connection.execute(
            "INSERT INTO imported (source, at) VALUES (?1, ?2) ON CONFLICT (source) DO NOTHING",
            params![LEDGER, now()],
        )?;
        if moved == 0 {
            return Ok(0); // moved before
        }
        let at = ledger.get("since").and_then(Value::as_str).map_or_else(now, |day| format!("{day}T00:00:00Z"));
        let mut rows = 0;
        for (model, entry) in ledger.get("models").and_then(Value::as_object).into_iter().flatten() {
            let Ok(totals) = serde_json::from_value::<KeptTotals>(entry.clone()) else {
                *skipped += 1;
                continue;
            };
            let counts = [totals.calls, totals.input_tokens, totals.output_tokens, totals.unpriced_calls];
            if counts.iter().any(|count| *count < 0) || totals.unpriced_calls > totals.calls || totals.usd < 0.0 {
                *skipped += 1; // not totals the database could keep: left out rather than refusing everything
                continue;
            }
            let priced = totals.calls - totals.unpriced_calls;
            let call = |input_tokens, output_tokens, price_micros| Call {
                model,
                purpose: "imported",
                input_tokens,
                output_tokens,
                cache_read_tokens: 0, // the old ledger did not count them
                cache_write_tokens: 0,
                price_micros,
            };
            // tokens go with the priced calls when there are any: the old ledger did not split them
            if priced > 0 {
                let micros = (totals.usd * 1e6).round() as i64;
                rows +=
                    put_calls(connection, &at, &call(totals.input_tokens, totals.output_tokens, Some(micros)), priced)?;
            }
            if totals.unpriced_calls > 0 {
                let tokens = if priced > 0 { (0, 0) } else { (totals.input_tokens, totals.output_tokens) };
                rows += put_calls(connection, &at, &call(tokens.0, tokens.1, None), totals.unpriced_calls)?;
            }
        }
        Ok(rows)
    }
}

#[derive(Deserialize)]
struct KeptDeclarations {
    parser: u32,
    #[serde(flatten)]
    declarations: Declarations,
}

#[derive(Deserialize)]
struct KeptRate {
    rate: f64,
    date: String,
    asked: String,
}

#[derive(Deserialize)]
struct KeptTotals {
    calls: i64,
    input_tokens: i64,
    output_tokens: i64,
    usd: f64,
    #[serde(default)]
    unpriced_calls: i64,
}

/// What the entries of a `{key: ...}` file are keyed by, with the table they go into.
#[derive(Clone, Copy)]
enum Keyed {
    /// A paper's id: kept only for a paper the library has.
    Paper(&'static str),
    /// A community key.
    Community(&'static str),
    /// A currency's code.
    Currency,
}

/// Write each entry the database does not have yet; returns how many were written, and counts into `skipped`
/// those that do not read as `T` or are about a paper the library does not have.
fn entries<T>(
    connection: &Connection,
    keyed: Keyed,
    entries: &Entries<T>,
    skipped: &mut usize,
    mut put: impl FnMut(&str, &T) -> Result<usize, LibraryError>,
) -> Result<usize, ImportError> {
    let (table, column) = match keyed {
        Keyed::Paper(table) => (table, "paper_id"),
        Keyed::Community(table) => (table, "community_key"),
        Keyed::Currency => ("exchange_rates", "code"),
    };
    let there = format!("SELECT 1 FROM {table} WHERE {column} = ?1");
    let mut written = 0;
    for (key, entry) in entries {
        if connection.query_row(&there, [key], |_| Ok(())).optional()?.is_some() {
            continue;
        }
        let Entry::Readable(value) = entry else {
            *skipped += 1;
            continue;
        };
        if matches!(keyed, Keyed::Paper(_)) && !exists(connection, key)? {
            *skipped += 1;
            continue;
        }
        written += put(key, value)?;
    }
    Ok(written)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::library::{Call, community_key, text_hash};
    use ndarray::array;
    use serde_json::json;

    /// A `.npy` file as numpy writes it: magic, version 1.0, a header padded to 64 bytes, the numbers.
    fn npy(rows: usize, columns: usize, numbers: &[f32], descr: &str) -> Vec<u8> {
        let mut header = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': ({rows}, {columns}), }}");
        while (10 + header.len() + 1) % 64 != 0 {
            header.push(' ');
        }
        header.push('\n');
        let mut bytes = b"\x93NUMPY\x01\x00".to_vec();
        bytes.extend(u16::try_from(header.len()).unwrap().to_le_bytes());
        bytes.extend(header.as_bytes());
        bytes.extend(numbers.iter().flat_map(|number| number.to_le_bytes()));
        bytes
    }

    fn put(path: &Path, content: &Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(content).unwrap()).unwrap();
    }

    fn record(id: &str, venues: &[&str]) -> Value {
        json!({
            "id": id, "title": format!("title of {id}"), "abstract": format!("abstract of {id}"),
            "authors": ["A. Author"], "published": "2026-03-02T10:00:00Z", "updated": "2026-03-02T10:00:00Z",
            "comment": "", "primary_category": "cs.LG", "categories": ["cs.LG"], "venues": venues,
            "url": format!("https://example.org/{id}"), "pdf_url": null
        })
    }

    fn hash_of(id: &str) -> String {
        text_hash(&format!("title of {id}\n\nabstract of {id}"))
    }

    /// A small library as the Python left it: `<data>/library` and `<data>/settings.json`.
    fn files_based_library() -> (tempfile::TempDir, PathBuf) {
        let data = tempfile::tempdir().unwrap();
        let folder = data.path().join("library");
        let lines: Vec<String> =
            [record("2603.00001", &["NeurIPS 2025"]), record("icml-2026-1", &["ICML 2026"]), record("2603.00002", &[])]
                .iter()
                .map(Value::to_string)
                .chain(["{\"id\": \"not a whole paper\", \"categories\": [null]}".to_owned()])
                .collect();
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("papers.jsonl"), lines.join("\n") + "\n\n").unwrap();
        put(
            &folder.join("download_done.json"),
            &json!([
                "venue|ICML 2026|False|official",
                "venue|NeurIPS 2025|False|official",
                "venue|CVPR 2026|False|official",
                "day|2026-03-02|cs.LG"
            ]),
        );
        let qwen = folder.join("embeddings").join("Qwen_Qwen3-Embedding-0.6B");
        fs::create_dir_all(&qwen).unwrap();
        fs::write(qwen.join("vectors.npy"), npy(3, 2, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], "<f4")).unwrap();
        put(
            &qwen.join("index.json"),
            &json!({"2603.00001": [0, hash_of("2603.00001")], "icml-2026-1": [2, "an older text"], "ghost": [1, "x"]}),
        );
        let other = folder.join("embeddings").join("someone_else_model");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("vectors.npy"), npy(1, 3, &[0.5, 0.25, 0.125], "<f4")).unwrap();
        put(&other.join("index.json"), &json!({"2603.00002": [0, hash_of("2603.00002")]}));
        let declared = json!({"parser": 2, "checklist": [{"number": 1, "title": "Claims", "answer": "Yes",
            "justification": "Section 1."}], "statements": [], "flags": []});
        put(
            &folder.join("proceedings").join("declarations.json"),
            &json!({"icml-2026-1": declared, "ghost": declared, "2603.00001": {"parser": 2, "nonsense": true}}),
        );
        // as text: the order of the fields is what is tested, and `json!` would sort them
        fs::write(
            folder.join("reviews.json"),
            r#"{"icml-2026-1": {"reviews": 3, "exact": true, "ratings": {
                "Rating": {"values": [5, 6, 4], "mean": 5.0, "meanings": ["", "", ""]},
                "Clarity": {"values": [3, 3, 2], "mean": 2.67, "meanings": ["good", "good", "fair"]}}},
              "2603.00001": {"reviews": 1, "ratings": {"Rating": {"values": [3], "mean": 3.0, "meanings": [""]}}}}"#,
        )
        .unwrap();
        put(
            &folder.join("ai_detection").join("scores.json"),
            &json!({"2603.00001": [hash_of("2603.00001"), "v1", 1.5], "2603.00002": [hash_of("2603.00002"), "v1", null]}),
        );
        put(
            &folder.join("descriptions.json"),
            &json!({"key-1": {"name": "Verifier rewards", "description": "About."}}),
        );
        put(
            &folder.join("overviews.json"),
            &json!({"key-2": {"title": "T", "summary": "S", "sections": [], "model": "claude-opus-5-5", "papers": 40}}),
        );
        put(
            &folder.join("usage.json"),
            &json!({"since": "2026-10-01",
                "models": {
                    "claude-opus-5-5": {"calls": 3, "input_tokens": 100, "output_tokens": 50, "usd": 0.0123, "unpriced_calls": 0},
                    "mystery": {"calls": 2, "input_tokens": 10, "output_tokens": 5, "usd": 0.0, "unpriced_calls": 2}},
                "purposes": {"naming communities": {"calls": 5, "input_tokens": 110, "output_tokens": 55, "usd": 0.0123,
                    "unpriced_calls": 2}}}),
        );
        put(
            &folder.join("rates.json"),
            &json!({"CAD": {"rate": 1.42, "date": "2026-11-25", "asked": "2026-11-26"},
                    "XXX": {"rate": 0.0, "date": "2026-11-25", "asked": "2026-11-26"}}),
        );
        put(
            &data.path().join("settings.json"),
            &json!({"max_links": 12, "embedding_model": "Qwen/Qwen3-Embedding-0.6B"}),
        );
        (data, folder)
    }

    #[test]
    fn a_files_based_library_is_moved_in_whole() {
        let (_data, folder) = files_based_library();
        let imported = import_files(&folder).unwrap();
        assert_eq!(imported.papers, 3);
        assert_eq!(imported.venue_tags, 2);
        assert_eq!(imported.slices, 4);
        assert_eq!(imported.embeddings["Qwen/Qwen3-Embedding-0.6B"], 2);
        assert_eq!(imported.embeddings["someone_else_model"], 1);
        assert_eq!((imported.declarations, imported.ratings, imported.ai_scores), (1, 2, 2));
        assert_eq!((imported.descriptions, imported.overviews, imported.settings, imported.rates), (1, 1, 2, 1));
        assert_eq!(imported.ledger_rows, 2);
        // a record that is not a paper, a vector and a declaration of a paper not in the library, a damaged one
        assert_eq!(imported.skipped, 4);

        let library = Library::open(&folder).unwrap();
        let papers = library.papers(&["2603.00001", "icml-2026-1", "2603.00002"]).unwrap();
        let papers: Vec<Paper> = papers.into_iter().map(Option::unwrap).collect();
        assert_eq!(papers[0].venues, ["NeurIPS 2025"]);
        assert_eq!(papers[1].url.as_deref(), Some("https://example.org/icml-2026-1"));

        // the conference with its own list is finished; the one that fell back to arXiv, and the one with no
        // papers at all, are fetched again
        assert!(!library.needs_fetching("venue|ICML 2026|False|official").unwrap());
        assert_eq!(
            library.slice_outcome("venue|NeurIPS 2025|False|official").unwrap(),
            Some(SliceOutcome::ArxivComments)
        );
        assert_eq!(library.slice_outcome("venue|CVPR 2026|False|official").unwrap(), Some(SliceOutcome::Incomplete));
        assert!(!library.needs_fetching("day|2026-03-02|cs.LG").unwrap());

        let qwen = "Qwen/Qwen3-Embedding-0.6B";
        assert_eq!(library.embedded(qwen, &papers).unwrap(), [true, false, false]); // icml-2026-1's text changed
        assert_eq!(library.vectors(qwen, &papers[..1]).unwrap(), array![[1.0_f32, 2.0]]);
        assert_eq!(library.vectors("someone_else_model", &papers[2..]).unwrap(), array![[0.5_f32, 0.25, 0.125]]);

        assert!(matches!(library.declarations("icml-2026-1", 2).unwrap(), Some(Reading::Read(_))));
        let ratings = library.ratings("icml-2026-1").unwrap().unwrap();
        assert_eq!(ratings.ratings[1].0, "Clarity");
        assert!(!library.ratings("2603.00001").unwrap().unwrap().exact); // read before the field existed
        assert_eq!(library.rate("XXX").unwrap(), None);
        assert_eq!(
            library.ai_scores("v1", &papers).unwrap(),
            [Some(AiScore::Score(1.5)), None, Some(AiScore::TooShort)]
        );
        assert_eq!(library.description("key-1").unwrap().unwrap().name, "Verifier rewards");
        assert_eq!(library.overview("key-2").unwrap().unwrap().papers, 40);
        assert_eq!(library.rate("CAD").unwrap().unwrap().rate, 1.42);
        assert_eq!(library.settings().unwrap()["max_links"], 12);

        let spent = library.spending().unwrap();
        assert_eq!(spent.since.as_deref(), Some("2026-10-01"));
        assert_eq!(spent.total.calls, 5);
        assert_eq!(spent.total.price_micros, 12_300);
        assert_eq!(spent.total.unpriced_calls, 2);
        assert_eq!((spent.total.input_tokens, spent.total.output_tokens), (110, 55));
        assert_eq!(spent.models["mystery"].input_tokens, 10);
    }

    #[test]
    fn importing_twice_is_importing_once_and_newer_rows_are_kept() {
        let (_data, folder) = files_based_library();
        let library = Library::open(&folder).unwrap();
        library.import_files(&folder).unwrap();
        // the engine goes on working: a newer reading and a newer setting than the files have
        library.change_settings(json!({"max_links": 30}).as_object().unwrap()).unwrap();
        library
            .record_call(&Call {
                model: "m",
                purpose: "p",
                input_tokens: 1,
                output_tokens: 1,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                price_micros: Some(1),
            })
            .unwrap();
        // NeurIPS 2025 is fetched again and its official list replaces the arXiv papers that stood in for it
        let official = serde_json::from_value::<Paper>(record("neurips-2025-1", &[])).unwrap();
        library
            .add_slice(
                "venue|NeurIPS 2025|False|official",
                SliceOutcome::Complete,
                &[official],
                Some("NeurIPS 2025"),
                Known::Keep,
            )
            .unwrap();
        let before = library.spending().unwrap();
        let again = library.import_files(&folder).unwrap();
        assert_eq!(again, Imported { skipped: 4, embeddings: again.embeddings.clone(), ..Imported::default() });
        assert!(library.paper("2603.00001").unwrap().unwrap().venues.is_empty()); // not tagged again
        assert!(!library.needs_fetching("venue|NeurIPS 2025|False|official").unwrap());
        assert!(again.embeddings.values().all(|added| *added == 0));
        assert_eq!(library.spending().unwrap(), before);
        assert_eq!(library.settings().unwrap()["max_links"], 30);
        assert_eq!(library.paper_count().unwrap(), 4); // the three moved in, and the official one fetched since
    }

    #[test]
    fn the_move_at_start_is_done_once_and_one_interrupted_is_done_again_whole() {
        let (_data, folder) = files_based_library();
        // interrupted: a damaged file stops it, and nothing of it is kept, nor its record
        let papers = fs::read_to_string(folder.join("papers.jsonl")).unwrap();
        fs::write(folder.join("papers.jsonl"), format!("{{\"id\": \"damaged\n{papers}")).unwrap();
        let library = Library::open(&folder).unwrap();
        assert!(library.import_once(&folder).is_err());
        assert_eq!(library.paper_count().unwrap(), 0);
        // the next start goes through, whole
        fs::write(folder.join("papers.jsonl"), papers).unwrap();
        let first = library.import_once(&folder).unwrap().unwrap();
        assert_eq!((first.papers, first.embeddings["Qwen/Qwen3-Embedding-0.6B"]), (3, 2));
        // and is never done again: no duplicate, the old files not even read
        fs::write(folder.join("papers.jsonl"), "this file is not read again").unwrap();
        assert_eq!(library.import_once(&folder).unwrap(), None);
        assert_eq!(library.paper_count().unwrap(), 3);
        assert_eq!(library.embedding_counts().unwrap()["Qwen/Qwen3-Embedding-0.6B"], 2);
    }

    /// The Python's detector files beside the fixture library (synthetic content): a pool of two, one mirror.
    fn with_detector(folder: &Path) {
        put(
            &folder.join("ai_detection").join("training.json"),
            &json!({
                "pool": [{"id": "1906.00001", "title": "One", "abstract": "Abstract one."},
                         {"id": "1906.00002", "title": "Two", "abstract": "Abstract two."}],
                "machine": [{"id": "1906.00001", "kind": "written", "generator": "claude-sonnet-5-5", "text": "W."}],
            }),
        );
    }

    #[test]
    fn the_old_data_folder_is_moved_in_part_by_part_in_order_and_never_again() {
        let (_data, folder) = files_based_library();
        with_detector(&folder);
        let library = Library::open(&folder).unwrap();
        let moved = library.import_old(&folder);
        let Part::Moved { added: files } = &moved.files else { panic!("{moved:?}") };
        assert_eq!((files.papers, files.ledger_rows), (3, 2), "the ledger goes with the files when they move");
        assert_eq!(moved.ledger, Part::Already, "and is not moved again on its own");
        let Part::Moved { added: detector } = &moved.detector else { panic!("{moved:?}") };
        assert_eq!((detector.humans, detector.mirrors), (2, 1));
        assert!(moved.problems().is_empty());
        // the next start: every part is recorded as done, and no file is read again
        fs::write(folder.join("papers.jsonl"), "not read again").unwrap();
        fs::write(folder.join("ai_detection").join("training.json"), "not read again").unwrap();
        assert_eq!(library.import_old(&folder), Moved::default());
        assert_eq!(library.detector_counts(crate::library::detector::POOL).unwrap(), (2, 1), "nothing twice");
        assert_eq!(library.spending().unwrap().total.calls, 5);
    }

    #[test]
    fn a_part_that_cannot_be_moved_is_told_keeps_nothing_and_is_moved_at_the_next_start_while_the_others_are_kept() {
        let (_data, folder) = files_based_library();
        with_detector(&folder);
        let papers = fs::read_to_string(folder.join("papers.jsonl")).unwrap();
        fs::write(folder.join("papers.jsonl"), format!("{{\"id\": \"damaged\n{papers}")).unwrap();
        let library = Library::open(&folder).unwrap();
        let moved = library.import_old(&folder);
        assert!(matches!(&moved.files, Part::Failed { why } if why.contains("papers.jsonl, line 1")), "{moved:?}");
        assert_eq!(moved.ledger, Part::Moved { added: 2 }, "what was spent is moved without the papers");
        assert!(matches!(moved.detector, Part::Moved { .. }));
        let problems = moved.problems();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("papers.jsonl") && problems[0].contains("next start"), "{problems:?}");
        assert_eq!(library.paper_count().unwrap(), 0, "nothing of the failed part is kept");
        // mended before the next start: that part is moved, whole, and only it
        fs::write(folder.join("papers.jsonl"), papers).unwrap();
        let again = library.import_old(&folder);
        let Part::Moved { added: files } = &again.files else { panic!("{again:?}") };
        assert_eq!((files.papers, files.ledger_rows), (3, 0), "the ledger is not counted twice");
        assert_eq!((again.ledger, again.detector), (Part::Already, Part::Already));
        assert_eq!(library.spending().unwrap().total.calls, 5);
    }

    #[test]
    fn a_damaged_ledger_or_training_file_keeps_out_only_its_own_part_says_so_without_the_folders_place_and_is_moved_once_mended()
     {
        let (data, folder) = files_based_library();
        with_detector(&folder);
        let (ledger, training) = (folder.join("usage.json"), folder.join("ai_detection").join("training.json"));
        let (ledger_text, training_text) =
            (fs::read_to_string(&ledger).unwrap(), fs::read_to_string(&training).unwrap());
        fs::write(&ledger, "{ cut short").unwrap();
        fs::write(&training, "{ cut short").unwrap();
        let library = Library::open(&folder).unwrap();
        let moved = library.import_old(&folder);
        let Part::Moved { added: files } = &moved.files else { panic!("{moved:?}") };
        assert_eq!((files.papers, files.ledger_rows), (3, 0), "the papers are moved without the ledger");
        assert!(matches!(&moved.ledger, Part::Failed { why } if why.contains("usage.json")), "{moved:?}");
        assert!(matches!(&moved.detector, Part::Failed { why } if why.contains("training.json")), "{moved:?}");
        let problems = moved.problems();
        assert_eq!(problems.len(), 2);
        let place = data.path().display().to_string();
        assert!(
            problems.iter().all(|problem| !problem.contains(&place) && problem.contains("the data folder")),
            "{problems:?}"
        );
        fs::write(&ledger, ledger_text).unwrap();
        fs::write(&training, training_text).unwrap();
        let again = library.import_old(&folder);
        assert_eq!((again.files.clone(), again.ledger.clone()), (Part::Already, Part::Moved { added: 2 }));
        assert!(matches!(again.detector, Part::Moved { .. }), "{again:?}");
        assert_eq!(library.detector_counts(crate::library::detector::POOL).unwrap(), (2, 1));
    }

    #[test]
    fn a_detector_move_stopped_before_its_record_is_done_again_without_doubling() {
        let (_data, folder) = files_based_library();
        with_detector(&folder);
        let library = Library::open(&folder).unwrap();
        library.import_detector(&folder).unwrap(); // moved, and then the app closed before it was recorded
        let moved = library.import_old(&folder);
        assert!(matches!(moved.detector, Part::Moved { .. }), "{moved:?}");
        assert_eq!(library.detector_counts(crate::library::detector::POOL).unwrap(), (2, 1));
    }

    #[test]
    fn a_last_paper_cut_short_by_a_crash_is_skipped_and_the_rest_moved_in() {
        let (_data, folder) = files_based_library();
        let papers = fs::read_to_string(folder.join("papers.jsonl")).unwrap();
        fs::write(folder.join("papers.jsonl"), format!("{papers}{{\"id\": \"2603.0000")).unwrap();
        let imported = import_files(&folder).unwrap();
        assert_eq!((imported.papers, imported.skipped), (3, 5));
    }

    #[test]
    fn a_data_folder_without_an_old_library_has_nothing_to_move() {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(&data.path().join("library")).unwrap();
        assert_eq!(library.import_once(&data.path().join("library")).unwrap(), None);
    }

    #[test]
    fn the_ledger_alone_is_moved_once_and_a_later_whole_import_does_not_count_it_again() {
        let (_data, folder) = files_based_library();
        let library = Library::open(&folder).unwrap();
        assert_eq!(library.import_ledger(&folder).unwrap(), 2);
        assert_eq!(library.paper_count().unwrap(), 0, "only the ledger is moved");
        let spent = library.spending().unwrap();
        assert_eq!(
            (spent.since.as_deref(), spent.total.calls, spent.total.price_micros),
            (Some("2026-10-01"), 5, 12_300)
        );
        assert_eq!(library.import_ledger(&folder).unwrap(), 0, "moving it twice is moving it once");
        let whole = library.import_files(&folder).unwrap();
        assert_eq!((whole.papers, whole.ledger_rows), (3, 0));
        assert_eq!(library.spending().unwrap(), spent);
    }

    #[test]
    fn a_ledger_moved_with_the_whole_library_is_not_moved_again_alone() {
        let (_data, folder) = files_based_library();
        let library = Library::open(&folder).unwrap();
        assert_eq!(library.import_files(&folder).unwrap().ledger_rows, 2);
        assert_eq!(library.import_ledger(&folder).unwrap(), 0);
        assert_eq!(library.spending().unwrap().total.calls, 5);
    }

    #[test]
    fn no_old_ledger_is_nothing_to_move_and_a_damaged_one_is_an_error_naming_it() {
        let data = tempfile::tempdir().unwrap();
        let folder = data.path().join("library");
        let library = Library::open(&folder).unwrap();
        assert_eq!(library.import_ledger(&folder).unwrap(), 0);
        fs::write(folder.join("usage.json"), "{ not json").unwrap();
        let error = library.import_ledger(&folder).unwrap_err();
        assert!(error.to_string().contains("usage.json"), "{error}");
        assert_eq!(library.spending().unwrap().total.calls, 0);
        // once it can be read, it is moved: a damaged file marked nothing as moved
        put(
            &folder.join("usage.json"),
            &json!({"since": "2026-09-01", "models": {"claude-haiku-5-5":
            {"calls": 1, "input_tokens": 7, "output_tokens": 3, "usd": 0.0000022, "unpriced_calls": 0}}}),
        );
        assert_eq!(library.import_ledger(&folder).unwrap(), 1);
    }

    #[test]
    fn an_empty_folder_imports_nothing() {
        let data = tempfile::tempdir().unwrap();
        let imported = import_files(&data.path().join("library")).unwrap();
        assert_eq!(imported, Imported::default());
    }

    #[test]
    fn a_damaged_file_stops_the_import_and_names_the_file_and_line() {
        let (_data, folder) = files_based_library();
        let mut text = fs::read_to_string(folder.join("papers.jsonl")).unwrap();
        text.insert_str(0, "{\"id\": \"half a rec\n");
        fs::write(folder.join("papers.jsonl"), text).unwrap();
        let error = import_files(&folder).unwrap_err();
        assert!(matches!(error, ImportError::Line { line: 1, .. }), "{error:?}");
        assert!(error.to_string().contains("papers.jsonl"));
        assert_eq!(Library::open(&folder).unwrap().paper_count().unwrap(), 0); // nothing half-imported
        let (_data, folder) = files_based_library();
        fs::write(folder.join("reviews.json"), "{ not json").unwrap();
        assert!(import_files(&folder).unwrap_err().to_string().contains("reviews.json"));
        assert_eq!(Library::open(&folder).unwrap().paper_count().unwrap(), 0);
    }

    #[test]
    fn only_little_endian_float32_matrices_are_read_as_vectors() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("vectors.npy");
        fs::write(&path, npy(2, 2, &[1.0, 2.0, 3.0, 4.0], "<f4")).unwrap();
        let (rows, columns, bytes) = read_npy(&path).unwrap();
        assert_eq!((rows, columns, bytes.len()), (2, 2, 16));
        assert_eq!(f32::from_le_bytes(bytes[12..16].try_into().unwrap()), 4.0);
        for (bytes, why) in [
            (npy(2, 2, &[1.0, 2.0, 3.0, 4.0], "<f8"), "float64"),
            (npy(2, 2, &[1.0, 2.0, 3.0, 4.0], ">f4"), "big-endian"),
            (npy(2, 2, &[1.0, 2.0, 3.0], "<f4"), "cut short"),
            (b"not numpy at all".to_vec(), "not .npy"),
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(matches!(read_npy(&path), Err(ImportError::Vectors { .. })), "{why}");
        }
    }

    #[test]
    fn a_model_is_found_from_its_folder_name() {
        assert_eq!(folder_name("Qwen/Qwen3-Embedding-0.6B"), "Qwen_Qwen3-Embedding-0.6B");
        assert_eq!(folder_name("test/model"), "test_model");
        assert_eq!(community_key("m", ["a"]).len(), 40); // the keys of the files are moved as they are
    }
}
