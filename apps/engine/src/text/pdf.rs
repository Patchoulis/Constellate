//! The text of a paper's PDF, in reading order, for its reference list to be read from ([`super::references`]).
//!
//! The text comes from `pdf-extract`, which is pure Rust: it builds on Windows and Linux with no system library,
//! which `pdfium` (a native library per platform) and `poppler` (a system package) do not. Its reading order is
//! the order the page draws its text in, which for the two-column LaTeX templates of the conferences read here
//! (ICML, NeurIPS, ICLR) is column by column, and it leaves a blank line between paragraphs, so reference entries
//! come out whole and apart. A page break still drops a page number and a running header into an entry; the
//! reference reader expects that.
//!
//! On the papers it was tried on (five ICML 2026 and arXiv papers, 30 to 60 pages each) a PDF takes well under a
//! second. A damaged or unusual PDF can make the library panic rather than fail; that is caught here and reported
//! as an unreadable PDF, so one paper cannot stop a job that reads many.

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::threads::{self, Outcome, Progress};

/// A PDF larger than this is not one paper (a scanned book, or not a PDF at all).
pub const MAX_PDF_BYTES: usize = 80 * 1024 * 1024;

/// Why a PDF gave no text, in words for the person who asked.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unreadable {
    #[error("The PDF is larger than any paper ({0} bytes)")]
    TooLarge(usize),
    #[error("The PDF could not be read: {0}")]
    Damaged(String),
    #[error("The PDF could not be read: its layout is one the PDF reader does not handle")]
    Unhandled,
    #[error("The PDF has no text in it (it may be scanned images of pages)")]
    NoText,
}

/// The text of the PDF in `bytes`.
pub fn pdf_text(bytes: &[u8]) -> Result<String, Unreadable> {
    if bytes.len() > MAX_PDF_BYTES {
        return Err(Unreadable::TooLarge(bytes.len()));
    }
    let text = catch_unwind(AssertUnwindSafe(|| pdf_extract::extract_text_from_mem(bytes)))
        .map_err(|_| Unreadable::Unhandled)?
        .map_err(|error| Unreadable::Damaged(error.to_string()))?;
    if text.trim().is_empty() {
        return Err(Unreadable::NoText);
    }
    Ok(text)
}

/// The text of many PDFs on every core, one answer per PDF in their order, and `progress` counted as they are
/// read ([`crate::threads::each`]). Each PDF is read from its own bytes alone, so the answers are the same on one
/// thread or many, and a PDF that cannot be read (or that makes the reader panic) is that PDF's answer: the
/// others are read as usual.
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
/// It is the same reader as [`pdf_text`], run page by page, so each page's text is in the same reading order.
pub fn pdf_pages(bytes: &[u8]) -> Result<Vec<String>, Unreadable> {
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A one-page PDF that writes `lines` in Helvetica, one below the other, made here so the test needs no file.
    fn pdf_with(lines: &[&str]) -> Vec<u8> {
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
    }
}
