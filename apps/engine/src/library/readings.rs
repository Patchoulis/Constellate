//! What was read from a paper: its authors' declarations, its reviewers' ratings, and the AI-text estimate of its
//! abstract. Each is a program's reading, kept so that it is done once, and each is kept with what decides
//! whether it is still valid: the version of the reader, or the classifier and the hash of the text it scored.
//!
//! A paper whose PDF could not be had, or had no text to read, is kept as exactly that ([`Reading::NoPdf`],
//! [`Reading::Unreadable`]). The Python did not keep those, so they were tried again on every run; and an
//! outcome kept as an empty reading would have looked like "read, nothing declared", which is a statement the
//! authors did not make.

use std::collections::HashMap;
use std::fmt;

use rusqlite::{Connection, OptionalExtension, params};
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{Library, LibraryError, Paper, json_column, now};

/// A checklist answer, as the reader accepts them; anything else ("[TODO]") is not an answer and is left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Answer {
    Yes,
    No,
    #[serde(rename = "NA")]
    NotApplicable,
}

/// One item of a NeurIPS-style checklist.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItem {
    pub number: u32,
    pub title: String,
    pub answer: Answer,
    pub justification: String,
}

/// A free-text statement found by its heading: impact, ethics, use of language models, ...
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    pub kind: String,
    pub title: String,
    pub text: String,
}

/// What the authors of a paper declare in it (the contract's `Declarations`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declarations {
    pub checklist: Vec<ChecklistItem>,
    pub statements: Vec<Statement>,
    /// The flags these raise, in the reader's order.
    pub flags: Vec<String>,
}

/// What came of reading a paper's PDF.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reading {
    Read(Declarations),
    /// The paper has no PDF a program may fetch.
    NoPdf,
    /// Its PDF has no text that can be read (a scan, a broken file).
    Unreadable,
}

/// One field's scores, one per review.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldRatings {
    pub values: Vec<u32>,
    pub mean: f64,
    /// What each score means, as the page said ("fair").
    pub meanings: Vec<String>,
}

/// The scores reviewers gave a paper (the contract's `Ratings`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ratings {
    /// How many reviews gave scores.
    pub reviews: u32,
    /// Were the reviews told apart by their headings? If not, the count is a guess. Readings older than this field
    /// lack it and are taken as not exact, as the Python took them.
    #[serde(default)]
    pub exact: bool,
    /// By field, in the order shown: overall judgement first. A JSON object, whose order is kept.
    #[serde(serialize_with = "fields_as_object", deserialize_with = "fields_from_object")]
    pub ratings: Vec<(String, FieldRatings)>,
}

fn fields_as_object<S: Serializer>(fields: &[(String, FieldRatings)], serializer: S) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(fields.len()))?;
    for (field, ratings) in fields {
        map.serialize_entry(field, ratings)?;
    }
    map.end()
}

fn fields_from_object<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<(String, FieldRatings)>, D::Error> {
    struct InOrder;
    impl<'de> Visitor<'de> for InOrder {
        type Value = Vec<(String, FieldRatings)>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object of fields and their scores")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut fields = Vec::new();
            while let Some(entry) = map.next_entry()? {
                fields.push(entry);
            }
            Ok(fields)
        }
    }
    deserializer.deserialize_map(InOrder)
}

impl Ratings {
    /// Does this reading say more than `other`: told apart by headings where the other was not, or as exact and
    /// with more reviews? A page still loading shows fewer reviews and must not replace a full reading.
    fn says_less_than(&self, other: &Self) -> bool {
        (self.exact, self.reviews) < (other.exact, other.reviews)
    }
}

/// The AI-text estimate of one abstract.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AiScore {
    /// Log-odds of "machine": higher is more machine-like.
    Score(f64),
    /// Too short to judge: there is no verdict, and none is to be computed again.
    TooShort,
}

pub(super) fn put_declarations(
    connection: &Connection,
    paper_id: &str,
    reader: u32,
    reading: &Reading,
) -> Result<usize, LibraryError> {
    let (outcome, content) = match reading {
        Reading::Read(declarations) => ("read", Some(serde_json::to_string(declarations)?)),
        Reading::NoPdf => ("no_pdf", None),
        Reading::Unreadable => ("unreadable", None),
    };
    let mut statement = connection.prepare_cached(
        "INSERT INTO declarations (paper_id, reader, outcome, content, read_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (paper_id) DO UPDATE SET
             reader = excluded.reader, outcome = excluded.outcome, content = excluded.content, read_at = excluded.read_at",
    )?;
    Ok(statement.execute(params![paper_id, reader, outcome, content, now()])?)
}

pub(super) fn put_ratings(connection: &Connection, paper_id: &str, ratings: &Ratings) -> Result<usize, LibraryError> {
    let mut statement = connection.prepare_cached(
        "INSERT INTO ratings (paper_id, content, read_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (paper_id) DO UPDATE SET content = excluded.content, read_at = excluded.read_at",
    )?;
    Ok(statement.execute(params![paper_id, serde_json::to_string(ratings)?, now()])?)
}

pub(super) fn put_ai_score(
    connection: &Connection,
    paper_id: &str,
    text_hash: &str,
    classifier: &str,
    score: AiScore,
) -> rusqlite::Result<usize> {
    let score = match score {
        AiScore::Score(score) => Some(score),
        AiScore::TooShort => None,
    };
    let mut statement = connection.prepare_cached(
        "INSERT INTO ai_scores (paper_id, text_hash, classifier, score, scored_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (paper_id) DO UPDATE SET
             text_hash = excluded.text_hash, classifier = excluded.classifier, score = excluded.score,
             scored_at = excluded.scored_at",
    )?;
    statement.execute(params![paper_id, text_hash, classifier, score, now()])
}

fn reading_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Reading> {
    let outcome: String = row.get(0)?;
    Ok(match outcome.as_str() {
        "no_pdf" => Reading::NoPdf,
        "unreadable" => Reading::Unreadable,
        _ => Reading::Read(json_column(row, 1)?),
    })
}

pub(super) fn ratings_of(connection: &Connection, paper_id: &str) -> rusqlite::Result<Option<Ratings>> {
    connection
        .query_row("SELECT content FROM ratings WHERE paper_id = ?1", [paper_id], |row| json_column(row, 0))
        .optional()
}

/// [`Library::keep_ratings`] inside a write that may do more (keeping a paper's whole reviews page with them).
pub(super) fn keep_ratings_in(
    connection: &Connection,
    paper_id: &str,
    ratings: Ratings,
) -> Result<Ratings, LibraryError> {
    if let Some(before) = ratings_of(connection, paper_id)?
        && ratings.says_less_than(&before)
    {
        return Ok(before);
    }
    put_ratings(connection, paper_id, &ratings)?;
    Ok(ratings)
}

impl From<&crate::text::reviews::Ratings> for Ratings {
    /// The scores read from a reviews page, as the library keeps them: fields in the order shown.
    fn from(read: &crate::text::reviews::Ratings) -> Self {
        Ratings {
            reviews: u32::try_from(read.reviews).unwrap_or(u32::MAX),
            exact: read.exact,
            ratings: read
                .ratings
                .iter()
                .map(|(field, scores)| {
                    let ratings = FieldRatings {
                        values: scores.values.iter().map(|&value| u32::from(value)).collect(),
                        mean: scores.mean,
                        meanings: scores.meanings.clone(),
                    };
                    (field.name().to_owned(), ratings)
                })
                .collect(),
        }
    }
}

impl Library {
    /// What `reader` found in the paper, if this version of the reader has read it (an older version's reading
    /// is not returned: it is to be redone).
    pub fn declarations(&self, paper_id: &str, reader: u32) -> Result<Option<Reading>, LibraryError> {
        self.read(|connection| {
            Ok(connection
                .query_row(
                    "SELECT outcome, content FROM declarations WHERE paper_id = ?1 AND reader = ?2",
                    params![paper_id, reader],
                    reading_from_row,
                )
                .optional()?)
        })
    }

    /// Every paper this version of the reader has read, with what came of it.
    pub fn all_declarations(&self, reader: u32) -> Result<HashMap<String, Reading>, LibraryError> {
        self.read(|connection| {
            let mut statement =
                connection.prepare("SELECT outcome, content, paper_id FROM declarations WHERE reader = ?1")?;
            let rows = statement.query_map([reader], |row| Ok((row.get(2)?, reading_from_row(row)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Keep what `reader` found in several papers, in one transaction.
    pub fn keep_declarations(&self, reader: u32, readings: &[(&str, Reading)]) -> Result<(), LibraryError> {
        self.write(|transaction| {
            for (paper_id, reading) in readings {
                put_declarations(transaction, paper_id, reader, reading)?;
            }
            Ok(())
        })
    }

    pub fn ratings(&self, paper_id: &str) -> Result<Option<Ratings>, LibraryError> {
        self.read(|connection| Ok(ratings_of(connection, paper_id)?))
    }

    /// Every paper's ratings.
    pub fn all_ratings(&self) -> Result<HashMap<String, Ratings>, LibraryError> {
        self.read(|connection| {
            let mut statement = connection.prepare("SELECT paper_id, content FROM ratings")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, json_column(row, 1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Keep a reading of a paper's reviews page, unless the one kept says more; returns what is kept now. The
    /// comparison and the write are one transaction, so two pages read at once cannot both win.
    pub fn keep_ratings(&self, paper_id: &str, ratings: Ratings) -> Result<Ratings, LibraryError> {
        self.write(|transaction| keep_ratings_in(transaction, paper_id, ratings))
    }

    /// For each paper, its score by `classifier` if that classifier scored exactly its present text.
    pub fn ai_scores(&self, classifier: &str, papers: &[Paper]) -> Result<Vec<Option<AiScore>>, LibraryError> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT score FROM ai_scores WHERE paper_id = ?1 AND classifier = ?2 AND text_hash = ?3",
            )?;
            papers
                .iter()
                .map(|paper| {
                    let found: Option<Option<f64>> = statement
                        .query_row(params![paper.id, classifier, paper.text_hash()], |row| row.get(0))
                        .optional()?;
                    Ok(found.map(|score| score.map_or(AiScore::TooShort, AiScore::Score)))
                })
                .collect()
        })
    }

    /// Keep scores `classifier` gave papers' present texts, in one transaction.
    pub fn keep_ai_scores(&self, classifier: &str, scores: &[(&Paper, AiScore)]) -> Result<(), LibraryError> {
        self.write(|transaction| {
            for (paper, score) in scores {
                put_ai_score(transaction, &paper.id, &paper.text_hash(), classifier, *score)?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::library::Known;
    use crate::library::papers::tests::paper;

    fn library_with(papers: &[Paper]) -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        library.add_papers(papers, None, Known::Keep).unwrap();
        (folder, library)
    }

    fn declared() -> Declarations {
        Declarations {
            checklist: vec![ChecklistItem {
                number: 1,
                title: "Claims".into(),
                answer: Answer::NotApplicable,
                justification: "See section 3.".into(),
            }],
            statements: vec![Statement { kind: "llm".into(), title: "LLM Usage".into(), text: "We used one.".into() }],
            flags: vec!["llm_declared".into()],
        }
    }

    fn ratings(reviews: u32, exact: bool) -> Ratings {
        let field = |values: Vec<u32>| FieldRatings {
            mean: f64::from(values.iter().sum::<u32>()) / values.len() as f64,
            meanings: values.iter().map(|_| "fair".to_owned()).collect(),
            values,
        };
        Ratings {
            reviews,
            exact,
            ratings: vec![("Rating".into(), field(vec![5; reviews as usize])), ("Clarity".into(), field(vec![2, 3]))],
        }
    }

    #[test]
    fn declarations_and_the_outcomes_without_any_are_kept_as_such() {
        let (_folder, library) = library_with(&[paper("read"), paper("scan"), paper("nowhere")]);
        library
            .keep_declarations(
                2,
                &[("read", Reading::Read(declared())), ("scan", Reading::Unreadable), ("nowhere", Reading::NoPdf)],
            )
            .unwrap();
        assert_eq!(library.declarations("read", 2).unwrap(), Some(Reading::Read(declared())));
        assert_eq!(library.declarations("scan", 2).unwrap(), Some(Reading::Unreadable));
        assert_eq!(library.declarations("nowhere", 2).unwrap(), Some(Reading::NoPdf));
        assert_eq!(library.all_declarations(2).unwrap().len(), 3);
        // a better reader reads again: what the older one found is not handed out as its own
        assert_eq!(library.declarations("read", 3).unwrap(), None);
        assert!(library.all_declarations(3).unwrap().is_empty());
    }

    #[test]
    fn a_reading_of_a_paper_the_library_does_not_have_is_refused_whole() {
        let (_folder, library) = library_with(&[paper("known")]);
        let readings = [("known", Reading::NoPdf), ("unknown", Reading::NoPdf)];
        assert!(library.keep_declarations(2, &readings).is_err());
        assert_eq!(library.declarations("known", 2).unwrap(), None);
    }

    #[test]
    fn the_checklist_answers_are_written_as_the_interface_reads_them() {
        let json = serde_json::to_value(declared()).unwrap();
        assert_eq!(json["checklist"][0]["answer"], "NA");
    }

    #[test]
    fn ratings_keep_their_field_order_and_a_fuller_reading_wins() {
        let (_folder, library) = library_with(&[paper("p")]);
        assert_eq!(library.ratings("p").unwrap(), None);
        assert_eq!(library.keep_ratings("p", ratings(3, true)).unwrap(), ratings(3, true));
        assert_eq!(library.keep_ratings("p", ratings(2, true)).unwrap(), ratings(3, true)); // still loading
        assert_eq!(library.keep_ratings("p", ratings(9, false)).unwrap(), ratings(3, true)); // a guess
        assert_eq!(library.keep_ratings("p", ratings(4, true)).unwrap(), ratings(4, true));
        let kept = library.ratings("p").unwrap().unwrap();
        let fields: Vec<&str> = kept.ratings.iter().map(|(field, _)| field.as_str()).collect();
        assert_eq!(fields, ["Rating", "Clarity"]);
        let text = serde_json::to_string(&kept).unwrap();
        assert!(text.find("\"Rating\"").unwrap() < text.find("\"Clarity\"").unwrap());
        assert_eq!(library.all_ratings().unwrap().len(), 1);
    }

    #[test]
    fn an_ai_score_is_reused_only_for_the_same_classifier_and_text() {
        let (short, long) = (paper("short"), paper("long"));
        let (_folder, library) = library_with(&[short.clone(), long.clone()]);
        library.keep_ai_scores("c1", &[(&short, AiScore::TooShort), (&long, AiScore::Score(-1.25))]).unwrap();
        assert_eq!(
            library.ai_scores("c1", &[long.clone(), short.clone()]).unwrap(),
            [Some(AiScore::Score(-1.25)), Some(AiScore::TooShort)]
        );
        assert_eq!(library.ai_scores("c2", std::slice::from_ref(&long)).unwrap(), [None]);
        let mut revised = long.clone();
        revised.r#abstract = "rewritten".into();
        assert_eq!(library.ai_scores("c1", std::slice::from_ref(&revised)).unwrap(), [None]);
        library.keep_ai_scores("c2", &[(&revised, AiScore::Score(3.5))]).unwrap();
        assert_eq!(library.ai_scores("c2", &[revised]).unwrap(), [Some(AiScore::Score(3.5))]);
        assert_eq!(library.ai_scores("c1", &[long]).unwrap(), [None]); // one score per paper: the latest
    }

    #[test]
    fn ratings_from_many_threads_settle_on_the_fullest() {
        let (_folder, library) = library_with(&[paper("p")]);
        std::thread::scope(|scope| {
            for reviews in 1..=8 {
                let library = &library;
                scope.spawn(move || library.keep_ratings("p", ratings(reviews, true)).unwrap());
            }
        });
        assert_eq!(library.ratings("p").unwrap().unwrap().reviews, 8);
    }
}
