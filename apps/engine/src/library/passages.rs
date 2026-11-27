//! A paper's text as every feature that asks about papers starts from it: its PDF read once into passages
//! ([`crate::text::passages`]), kept, and handed out by paper id.
//!
//! # Where a paper's PDF is
//!
//! In the library's folder, at `pdfs/<id>.pdf`, the id with every character other than a letter, a digit, `_`,
//! `.` or `-` written `_` ([`Library::pdf_file`]). That is where the Python backend keeps the PDFs it fetches
//! (`rp/proceedings.py`, `fetch_pdf` into `LIBRARY / "pdfs"`), so those are found as they are. The file itself is
//! the record that a paper's PDF is stored: [`Library::keep_pdf`] puts one there, replaced whole
//! ([`super::store::replace`]), so a reader never sees half a PDF. Nothing here downloads: fetching a PDF is the
//! sources' work (`sources::proceedings::download_pdf`), and a paper without one is read from its record.
//!
//! # Read once
//!
//! What a version of the reader ([`READER`]) made of a PDF is kept by paper, with the PDF's SHA-256: its passages,
//! or why it could not be read, so that a PDF that cannot be read (damaged, scanned, a layout the reader does not
//! handle, a panic in the PDF library) is not tried again every time it is asked for. It is read again only when
//! the reader's version or the file changes. Whether the file changed is first told from its size and time of
//! change, so that asking for 739 kept papers reads no PDF at all; only when those differ is the file hashed, and
//! read again only when its hash differs too.
//!
//! # Many threads asking at once
//!
//! Reading a PDF takes up to a second of one core, so a batch reads the PDFs it needs on every core (rayon). Two
//! batches asking for the same paper at the same moment must not both read it: a paper being read is claimed in
//! [`InReading`], and whoever asks for it meanwhile waits for that reading instead of starting its own. A claim is
//! given up only after what was read is in the database, so whoever comes after finds it there. Each batch reads
//! the PDFs it claimed before it waits for any it did not, so no two batches can wait on each other.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

use super::{Library, LibraryError, Paper, now, placeholders, store};
use crate::evidence::{PaperText, Papers};
use crate::text::passages::{Passages, READER, passages_of_pdf};
use crate::text::pdf::Unreadable;

/// The folder inside the library's folder that holds the papers' PDFs.
pub const PDF_FOLDER: &str = "pdfs";
/// Papers asked about in one statement: well under SQLite's limit on bound values.
const IDS_AT_ONCE: usize = 500;

/// What a paper's text came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaperReading {
    /// Its PDF was read (now, or before and kept).
    Read(Passages),
    /// The library has no PDF of it: its title and abstract are what there is.
    NoPdf { title: String, abstract_text: String },
    /// Its PDF could not be read, for the reason given; remembered until the reader or the file changes.
    Unreadable { title: String, abstract_text: String, why: String },
    /// Not a paper of the library.
    NotInLibrary,
}

impl PaperReading {
    pub fn passages(&self) -> Option<&Passages> {
        match self {
            Self::Read(passages) => Some(passages),
            Self::NoPdf { .. } | Self::Unreadable { .. } | Self::NotInLibrary => None,
        }
    }

    /// What its quotes are checked against ([`crate::evidence::check`]): its passages, or else its record. None
    /// for a paper the library does not have.
    pub fn into_text(self) -> Option<PaperText> {
        match self {
            Self::Read(passages) => Some(PaperText::Read(passages)),
            Self::NoPdf { title, abstract_text } | Self::Unreadable { title, abstract_text, .. } => {
                Some(PaperText::Unread { title, abstract_text })
            }
            Self::NotInLibrary => None,
        }
    }

    fn of(record: &Paper, pdf: Pdf) -> Self {
        let (title, abstract_text) = (record.title.clone(), record.r#abstract.clone());
        match pdf {
            Pdf::Read(passages) => Self::Read(passages),
            Pdf::Missing => Self::NoPdf { title, abstract_text },
            Pdf::Unreadable(why) => Self::Unreadable { title, abstract_text, why },
        }
    }
}

/// What a paper's PDF came to, apart from the paper's record: what one reading hands to everyone waiting for it.
#[derive(Clone, Debug)]
enum Pdf {
    Read(Passages),
    Unreadable(String),
    Missing,
}

/// A PDF file as the file system describes it, which tells that it may have changed without reading it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    size: i64,
    /// Nanoseconds since 1970.
    modified: i64,
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    let found = fs::metadata(path).ok().filter(fs::Metadata::is_file)?;
    let modified = found
        .modified()
        .ok()
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| i64::try_from(since.as_nanos()).unwrap_or(i64::MAX));
    Some(Stamp { size: i64::try_from(found.len()).unwrap_or(i64::MAX), modified })
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A kept reading, as its row has it.
struct Row {
    reader: i64,
    hash: String,
    stamp: Stamp,
    outcome: String,
    content: String,
}

impl Row {
    fn by_this_reader(&self) -> bool {
        self.reader == i64::from(READER)
    }

    /// What it says, unless it cannot be read back (then the PDF is read again).
    fn decoded(&self) -> Option<Pdf> {
        match self.outcome.as_str() {
            "read" => serde_json::from_str(&self.content).ok().map(Pdf::Read),
            "unreadable" => Some(Pdf::Unreadable(self.content.clone())),
            _ => None,
        }
    }
}

const ROW: &str = "SELECT paper_id, reader, pdf_hash, pdf_size, pdf_modified, outcome, content FROM paper_passages";

fn row_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, Row)> {
    Ok((
        row.get(0)?,
        Row {
            reader: row.get(1)?,
            hash: row.get(2)?,
            stamp: Stamp { size: row.get(3)?, modified: row.get(4)? },
            outcome: row.get(5)?,
            content: row.get(6)?,
        },
    ))
}

/// A reading under way, which whoever else asks for the paper waits for.
#[derive(Default)]
struct Pending {
    outcome: Mutex<Option<Pdf>>,
    ready: Condvar,
}

impl Pending {
    fn publish(&self, pdf: Pdf) {
        *self.outcome.lock().unwrap_or_else(PoisonError::into_inner) = Some(pdf);
        self.ready.notify_all();
    }

    fn wait(&self) -> Pdf {
        let mut outcome = self.outcome.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(pdf) = outcome.as_ref() {
                return pdf.clone();
            }
            outcome = self.ready.wait(outcome).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// The papers whose PDF is being read right now, each with the reading others wait for.
#[derive(Default)]
pub(super) struct InReading {
    papers: Mutex<HashMap<String, Arc<Pending>>>,
}

impl fmt::Debug for InReading {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reading = self.papers.lock().map_or(0, |papers| papers.len());
        write!(formatter, "InReading({reading} papers)")
    }
}

/// The claim on one paper's reading. Dropped, it hands what was read to whoever waits and lets the paper be
/// claimed again, also when the reading was cut short by a panic.
struct Claimed<'a> {
    reading: &'a InReading,
    paper_id: &'a str,
    pending: Arc<Pending>,
    outcome: Option<Pdf>,
}

impl Drop for Claimed<'_> {
    fn drop(&mut self) {
        let outcome = self
            .outcome
            .take()
            .unwrap_or_else(|| Pdf::Unreadable("the reading of this PDF stopped unexpectedly".into()));
        let mut papers = self.reading.papers.lock().unwrap_or_else(PoisonError::into_inner);
        if papers.get(self.paper_id).is_some_and(|pending| Arc::ptr_eq(pending, &self.pending)) {
            papers.remove(self.paper_id);
        }
        drop(papers);
        self.pending.publish(outcome);
    }
}

/// Is a file named `name` (with any extension after it) one Windows opens as a device? Its part before the first
/// `.`, spaces at its end dropped, in any case: `CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, `COM0`-`COM9` and
/// `LPT0`-`LPT9`, the superscript digits `¹²³` counting as digits, as Windows counts them.
fn windows_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end().to_uppercase();
    let numbered = |prefix: &str| {
        let mut rest = stem.strip_prefix(prefix).unwrap_or("x").chars();
        matches!((rest.next(), rest.next()), (Some('0'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'), None))
    };
    ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str()) || numbered("COM") || numbered("LPT")
}

/// How a PDF's bytes are made passages: [`passages_of_pdf`], or the tests' own.
type ReadPdf<'a> = dyn Fn(&[u8]) -> Result<Passages, Unreadable> + Sync + 'a;

impl Library {
    /// Where the PDF of `paper_id` is kept, whether or not it is there.
    ///
    /// This is the one place a paper id becomes a file name. Every character but a letter, a digit, `_`, `.` and
    /// `-` is written `_`, as the Python backend did, so the files it kept are found under their names; and a
    /// name Windows takes for a device (`CON`, `NUL`, `COM1`, also before an extension: `nul.x.pdf`) gets a `_`
    /// in front, since opening it would open the device. No paper's id is such a name today, so no kept file is
    /// renamed by that.
    pub fn pdf_file(&self, paper_id: &str) -> PathBuf {
        let name: String =
            paper_id.chars().map(|c| if c.is_alphanumeric() || "_.-".contains(c) { c } else { '_' }).collect();
        let name = if windows_device(&name) { format!("_{name}") } else { name };
        self.path.with_file_name(PDF_FOLDER).join(format!("{name}.pdf"))
    }

    /// Keep `bytes` as the PDF of `paper_id` (replacing any), where [`Library::paper_text`] finds it. The bytes are
    /// taken as they are: whoever downloads checks that they are a PDF.
    pub fn keep_pdf(&self, paper_id: &str, bytes: &[u8]) -> Result<PathBuf, LibraryError> {
        self.paper(paper_id)?.ok_or_else(|| LibraryError::UnknownPaper(paper_id.to_owned()))?;
        let path = self.pdf_file(paper_id);
        store::replace(&path, bytes)?;
        Ok(path)
    }

    /// The text of one paper to check quotes against: its passages when its PDF can be read, its title and
    /// abstract when not. [`LibraryError::UnknownPaper`] for a paper the library does not have.
    pub fn paper_text(&self, paper_id: &str) -> Result<PaperText, LibraryError> {
        self.paper_texts(&[paper_id])?
            .pop()
            .and_then(PaperReading::into_text)
            .ok_or_else(|| LibraryError::UnknownPaper(paper_id.to_owned()))
    }

    /// What each of `paper_ids` reads as, in the order asked. The PDFs not read before are read now, on every
    /// core; one that cannot be read, or makes the PDF reader panic, is that paper's [`PaperReading::Unreadable`]
    /// and fails nothing else. Several hundred at a time is a good size: what is handed back is held in memory.
    ///
    /// Call it from a thread of its own (a request's blocking thread, a job), never from inside a rayon task: it
    /// may wait for a reading another caller holds, and a pool worker waiting so can starve the pool it waits on.
    pub fn paper_texts(&self, paper_ids: &[&str]) -> Result<Vec<PaperReading>, LibraryError> {
        self.texts_read_with(paper_ids, &passages_of_pdf)
    }

    /// The papers among `paper_ids` the library has, by id, as [`crate::evidence::check`] takes them.
    pub fn citable(&self, paper_ids: &[&str]) -> Result<Papers, LibraryError> {
        let readings = self.paper_texts(paper_ids)?;
        Ok(paper_ids
            .iter()
            .zip(readings)
            .filter_map(|(id, reading)| Some(((*id).to_owned(), reading.into_text()?)))
            .collect())
    }

    fn texts_read_with(&self, paper_ids: &[&str], read: &ReadPdf<'_>) -> Result<Vec<PaperReading>, LibraryError> {
        let mut unique: Vec<&str> = paper_ids.to_vec();
        unique.sort_unstable();
        unique.dedup();
        let records = self.papers(&unique)?;
        let mut rows = self.passage_rows(&unique)?;

        // what is on disk for each paper, and whether what is kept was read from it
        let mut found: HashMap<&str, Pdf> = HashMap::new();
        let mut kept: Vec<(&str, Row)> = Vec::new();
        let mut to_read: Vec<&str> = Vec::new();
        for (id, record) in unique.iter().zip(&records) {
            if record.is_none() {
                continue;
            }
            match (stamp_of(&self.pdf_file(id)), rows.remove(*id)) {
                (None, _) => {
                    found.insert(id, Pdf::Missing);
                }
                (Some(stamp), Some(row)) if row.by_this_reader() && row.stamp == stamp => kept.push((id, row)),
                (Some(_), _) => to_read.push(id),
            }
        }
        let decoded: Vec<(&str, Option<Pdf>)> = kept.par_iter().map(|(id, row)| (*id, row.decoded())).collect();
        for (id, pdf) in decoded {
            match pdf {
                Some(pdf) => {
                    found.insert(id, pdf);
                }
                None => to_read.push(id),
            }
        }

        // claim what no one else is reading; read what was claimed, on every core; then wait for the rest. A
        // claim's guard is made with it, so that a panic anywhere after (even one that stops the parallel loop
        // before it reaches a paper) gives every claim up: nobody is left waiting for a reading that never comes.
        let mut mine: Vec<Claimed<'_>> = Vec::new();
        let mut theirs: Vec<(&str, Arc<Pending>)> = Vec::new();
        {
            let mut papers = self.in_reading.papers.lock().unwrap_or_else(PoisonError::into_inner);
            for id in &to_read {
                match papers.get(*id) {
                    Some(pending) => theirs.push((id, Arc::clone(pending))),
                    None => {
                        let pending = Arc::new(Pending::default());
                        papers.insert((*id).to_owned(), Arc::clone(&pending));
                        mine.push(Claimed { reading: &self.in_reading, paper_id: id, pending, outcome: None });
                    }
                }
            }
        }
        let read_now: Vec<(&str, Pdf)> = mine
            .into_par_iter()
            .map(|mut claimed| {
                let pdf = self.read_pdf(claimed.paper_id, read);
                claimed.outcome = Some(pdf.clone());
                (claimed.paper_id, pdf)
            })
            .collect();
        found.extend(read_now);
        for (id, pending) in theirs {
            found.insert(id, pending.wait());
        }

        Ok(paper_ids
            .iter()
            .map(|id| {
                let record = unique.binary_search(id).ok().and_then(|index| records[index].as_ref());
                match (record, found.get(id)) {
                    (Some(record), Some(pdf)) => PaperReading::of(record, pdf.clone()),
                    (Some(record), None) => PaperReading::of(record, Pdf::Missing),
                    (None, _) => PaperReading::NotInLibrary,
                }
            })
            .collect())
    }

    /// Read one claimed paper's PDF, unless what is kept is still its reading, and keep what was read.
    fn read_pdf(&self, paper_id: &str, read: &ReadPdf<'_>) -> Pdf {
        let path = self.pdf_file(paper_id);
        let Some(stamp) = stamp_of(&path) else { return Pdf::Missing };
        // kept by whoever held the claim just before this one, or a file touched but not changed
        let kept = self.passage_row(paper_id).unwrap_or_else(|error| {
            eprintln!("library: the kept passages of {paper_id} could not be looked at: {error}");
            None
        });
        let kept = kept.filter(Row::by_this_reader);
        if let Some(pdf) = kept.as_ref().filter(|row| row.stamp == stamp).and_then(Row::decoded) {
            return pdf;
        }
        let Ok(bytes) = fs::read(&path) else { return Pdf::Missing };
        let hash = sha256(&bytes);
        if let Some(pdf) = kept.as_ref().filter(|row| row.hash == hash).and_then(Row::decoded) {
            if let Err(error) = self.restamp_passages(paper_id, stamp) {
                eprintln!("library: the kept passages of {paper_id} could not be updated: {error}");
            }
            return pdf;
        }
        let pdf = match catch_unwind(AssertUnwindSafe(|| read(&bytes))) {
            Ok(Ok(passages)) => Pdf::Read(passages),
            // the reader could not be started: nothing is known of the PDF, so nothing is kept and it is tried again
            Ok(Err(why)) if !why.is_lasting() => {
                eprintln!("library: the PDF of {paper_id} was not read: {why}");
                return Pdf::Unreadable(why.to_string());
            }
            Ok(Err(why)) => Pdf::Unreadable(why.to_string()),
            Err(_) => Pdf::Unreadable(Unreadable::Unhandled.to_string()),
        };
        if let Err(error) = self.put_passages(paper_id, &hash, stamp, &pdf) {
            // what was read is still handed out; it is read again next time
            eprintln!("library: the passages of {paper_id} could not be kept: {error}");
        }
        pdf
    }

    fn passage_rows(&self, paper_ids: &[&str]) -> Result<HashMap<String, Row>, LibraryError> {
        self.read(|connection| {
            let mut rows = HashMap::new();
            for ids in paper_ids.chunks(IDS_AT_ONCE) {
                let mut statement =
                    connection.prepare(&format!("{ROW} WHERE paper_id IN ({})", placeholders(ids.len())))?;
                let found = statement.query_map(rusqlite::params_from_iter(ids), row_of)?;
                for row in found {
                    let (id, row) = row?;
                    rows.insert(id, row);
                }
            }
            Ok(rows)
        })
    }

    fn passage_row(&self, paper_id: &str) -> Result<Option<Row>, LibraryError> {
        self.read(|connection| {
            Ok(connection
                .query_row(&format!("{ROW} WHERE paper_id = ?1"), [paper_id], row_of)
                .optional()?
                .map(|(_, row)| row))
        })
    }

    fn put_passages(&self, paper_id: &str, hash: &str, stamp: Stamp, pdf: &Pdf) -> Result<(), LibraryError> {
        let (outcome, content) = match pdf {
            Pdf::Read(passages) => ("read", serde_json::to_string(passages)?),
            Pdf::Unreadable(why) => ("unreadable", why.clone()),
            Pdf::Missing => return Ok(()),
        };
        self.write(|transaction| {
            transaction
                .prepare_cached(
                    "INSERT INTO paper_passages
                         (paper_id, reader, pdf_hash, pdf_size, pdf_modified, outcome, content, read_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT (paper_id) DO UPDATE SET
                         reader = excluded.reader, pdf_hash = excluded.pdf_hash, pdf_size = excluded.pdf_size,
                         pdf_modified = excluded.pdf_modified, outcome = excluded.outcome,
                         content = excluded.content, read_at = excluded.read_at",
                )?
                .execute(params![paper_id, READER, hash, stamp.size, stamp.modified, outcome, content, now()])?;
            Ok(())
        })
    }

    fn restamp_passages(&self, paper_id: &str, stamp: Stamp) -> Result<(), LibraryError> {
        self.write(|transaction| {
            transaction.execute(
                "UPDATE paper_passages SET pdf_size = ?2, pdf_modified = ?3 WHERE paper_id = ?1",
                params![paper_id, stamp.size, stamp.modified],
            )?;
            Ok(())
        })
    }
}

/// PDFs made for tests, so that no test needs a file.
#[cfg(test)]
pub(crate) mod fixtures {
    /// A one-page PDF that writes `lines` in Helvetica, one below the other.
    pub(crate) fn pdf_with(lines: &[&str]) -> Vec<u8> {
        let mut content = String::from("BT /F1 10 Tf 72 720 Td 14 TL\n");
        for line in lines {
            content.push_str(&format!("({line}) Tj T*\n"));
        }
        content.push_str("ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
                .to_owned(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_owned(),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (number, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend(format!("{} 0 obj\n{object}\nendobj\n", number + 1).bytes());
        }
        let table = pdf.len();
        pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            pdf.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        pdf.extend(
            format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n", objects.len() + 1).bytes(),
        );
        pdf
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use super::*;
    use crate::library::Known;
    use crate::library::papers::tests::paper;
    use crate::text::passages::read_passages;

    fn library_with(ids: &[&str]) -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let papers: Vec<_> = ids.iter().map(|id| paper(id)).collect();
        library.add_papers(&papers, None, Known::Keep).unwrap();
        (folder, library)
    }

    /// A stand-in for the PDF reader that counts what it reads: "%PDF <words>" reads as a page of those words;
    /// "%PDF bad" cannot be read; "%PDF panic" makes it panic, as the PDF library sometimes does.
    #[derive(Default)]
    struct Counting {
        read: Mutex<BTreeMap<String, usize>>,
        slow: Option<Duration>,
    }

    impl Counting {
        fn read(&self, bytes: &[u8]) -> Result<Passages, Unreadable> {
            let text = String::from_utf8_lossy(bytes).trim_start_matches("%PDF ").to_owned();
            *self.read.lock().unwrap().entry(text.clone()).or_default() += 1;
            if let Some(slow) = self.slow {
                std::thread::sleep(slow);
            }
            match text.as_str() {
                "bad" => Err(Unreadable::NoText),
                "panic" => panic!("the PDF library gave up"),
                _ => Ok(read_passages(&[text])),
            }
        }

        fn times(&self) -> BTreeMap<String, usize> {
            self.read.lock().unwrap().clone()
        }
    }

    fn texts(library: &Library, reader: &Counting, ids: &[&str]) -> Vec<PaperReading> {
        library.texts_read_with(ids, &|bytes| reader.read(bytes)).unwrap()
    }

    const PAGE: &str = "We study how a paper is read once and kept for every feature that asks about it.";

    #[test]
    fn a_pdf_is_read_once_and_kept_until_the_reader_or_the_file_changes() {
        let (_folder, library) = library_with(&["2601.00001"]);
        library.keep_pdf("2601.00001", format!("%PDF {PAGE}").as_bytes()).unwrap();
        let reader = Counting::default();
        let first = texts(&library, &reader, &["2601.00001"]);
        assert!(matches!(&first[0], PaperReading::Read(passages) if passages.reader == READER));
        assert_eq!(texts(&library, &reader, &["2601.00001", "2601.00001"]), [first[0].clone(), first[0].clone()]);
        assert_eq!(reader.times()[PAGE], 1, "kept: not read again");

        // the same bytes written again: the file's time changed, its hash did not, so it is not read again
        std::thread::sleep(Duration::from_millis(20));
        library.keep_pdf("2601.00001", format!("%PDF {PAGE}").as_bytes()).unwrap();
        assert_eq!(texts(&library, &reader, &["2601.00001"]), first);
        assert_eq!(reader.times()[PAGE], 1);

        // another version of the reader reads it again
        library.connection().execute("UPDATE paper_passages SET reader = ?1", [READER - 1]).unwrap();
        texts(&library, &reader, &["2601.00001"]);
        assert_eq!(reader.times()[PAGE], 2);

        // another PDF is read
        library.keep_pdf("2601.00001", b"%PDF A different paper altogether, read when its file changed.").unwrap();
        let changed = texts(&library, &reader, &["2601.00001"]);
        assert_ne!(changed, first);
        assert_eq!(reader.times().values().sum::<usize>(), 3);
    }

    #[test]
    fn a_paper_without_a_pdf_or_not_in_the_library_says_so_and_still_has_its_record() {
        let (_folder, library) = library_with(&["iclr-2026-4"]);
        let reader = Counting::default();
        let read = texts(&library, &reader, &["iclr-2026-4", "nowhere"]);
        assert_eq!(
            read,
            [
                PaperReading::NoPdf {
                    title: "title of iclr-2026-4".to_owned(),
                    abstract_text: "abstract of iclr-2026-4".to_owned()
                },
                PaperReading::NotInLibrary
            ]
        );
        assert_eq!(
            library.paper_text("iclr-2026-4").unwrap(),
            PaperText::Unread {
                title: "title of iclr-2026-4".to_owned(),
                abstract_text: "abstract of iclr-2026-4".to_owned()
            }
        );
        assert!(matches!(library.paper_text("nowhere"), Err(LibraryError::UnknownPaper(_))));
        assert!(matches!(library.keep_pdf("nowhere", b"%PDF x"), Err(LibraryError::UnknownPaper(_))));
        assert_eq!(library.citable(&["iclr-2026-4", "nowhere"]).unwrap().len(), 1);
        assert!(reader.times().is_empty());
    }

    #[test]
    fn an_unreadable_pdf_or_a_panic_fails_only_its_paper_and_is_not_tried_again() {
        let (_folder, library) = library_with(&["a", "b", "c"]);
        library.keep_pdf("a", b"%PDF bad").unwrap();
        library.keep_pdf("b", b"%PDF panic").unwrap();
        library.keep_pdf("c", format!("%PDF {PAGE}").as_bytes()).unwrap();
        let reader = Counting::default();
        let read = texts(&library, &reader, &["a", "b", "c"]);
        assert!(matches!(&read[0], PaperReading::Unreadable { why, .. } if why.contains("no text")), "{:?}", read[0]);
        assert!(matches!(&read[1], PaperReading::Unreadable { .. }), "{:?}", read[1]);
        assert!(matches!(&read[2], PaperReading::Read(_)));
        assert_eq!(texts(&library, &reader, &["a", "b", "c"]), read);
        assert_eq!(reader.times().values().copied().collect::<Vec<_>>(), [1, 1, 1], "remembered, not retried");
        // the record is what quotes are checked against
        assert!(matches!(read[0].clone().into_text(), Some(PaperText::Unread { .. })));
    }

    #[test]
    fn the_real_reader_reads_a_kept_pdf() {
        let (_folder, library) = library_with(&["p"]);
        let bytes =
            fixtures::pdf_with(&["Introduction", "We study how passages are read from a paper's own PDF file."]);
        let path = library.keep_pdf("p", &bytes).unwrap();
        assert!(path.ends_with(Path::new(PDF_FOLDER).join("p.pdf")));
        let PaperText::Read(passages) = library.paper_text("p").unwrap() else { panic!() };
        assert!(passages.locate("passages are read from a paper's own PDF").is_some());
    }

    #[test]
    fn a_pdf_is_kept_where_the_python_backend_keeps_it() {
        let (_folder, library) = library_with(&[]);
        let file = |id: &str| library.pdf_file(id).file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(file("2601.00001"), "2601.00001.pdf");
        assert_eq!(file("neurips-2025-x9"), "neurips-2025-x9.pdf");
        assert_eq!(file("hep-th/9901001"), "hep-th_9901001.pdf");
        assert_eq!(file("../../etc"), ".._.._etc.pdf", "never outside the folder");
        assert_eq!(library.pdf_file("x").parent().unwrap(), library.path().with_file_name(PDF_FOLDER));
    }

    #[test]
    fn an_id_that_windows_would_take_for_a_device_never_names_a_file_as_one() {
        let (_folder, library) = library_with(&[]);
        let file = |id: &str| library.pdf_file(id).file_name().unwrap().to_string_lossy().into_owned();
        for (id, kept) in [
            ("CON", "_CON.pdf"),
            ("nul", "_nul.pdf"),
            ("Com1", "_Com1.pdf"),
            ("lpt9.v2", "_lpt9.v2.pdf"),
            ("AUX", "_AUX.pdf"),
            ("COM\u{b9}", "_COM\u{b9}.pdf"),
            ("prn.x.y", "_prn.x.y.pdf"),
        ] {
            assert_eq!(file(id), kept, "{id:?}");
        }
        // names that only look like one keep their name: those of files kept before
        for id in ["console", "com10", "nullable-2601", "2601.00001", "CONIN$"] {
            assert!(!file(id).starts_with('_'), "{id:?}");
        }
    }

    #[test]
    fn many_threads_asking_for_overlapping_papers_at_once_read_each_pdf_once() {
        const PAPERS: usize = 24;
        const ASKERS: usize = 8;
        let ids: Vec<String> = (0..PAPERS).map(|paper| format!("p{paper}")).collect();
        let (_folder, library) = library_with(&ids.iter().map(String::as_str).collect::<Vec<_>>());
        for id in &ids {
            library.keep_pdf(id, format!("%PDF The text of paper {id}, which is read only once.").as_bytes()).unwrap();
        }
        let reader = Counting { slow: Some(Duration::from_millis(15)), ..Counting::default() };
        let start = std::sync::Barrier::new(ASKERS);
        let seen: Vec<Vec<PaperReading>> = std::thread::scope(|scope| {
            let askers: Vec<_> = (0..ASKERS)
                .map(|asker| {
                    let (library, reader, start, ids) = (&library, &reader, &start, &ids);
                    scope.spawn(move || {
                        // each asker half the papers, from its own starting point: every paper asked by several
                        let wanted: Vec<&str> =
                            (0..PAPERS / 2).map(|step| ids[(asker * 3 + step) % PAPERS].as_str()).collect();
                        start.wait();
                        (wanted.clone(), texts(library, reader, &wanted))
                    })
                })
                .collect();
            askers
                .into_iter()
                .map(|asker| {
                    let (wanted, read) = asker.join().unwrap();
                    for (id, reading) in wanted.iter().zip(&read) {
                        let PaperReading::Read(passages) = reading else { panic!("{id}: {reading:?}") };
                        assert!(passages.pages[0].text.contains(&format!("paper {id},")), "{id} got another's text");
                    }
                    read
                })
                .collect()
        });
        assert_eq!(seen.len(), ASKERS);
        let times = reader.times();
        assert_eq!(times.len(), PAPERS, "every paper was asked for");
        assert!(times.values().all(|&times| times == 1), "each PDF parsed once, not once per asker: {times:?}");
        assert!(library.in_reading.papers.lock().unwrap().is_empty(), "no claim is left behind");
    }
}
