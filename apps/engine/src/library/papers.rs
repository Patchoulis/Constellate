//! Papers, the venues they are listed at, the papers a selection picks, and which download slices are finished.
//!
//! A paper already in the library keeps its record when it is downloaded again, unless the caller asks for the
//! new one ([`Known::Replace`]). A venue's list is the whole venue: fetching it again tags the papers on the new
//! list and untags those no longer on it, in the same transaction as the papers themselves.
//!
//! A slice (one day of arXiv, one conference) is finished only when it was fetched [`SliceOutcome::Complete`]ly.
//! The Python recorded every slice it got through as finished, so a conference whose site was down for a moment
//! was filed as downloaded from its official list when it had fallen back to arXiv comments, and was never asked
//! again. Here the outcome is kept as what it was, and anything short of complete is fetched again next time.

use std::sync::LazyLock;

use chrono::Days;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use super::{Library, LibraryError, json_column, now, placeholders};
use crate::selection::Selection;

#[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
static CONFERENCE_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z]+-\d{4}-\d+$").unwrap());

/// A paper as downloaded, with the field names the Python and the interface use.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Paper {
    pub id: String,
    pub title: String,
    pub r#abstract: String,
    #[serde(default)]
    pub authors: Vec<String>,
    /// arXiv: when it was submitted. A conference: when it is presented.
    pub published: String,
    #[serde(default)]
    pub updated: String,
    /// arXiv's comment, or a conference's decision ("Accept (oral)").
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub primary_category: String,
    #[serde(default)]
    pub categories: Vec<String>,
    /// The venues whose lists the paper is on, sorted. Kept by the library: what a download hands in is ignored.
    #[serde(default)]
    pub venues: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub pdf_url: Option<String>,
}

/// Where a paper's record came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Arxiv,
    /// A conference's own list; its "published" is when the paper is presented, not when it was submitted.
    Conference,
}

impl Origin {
    fn as_str(self) -> &'static str {
        match self {
            Self::Arxiv => "arxiv",
            Self::Conference => "conference",
        }
    }
}

impl Paper {
    /// Conference records are named "<conference>-<year>-<number>"; anything else is an arXiv id.
    pub fn origin(&self) -> Origin {
        if CONFERENCE_ID.is_match(&self.id) { Origin::Conference } else { Origin::Arxiv }
    }

    /// What is embedded and scored: the title, a blank line, the abstract.
    pub fn text(&self) -> String {
        format!("{}\n\n{}", self.title, self.r#abstract)
    }

    pub fn text_hash(&self) -> String {
        text_hash(&self.text())
    }
}

/// The first 16 hex digits of the SHA-1 of a text, as the Python computes it: an embedding or score is reused only
/// while its paper's text has this hash.
pub fn text_hash(text: &str) -> String {
    Sha1::digest(text.as_bytes()).iter().take(8).map(|byte| format!("{byte:02x}")).collect()
}

/// What to do with a paper the library already has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Known {
    /// Keep the record the library has.
    Keep,
    /// Replace it with the one handed in (a download asked to be redone).
    Replace,
}

/// How many papers handed in were new to the library, and how many it had already.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Added {
    pub new: usize,
    pub known: usize,
}

/// How a slice of a selection was fetched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceOutcome {
    /// The source's own whole list: a conference's official list of accepted papers, or arXiv's answer for a day
    /// that can no longer gain papers. Not fetched again.
    Complete,
    /// The conference had no official list (or its site did not answer), so arXiv papers whose comment names it
    /// stood in. Fetched again, in case the list appears.
    ArxivComments,
    /// Fetched as far as it could be (a recent day that can still grow, a download cut short). Fetched again.
    Incomplete,
}

impl SliceOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::ArxivComments => "arxiv_comments",
            Self::Incomplete => "incomplete",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [Self::Complete, Self::ArxivComments, Self::Incomplete].into_iter().find(|outcome| outcome.as_str() == text)
    }
}

const COLUMNS: &str =
    "id, title, abstract, authors, published, updated, comment, primary_category, categories, url, pdf_url";

fn paper_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Paper> {
    Ok(Paper {
        id: row.get(0)?,
        title: row.get(1)?,
        r#abstract: row.get(2)?,
        authors: json_column(row, 3)?,
        published: row.get(4)?,
        updated: row.get(5)?,
        comment: row.get(6)?,
        primary_category: row.get(7)?,
        categories: json_column(row, 8)?,
        venues: Vec::new(),
        url: row.get(9)?,
        pdf_url: row.get(10)?,
    })
}

/// Papers read by `sql` (which selects [`COLUMNS`]), each with its venues.
fn papers_where(connection: &Connection, sql: &str, values: impl rusqlite::Params) -> Result<Vec<Paper>, LibraryError> {
    let mut statement = connection.prepare_cached(sql)?;
    let mut papers = statement.query_map(values, paper_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut venues = connection.prepare_cached("SELECT venue FROM paper_venues WHERE paper_id = ?1 ORDER BY venue")?;
    for paper in &mut papers {
        paper.venues = venues.query_map([&paper.id], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?;
    }
    Ok(papers)
}

pub(super) fn exists(connection: &Connection, paper_id: &str) -> rusqlite::Result<bool> {
    let mut statement = connection.prepare_cached("SELECT 1 FROM papers WHERE id = ?1")?;
    Ok(statement.query_row([paper_id], |_| Ok(())).optional()?.is_some())
}

/// Write one paper's record: inserted when new, or, with `replace`, over the one there. Says whether it was new.
pub(super) fn put_paper(connection: &Connection, paper: &Paper, known: Known) -> Result<bool, LibraryError> {
    let new = !exists(connection, &paper.id)?;
    if !new && known == Known::Keep {
        return Ok(false);
    }
    let mut statement = connection.prepare_cached(
        "INSERT INTO papers (id, origin, title, abstract, authors, published, updated, comment, primary_category,
                             categories, url, pdf_url, text_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT (id) DO UPDATE SET
             origin = excluded.origin, title = excluded.title, abstract = excluded.abstract,
             authors = excluded.authors, published = excluded.published, updated = excluded.updated,
             comment = excluded.comment, primary_category = excluded.primary_category,
             categories = excluded.categories, url = excluded.url, pdf_url = excluded.pdf_url,
             text_hash = excluded.text_hash",
    )?;
    statement.execute(params![
        paper.id,
        paper.origin().as_str(),
        paper.title,
        paper.r#abstract,
        serde_json::to_string(&paper.authors)?,
        paper.published,
        paper.updated,
        paper.comment,
        paper.primary_category,
        serde_json::to_string(&paper.categories)?,
        paper.url,
        paper.pdf_url,
        paper.text_hash(),
    ])?;
    Ok(new)
}

pub(super) fn tag(connection: &Connection, paper_id: &str, venue: &str) -> rusqlite::Result<usize> {
    let mut statement = connection.prepare_cached(
        "INSERT INTO paper_venues (venue, paper_id) VALUES (?1, ?2) ON CONFLICT (venue, paper_id) DO NOTHING",
    )?;
    statement.execute([venue, paper_id])
}

fn add(connection: &Connection, papers: &[Paper], venue: Option<&str>, known: Known) -> Result<Added, LibraryError> {
    let mut added = Added::default();
    for paper in papers {
        if put_paper(connection, paper, known)? {
            added.new += 1;
        } else {
            added.known += 1;
        }
    }
    if let Some(venue) = venue {
        // the list handed in is the venue's whole list
        connection.execute("DELETE FROM paper_venues WHERE venue = ?1", [venue])?;
        for paper in papers {
            tag(connection, &paper.id, venue)?;
        }
    }
    Ok(added)
}

pub(super) fn put_slice(connection: &Connection, slice: &str, outcome: SliceOutcome) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO download_slices (slice, outcome, fetched_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (slice) DO UPDATE SET outcome = excluded.outcome, fetched_at = excluded.fetched_at",
        params![slice, outcome.as_str(), now()],
    )?;
    Ok(())
}

impl Library {
    /// Add downloaded papers. With `venue`, `papers` is that venue's whole list: they get its tag and no other
    /// paper keeps it.
    pub fn add_papers(&self, papers: &[Paper], venue: Option<&str>, known: Known) -> Result<Added, LibraryError> {
        self.write(|transaction| add(transaction, papers, venue, known))
    }

    /// Add the papers of one slice and record how it was fetched, together: a slice is never recorded without its
    /// papers, nor its papers kept with the slice forgotten.
    pub fn add_slice(
        &self,
        slice: &str,
        outcome: SliceOutcome,
        papers: &[Paper],
        venue: Option<&str>,
        known: Known,
    ) -> Result<Added, LibraryError> {
        self.write(|transaction| {
            let added = add(transaction, papers, venue, known)?;
            put_slice(transaction, slice, outcome)?;
            Ok(added)
        })
    }

    /// How a slice was last fetched, if it ever was.
    pub fn slice_outcome(&self, slice: &str) -> Result<Option<SliceOutcome>, LibraryError> {
        self.read(|connection| {
            let outcome: Option<String> = connection
                .query_row("SELECT outcome FROM download_slices WHERE slice = ?1", [slice], |row| row.get(0))
                .optional()?;
            Ok(outcome.as_deref().and_then(SliceOutcome::parse))
        })
    }

    /// Is the slice still to be fetched? Everything is, except what was fetched completely.
    pub fn needs_fetching(&self, slice: &str) -> Result<bool, LibraryError> {
        Ok(self.slice_outcome(slice)? != Some(SliceOutcome::Complete))
    }

    pub fn paper(&self, id: &str) -> Result<Option<Paper>, LibraryError> {
        let sql = format!("SELECT {COLUMNS} FROM papers WHERE id = ?1");
        self.read(|connection| Ok(papers_where(connection, &sql, [id])?.pop()))
    }

    /// The papers with these ids, in the order asked; `None` for an id the library does not have.
    pub fn papers(&self, ids: &[&str]) -> Result<Vec<Option<Paper>>, LibraryError> {
        let sql = format!("SELECT {COLUMNS} FROM papers WHERE id = ?1");
        self.read(|connection| ids.iter().map(|id| Ok(papers_where(connection, &sql, [id])?.pop())).collect())
    }

    pub fn paper_count(&self) -> Result<usize, LibraryError> {
        self.read(|connection| {
            let count: i64 = connection.query_row("SELECT COUNT(*) FROM papers", [], |row| row.get(0))?;
            Ok(usize::try_from(count).unwrap_or(0))
        })
    }

    /// The selection's papers, ordered by id: those listed at any of its conferences, or the arXiv papers
    /// submitted in its date range, in any of its categories (all, when it names none).
    pub fn select(&self, selection: &Selection) -> Result<Vec<Paper>, LibraryError> {
        self.read(|connection| match selection {
            Selection::Conferences { venues, .. } => {
                if venues.is_empty() {
                    return Ok(Vec::new());
                }
                let sql = format!(
                    "SELECT {COLUMNS} FROM papers
                     WHERE id IN (SELECT paper_id FROM paper_venues WHERE venue IN ({}))
                     ORDER BY id",
                    placeholders(venues.len())
                );
                papers_where(connection, &sql, params_from_iter(venues))
            }
            Selection::Dates { start, end, categories } => {
                // "published" is an RFC 3339 time, so the range is a comparison of text that an index can serve:
                // from the first day's date up to, not including, the day after the last
                let after = end.checked_add_days(Days::new(1)).unwrap_or(*end);
                let sql = format!(
                    "SELECT {COLUMNS} FROM papers
                     WHERE origin = 'arxiv' AND published >= ?1 AND published < ?2
                     ORDER BY id"
                );
                let range = [start.format("%Y-%m-%d").to_string(), after.format("%Y-%m-%d").to_string()];
                let mut papers = papers_where(connection, &sql, range)?;
                if !categories.is_empty() {
                    papers.retain(|paper| paper.categories.iter().any(|category| categories.contains(category)));
                }
                Ok(papers)
            }
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;
    use std::thread;

    pub(crate) fn paper(id: &str) -> Paper {
        Paper {
            id: id.into(),
            title: format!("title of {id}"),
            r#abstract: format!("abstract of {id}"),
            authors: vec!["A. Author".into(), "B. Author".into()],
            published: "2026-03-02T10:00:00Z".into(),
            updated: "2026-03-03T10:00:00Z".into(),
            comment: String::new(),
            primary_category: "cs.LG".into(),
            categories: vec!["cs.LG".into(), "stat.ML".into()],
            venues: Vec::new(),
            url: Some(format!("https://arxiv.org/abs/{id}")),
            pdf_url: None,
        }
    }

    fn library() -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        (folder, library)
    }

    fn ids(papers: &[Paper]) -> Vec<&str> {
        papers.iter().map(|paper| paper.id.as_str()).collect()
    }

    fn venues(venues: &[&str]) -> Selection {
        Selection::Conferences {
            venues: venues.iter().map(|venue| (*venue).to_owned()).collect(),
            include_workshops: false,
        }
    }

    #[test]
    fn a_paper_is_read_back_as_it_was_written() {
        let (_folder, library) = library();
        let mut written = paper("2603.01234");
        written.pdf_url = Some("https://arxiv.org/pdf/2603.01234".into());
        written.comment = "Accepted at ICML 2026; 9 pages".into();
        written.title = "Ünïcode, \"quotes\" and a\nnew line".into();
        library.add_papers(std::slice::from_ref(&written), None, Known::Keep).unwrap();
        assert_eq!(library.paper("2603.01234").unwrap(), Some(written));
        assert_eq!(library.paper("missing").unwrap(), None);
        assert_eq!(library.paper_count().unwrap(), 1);
    }

    #[test]
    fn the_text_hash_is_the_pythons() {
        // hashlib.sha1(text.encode("utf-8")).hexdigest()[:16], from the SHA-1 test vectors
        assert_eq!(text_hash(""), "da39a3ee5e6b4b0d");
        assert_eq!(text_hash("abc"), "a9993e364706816a");
        let mut one = paper("p");
        one.title = "Ünïcode title".into();
        one.r#abstract = "An abstract.".into();
        assert_eq!(one.text_hash(), "a7bfc44263b2f8f0"); // library.text_hash in the Python, for the same paper
    }

    #[test]
    fn a_known_paper_is_not_stored_again_unless_asked() {
        let (_folder, library) = library();
        let mut first = paper("a");
        first.title = "first".into();
        assert_eq!(library.add_papers(&[first.clone()], None, Known::Keep).unwrap(), Added { new: 1, known: 0 });
        let mut second = first.clone();
        second.title = "second".into();
        assert_eq!(library.add_papers(&[second.clone()], None, Known::Keep).unwrap(), Added { new: 0, known: 1 });
        assert_eq!(library.paper("a").unwrap().unwrap().title, "first");
        library.add_papers(&[second], None, Known::Replace).unwrap();
        assert_eq!(library.paper("a").unwrap().unwrap().title, "second");
    }

    #[test]
    fn a_venue_list_is_the_whole_venue() {
        let (_folder, library) = library();
        library.add_papers(&[paper("a"), paper("b")], Some("X 2026"), Known::Keep).unwrap();
        library.add_papers(&[paper("b")], Some("Y 2026"), Known::Keep).unwrap();
        library.add_papers(&[paper("b"), paper("c")], Some("X 2026"), Known::Keep).unwrap(); // "a" is off the list
        assert!(library.paper("a").unwrap().unwrap().venues.is_empty());
        assert_eq!(library.paper("b").unwrap().unwrap().venues, ["X 2026", "Y 2026"]);
        assert_eq!(ids(&library.select(&venues(&["X 2026"])).unwrap()), ["b", "c"]);
        assert_eq!(ids(&library.select(&venues(&["X 2026", "Y 2026"])).unwrap()), ["b", "c"]); // each paper once
        assert!(library.select(&venues(&["Z 2026"])).unwrap().is_empty());
    }

    #[test]
    fn the_venues_a_download_hands_in_are_not_taken_as_tags() {
        let (_folder, library) = library();
        let mut claimed = paper("a");
        claimed.venues = vec!["Made Up 2026".into()];
        library.add_papers(&[claimed], None, Known::Keep).unwrap();
        assert!(library.paper("a").unwrap().unwrap().venues.is_empty());
    }

    #[test]
    fn selecting_by_dates_takes_arxiv_papers_of_those_days_in_those_categories() {
        let (_folder, library) = library();
        let mut late = paper("late");
        late.published = "2026-03-04T00:00:00Z".into();
        let mut early = paper("early");
        early.published = "2026-02-28T23:59:59Z".into();
        let mut last_day = paper("last");
        last_day.published = "2026-03-03T23:59:59Z".into();
        let mut other = paper("other");
        other.categories = vec!["math.PR".into()];
        let mut conference = paper("icml-2026-17"); // presented in the range, but not an arXiv submission
        conference.published = "2026-03-02T00:00:00Z".into();
        library.add_papers(&[late, early, last_day, other, paper("in"), conference], None, Known::Keep).unwrap();
        let dates = |categories: &[&str]| Selection::Dates {
            start: chrono::NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(),
            end: chrono::NaiveDate::from_ymd_opt(2026, 3, 3).unwrap(),
            categories: categories.iter().map(|category| (*category).to_owned()).collect(),
        };
        assert_eq!(ids(&library.select(&dates(&["cs.LG"])).unwrap()), ["in", "last"]);
        assert_eq!(ids(&library.select(&dates(&[])).unwrap()), ["in", "last", "other"]);
        assert_eq!(paper("icml-2026-17").origin(), Origin::Conference);
        assert_eq!(paper("2603.01234").origin(), Origin::Arxiv);
    }

    #[test]
    fn papers_are_returned_in_the_order_asked() {
        let (_folder, library) = library();
        library.add_papers(&[paper("a"), paper("b")], None, Known::Keep).unwrap();
        let found = library.papers(&["b", "missing", "a"]).unwrap();
        let found: Vec<Option<&str>> =
            found.iter().map(|paper| paper.as_ref().map(|paper| paper.id.as_str())).collect();
        assert_eq!(found, [Some("b"), None, Some("a")]);
    }

    #[test]
    fn only_a_complete_slice_is_finished() {
        let (_folder, library) = library();
        assert!(library.needs_fetching("day|2026-03-02|").unwrap());
        assert_eq!(library.slice_outcome("day|2026-03-02|").unwrap(), None);
        library
            .add_slice("venue|X 2026", SliceOutcome::ArxivComments, &[paper("a")], Some("X 2026"), Known::Keep)
            .unwrap();
        assert!(library.needs_fetching("venue|X 2026").unwrap()); // the official list may be there next time
        assert_eq!(library.paper("a").unwrap().unwrap().venues, ["X 2026"]);
        library
            .add_slice("venue|X 2026", SliceOutcome::Complete, &[paper("x-2026-1")], Some("X 2026"), Known::Keep)
            .unwrap();
        assert!(!library.needs_fetching("venue|X 2026").unwrap());
        assert_eq!(ids(&library.select(&venues(&["X 2026"])).unwrap()), ["x-2026-1"]);
        library.add_slice("day|2026-11-26|", SliceOutcome::Incomplete, &[], None, Known::Keep).unwrap();
        assert!(library.needs_fetching("day|2026-11-26|").unwrap());
        assert_eq!(library.slice_outcome("day|2026-11-26|").unwrap(), Some(SliceOutcome::Incomplete));
    }

    #[test]
    fn a_slice_that_fails_part_way_keeps_neither_its_papers_nor_its_record() {
        let (_folder, library) = library();
        let result = library.write(|transaction| {
            add(transaction, &[paper("a")], Some("X 2026"), Known::Keep)?;
            put_slice(transaction, "venue|X 2026", SliceOutcome::Complete)?;
            Err::<(), _>(LibraryError::Shape("the download broke off".into()))
        });
        assert!(result.is_err());
        assert_eq!(library.paper_count().unwrap(), 0);
        assert_eq!(library.slice_outcome("venue|X 2026").unwrap(), None);
    }

    #[test]
    fn writers_on_several_threads_and_connections_all_land() {
        let folder = tempfile::tempdir().unwrap();
        let first = Library::open(folder.path()).unwrap();
        let second = Library::open(folder.path()).unwrap(); // as a second process would have it
        thread::scope(|scope| {
            for writer in 0..8 {
                let library = if writer % 2 == 0 { &first } else { &second };
                scope.spawn(move || {
                    for number in 0..25 {
                        let id = format!("{writer}.{number}");
                        library.add_papers(&[paper(&id)], Some("Shared 2026"), Known::Keep).unwrap();
                        library.add_slice(&id, SliceOutcome::Complete, &[], None, Known::Keep).unwrap();
                    }
                });
            }
        });
        assert_eq!(second.paper_count().unwrap(), 200);
        // each list handed in was the venue's whole list, so the last writer's one paper holds the tag
        assert_eq!(first.select(&venues(&["Shared 2026"])).unwrap().len(), 1);
    }

    #[test]
    fn a_second_connection_reads_while_the_first_is_writing() {
        let folder = tempfile::tempdir().unwrap();
        let writer = Library::open(folder.path()).unwrap();
        let reader = Library::open(folder.path()).unwrap();
        writer.add_papers(&[paper("before")], None, Known::Keep).unwrap();
        {
            let mut connection = writer.connection();
            let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
            put_paper(&transaction, &paper("during"), Known::Keep).unwrap();
            let started = std::time::Instant::now();
            assert_eq!(reader.paper_count().unwrap(), 1); // the last committed state, without waiting
            assert!(started.elapsed() < std::time::Duration::from_secs(1));
            transaction.commit().unwrap();
        }
        assert_eq!(reader.paper_count().unwrap(), 2);
    }
}
