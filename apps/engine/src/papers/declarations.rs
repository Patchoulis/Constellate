//! What a paper's authors declare, read from its PDF once per version of the reading and per PDF, and kept: for
//! one paper when the reader asks, for every paper of a run as a job.
//!
//! The text is the PDF's pages one after the other, with their lines as the PDF draws them (headings and checklist
//! items are lines, which a paper's passages join into paragraphs), read in a child process like every PDF
//! ([`crate::text::pdf`]), several at once on the engine's pool. A PDF that is not kept is fetched first
//! ([`super::pdf`]), a few at once, each site at its own pace.
//!
//! A reading is kept with the version of the reading ([`PARSER`]) and the SHA-256 of the PDF read, and looked up
//! before anything is fetched or read. What is kept: what was read, or that the PDF has no text to read (fewer than
//! [`MIN_TEXT`] characters: a scan, a broken file, a PDF the reader cannot read). That a PDF could not be had is
//! not kept, since it can change (a conference's proceedings appear track by track); it is counted and said.
//!
//! Unlike the Python backend, which fetched a run's PDFs into a temporary folder and dropped them, the PDFs
//! fetched are kept in the library: the reader, the passages and every feature need them too, and a reading is
//! only known to be of the PDF it was read from by that PDF's hash.

use std::collections::HashMap;
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use super::pdf::{PdfOutcome, Pdfs};
use crate::library::proceedings::KeptDeclarations;
use crate::library::{Declarations, Library, LibraryError, Reading};
use crate::sources::polite::IN_FLIGHT;
use crate::text::declarations::{PARSER, read_declarations};
use crate::text::pdf::pdf_pages;
use crate::threads::{self, Outcome, Progress};

/// Less text than this, in characters, is a scan or a broken file, not a paper.
pub const MIN_TEXT: usize = 2000;
/// Papers fetched and read together: their PDFs' texts are held in memory until the batch is kept.
const BATCH: usize = 24;

/// What reading one paper's declarations came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Declared {
    /// Read now or before: what the authors declare.
    Read(Declarations),
    /// The PDF has no text to read.
    NoText,
    /// There is no PDF: why.
    NoPdf(PdfOutcome),
    /// The PDF could not be read, for this reason.
    Unreadable(String),
}

fn sha256_of_file(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect())
}

/// A kept reading as told.
fn told(kept: &KeptDeclarations) -> Option<Declared> {
    match &kept.reading {
        Reading::Read(declarations) => Some(Declared::Read(declarations.clone())),
        Reading::Unreadable => Some(Declared::NoText),
        // never kept by this reading; one moved in is not trusted (the PDF may be had now)
        Reading::NoPdf => None,
    }
}

/// What the reading of a paper's text says, in the library's shape.
fn declared_in(text: &str) -> Result<Reading, LibraryError> {
    if text.chars().count() < MIN_TEXT {
        return Ok(Reading::Unreadable);
    }
    let read = read_declarations(text);
    // the two shapes are the same JSON (the contract's `Declarations`): the library's keeps plain strings
    Ok(Reading::Read(serde_json::from_value(serde_json::to_value(read)?)?))
}

/// The declarations of one paper: kept, or read now (its PDF fetched first when needed) and kept.
pub fn declarations_of(library: &Library, pdfs: &Pdfs, paper_id: &str) -> Result<Declared, LibraryError> {
    // every paper asked has an answer; a paper the library does not have is an error before that
    read_many(library, pdfs, &[paper_id], &Progress::new())?
        .remove(paper_id)
        .ok_or_else(|| LibraryError::UnknownPaper(paper_id.to_owned()))
}

/// What reading a run's papers came to, for the job's log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub kept_before: usize,
    pub read: usize,
    pub no_text: usize,
    pub no_pdf: usize,
    pub unreadable: usize,
}

impl Counts {
    fn add(&mut self, declared: &Declared, before: bool) {
        match (declared, before) {
            (Declared::Read(_) | Declared::NoText, true) => self.kept_before += 1,
            (Declared::Read(_), false) => self.read += 1,
            (Declared::NoText, false) => self.no_text += 1,
            (Declared::NoPdf(_), _) => self.no_pdf += 1,
            (Declared::Unreadable(_), _) => self.unreadable += 1,
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "{} papers read before; {} read now; {} have no text that can be read; {} have no PDF a program may \
             fetch; {} could not be read",
            self.kept_before, self.read, self.no_text, self.no_pdf, self.unreadable
        )
    }
}

/// Read the declarations of every one of `paper_ids`, `BATCH` at a time, keeping each batch before the next;
/// `progress` counts the papers and is looked at between batches for a cancel, so a cancelled job keeps what it
/// read and the next one carries on from there. Answers what came of each paper, and whether it was kept before.
fn read_with_origin(
    library: &Library,
    pdfs: &Pdfs,
    paper_ids: &[&str],
    progress: &Progress,
) -> Result<HashMap<String, (Declared, bool)>, LibraryError> {
    let mut ids: Vec<&str> = paper_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    progress.expect(ids.len());
    let kept = library.kept_declarations(&ids, PARSER)?;
    let mut answers = HashMap::new();
    for batch in ids.chunks(BATCH) {
        if progress.is_cancelled() {
            break;
        }
        // the batch's PDFs, a few fetched at once (each site's own pace is kept by the fetcher)
        let fetched: Mutex<Vec<(&str, Result<PdfOutcome, LibraryError>)>> = Mutex::new(Vec::new());
        let next = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..IN_FLIGHT.min(batch.len()) {
                scope.spawn(|| {
                    loop {
                        let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(id) = batch.get(index) else { break };
                        // a reading kept without its PDF's hash, or of a PDF no longer kept, needs no PDF fetched
                        if kept.get(*id).is_some_and(|kept| {
                            told(kept).is_some() && (kept.pdf_hash.is_none() || !library.pdf_file(id).is_file())
                        }) {
                            continue;
                        }
                        let outcome = pdfs.pdf(library, id);
                        fetched.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((id, outcome));
                    }
                });
            }
        });
        let mut files: HashMap<&str, PdfOutcome> = HashMap::new();
        for (id, outcome) in fetched.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner) {
            files.insert(id, outcome?);
        }

        // what is kept and still of this PDF; the rest is read
        let hashes: Vec<Outcome<Option<String>>> = threads::each(batch, &Progress::new(), |id| {
            files.get(id).and_then(PdfOutcome::file).and_then(|path| sha256_of_file(path))
        });
        let mut to_read: Vec<(&str, String)> = Vec::new();
        for (id, hash) in batch.iter().zip(hashes) {
            let hash = match hash {
                Outcome::Done(hash) => hash,
                Outcome::Panicked(_) | Outcome::Cancelled => None,
            };
            let before = kept.get(*id).filter(|kept| hash.as_deref().is_none_or(|hash| kept.is_of(hash)));
            match (before.and_then(told), hash, files.get(id)) {
                (Some(declared), _, _) => {
                    answers.insert((*id).to_owned(), (declared, true));
                }
                (None, Some(hash), _) => to_read.push((id, hash)),
                (None, None, Some(outcome)) if outcome.file().is_none() => {
                    answers.insert((*id).to_owned(), (Declared::NoPdf(outcome.clone()), false));
                }
                (None, None, _) => {
                    answers.insert((*id).to_owned(), (Declared::Unreadable("its PDF could not be read".into()), false));
                }
            }
        }

        // the PDFs' texts with their lines as the PDF has them (headings and checklist items are lines, which the
        // passages join), read on every core, each in a child process; then the declarations in them
        let texts = threads::each(&to_read, &Progress::new(), |(id, _)| {
            let bytes = std::fs::read(library.pdf_file(id)).map_err(|error| (error.to_string(), false))?;
            pdf_pages(&bytes).map(|pages| pages.join("\n")).map_err(|why| (why.to_string(), why.is_lasting()))
        });
        let mut keep: Vec<(&str, &str, Reading)> = Vec::new();
        for ((id, hash), text) in to_read.iter().zip(texts) {
            let text = match text {
                Outcome::Done(text) => text,
                Outcome::Panicked(why) => Err((why, false)),
                Outcome::Cancelled => Err(("not read: the job was stopped".to_owned(), false)),
            };
            let declared = match text {
                // a PDF that cannot be read is kept as such (by its hash), so that it is not read at every run
                Err((why, true)) => {
                    keep.push((id, hash, Reading::Unreadable));
                    Declared::Unreadable(why)
                }
                Err((why, false)) => Declared::Unreadable(why),
                Ok(text) => {
                    let reading = declared_in(&text)?;
                    let declared = match &reading {
                        Reading::Read(declarations) => Declared::Read(declarations.clone()),
                        Reading::Unreadable | Reading::NoPdf => Declared::NoText,
                    };
                    keep.push((id, hash, reading));
                    declared
                }
            };
            answers.insert((*id).to_owned(), (declared, false));
        }
        library.keep_declarations_of(PARSER, &keep)?;
        for _ in batch {
            progress.advance();
        }
    }
    Ok(answers)
}

/// [`read_with_origin`] without saying which were kept before.
pub fn read_many(
    library: &Library,
    pdfs: &Pdfs,
    paper_ids: &[&str],
    progress: &Progress,
) -> Result<HashMap<String, Declared>, LibraryError> {
    let answers = read_with_origin(library, pdfs, paper_ids, progress)?;
    Ok(answers.into_iter().map(|(id, (declared, _))| (id, declared)).collect())
}

/// [`read_with_origin`], with what came of the papers counted.
pub fn read_and_count(
    library: &Library,
    pdfs: &Pdfs,
    paper_ids: &[&str],
    progress: &Progress,
) -> Result<Counts, LibraryError> {
    let mut counts = Counts::default();
    for (declared, before) in read_with_origin(library, pdfs, paper_ids, progress)?.values() {
        counts.add(declared, *before);
    }
    Ok(counts)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::library::papers::tests::paper;
    use crate::library::passages::fixtures::pdf_with;
    use crate::library::{Known, Paper};
    use crate::sources::Response;
    use crate::sources::fake::{Fake, status};

    /// A NeurIPS-style checklist long enough to be a paper's text, answering No to open code.
    fn checklist_lines() -> Vec<String> {
        let mut lines: Vec<String> = (0..40)
            .map(|line| format!("Filler line {line} so that the text is as long as a paper's text is."))
            .collect();
        for line in [
            "NeurIPS Paper Checklist",
            "1. Claims",
            "Question: Do the main claims reflect the paper's contributions?",
            "Answer: [Yes]",
            "Justification: The abstract says what the paper shows.",
            "5. Open access to data and code",
            "Question: Does the paper provide open access to the data and code?",
            "Answer: [No]",
            "Justification: The code is proprietary.",
        ] {
            lines.push(line.to_owned());
        }
        lines
    }

    fn checklist_pdf() -> Vec<u8> {
        let lines = checklist_lines();
        pdf_with(&lines.iter().map(String::as_str).collect::<Vec<_>>())
    }

    fn on_arxiv(id: &str) -> Paper {
        Paper { pdf_url: Some(format!("https://arxiv.org/pdf/{id}")), ..paper(id) }
    }

    fn library_with(papers: &[Paper]) -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        library.add_papers(papers, None, Known::Keep).unwrap();
        (folder, library)
    }

    fn pdf_answer(bytes: &[u8]) -> Response {
        Response {
            status: 200,
            content_type: Some("application/pdf".into()),
            location: None,
            retry_after: None,
            body: bytes.to_vec(),
        }
    }

    #[test]
    fn a_papers_declarations_are_read_from_its_fetched_pdf_once_and_kept_by_its_hash() {
        let (_folder, library) = library_with(&[on_arxiv("2601.00001")]);
        let fake = Arc::new(Fake::new().with("https://arxiv.org/pdf/2601.00001", pdf_answer(&checklist_pdf())));
        let pdfs = Pdfs::new(fake.clone());
        let Declared::Read(read) = declarations_of(&library, &pdfs, "2601.00001").unwrap() else { panic!() };
        assert_eq!(read.flags, ["closed_code"]);
        assert_eq!(read.checklist.len(), 2);
        let kept = library.kept_declarations(&["2601.00001"], PARSER).unwrap();
        assert!(kept["2601.00001"].pdf_hash.is_some());
        // asked again: nothing fetched, nothing read
        assert_eq!(declarations_of(&library, &pdfs, "2601.00001").unwrap(), Declared::Read(read.clone()));
        assert_eq!(fake.asked().len(), 1);
        // another PDF in its place is read again
        library.keep_pdf("2601.00001", &pdf_with(&["Too short to be a paper."])).unwrap();
        assert_eq!(declarations_of(&library, &pdfs, "2601.00001").unwrap(), Declared::NoText);
    }

    #[test]
    fn a_reading_moved_in_from_the_python_backend_is_used_without_its_pdf() {
        let (_folder, library) = library_with(&[paper("neurips-2024-7")]);
        let read = Declarations { checklist: vec![], statements: vec![], flags: vec!["llm_declared".into()] };
        library.keep_declarations(PARSER, &[("neurips-2024-7", Reading::Read(read.clone()))]).unwrap();
        let fake = Arc::new(Fake::new());
        let pdfs = Pdfs::new(fake.clone());
        assert_eq!(declarations_of(&library, &pdfs, "neurips-2024-7").unwrap(), Declared::Read(read));
        assert!(fake.asked().is_empty(), "no index, no PDF fetched");
    }

    #[test]
    fn a_run_is_read_paper_by_paper_counting_what_came_of_each_and_carries_on_after_a_cancel() {
        let papers: Vec<Paper> = (1..=5).map(|n| on_arxiv(&format!("2601.0000{n}"))).collect();
        let mut with_iclr = papers.clone();
        with_iclr.push(paper("iclr-2026-9"));
        let (_folder, library) = library_with(&with_iclr);
        for paper in &papers[..3] {
            library.keep_pdf(&paper.id, &checklist_pdf()).unwrap();
        }
        library.keep_pdf(&papers[3].id, b"%PDF-1.4 nothing a reader can use").unwrap();
        let fake = Arc::new(Fake::new().with("https://arxiv.org/pdf/2601.00005", status(404)));
        let pdfs = Pdfs::new(fake);
        let ids: Vec<&str> = with_iclr.iter().map(|paper| paper.id.as_str()).collect();

        let cancelled = Progress::new();
        cancelled.cancel();
        assert_eq!(read_and_count(&library, &pdfs, &ids, &cancelled).unwrap(), Counts::default());

        let progress = Progress::new();
        let counts = read_and_count(&library, &pdfs, &ids, &progress).unwrap();
        assert_eq!(counts, Counts { kept_before: 0, read: 3, no_text: 0, no_pdf: 2, unreadable: 1 });
        assert_eq!((progress.done(), progress.total()), (6, 6));
        let again = read_and_count(&library, &pdfs, &ids, &Progress::new()).unwrap();
        // the PDF that could not be read is not read again either: that is kept too
        assert_eq!(again, Counts { kept_before: 4, read: 0, no_text: 0, no_pdf: 2, unreadable: 0 });
        assert!(counts.summary().contains("3 read now"));
    }

    #[test]
    fn the_answers_are_the_same_on_one_thread_and_on_many() {
        let papers: Vec<Paper> = (1..=6).map(|n| on_arxiv(&format!("2601.0000{n}"))).collect();
        let ids: Vec<&str> = papers.iter().map(|paper| paper.id.as_str()).collect();
        let (one, many) = threads::on_one_and_many(|| {
            let (_folder, library) = library_with(&papers);
            for paper in &papers {
                library.keep_pdf(&paper.id, &checklist_pdf()).unwrap();
            }
            let mut read: Vec<(String, Declared)> =
                read_many(&library, &Pdfs::new(Arc::new(Fake::new())), &ids, &Progress::new())
                    .unwrap()
                    .into_iter()
                    .collect();
            read.sort_by(|a, b| a.0.cmp(&b.0));
            read
        });
        assert_eq!(one, many);
    }
}
