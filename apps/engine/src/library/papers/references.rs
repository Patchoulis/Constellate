//! Each paper's reference list ([`crate::text::pdf::references`]), read once from its PDF and kept, with what arXiv
//! said about the entries that were looked up there ([`crate::sources::citations`]): what the citation network
//! around a paper is built from ([`crate::analysis::citations::network`]).
//!
//! A list is kept with the version of the reader ([`READER`]) and the SHA-256 of the PDF it was read from, and
//! counts only for that reader and that PDF: a PDF replaced since, or a reader changed since, is read again when
//! the list is next asked for. The entries are kept as read, not linked: they are linked to the library as it is
//! when the network is built, so that a paper added since is found.
//!
//! The citing sentences ([`crate::features::citation_contexts::store`]) keep only the entries their paper's text cites, and only
//! once its passages are read; this keeps every entry, which is what a network of prior work needs.
//!
//! Looking an entry up on arXiv is slow (one request every three seconds), so it is done only when asked
//! ([`look_up`]), at most a given number of entries at a time, and what was found (or that nothing was) is kept
//! with the entry ([`Lookup`]): an entry is never asked about twice.

use std::fs;

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::analysis::citations::Linker;
use crate::library::{Library, LibraryError, now};
use crate::sources::citations::{Answer, Ask, Get, LookupError, ask_for, read_answer, send, url_for};
use crate::text::pdf::references::{Citing, Confidence, Reference, ReferenceList, read_reference_list};
use crate::text::pdf::{Unreadable, pdf_pages};

/// The version of what is kept: raised when [`crate::text::pdf::references`] reads lists differently, so that the
/// lists kept by an older reader are read again.
pub const READER: u32 = 1;

/// What looking an entry up on arXiv came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lookup {
    /// Not looked up (yet): either not worth asking about, or not reached.
    NotAsked,
    /// Found on arXiv: the entry carries the id (and the title, when none was read) since.
    Found,
    /// arXiv has no paper of that title, or none of that id.
    NotFound,
    /// arXiv has more than one paper of that title: none is taken.
    Ambiguous,
}

/// One entry of a kept reference list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeptEntry {
    pub reference: Reference,
    pub lookup: Lookup,
}

/// A paper's reference list as it is kept.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeptReferences {
    pub citing: Citing,
    pub entries: Vec<KeptEntry>,
}

impl KeptReferences {
    /// A list just read: nothing looked up yet.
    pub fn read(list: ReferenceList) -> Self {
        let entries =
            list.references.into_iter().map(|reference| KeptEntry { reference, lookup: Lookup::NotAsked }).collect();
        Self { citing: list.citing, entries }
    }

    /// The entries, as the linker reads them.
    pub fn references(&self) -> Vec<Reference> {
        self.entries.iter().map(|entry| entry.reference.clone()).collect()
    }
}

/// What asking for a paper's reference list came to.
#[derive(Clone, Debug, PartialEq)]
pub enum ReferencesReading {
    /// Kept before, read by this reader from this very PDF: not read again.
    Kept { pdf_hash: String, list: KeptReferences },
    /// Read now from the PDF, and kept.
    ReadNow { pdf_hash: String, list: KeptReferences },
    /// The library has no PDF of it.
    NoPdf,
    /// Its PDF cannot be read, for the reason given.
    Unreadable(String),
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What is kept of reading a paper's PDF for its references, by this reader and that PDF: its list, or a lasting
/// reason it cannot be read ([`Unreadable::is_lasting`]), so that a PDF that cannot be read (a hostile one that
/// takes a reader its whole time or memory) is read once, not at every request. Another PDF, or another reader,
/// is read again. Kept as the list alone, as before, or as `{"unreadable": ...}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum KeptReading {
    List(KeptReferences),
    Unreadable { unreadable: Unreadable },
}

impl Library {
    /// What was kept of reading `paper_id`'s PDF of hash `pdf_hash` with this reader.
    fn kept_reading(&self, paper_id: &str, pdf_hash: &str) -> Result<Option<KeptReading>, LibraryError> {
        self.read(|connection| {
            let content: Option<String> = connection
                .prepare_cached(
                    "SELECT content FROM paper_references WHERE paper_id = ?1 AND reader = ?2 AND pdf_hash = ?3",
                )?
                .query_row(params![paper_id, READER, pdf_hash], |row| row.get(0))
                .optional()?;
            // a content that no longer reads as either is read again
            Ok(content.and_then(|content| serde_json::from_str(&content).ok()))
        })
    }

    /// The reference list kept for `paper_id`, if this reader read it from the PDF of hash `pdf_hash`.
    pub fn kept_references(&self, paper_id: &str, pdf_hash: &str) -> Result<Option<KeptReferences>, LibraryError> {
        Ok(match self.kept_reading(paper_id, pdf_hash)? {
            Some(KeptReading::List(list)) => Some(list),
            Some(KeptReading::Unreadable { .. }) | None => None,
        })
    }

    /// Every reference list kept by this reader, by paper id in order: what the citation network is built from.
    /// A list of a PDF replaced since still counts here until the paper's list is asked for again.
    pub fn every_kept_reference_list(&self) -> Result<Vec<(String, KeptReferences)>, LibraryError> {
        self.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT paper_id, content FROM paper_references WHERE reader = ?1 ORDER BY paper_id")?;
            let rows = statement.query_map([READER], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
            let mut kept = Vec::new();
            for row in rows {
                let (id, content) = row?;
                if let Ok(list) = serde_json::from_str(&content) {
                    kept.push((id, list));
                }
            }
            Ok(kept)
        })
    }

    /// Keep `list` as the reference list of `paper_id`, read from the PDF of hash `pdf_hash` (replacing any).
    pub fn keep_references(&self, paper_id: &str, pdf_hash: &str, list: &KeptReferences) -> Result<(), LibraryError> {
        self.keep_reading(paper_id, pdf_hash, &KeptReading::List(list.clone()))
    }

    fn keep_reading(&self, paper_id: &str, pdf_hash: &str, reading: &KeptReading) -> Result<(), LibraryError> {
        let content = serde_json::to_string(reading)?;
        self.write(|transaction| {
            transaction
                .prepare_cached(
                    "INSERT INTO paper_references (paper_id, reader, pdf_hash, content, read_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT (paper_id) DO UPDATE SET
                         reader = excluded.reader, pdf_hash = excluded.pdf_hash, content = excluded.content,
                         read_at = excluded.read_at",
                )?
                .execute(params![paper_id, READER, pdf_hash, content, now()])?;
            Ok(())
        })
    }

    /// Keep what was looked up in `list`, the kept list of `paper_id` from the PDF of hash `pdf_hash`: nothing
    /// when that list was replaced meanwhile (another PDF), so that lookups of one list never land in another.
    pub fn keep_lookups(&self, paper_id: &str, pdf_hash: &str, list: &KeptReferences) -> Result<bool, LibraryError> {
        let content = serde_json::to_string(list)?;
        self.write(|transaction| {
            let changed = transaction
                .prepare_cached(
                    "UPDATE paper_references SET content = ?4 WHERE paper_id = ?1 AND reader = ?2 AND pdf_hash = ?3",
                )?
                .execute(params![paper_id, READER, pdf_hash, content])?;
            Ok(changed > 0)
        })
    }

    /// The reference list of `paper_id`: the kept one when it was read by this reader from the PDF kept now, else
    /// read now from that PDF and kept. Blocks while the PDF is read (in a child process): call it off the async
    /// threads.
    pub fn references_of(&self, paper_id: &str) -> Result<ReferencesReading, LibraryError> {
        self.paper(paper_id)?.ok_or_else(|| LibraryError::UnknownPaper(paper_id.to_owned()))?;
        let Ok(bytes) = fs::read(self.pdf_file(paper_id)) else { return Ok(ReferencesReading::NoPdf) };
        let pdf_hash = sha256(&bytes);
        match self.kept_reading(paper_id, &pdf_hash)? {
            Some(KeptReading::List(list)) => return Ok(ReferencesReading::Kept { pdf_hash, list }),
            Some(KeptReading::Unreadable { unreadable }) => {
                return Ok(ReferencesReading::Unreadable(unreadable.to_string()));
            }
            None => {}
        }
        let pages = match pdf_pages(&bytes) {
            Ok(pages) => pages,
            Err(why) => {
                // kept only when it says something of the PDF: a reader that could not start is tried again
                if why.is_lasting() {
                    self.keep_reading(paper_id, &pdf_hash, &KeptReading::Unreadable { unreadable: why.clone() })?;
                }
                return Ok(ReferencesReading::Unreadable(why.to_string()));
            }
        };
        let list = KeptReferences::read(read_reference_list(&pages.join("\n")));
        self.keep_references(paper_id, &pdf_hash, &list)?;
        Ok(ReferencesReading::ReadNow { pdf_hash, list })
    }
}

/// Why a round of lookups ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// Every entry worth asking about has been asked about.
    AllAsked,
    /// The most a round may ask was asked; this many entries are left for another round.
    AtMost { left: usize },
    /// arXiv refused or could not be reached; what was found before is kept.
    Failed(LookupError),
}

/// What a round of lookups came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Looked {
    /// Requests that had an answer.
    pub asked: usize,
    pub found: usize,
    pub stopped: Stopped,
}

/// How arXiv is asked: at its API's address `base`, through `get` (which keeps arXiv's pace: the network does).
pub struct Arxiv<'a, 'g> {
    pub base: &'a str,
    pub get: &'a mut Get<'g>,
}

/// The entries of `list` (the list of `paper`) not in `library` that are worth asking arXiv about and were never
/// asked about, by their place in the list, in the order listed, with what to ask.
pub fn to_look_up(list: &KeptReferences, paper: &str, library: &Linker) -> Vec<(usize, Ask)> {
    let linked = library.read(paper, list.references());
    linked
        .references
        .iter()
        .enumerate()
        .filter(|(at, _)| list.entries[*at].lookup == Lookup::NotAsked)
        .filter_map(|(at, linked)| Some((at, ask_for(linked)?)))
        .collect()
}

/// Look up on `arxiv` the entries [`to_look_up`] names: at most `most` of them, in the order listed. Each answer
/// is written into its entry ([`Lookup`]); an entry found is identified from then on.
pub fn look_up(list: &mut KeptReferences, paper: &str, library: &Linker, most: usize, arxiv: Arxiv) -> Looked {
    let Arxiv { base, get } = arxiv;
    let wanted = to_look_up(list, paper, library);
    let (mut asked, mut found) = (0, 0);
    for (done, (at, ask)) in wanted.iter().enumerate() {
        if done == most {
            return Looked { asked, found, stopped: Stopped::AtMost { left: wanted.len() - done } };
        }
        let feed = match send(&url_for(base, ask), get) {
            Ok(feed) => feed,
            Err(error) => return Looked { asked, found, stopped: Stopped::Failed(error) },
        };
        asked += 1;
        let entry = &mut list.entries[*at];
        entry.lookup = match read_answer(ask, &entry.reference, &feed) {
            Answer::Found { arxiv, title, year } => {
                let reference = &mut entry.reference;
                reference.arxiv = Some(arxiv);
                reference.title = reference.title.take().or(Some(title));
                reference.year = reference.year.or(year);
                reference.confidence = Confidence::Identified;
                found += 1;
                Lookup::Found
            }
            Answer::NotFound => Lookup::NotFound,
            Answer::Ambiguous { .. } => Lookup::Ambiguous,
        };
    }
    Looked { asked, found, stopped: Stopped::AllAsked }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::library::Known;
    use crate::library::papers::passages::fixtures::pdf_with;
    use crate::library::papers::tests::paper;
    use crate::sources::citations::Response;

    fn library() -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        library.add_papers(&[paper("seed")], None, Known::Keep).unwrap();
        (folder, library)
    }

    fn seed_pdf() -> Vec<u8> {
        pdf_with(&[
            "1 Introduction",
            "",
            "We build on [1] and [2], not on [3].",
            "",
            "References",
            "",
            "[1] A. Lovelace. Editing models with task arithmetic. In ICLR, 2023.",
            "",
            "[2] A. Turing. Resolving interference when merging models. In NeurIPS, 2023.",
            "",
            "[3] G. Hopper. A work that is not in the library. In ICML, 2022.",
        ])
    }

    fn read_at(library: &Library) -> String {
        library
            .connection()
            .query_row("SELECT read_at FROM paper_references WHERE paper_id = 'seed'", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn a_reference_list_is_read_once_kept_and_read_again_only_for_another_pdf() {
        let (_folder, library) = library();
        assert_eq!(library.references_of("seed").unwrap(), ReferencesReading::NoPdf);
        assert!(matches!(library.references_of("nowhere"), Err(LibraryError::UnknownPaper(_))));
        library.keep_pdf("seed", &seed_pdf()).unwrap();
        let ReferencesReading::ReadNow { list, .. } = library.references_of("seed").unwrap() else { panic!() };
        assert_eq!(list.entries.len(), 3);
        assert_eq!(list.entries[0].reference.title.as_deref(), Some("Editing models with task arithmetic"));
        assert!(list.entries.iter().all(|entry| entry.lookup == Lookup::NotAsked));

        library.connection().execute("UPDATE paper_references SET read_at = 'kept'", []).unwrap();
        assert!(matches!(library.references_of("seed").unwrap(), ReferencesReading::Kept { .. }));
        assert_eq!(read_at(&library), "kept", "not read again");

        library.keep_pdf("seed", &pdf_with(&["1 Introduction", "", "Nothing is cited."])).unwrap();
        let ReferencesReading::ReadNow { list, .. } = library.references_of("seed").unwrap() else { panic!() };
        assert!(list.entries.is_empty());
        assert_ne!(read_at(&library), "kept", "another PDF is read again");
        assert_eq!(library.every_kept_reference_list().unwrap().len(), 1, "replaced, not added");
    }

    #[test]
    fn a_pdf_that_cannot_be_read_is_read_once_and_read_again_only_when_the_pdf_is_replaced() {
        let (_folder, library) = library();
        library.keep_pdf("seed", b"%PDF-1.4 not a real PDF at all").unwrap();
        let ReferencesReading::Unreadable(first) = library.references_of("seed").unwrap() else { panic!() };
        // marked: a second asking that read the PDF again would say the first reason, not the mark
        library
            .connection()
            .execute(r#"UPDATE paper_references SET content = '{"unreadable":{"kind":"no_text"}}'"#, [])
            .unwrap();
        let ReferencesReading::Unreadable(again) = library.references_of("seed").unwrap() else { panic!() };
        assert_eq!(again, Unreadable::NoText.to_string(), "kept, not read again (first: {first})");
        assert!(library.every_kept_reference_list().unwrap().is_empty(), "no list is made of it");
        library.keep_pdf("seed", &seed_pdf()).unwrap();
        assert!(matches!(library.references_of("seed").unwrap(), ReferencesReading::ReadNow { .. }));
    }

    #[test]
    fn a_list_kept_by_an_older_reader_is_read_again() {
        let (_folder, library) = library();
        library.keep_pdf("seed", &seed_pdf()).unwrap();
        library.references_of("seed").unwrap();
        library.connection().execute("UPDATE paper_references SET reader = reader - 1", []).unwrap();
        assert!(library.every_kept_reference_list().unwrap().is_empty());
        assert!(matches!(library.references_of("seed").unwrap(), ReferencesReading::ReadNow { .. }));
    }

    #[test]
    fn lookups_are_kept_only_on_the_list_they_were_made_for() {
        let (_folder, library) = library();
        library.keep_pdf("seed", &seed_pdf()).unwrap();
        let ReferencesReading::ReadNow { pdf_hash, mut list } = library.references_of("seed").unwrap() else {
            panic!()
        };
        list.entries[0].lookup = Lookup::NotFound;
        assert!(!library.keep_lookups("seed", "another-pdf", &list).unwrap());
        assert!(library.keep_lookups("seed", &pdf_hash, &list).unwrap());
        assert_eq!(library.kept_references("seed", &pdf_hash).unwrap().unwrap().entries[0].lookup, Lookup::NotFound);
    }

    const FOUND: &str = r#"<feed><entry><id>http://arxiv.org/abs/2505.11792v3</id><title>Unresolved work number 0 on arXiv</title><published>2025-05-17T02:32:03Z</published></entry></feed>"#;
    const NOTHING: &str = "<feed></feed>";

    fn unresolved(count: usize) -> KeptReferences {
        let entries = (0..count)
            .map(|n| KeptEntry {
                reference: Reference {
                    raw: format!("A. Author. Unresolved work number {n} on arxiv. 2025."),
                    authors: vec!["A. Author".to_owned()],
                    title: Some(format!("Unresolved work number {n} on arxiv")),
                    year: Some(2025),
                    venue: None,
                    arxiv: None,
                    doi: None,
                    url: None,
                    confidence: Confidence::TitleParsed,
                },
                lookup: Lookup::NotAsked,
            })
            .collect();
        KeptReferences { citing: Citing::ByNumber, entries }
    }

    #[test]
    fn a_round_of_lookups_asks_at_most_so_many_keeps_each_answer_and_never_asks_twice() {
        let papers = vec![];
        let linker = Linker::new(&papers);
        let mut list = unresolved(4);
        let asked = RefCell::new(Vec::new());
        let mut get = |url: &str| -> Result<Response, String> {
            asked.borrow_mut().push(url.to_owned());
            let body = if url.contains("number%200%20") { FOUND } else { NOTHING };
            Ok(Response { status: 200, retry_after: None, body: body.to_owned() })
        };
        let base = "https://export.arxiv.org/api/query";
        let mut round = |list: &mut KeptReferences| {
            let arxiv = Arxiv { base, get: &mut get };
            look_up(list, "seed", &linker, 3, arxiv)
        };
        let first = round(&mut list);
        assert_eq!(first, Looked { asked: 3, found: 1, stopped: Stopped::AtMost { left: 1 } });
        assert_eq!(list.entries[0].lookup, Lookup::Found);
        assert_eq!(list.entries[0].reference.arxiv.as_deref(), Some("2505.11792"));
        assert_eq!(list.entries[0].reference.confidence, Confidence::Identified);
        assert_eq!(list.entries[1].lookup, Lookup::NotFound);
        assert_eq!(list.entries[3].lookup, Lookup::NotAsked, "not reached");

        let second = round(&mut list);
        assert_eq!(second, Looked { asked: 1, found: 0, stopped: Stopped::AllAsked });
        let third = round(&mut list);
        drop(round);
        assert_eq!(asked.borrow().len(), 4, "each entry asked about once");
        assert_eq!(third, Looked { asked: 0, found: 0, stopped: Stopped::AllAsked });
        assert_eq!(asked.borrow().len(), 4);
    }

    #[test]
    fn a_failed_lookup_ends_the_round_and_leaves_the_entry_to_be_asked_again() {
        let papers = vec![];
        let linker = Linker::new(&papers);
        let mut list = unresolved(2);
        let mut get = |_: &str| -> Result<Response, String> { Err("no connection".to_owned()) };
        let arxiv = Arxiv { base: "https://x", get: &mut get };
        let looked = look_up(&mut list, "seed", &linker, 10, arxiv);
        assert_eq!(looked.asked, 0);
        assert!(matches!(looked.stopped, Stopped::Failed(LookupError::Unreachable(_))));
        assert!(list.entries.iter().all(|entry| entry.lookup == Lookup::NotAsked));
    }
}
