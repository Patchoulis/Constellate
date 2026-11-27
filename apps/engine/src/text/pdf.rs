//! The text of a paper's PDF, page by page, in reading order: what passages ([`super::passages`]), reference
//! lists ([`super::references`]) and citing sentences are read from.
//!
//! The text comes from `pdf-extract`, which is pure Rust: it builds on Windows and Linux with no system library,
//! which `pdfium` (a native library per platform) and `poppler` (a system package) do not. Its reading order is
//! the order the page draws its text in, which for the two-column LaTeX templates of the conferences read here
//! (ICML, NeurIPS, ICLR) is column by column, and it leaves a blank line between paragraphs, so reference entries
//! come out whole and apart. A page break still drops a page number and a running header into an entry; the
//! reference reader expects that.
//!
//! # A PDF is read in a process of its own
//!
//! A PDF is a file from somewhere else, and the PDF library trusts it more than it should: a stream that inflates
//! to gigabytes, objects that refer to themselves or nest thousands deep. A panic can be caught, but running out of
//! memory or overflowing the stack aborts the whole process, and that would take the engine down every time it met
//! that paper, in every run that includes it. So the engine reads each PDF in a short-lived child process of its
//! own program (the hidden argument [`CHILD_ARGUMENT`], started through [`crate::api::config::child_process`], so
//! it inherits none of the engine's secrets):
//!
//! ```text
//!     engine ──PDF bytes on stdin──▶ child: heap capped at CHILD_HEAP, reads the pages
//!     engine ◀──pages as JSON on stdout── child (at most LONGEST_ANSWER bytes)
//!     engine kills a child that is still reading after TIME_LIMIT
//! ```
//!
//! The child's heap is capped by its allocator (the program's `#[global_allocator]`, the `cap` crate), the same
//! on every system and without `unsafe` code here: an allocation past the cap aborts the child, which the engine
//! reads as [`Unreadable::TooMuchMemory`]. A stack overflow, any other crash, a hang and an answer too large are
//! each a reason of their own. They are lasting reasons ([`Unreadable::is_lasting`]): the library keeps them by the
//! PDF's hash, so a hostile PDF is tried once, not at every run. Only "the reader could not be started" is not
//! kept, since that says nothing about the PDF.
//!
//! The process costs a few milliseconds per PDF (measured in the tests' `--ignored` benchmark), next to the tens
//! to hundreds a paper's PDF takes to read. Several are read at once, one per computing thread that asks: the
//! library reads a batch of PDFs on the engine's pool ([`crate::threads`]). Since each child may take up to
//! [`CHILD_HEAP`], no more run at once than the memory free holds at that cap ([`readers_at_once`]), and never
//! more than the pool has threads: a thread past that waits for a place.
//!
//! What the cap does not cover: memory the PDF library maps outside the allocator (it maps none), and the
//! operating system's own accounting (the child's address space is not limited, only what it allocates).
//!
//! Until the program says where it is ([`read_in_children`]), a PDF is read in this process, as before: that is
//! for tests and examples, which have no such program. The engine's `main` always says it.

use std::io::{self, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, ExitStatus, Stdio};
use std::sync::{Condvar, LazyLock, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::threads::{self, Outcome, Progress};

/// A PDF larger than this is not one paper (a scanned book, or not a PDF at all).
pub const MAX_PDF_BYTES: usize = 80 * 1024 * 1024;
/// The argument that makes the engine's program read one PDF from standard input and say its pages as JSON on
/// standard output, then stop. Not meant for people, so not in `--help` (there is none).
pub const CHILD_ARGUMENT: &str = "--read-one-pdf";
/// The most a child may allocate: a PDF of a paper (at most [`MAX_PDF_BYTES`]) with its streams inflated fits in
/// it many times over, so a child that needs more is reading something that is not a paper. At most as many
/// children run as the free memory holds at this cap ([`readers_at_once`]).
pub const CHILD_HEAP: usize = 1024 * 1024 * 1024;
/// The exit code of a child whose own input, output or thread failed: a fault of this computer, not of the PDF.
const CHILD_TROUBLE: u8 = 3;
/// How long a child may take to read one PDF. Real papers take well under a second each.
pub const TIME_LIMIT: Duration = Duration::from_secs(60);
/// The most a child may answer: a paper's text as JSON is a few hundred kilobytes, a book's a few megabytes.
pub const LONGEST_ANSWER: usize = 64 * 1024 * 1024;
/// What is kept of a child's error output, to tell why it stopped.
const LONGEST_ERROR: usize = 16 * 1024;
/// The stack the child reads on: deep enough for any real PDF; a deeper one overflows it, which ends the child.
const CHILD_STACK: usize = 16 * 1024 * 1024;

/// Why a PDF gave no text, in words for the person who asked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum Unreadable {
    #[error("The PDF is larger than any paper ({0} bytes)")]
    TooLarge(usize),
    #[error("The PDF could not be read: {0}")]
    Damaged(String),
    #[error("The PDF could not be read: its layout is one the PDF reader does not handle")]
    Unhandled,
    #[error("The PDF has no text in it (it may be scanned images of pages)")]
    NoText,
    #[error("The PDF could not be read: reading it took longer than {0} seconds")]
    TooSlow(u64),
    #[error("The PDF could not be read: it needed more than {0} MB of memory")]
    TooMuchMemory(usize),
    #[error("The PDF could not be read: its text is larger than any paper's")]
    TooMuchText,
    #[error("The PDF could not be read: the PDF reader stopped ({0})")]
    Crashed(String),
    /// The reader's process could not be started: a fault of this computer, not of the PDF.
    #[error("The PDF reader could not be started: {0}")]
    ReaderUnavailable(String),
}

impl Unreadable {
    /// Does this say something about the PDF itself, so that reading it again would come to the same? Only those
    /// are kept: a reader that could not be started is tried again next time.
    pub fn is_lasting(&self) -> bool {
        !matches!(self, Self::ReaderUnavailable(_))
    }
}

/// The engine's own program, once `main` says it: from then on every PDF is read in a child of it.
static PROGRAM: OnceLock<PathBuf> = OnceLock::new();

/// Read every PDF from now on in a child process of `program` (the engine's own, started with
/// [`CHILD_ARGUMENT`]). Said once, at start; false when it was already said.
pub fn read_in_children(program: PathBuf) -> bool {
    PROGRAM.set(program).is_ok()
}

/// The text of the PDF in `bytes`, its pages one after the other.
pub fn pdf_text(bytes: &[u8]) -> Result<String, Unreadable> {
    pdf_pages(bytes).map(|pages| pages.join("\n"))
}

/// The text of many PDFs on every core, one answer per PDF in their order, and `progress` counted as they are
/// read ([`crate::threads::each`]). Each PDF is read from its own bytes alone, so the answers are the same on one
/// thread or many, and a PDF that cannot be read is that PDF's answer: the others are read as usual.
///
/// Every PDF of the batch is held in memory at once: a caller reading a conference hands them over a few dozen
/// at a time, and keeps what was read from each batch (in one write) before the next.
pub fn pdf_texts<B: AsRef<[u8]> + Sync>(pdfs: &[B], progress: &Progress) -> Vec<Outcome<Result<String, Unreadable>>> {
    threads::each(pdfs, progress, |bytes| pdf_text(bytes.as_ref()))
}

/// The text of the PDF in `bytes`, page by page: `pages[0]` is page 1, as the PDF numbers its pages (not as the
/// paper prints its page numbers). A page with no text (a full-page figure) is an empty string, so that the
/// position of every later page stays its page number: a link back to a place in the paper needs the page.
///
/// Read in a child process once the engine's program is known ([`read_in_children`]), else here.
pub fn pdf_pages(bytes: &[u8]) -> Result<Vec<String>, Unreadable> {
    if bytes.len() > MAX_PDF_BYTES {
        return Err(Unreadable::TooLarge(bytes.len()));
    }
    match PROGRAM.get() {
        Some(program) => read_in_child(program, bytes, TIME_LIMIT),
        None => read_in_this_process(bytes),
    }
}

/// The reading itself, in whatever process runs it. A panic of the PDF library is caught and told as
/// [`Unreadable::Unhandled`]; an abort (memory, stack) is not catchable, which is why the engine runs this in a
/// child. Public for the child and for measuring what the process costs; everything else calls [`pdf_pages`].
pub fn read_in_this_process(bytes: &[u8]) -> Result<Vec<String>, Unreadable> {
    if bytes.len() > MAX_PDF_BYTES {
        return Err(Unreadable::TooLarge(bytes.len()));
    }
    let pages = catch_unwind(AssertUnwindSafe(|| pdf_extract::extract_text_from_mem_by_pages(bytes)))
        .map_err(|_| Unreadable::Unhandled)?
        .map_err(|error| Unreadable::Damaged(error.to_string()))?;
    if pages.iter().all(|page| page.trim().is_empty()) {
        return Err(Unreadable::NoText);
    }
    Ok(pages)
}

/// What a child says on its standard output.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ChildAnswer {
    Pages(Vec<String>),
    Unreadable(Unreadable),
}

/// The child's side: read one PDF from standard input, say its pages on standard output, stop. `main` calls this
/// when started with [`CHILD_ARGUMENT`], after capping the heap at [`CHILD_HEAP`].
pub fn serve_one_reading() -> ExitCode {
    let mut bytes = Vec::new();
    // one byte more than allowed, to tell "too large" from "exactly the largest"
    let limit = u64::try_from(MAX_PDF_BYTES).unwrap_or(u64::MAX).saturating_add(1);
    if io::stdin().lock().take(limit).read_to_end(&mut bytes).is_err() {
        return ExitCode::from(CHILD_TROUBLE);
    }
    // on a thread of its own, for a stack of a known size whatever the system gives the main thread
    let reading = std::thread::Builder::new().stack_size(CHILD_STACK).spawn(move || read_in_this_process(&bytes));
    let answer = match reading.map(std::thread::JoinHandle::join) {
        Ok(Ok(Ok(pages))) => ChildAnswer::Pages(pages),
        Ok(Ok(Err(why))) => ChildAnswer::Unreadable(why),
        Ok(Err(_)) => ChildAnswer::Unreadable(Unreadable::Unhandled),
        Err(_) => return ExitCode::from(CHILD_TROUBLE),
    };
    let mut out = io::stdout().lock();
    match serde_json::to_writer(&mut out, &answer).map_err(io::Error::from).and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(CHILD_TROUBLE),
    }
}

/// A child that is killed and waited for whatever happens to its reader, so none is ever left behind.
struct Reaped(std::process::Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

/// At most `most` bytes of `from`, and whether there was more (which is read and dropped, so that the writer is
/// never left blocked on a full pipe).
fn read_at_most(from: Option<impl Read>, most: usize) -> (Vec<u8>, bool) {
    let Some(mut from) = from else { return (Vec::new(), false) };
    let mut kept = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut over = false;
    loop {
        match from.read(&mut chunk) {
            Ok(0) | Err(_) => return (kept, over),
            Ok(read) if !over && kept.len() + read <= most => kept.extend_from_slice(&chunk[..read]),
            Ok(_) => over = true,
        }
    }
}

/// The child's exit, or `None` when it was still reading at `deadline` (and has been killed).
fn wait_until(child: &mut Reaped, deadline: Instant) -> Option<ExitStatus> {
    let mut pause = Duration::from_millis(1);
    loop {
        match child.0.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.0.kill();
                let _ = child.0.wait();
                return None;
            }
            Ok(None) => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(Duration::from_millis(20));
            }
            Err(_) => return child.0.wait().ok(),
        }
    }
}

/// How many PDF readers may run at once: one per computing thread (`cores`), and no more than `available` bytes
/// of free memory hold at `cap` bytes each (the most one child may allocate); at least one, so that a PDF is
/// always read, one at a time on a computer short of memory.
pub fn readers_at_once(cores: usize, available: u64, cap: u64) -> usize {
    let by_memory = usize::try_from(available / cap.max(1)).unwrap_or(usize::MAX);
    cores.min(by_memory).max(1)
}

/// The memory this computer has free now, in bytes; None when the system does not say.
fn available_memory() -> Option<u64> {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    Some(system.available_memory()).filter(|bytes| *bytes > 0)
}

/// Places for children: at most [`readers_at_once`] of them read at once, whoever asks.
struct Places {
    most: usize,
    taken: Mutex<usize>,
    freed: Condvar,
}

/// A child's place, given back when dropped (on a panic too).
struct Place<'a>(&'a Places);

impl Places {
    fn take(&self) -> Place<'_> {
        let mut taken = self.taken.lock().unwrap_or_else(PoisonError::into_inner);
        while *taken >= self.most {
            taken = self.freed.wait(taken).unwrap_or_else(PoisonError::into_inner);
        }
        *taken += 1;
        Place(self)
    }
}

impl Drop for Place<'_> {
    fn drop(&mut self) {
        let mut taken = self.0.taken.lock().unwrap_or_else(PoisonError::into_inner);
        *taken = taken.saturating_sub(1);
        drop(taken);
        self.0.freed.notify_one();
    }
}

/// The engine's places, counted once, when the first PDF is read in a child: by the computing threads and the
/// memory free then. A system that does not say its free memory is taken to have one cap's worth per thread.
static PLACES: LazyLock<Places> = LazyLock::new(|| {
    let cores = threads::computing();
    let cap = u64::try_from(CHILD_HEAP).unwrap_or(u64::MAX);
    let available = available_memory().unwrap_or_else(|| cap.saturating_mul(cores as u64));
    Places { most: readers_at_once(cores, available, cap), taken: Mutex::new(0), freed: Condvar::new() }
});

/// Read `bytes` in a child process of `program` within `time`: the pages, or why not. Waits first for a place
/// among the children ([`readers_at_once`]); `time` runs from when the child starts.
pub fn read_in_child(program: &Path, bytes: &[u8], time: Duration) -> Result<Vec<String>, Unreadable> {
    let _place = PLACES.take();
    let mut command = crate::api::config::child_process(program);
    command.arg(CHILD_ARGUMENT).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // a group of its own: Ctrl-C in the engine's terminal stops the engine, which stops its children, rather than
    // reaching them first and being taken for something the PDF did
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000; // no console window flashing up for each PDF
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let deadline = Instant::now() + time;
    let mut child = Reaped(command.spawn().map_err(|error| Unreadable::ReaderUnavailable(error.to_string()))?);
    let (input, output, errors) = (child.0.stdin.take(), child.0.stdout.take(), child.0.stderr.take());
    let (status, (answer, too_long), (said, _)) = std::thread::scope(|scope| {
        scope.spawn(move || {
            // a child that stopped early closes its end: the rest is not needed
            if let Some(mut input) = input {
                let _ = input.write_all(bytes);
            }
        });
        let answer = scope.spawn(move || read_at_most(output, LONGEST_ANSWER));
        let said = scope.spawn(move || read_at_most(errors, LONGEST_ERROR));
        let status = wait_until(&mut child, deadline);
        (status, answer.join().unwrap_or_default(), said.join().unwrap_or_default())
    });
    let Some(status) = status else { return Err(Unreadable::TooSlow(time.as_secs())) };
    if too_long {
        return Err(Unreadable::TooMuchText);
    }
    if !status.success() {
        return Err(why_it_stopped(status, &String::from_utf8_lossy(&said)));
    }
    match serde_json::from_slice(&answer) {
        Ok(ChildAnswer::Pages(pages)) => Ok(pages),
        Ok(ChildAnswer::Unreadable(why)) => Err(why),
        Err(_) => Err(Unreadable::Crashed("its answer could not be read".to_owned())),
    }
}

/// What a child that did not finish died of, from what it said last. Rust's runtime writes these words itself.
fn why_it_stopped(status: ExitStatus, said: &str) -> Unreadable {
    if said.contains("memory allocation of") {
        Unreadable::TooMuchMemory(CHILD_HEAP / (1024 * 1024))
    } else if said.contains("overflowed its stack") {
        Unreadable::Crashed("objects nested too deeply".to_owned())
    } else {
        match status.code() {
            Some(code) if code == i32::from(CHILD_TROUBLE) => {
                Unreadable::ReaderUnavailable("the reader's own input or output failed".to_owned())
            }
            Some(code) => Unreadable::Crashed(format!("exit code {code}")),
            // stopped from outside (a signal the engine did not send: its own kill is the time limit, told
            // apart before this): nothing is known of the PDF
            None => Unreadable::ReaderUnavailable("the reader was stopped from outside".to_owned()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A one-page PDF that writes `lines` in Helvetica, one below the other, made here so the test needs no file.
    fn pdf_with(lines: &[&str]) -> Vec<u8> {
        crate::library::papers::passages::fixtures::pdf_with(lines)
    }

    #[test]
    fn no_more_readers_run_at_once_than_the_free_memory_holds_at_their_cap_nor_than_the_threads() {
        const GIB: u64 = 1 << 30;
        assert_eq!(readers_at_once(16, 64 * GIB, GIB), 16, "plenty of memory: one per thread");
        assert_eq!(readers_at_once(16, 6 * GIB + GIB / 2, GIB), 6, "six caps' worth free: six");
        assert_eq!(readers_at_once(4, GIB / 2, GIB), 1, "less than one cap free: still one, one at a time");
        assert_eq!(readers_at_once(0, 64 * GIB, GIB), 1);
        assert_eq!(readers_at_once(8, u64::MAX, 0), 8, "a cap of nothing bounds nothing");
        assert!(available_memory().is_some_and(|bytes| bytes > 0), "this system says what it has free");
    }

    #[test]
    fn a_place_is_given_back_when_its_reading_ends_and_whoever_waits_takes_it() {
        let places = Places { most: 2, taken: Mutex::new(0), freed: Condvar::new() };
        let most = Mutex::new(0usize);
        let now = Mutex::new(0usize);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    let _place = places.take();
                    let at_once = {
                        let mut now = now.lock().unwrap();
                        *now += 1;
                        *now
                    };
                    let mut seen = most.lock().unwrap();
                    *seen = (*seen).max(at_once);
                    drop(seen);
                    std::thread::sleep(Duration::from_millis(10));
                    *now.lock().unwrap() -= 1;
                });
            }
        });
        assert_eq!(*most.lock().unwrap(), 2);
        assert_eq!(*places.taken.lock().unwrap(), 0, "every place given back");
    }

    #[test]
    fn the_text_of_a_pdf_comes_out_line_by_line_in_order() {
        let text =
            pdf_text(&pdf_with(&["References", "Vaswani, A. Attention is all you need. In NeurIPS, 2017."])).unwrap();
        let lines: Vec<&str> = text.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, ["References", "Vaswani, A. Attention is all you need. In NeurIPS, 2017."]);
    }

    #[test]
    fn a_pdf_is_read_page_by_page_with_the_same_text() {
        let bytes = pdf_with(&["Introduction", "We study passages."]);
        let pages = pdf_pages(&bytes).unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(
            pages[0].split_whitespace().collect::<Vec<_>>(),
            pdf_text(&bytes).unwrap().split_whitespace().collect::<Vec<_>>()
        );
        assert!(matches!(pdf_pages(b"not a pdf"), Err(Unreadable::Damaged(_) | Unreadable::Unhandled)));
        assert_eq!(pdf_pages(&pdf_with(&[])), Err(Unreadable::NoText));
    }

    #[test]
    fn many_pdfs_are_read_side_by_side_each_to_its_own_answer_the_same_on_one_thread_and_on_many() {
        let pdfs: Vec<Vec<u8>> = (0..40)
            .map(|paper| match paper % 5 {
                0 => b"not a pdf at all".to_vec(),
                1 => pdf_with(&[]),
                _ => pdf_with(&["References", &format!("Author, A. Paper number {paper}. In ICML, 2025.")]),
            })
            .collect();
        let progress = Progress::new();
        let (one, many) = threads::on_one_and_many(|| pdf_texts(&pdfs, &progress));
        assert_eq!(one, many);
        assert_eq!(progress.done(), 80); // both batches counted
        for (paper, outcome) in many.iter().enumerate() {
            match (paper % 5, outcome) {
                (0, Outcome::Done(Err(Unreadable::Damaged(_) | Unreadable::Unhandled))) => {}
                (1, Outcome::Done(Err(Unreadable::NoText))) => {}
                (_, Outcome::Done(Ok(text))) => assert!(text.contains(&format!("Paper number {paper}."))),
                (_, other) => panic!("paper {paper}: {other:?}"),
            }
        }
    }

    #[test]
    fn what_is_not_a_readable_pdf_is_refused_with_a_reason() {
        assert!(matches!(pdf_text(b"<html>not a paper</html>"), Err(Unreadable::Damaged(_) | Unreadable::Unhandled)));
        assert_eq!(pdf_text(&pdf_with(&[])), Err(Unreadable::NoText));
        let huge = vec![b' '; MAX_PDF_BYTES + 1];
        assert_eq!(pdf_pages(&huge), Err(Unreadable::TooLarge(MAX_PDF_BYTES + 1)));
    }

    #[test]
    fn only_a_reader_that_could_not_be_started_is_not_a_lasting_reason() {
        for lasting in [
            Unreadable::TooSlow(60),
            Unreadable::TooMuchMemory(1024),
            Unreadable::Crashed("exit code 3".into()),
            Unreadable::TooMuchText,
            Unreadable::NoText,
        ] {
            assert!(lasting.is_lasting(), "{lasting:?}");
        }
        assert!(!Unreadable::ReaderUnavailable("no such file".into()).is_lasting());
    }

    #[test]
    fn a_program_that_cannot_be_started_is_told_as_such() {
        let missing = Path::new("this program does not exist anywhere");
        let read = read_in_child(missing, b"%PDF-1.4", Duration::from_secs(5));
        assert!(matches!(read, Err(Unreadable::ReaderUnavailable(_))), "{read:?}");
    }

    #[test]
    fn a_childs_death_is_told_by_what_it_said_last() {
        let status = crate::api::config::child_process(std::env::current_exe().unwrap())
            .arg("--this-flag-is-not-one-the-test-harness-takes")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success());
        assert_eq!(
            why_it_stopped(status, "memory allocation of 1073741824 bytes failed"),
            Unreadable::TooMuchMemory(1024)
        );
        assert_eq!(
            why_it_stopped(status, "thread 'x' has overflowed its stack\nfatal runtime error: stack overflow"),
            Unreadable::Crashed("objects nested too deeply".into())
        );
        assert!(matches!(why_it_stopped(status, ""), Unreadable::Crashed(_)));
    }

    #[test]
    fn the_reasons_survive_the_trip_from_the_child_as_json() {
        for answer in [
            ChildAnswer::Pages(vec!["one".into(), String::new()]),
            ChildAnswer::Unreadable(Unreadable::Damaged("bad xref".into())),
        ] {
            let told = serde_json::to_string(&answer).unwrap();
            let back: ChildAnswer = serde_json::from_str(&told).unwrap();
            assert_eq!(format!("{back:?}"), format!("{answer:?}"));
        }
    }

    #[test]
    fn what_a_child_says_is_kept_up_to_a_limit_and_the_rest_is_drained() {
        let (kept, over) = read_at_most(Some(&b"0123456789"[..]), 4);
        assert_eq!((kept.as_slice(), over), (&b""[..], true), "the first chunk alone is past the limit");
        let (kept, over) = read_at_most(Some(&b"0123"[..]), 4);
        assert_eq!((kept.as_slice(), over), (&b"0123"[..], false));
        assert_eq!(read_at_most(None::<&[u8]>, 4), (Vec::new(), false));
    }
}
