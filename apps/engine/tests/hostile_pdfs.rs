//! PDFs made to hurt the reader, read the way the engine reads every PDF: in a child process of the engine's own
//! program (`src/text/pdf/mod.rs`). Each must end only its child, with a reason, and leave this process (standing in
//! for the engine) reading the next PDF as usual.
//!
//! `cargo test --release --test hostile_pdfs -- --ignored --nocapture` also measures what the process costs, on
//! made-up PDFs and, when `CONSTELLATE_PDF_SAMPLE` names a folder of real PDFs, on 20 of them (read only).

#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::time::{Duration, Instant};

use constellate::text::pdf::{TIME_LIMIT, Unreadable, read_in_child, read_in_this_process};

const ENGINE: &str = env!("CARGO_BIN_EXE_constellate");

/// A PDF of these objects (numbered from 1, the first the catalogue), with a cross-reference table that is right.
fn pdf_of(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (number, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n", number + 1).bytes());
        pdf.extend(object);
        pdf.extend(b"\nendobj\n");
    }
    let table = pdf.len();
    pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for offset in offsets {
        pdf.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    pdf.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n", objects.len() + 1).bytes());
    pdf
}

fn text(object: &str) -> Vec<u8> {
    object.as_bytes().to_vec()
}

/// A one-page PDF whose page draws `lines`.
fn paper(lines: &[&str]) -> Vec<u8> {
    let mut content = String::from("BT /F1 10 Tf 72 720 Td 14 TL\n");
    for line in lines {
        content.push_str(&format!("({line}) Tj T*\n"));
    }
    content.push_str("ET");
    pdf_of(&[
        text("<< /Type /Catalog /Pages 2 0 R >>"),
        text("<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        text(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        ),
        text(&format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len())),
        text("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"),
    ])
}

/// Bits written the way deflate packs them: from the lowest bit of each byte up.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    filled: u32,
}

impl Bits {
    fn put(&mut self, value: u32, count: u32) {
        for bit in 0..count {
            if self.filled % 8 == 0 {
                self.bytes.push(0);
            }
            let last = self.bytes.len() - 1;
            self.bytes[last] |= (((value >> bit) & 1) as u8) << (self.filled % 8);
            self.filled += 1;
        }
    }

    /// A Huffman code, which deflate writes from its highest bit.
    fn code(&mut self, code: u32, length: u32) {
        let reversed = (0..length).fold(0, |reversed, bit| (reversed << 1) | ((code >> bit) & 1));
        self.put(reversed, length);
    }
}

/// A zlib stream, a few megabytes long, that inflates to `zeros` zero bytes: one literal, then copies of 258
/// bytes from one byte back, in deflate's fixed codes (13 bits each).
fn bomb(zeros: u64) -> Vec<u8> {
    let mut bits = Bits::default();
    bits.put(1, 1); // the last block
    bits.put(1, 2); // of fixed codes
    bits.code(0x30, 8); // the literal 0
    for _ in 0..zeros / 258 {
        bits.code(0xC5, 8); // length 258
        bits.code(0, 5); // distance 1
    }
    bits.code(0, 7); // end of the block
    let mut stream = vec![0x78, 0x01];
    stream.extend(bits.bytes);
    stream.extend([0, 0, 0, 1]); // a checksum never reached
    stream
}

/// A page whose content stream inflates to more than the child may hold.
fn inflates_hugely() -> Vec<u8> {
    let stream = bomb(3 << 29); // 1.5 GiB
    let mut content = format!("<< /Length {} /Filter /FlateDecode >>\nstream\n", stream.len()).into_bytes();
    content.extend(stream);
    content.extend(b"\nendstream");
    pdf_of(&[
        text("<< /Type /Catalog /Pages 2 0 R >>"),
        text("<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        text("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << >> >>"),
        content,
    ])
}

/// A page tree whose node lists itself among its kids, and a page that is its own parent and contents.
fn refers_to_itself() -> Vec<u8> {
    pdf_of(&[
        text("<< /Type /Catalog /Pages 2 0 R >>"),
        text("<< /Type /Pages /Kids [2 0 R 3 0 R] /Count 2 >>"),
        text("<< /Type /Page /Parent 3 0 R /MediaBox [0 0 612 792] /Contents 3 0 R /Resources 3 0 R >>"),
    ])
}

/// An object nested a hundred thousand arrays deep.
fn nested_deeply() -> Vec<u8> {
    let depth = 100_000;
    let deep = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    pdf_of(&[
        text("<< /Type /Catalog /Pages 2 0 R >>"),
        text("<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        text(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Deep {deep} >>")),
    ])
}

/// Bytes that start like a PDF and are then noise.
fn garbage() -> Vec<u8> {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut bytes = b"%PDF-1.7\n".to_vec();
    bytes.extend((0..200_000).map(|_| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 24) as u8
    }));
    bytes
}

fn read(bytes: &[u8]) -> Result<Vec<String>, Unreadable> {
    read_in_child(Path::new(ENGINE), bytes, TIME_LIMIT).map(|reading| reading.pages)
}

fn still_reads_a_paper() {
    let pages = read(&paper(&["Introduction", "The engine is still here and reads the next paper."])).unwrap();
    assert!(pages[0].contains("still here"), "{pages:?}");
}

#[test]
fn a_paper_is_read_in_a_child_as_it_is_read_here() {
    let bytes = paper(&["Introduction", "We study what a child process reads from a paper."]);
    assert_eq!(read_in_child(Path::new(ENGINE), &bytes, TIME_LIMIT).unwrap(), read_in_this_process(&bytes).unwrap());
    assert_eq!(read(&paper(&[])), Err(Unreadable::NoText));
    assert!(matches!(read(b"<html>not a paper</html>"), Err(Unreadable::Damaged(_) | Unreadable::Unhandled)));
}

#[test]
fn a_small_file_that_inflates_to_gigabytes_ends_its_child_for_memory_and_nothing_else() {
    let hostile = inflates_hugely();
    assert!(hostile.len() < 20 << 20, "the file itself is small: {} bytes", hostile.len());
    let read = read(&hostile);
    match &read {
        Err(why @ (Unreadable::TooMuchMemory(_) | Unreadable::TooSlow(_))) => assert!(why.is_lasting()),
        // a reader that stops inflating on its own is fine too, as long as it says why
        Err(why) => assert!(why.is_lasting(), "{why:?}"),
        Ok(pages) => panic!("read {} pages from a bomb", pages.len()),
    }
    eprintln!("inflating bomb: {read:?}");
    still_reads_a_paper();
}

#[test]
fn objects_that_refer_to_themselves_or_nest_too_deeply_end_only_their_child() {
    for (what, hostile) in [("self-referring", refers_to_itself()), ("nested", nested_deeply()), ("garbage", garbage())]
    {
        let read = read(&hostile);
        eprintln!("{what}: {read:?}");
        match read {
            Err(why) => assert!(why.is_lasting(), "{what}: {why:?}"),
            Ok(pages) => assert!(pages.iter().all(|page| page.trim().is_empty()), "{what}: {pages:?}"),
        }
        still_reads_a_paper();
    }
}

#[test]
fn a_child_that_takes_too_long_is_stopped_and_said_to_be_too_slow() {
    // the bomb again, with no time to inflate it
    let started = Instant::now();
    let read = read_in_child(Path::new(ENGINE), &inflates_hugely(), Duration::from_millis(200));
    assert!(matches!(read, Err(Unreadable::TooSlow(_)) | Err(Unreadable::TooMuchMemory(_))), "{read:?}");
    assert!(started.elapsed() < Duration::from_secs(20), "stopped in time: {:?}", started.elapsed());
    still_reads_a_paper();
}

#[test]
fn several_children_read_side_by_side_each_its_own_paper() {
    let papers: Vec<Vec<u8>> =
        (0..8).map(|number| paper(&["Introduction", &format!("This is paper number {number}.")])).collect();
    std::thread::scope(|scope| {
        let readers: Vec<_> = papers.iter().map(|bytes| scope.spawn(move || read(bytes))).collect();
        for (number, reader) in readers.into_iter().enumerate() {
            let pages = reader.join().unwrap().unwrap();
            assert!(pages[0].contains(&format!("paper number {number}.")), "{pages:?}");
        }
    });
}

/// What the process boundary costs: the same PDFs read here and in children.
#[test]
#[ignore = "a measurement: run with --release -- --ignored --nocapture"]
fn what_reading_in_a_child_costs() {
    fn time(label: &str, pdfs: &[Vec<u8>]) {
        let started = Instant::now();
        let here: Vec<_> = pdfs.iter().map(|bytes| read_in_this_process(bytes).map(|reading| reading.pages)).collect();
        let in_process = started.elapsed();
        let started = Instant::now();
        let children: Vec<_> = pdfs.iter().map(|bytes| read(bytes)).collect();
        let in_children = started.elapsed();
        assert_eq!(here, children, "the same text either way");
        let started = Instant::now();
        std::thread::scope(|scope| {
            let threads: Vec<_> = pdfs.iter().map(|bytes| scope.spawn(move || read(bytes))).collect();
            threads.into_iter().for_each(|thread| drop(thread.join().unwrap()));
        });
        let side_by_side = started.elapsed();
        let count = pdfs.len() as u32;
        eprintln!(
            "{label}: {count} PDFs, {} MB; here {in_process:?} ({:?} each); in children one at a time \
             {in_children:?} ({:?} each, {:?} more per PDF); in {count} children at once {side_by_side:?}",
            pdfs.iter().map(Vec::len).sum::<usize>() / 1_000_000,
            in_process / count,
            in_children / count,
            in_children.saturating_sub(in_process) / count,
        );
    }
    let made: Vec<Vec<u8>> = (0..20).map(|number| paper(&["Introduction", &format!("Paper {number}.")])).collect();
    time("one-page made-up PDFs", &made);
    if let Ok(folder) = std::env::var("CONSTELLATE_PDF_SAMPLE") {
        let mut files: Vec<_> = std::fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "pdf"))
            .collect();
        files.sort();
        // 20 reads: the folder's PDFs again from the first when it has fewer
        let real: Vec<Vec<u8>> = files.iter().cycle().take(20).map(|path| std::fs::read(path).unwrap()).collect();
        eprintln!("{} different PDFs in the sample", files.len().min(20));
        time("real papers", &real);
    }
}
