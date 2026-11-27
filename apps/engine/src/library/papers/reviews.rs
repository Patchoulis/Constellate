//! A paper's reviews page as the user's reader showed it, and the notes read from it
//! ([`crate::text::reviews::discussion`]).
//!
//! Both are kept: the page's text, so that a newer reader ([`PARSER`] raised) reads it again without the user
//! opening the page again, and so that the character positions of a reading keep pointing at the reviewers'
//! words; and the reading, keyed by paper and by the reader's version, so that a page is read once per version
//! and never again ([`Library::read_stored_pages_again`] reads only what no current reading exists for).
//!
//! Keeping a page replaces the paper's page, every reading of it and its scores in one transaction, so that no
//! thread or process ever sees a page with another page's reading, or half of a paper's reviews. A page with
//! fewer reviews than the current reading of the kept one is not kept: the reader hands the page over several
//! times while it loads, and a page still loading, or one with replies collapsed, must not undo a fuller one.

use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::readings::{put_ratings, ratings_of};
use crate::library::{Library, LibraryError, Ratings, now, placeholders, text_hash};
use crate::text::reviews::discussion::{Discussion, PARSER, read_discussion};
use crate::threads::{self, Outcome, Progress};

/// Papers asked about in one statement: well under SQLite's limit on bound values.
const IDS_AT_ONCE: usize = 500;
/// Stored pages read again in one round: their texts are held in memory together.
const PAGES_AT_ONCE: usize = 256;

/// What is kept of a paper's reviews: where the page was, when it was read, and the notes read from it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct KeptReviews {
    pub paper_id: String,
    /// The address of the page the reader showed.
    pub address: String,
    /// When the page was read in the reader, UTC.
    pub read_at: String,
    pub discussion: Discussion,
}

/// A kept page's text: what a reading's character positions point into.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReviewPage {
    pub address: String,
    pub read_at: String,
    pub text: String,
}

/// What came of keeping a page.
#[derive(Clone, Debug, PartialEq)]
pub enum Keeping {
    /// The page and its reading now stand for the paper's reviews.
    Replaced(KeptReviews),
    /// What is kept has more reviews: nothing was changed, and this is what stays.
    KeptFuller(KeptReviews),
}

impl Keeping {
    pub fn kept(&self) -> &KeptReviews {
        match self {
            Keeping::Replaced(kept) | Keeping::KeptFuller(kept) => kept,
        }
    }
}

fn reviews_in(discussion: &Discussion) -> i64 {
    i64::try_from(discussion.review_count()).unwrap_or(i64::MAX)
}

/// The rows a reading is rebuilt from: (paper, address, read at, reading as JSON).
type Row = (String, String, String, String);

fn decoded((paper_id, address, read_at, content): Row) -> Result<KeptReviews, LibraryError> {
    Ok(KeptReviews { paper_id, address, read_at, discussion: serde_json::from_str(&content)? })
}

const KEPT: &str = "SELECT p.paper_id, p.address, p.read_at, r.content FROM review_pages p
                    JOIN review_readings r ON r.paper_id = p.paper_id AND r.parser = ?1 AND r.text_hash = p.text_hash";

fn kept_of(connection: &Connection, paper_id: &str) -> Result<Option<KeptReviews>, LibraryError> {
    let row: Option<Row> = connection
        .query_row(&format!("{KEPT} WHERE p.paper_id = ?2"), params![PARSER, paper_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional()?;
    row.map(decoded).transpose()
}

fn put_reading(
    connection: &Connection,
    paper_id: &str,
    hash: &str,
    discussion: &Discussion,
) -> Result<(), LibraryError> {
    connection.prepare_cached("DELETE FROM review_readings WHERE paper_id = ?1")?.execute([paper_id])?;
    connection
        .prepare_cached(
            "INSERT INTO review_readings (paper_id, parser, text_hash, reviews, content, read_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?
        .execute(params![
            paper_id,
            discussion.parser,
            hash,
            reviews_in(discussion),
            serde_json::to_string(discussion)?,
            now()
        ])?;
    Ok(())
}

/// Read a kept page again with this version of the reader, inside a write, and keep the reading.
fn read_kept_page_again(connection: &Connection, paper_id: &str) -> Result<KeptReviews, LibraryError> {
    let (address, read_at, text, hash): (String, String, String, String) = connection.query_row(
        "SELECT address, read_at, text, text_hash FROM review_pages WHERE paper_id = ?1",
        [paper_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let discussion = read_discussion(&text).unwrap_or(Discussion { parser: PARSER, venue: None, notes: Vec::new() });
    put_reading(connection, paper_id, &hash, &discussion)?;
    Ok(KeptReviews { paper_id: paper_id.to_owned(), address, read_at, discussion })
}

impl Library {
    /// Keep the text of a paper's reviews page, shown at `address`, with `discussion` read from it and the
    /// `ratings` read from it (the scores alone, as the interface has long shown them), all in one transaction,
    /// unless what is kept has more reviews ([`Keeping::KeptFuller`]).
    pub fn keep_reviews(
        &self,
        paper_id: &str,
        address: &str,
        text: &str,
        discussion: &Discussion,
        ratings: Option<Ratings>,
    ) -> Result<Keeping, LibraryError> {
        self.write(|transaction| {
            // the reviews the kept page has, by whichever version of the reader read it last
            let before: Option<i64> = transaction.query_row(
                "SELECT MAX(reviews) FROM review_readings WHERE paper_id = ?1",
                [paper_id],
                |row| row.get(0),
            )?;
            if before.is_some_and(|before| before > reviews_in(discussion)) {
                if let Some(kept) = kept_of(transaction, paper_id)? {
                    return Ok(Keeping::KeptFuller(kept));
                }
                // read by an older version only: the kept page is read again now, and stays
                return read_kept_page_again(transaction, paper_id).map(Keeping::KeptFuller);
            }
            let read_at = now();
            let hash = text_hash(text);
            transaction
                .prepare_cached(
                    "INSERT INTO review_pages (paper_id, address, text, text_hash, read_at) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT (paper_id) DO UPDATE SET
                         address = excluded.address, text = excluded.text, text_hash = excluded.text_hash,
                         read_at = excluded.read_at",
                )?
                .execute(params![paper_id, address, text, hash, read_at])
                .map_err(|error| match error.sqlite_error_code() {
                    Some(rusqlite::ErrorCode::ConstraintViolation) => LibraryError::UnknownPaper(paper_id.to_owned()),
                    _ => error.into(),
                })?;
            put_reading(transaction, paper_id, &hash, discussion)?;
            // the scores are always this page's, so that no one sees one page's notes with another's scores
            match ratings {
                Some(ratings) => {
                    put_ratings(transaction, paper_id, &ratings)?;
                }
                None => {
                    transaction.execute("DELETE FROM ratings WHERE paper_id = ?1", [paper_id])?;
                }
            }
            Ok(Keeping::Replaced(KeptReviews {
                paper_id: paper_id.to_owned(),
                address: address.to_owned(),
                read_at,
                discussion: discussion.clone(),
            }))
        })
    }

    /// The paper's scores and its reviews, as one moment of the library sees them.
    pub fn ratings_and_reviews(&self, paper_id: &str) -> Result<(Option<Ratings>, Option<KeptReviews>), LibraryError> {
        self.read(|connection| Ok((ratings_of(connection, paper_id)?, kept_of(connection, paper_id)?)))
    }

    /// The paper's reviews as this version of the reader read them; none if its page was never kept, or was
    /// kept but not read by this version yet ([`Library::read_stored_pages_again`]).
    pub fn reviews(&self, paper_id: &str) -> Result<Option<KeptReviews>, LibraryError> {
        self.read(|connection| kept_of(connection, paper_id))
    }

    /// The reviews of those of `paper_ids` that have any, ordered by paper id. The readings are decoded on every
    /// core.
    pub fn reviews_of(&self, paper_ids: &[&str]) -> Result<Vec<KeptReviews>, LibraryError> {
        let rows: Vec<Row> = self.read(|connection| {
            let mut rows = Vec::new();
            for ids in paper_ids.chunks(IDS_AT_ONCE) {
                let query = format!("{KEPT} WHERE p.paper_id IN ({})", placeholders(ids.len()));
                let mut statement = connection.prepare(&query)?;
                let bound = std::iter::once(rusqlite::types::Value::from(i64::from(PARSER)))
                    .chain(ids.iter().map(|id| rusqlite::types::Value::from((*id).to_owned())));
                let found = statement.query_map(rusqlite::params_from_iter(bound), |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?;
                rows.extend(found.collect::<rusqlite::Result<Vec<Row>>>()?);
            }
            Ok(rows)
        })?;
        let mut kept: Vec<KeptReviews> = rows.into_par_iter().map(decoded).collect::<Result<_, _>>()?;
        kept.sort_by(|a, b| a.paper_id.cmp(&b.paper_id));
        kept.dedup_by(|a, b| a.paper_id == b.paper_id); // an id asked for twice
        Ok(kept)
    }

    /// The text of the paper's kept reviews page, which its reading's positions point into.
    pub fn review_page(&self, paper_id: &str) -> Result<Option<ReviewPage>, LibraryError> {
        self.read(|connection| {
            Ok(connection
                .query_row("SELECT address, read_at, text FROM review_pages WHERE paper_id = ?1", [paper_id], |row| {
                    Ok(ReviewPage { address: row.get(0)?, read_at: row.get(1)?, text: row.get(2)? })
                })
                .optional()?)
        })
    }

    /// Read every kept page that this version of the reader has not read, on every core, and keep the readings;
    /// returns how many pages were read. A page replaced while it was being read is left to its new reading.
    pub fn read_stored_pages_again(&self) -> Result<usize, LibraryError> {
        let mut read = 0;
        let mut after = String::new();
        loop {
            let pages: Vec<(String, String, String)> = self.read(|connection| {
                let mut statement = connection.prepare_cached(
                    "SELECT p.paper_id, p.text, p.text_hash FROM review_pages p
                     WHERE p.paper_id > ?1 AND NOT EXISTS (
                         SELECT 1 FROM review_readings r
                         WHERE r.paper_id = p.paper_id AND r.parser = ?2 AND r.text_hash = p.text_hash)
                     ORDER BY p.paper_id LIMIT ?3",
                )?;
                let rows = statement.query_map(params![after, PARSER, PAGES_AT_ONCE as i64], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
                Ok(rows.collect::<rusqlite::Result<_>>()?)
            })?;
            let Some((last, _, _)) = pages.last() else {
                return Ok(read);
            };
            after = last.clone();
            // Read on every core; the readings are written below, by this thread alone, in one transaction. A
            // page whose reading panics (a reader's bug, not the page's) is that page's failure: it is said on the
            // engine's error output and left unread for a later reader, and the other pages are kept as usual.
            let readings: Vec<(&str, &str, Discussion)> = threads::each(&pages, &Progress::new(), |(_, text, _)| {
                read_discussion(text).unwrap_or(Discussion { parser: PARSER, venue: None, notes: Vec::new() })
            })
            .into_iter()
            .zip(&pages)
            .filter_map(|(outcome, (paper_id, _, hash))| match outcome {
                Outcome::Done(discussion) => Some((paper_id.as_str(), hash.as_str(), discussion)),
                Outcome::Panicked(said) => {
                    eprintln!("library: the reviews page of {paper_id} could not be read: {said}");
                    None
                }
                Outcome::Cancelled => None,
            })
            .collect();
            self.write(|transaction| {
                let mut still = transaction.prepare_cached("SELECT text_hash FROM review_pages WHERE paper_id = ?1")?;
                for (paper_id, hash, discussion) in &readings {
                    let now: Option<String> = still.query_row([paper_id], |row| row.get(0)).optional()?;
                    if now.as_deref() == Some(*hash) {
                        put_reading(transaction, paper_id, hash, discussion)?;
                    }
                }
                Ok(())
            })?;
            read += readings.len();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::library::Known;
    use crate::library::papers::tests::paper;
    use crate::text::reviews::discussion::fixtures::{ICLR_2024, NEURIPS_2023, synthetic_page};
    use crate::text::reviews::read_ratings;

    const ADDRESS: &str = "https://openreview.net/forum?id=abc123XYZ";

    fn library_with(ids: &[&str]) -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let papers: Vec<_> = ids.iter().map(|id| paper(id)).collect();
        library.add_papers(&papers, None, Known::Keep).unwrap();
        (folder, library)
    }

    fn keep(library: &Library, paper_id: &str, text: &str) -> Keeping {
        let ratings = read_ratings(text).map(|read| Ratings::from(&read));
        library.keep_reviews(paper_id, ADDRESS, text, &read_discussion(text).unwrap(), ratings).unwrap()
    }

    fn count(library: &Library, table: &str) -> i64 {
        library.connection().query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn a_kept_page_is_handed_back_with_its_reading_address_and_scores() {
        let (_folder, library) = library_with(&["p"]);
        assert_eq!(library.reviews("p").unwrap(), None);
        let kept = keep(&library, "p", NEURIPS_2023);
        assert!(matches!(kept, Keeping::Replaced(_)));
        let back = library.reviews("p").unwrap().unwrap();
        assert_eq!(back, *kept.kept());
        assert_eq!((back.address.as_str(), back.discussion.review_count()), (ADDRESS, 2));
        assert_eq!(library.review_page("p").unwrap().unwrap().text, NEURIPS_2023);
        assert_eq!(library.ratings("p").unwrap().unwrap().reviews, 2); // the scores alone, as before, kept with it
    }

    #[test]
    fn a_page_with_fewer_reviews_does_not_undo_a_fuller_one_and_a_fuller_one_replaces_it_whole() {
        let (_folder, library) = library_with(&["p"]);
        let loading = &NEURIPS_2023[..NEURIPS_2023.find("Official Review of Submission4321 by Reviewer eFgH").unwrap()];
        assert!(matches!(keep(&library, "p", loading), Keeping::Replaced(_)));
        assert!(matches!(keep(&library, "p", NEURIPS_2023), Keeping::Replaced(_)));
        let again = keep(&library, "p", loading);
        assert!(matches!(again, Keeping::KeptFuller(_)));
        assert_eq!(again.kept().discussion.review_count(), 2);
        assert_eq!(library.review_page("p").unwrap().unwrap().text, NEURIPS_2023);
        // another page of as many reviews replaces the paper's page, its reading and nothing else's
        assert!(matches!(keep(&library, "p", ICLR_2024), Keeping::Replaced(_)));
        assert_eq!((count(&library, "review_pages"), count(&library, "review_readings")), (1, 1));
        assert_eq!(library.reviews("p").unwrap().unwrap().discussion.notes.len(), 5);
    }

    #[test]
    fn a_page_read_by_an_older_reader_is_still_the_fuller_one_and_is_read_again() {
        let (_folder, library) = library_with(&["p"]);
        keep(&library, "p", NEURIPS_2023);
        library.connection().execute("UPDATE review_readings SET parser = ?1", [PARSER - 1]).unwrap();
        let loading = &NEURIPS_2023[..NEURIPS_2023.find("Official Review of Submission4321 by Reviewer eFgH").unwrap()];
        let kept = keep(&library, "p", loading);
        assert!(matches!(kept, Keeping::KeptFuller(_)));
        assert_eq!(library.reviews("p").unwrap().unwrap().discussion.review_count(), 2);
        assert_eq!(library.review_page("p").unwrap().unwrap().text, NEURIPS_2023);
    }

    #[test]
    fn the_scores_kept_are_always_those_of_the_page_kept() {
        let (_folder, library) = library_with(&["p"]);
        keep(&library, "p", ICLR_2024);
        let no_scores = "Official Review of Submission1 by Reviewer Aa11\nSummary:\nNo scores here.\n\
                         Official Review of Submission1 by Reviewer Bb22\nSummary:\nNone either.\n";
        assert!(matches!(keep(&library, "p", no_scores), Keeping::Replaced(_)));
        let (ratings, reviews) = library.ratings_and_reviews("p").unwrap();
        assert_eq!((ratings, reviews.unwrap().discussion.review_count()), (None, 2));
    }

    #[test]
    fn reviews_of_a_paper_the_library_does_not_have_are_refused_and_nothing_is_kept() {
        let (_folder, library) = library_with(&["p"]);
        let discussion = read_discussion(ICLR_2024).unwrap();
        let refused = library.keep_reviews("nowhere", ADDRESS, ICLR_2024, &discussion, None).unwrap_err();
        assert!(matches!(refused, LibraryError::UnknownPaper(ref id) if id == "nowhere"), "{refused:?}");
        assert_eq!(count(&library, "review_pages"), 0);
    }

    #[test]
    fn a_reading_by_another_version_of_the_reader_is_not_handed_out_and_is_read_again_once() {
        let (_folder, library) = library_with(&["a", "b", "c"]);
        for id in ["a", "b", "c"] {
            keep(&library, id, ICLR_2024);
        }
        assert_eq!(library.read_stored_pages_again().unwrap(), 0); // all read by this version: nothing redone
        library
            .connection()
            .execute("UPDATE review_readings SET parser = ?1 WHERE paper_id <> 'b'", [PARSER - 1])
            .unwrap();
        assert_eq!(library.reviews("a").unwrap(), None);
        assert_eq!(library.reviews_of(&["a", "b", "c"]).unwrap().len(), 1);
        assert_eq!(library.read_stored_pages_again().unwrap(), 2);
        assert_eq!(library.read_stored_pages_again().unwrap(), 0);
        let all = library.reviews_of(&["c", "a", "b", "a", "unknown"]).unwrap();
        let ids: Vec<&str> = all.iter().map(|kept| kept.paper_id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(all[0].discussion, read_discussion(ICLR_2024).unwrap());
    }

    #[test]
    fn many_threads_keeping_and_reading_reviews_lose_nothing_and_never_see_half_a_paper() {
        const WRITERS: usize = 6;
        const ROUNDS: usize = 50; // 300 writes in all, each read about while it happens
        let ids: Vec<String> = (0..WRITERS * 2).map(|paper| format!("p{paper}")).collect();
        let (_folder, library) = library_with(&ids.iter().map(String::as_str).collect::<Vec<_>>());
        let writing = std::sync::atomic::AtomicUsize::new(WRITERS);
        std::thread::scope(|scope| {
            for writer in 0..WRITERS {
                let (library, writing) = (&library, &writing);
                scope.spawn(move || {
                    for round in 0..ROUNDS {
                        let paper = writer * 2 + round % 2; // each writer its own two papers, in turn
                        let reviews = 1 + round / 2; // more reviews each time a paper comes round
                        let page = synthetic_page(paper, &vec![(6, 3); reviews]);
                        let address = format!("{ADDRESS}&reviews={reviews}");
                        let ratings = read_ratings(&page).map(|read| Ratings::from(&read));
                        let discussion = read_discussion(&page).unwrap();
                        library.keep_reviews(&format!("p{paper}"), &address, &page, &discussion, ratings).unwrap();
                    }
                    writing.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                });
            }
            for _ in 0..3 {
                let (library, writing, ids) = (&library, &writing, &ids);
                scope.spawn(move || {
                    while writing.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                        // one read: the page, its reading and its scores must all be of the same write
                        let seen: Vec<(String, i64, String, String, Option<String>)> = library
                            .read(|connection| {
                                let mut statement = connection.prepare(
                                    "SELECT p.address, r.reviews, r.text_hash, p.text_hash,
                                            (SELECT content FROM ratings s WHERE s.paper_id = p.paper_id)
                                     FROM review_pages p LEFT JOIN review_readings r ON r.paper_id = p.paper_id",
                                )?;
                                let rows = statement.query_map([], |row| {
                                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
                                })?;
                                Ok(rows.collect::<rusqlite::Result<_>>()?)
                            })
                            .unwrap();
                        for (address, reviews, read_hash, page_hash, ratings) in seen {
                            assert!(address.ends_with(&format!("reviews={reviews}")), "{address} with {reviews}");
                            assert_eq!(read_hash, page_hash);
                            let ratings: Ratings = serde_json::from_str(&ratings.unwrap()).unwrap();
                            assert_eq!(i64::from(ratings.reviews), reviews);
                        }
                        let kept = library.reviews_of(&ids.iter().map(String::as_str).collect::<Vec<_>>()).unwrap();
                        let papers: Vec<_> = kept
                            .iter()
                            .map(|kept| {
                                let venue = kept.discussion.venue.as_ref();
                                crate::analysis::reviews::Reviewed::new(&kept.paper_id, venue, &kept.discussion)
                            })
                            .collect();
                        crate::analysis::reviews::summarise(ids.len(), &papers);
                    }
                });
            }
        });
        for (writer, round) in (0..WRITERS).flat_map(|writer| [(writer, ROUNDS - 2), (writer, ROUNDS - 1)]) {
            let paper = writer * 2 + round % 2;
            let kept = library.reviews(&format!("p{paper}")).unwrap().unwrap();
            assert_eq!(kept.discussion.review_count(), 1 + round / 2, "p{paper}: the last write is the one kept");
        }
        assert_eq!(count(&library, "review_readings"), i64::try_from(ids.len()).unwrap());
    }

    #[test]
    fn a_write_that_panics_half_way_leaves_nothing_behind_and_the_library_goes_on() {
        let (_folder, library) = library_with(&["p", "q"]);
        keep(&library, "q", ICLR_2024);
        let panicked = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    library.write::<()>(|transaction| {
                        transaction.execute(
                            "INSERT INTO review_pages (paper_id, address, text, text_hash, read_at)
                             VALUES ('p', 'a', 'half', 'h', 't')",
                            [],
                        )?;
                        panic!("a worker failed in the middle of keeping a paper");
                    })
                })
                .join()
                .is_err()
        });
        assert!(panicked);
        assert!(library.connection.is_poisoned()); // the lock says so, and the library takes it anyway
        assert_eq!(library.review_page("p").unwrap(), None); // the half-done write was rolled back
        assert!(matches!(keep(&library, "p", NEURIPS_2023), Keeping::Replaced(_)));
        assert_eq!(library.reviews_of(&["p", "q"]).unwrap().len(), 2);
    }
}
