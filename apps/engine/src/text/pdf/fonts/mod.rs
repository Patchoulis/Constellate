//! What each character code of a PDF's fonts stands for, worked out before the text is read, so that `pdf-extract`
//! reads the paper's characters and not whatever a code happens to be in some other encoding.
//!
//! # Why
//!
//! A font of a PDF draws glyphs by one-byte codes, and only the font says which character each code is. `pdf-extract`
//! 0.12 asks the font's ToUnicode map first, and else decodes the code through a text encoding: the `/Encoding`'s
//! named base and `/Differences`, or `PDFDocEncoding` when there is no named base. That goes wrong for the
//! mathematics of many papers:
//!
//! - A font with `/Differences` but no named base is read on `PDFDocEncoding` for every code the differences leave
//!   out, where the PDF specification says the font program's own (built-in) encoding applies; and a font whose
//!   `/Encoding` is a dictionary never has its embedded Type 1 program's encoding read at all.
//! - A glyph name the Adobe list does not have ("summationdisplay", "uni2212") is dropped, and the code read as a
//!   letter of `PDFDocEncoding` (cmex's display sum, code 0x58, came out as "X").
//! - Worst, and what the papers that showed this have: TeX's mathabx fonts (`TeX-matha10`, `TeX-mathx10`, ...)
//!   name every glyph after the Cork text encoding's letter at the same slot, and pdfTeX writes their ToUnicode
//!   maps from those names. The map is there, `pdf-extract` trusts it, and "(k, u) ∈ [K] × [H − 1]" comes out as
//!   "pk, uq P rKs ˆ rH ´ 1s" ([`tex`] has the details).
//!
//! # What is done
//!
//! The PDF is loaded with lopdf (the same `Document` `pdf-extract` reads), and each simple Type 1 font (Type 1C and
//! MMType1 included) is given a ToUnicode map made here when it has none it can be trusted with. Each code is read,
//! in order, from:
//!
//! 1. the font's own ToUnicode map, when it has one that is sound: kept as it is, and the font left alone;
//! 2. its `/Differences` glyph names, through the Adobe Glyph List ([`glyphs`]), and the names of the base encoding
//!    `/Encoding` names (WinAnsi, MacRoman, Standard);
//! 3. the embedded font program's own encoding and glyph names (a Type 1 program's `/Encoding` array, a CFF
//!    program's encoding and charset), unless `/Encoding` names a base encoding;
//! 4. for a font whose name says it is one of TeX's math fonts, and that is not re-encoded to a named encoding,
//!    the table of its family ([`tex`]): used where the names say nothing, and in place of a ToUnicode map or names
//!    that disagree with the table for most codes;
//! 5. Adobe's StandardEncoding, for a font that says it is not symbolic.
//!
//! A code none of these reads is [`UNREADABLE`] in the map, never a guess: the text shows that a formula could not
//! be read rather than wrong characters (`super::unreadable_marked`). A font that says nothing at all (no ToUnicode,
//! no names, not a known family) is left to `pdf-extract` as before, and so are TrueType, Type 3 and composite
//! fonts.
//!
//! Only the font dictionaries of the document in memory change, in the child process that reads the PDF; the file
//! is never written. Each font is worked out under `catch_unwind`, so a font program that makes a parser panic
//! leaves that one font as `pdf-extract` would have read it.

mod glyphs;
mod tex;

use std::panic::{AssertUnwindSafe, catch_unwind};

use pdf_extract::{Dictionary, Document, Object, ObjectId, Stream};
use unicode_normalization::UnicodeNormalization;

/// A code whose character no part of the font says: shown as a formula that could not be read. A Unicode
/// noncharacter, which no font maps a code to, so that it can only come from here.
pub const UNREADABLE: char = '\u{FDD0}';
/// A code that draws a piece of another glyph (the middle of a tall brace): left out of the text.
pub const DROPPED: char = '\u{FDD1}';

/// The font program's flag for a symbolic font (PDF 1.7, 9.8.2): its codes are not a standard encoding's.
const NONSYMBOLIC: i64 = 1 << 5;

/// What one code of a font is read as.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Read {
    /// Left to `pdf-extract`: a code the font says nothing of, where pdf-extract has an encoding of its own to
    /// fall back on (a base encoding not known here, MacExpert).
    Left,
    Text(String),
    Unreadable,
}

/// Give each simple Type 1 font of `document` the ToUnicode map its codes are read by; how many fonts were given
/// one.
pub fn mend(document: &mut Document) -> usize {
    let fonts: Vec<ObjectId> = document
        .objects
        .iter()
        .filter(|(_, object)| object.as_dict().is_ok_and(is_simple_type1))
        .map(|(id, _)| *id)
        .collect();
    let mut mended = 0;
    for id in fonts {
        let reading = catch_unwind(AssertUnwindSafe(|| {
            document.get_dictionary(id).ok().and_then(|font| codes_of(document, font))
        }));
        let Ok(Some(codes)) = reading else { continue };
        let map = document.add_object(Stream::new(Dictionary::new(), to_unicode(&codes)));
        if let Some(Object::Dictionary(font)) = document.objects.get_mut(&id) {
            font.set("ToUnicode", Object::Reference(map));
            mended += 1;
        }
    }
    mended
}

fn is_simple_type1(font: &Dictionary) -> bool {
    let named = |key: &[u8], value: &[u8]| font.get(key).and_then(Object::as_name).is_ok_and(|name| name == value);
    named(b"Type", b"Font") && (named(b"Subtype", b"Type1") || named(b"Subtype", b"MMType1"))
}

fn resolved<'a>(document: &'a Document, object: &'a Object) -> &'a Object {
    document.dereference(object).map_or(object, |(_, object)| object)
}

fn entry<'a>(document: &'a Document, dictionary: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    dictionary.get(key).ok().map(|object| resolved(document, object))
}

/// How each of a font's 256 codes reads, or `None` when the font is best left as it is.
fn codes_of(document: &Document, font: &Dictionary) -> Option<Vec<Read>> {
    let base = entry(document, font, b"BaseFont")
        .and_then(|name| name.as_name().ok())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .unwrap_or_default();
    let encoding = entry(document, font, b"Encoding");
    let name_of = |object: Option<&Object>| object.and_then(|name| name.as_name().ok()).map(<[u8]>::to_vec);
    let (differences, base_encoding) = match encoding {
        Some(Object::Name(name)) => (vec![None; 256], Some(name.clone())),
        Some(Object::Dictionary(encoding)) => {
            (differences_of(document, encoding), name_of(entry(document, encoding, b"BaseEncoding")))
        }
        _ => (vec![None; 256], None),
    };
    let named_base = base_encoding.is_some();
    let base_names = base_encoding.as_deref().and_then(glyphs::named_encoding);
    // a font re-encoded to a named encoding (as Distiller writes TeX's fonts, on WinAnsiEncoding) has that
    // encoding's codes, not its family's slots
    let family = tex::family(&base).filter(|_| !named_base);
    let existing = font.get(b"ToUnicode").ok().map(|map| map_of(document, map));
    if existing.is_some() && family.is_none() {
        return None;
    }
    let descriptor = entry(document, font, b"FontDescriptor").and_then(|descriptor| descriptor.as_dict().ok());
    let program = if named_base { None } else { descriptor.and_then(|descriptor| program_names(document, descriptor)) };
    // what the font says of its own, apart from a standard encoding it names
    let own = differences.iter().any(Option::is_some) || program.is_some();
    let names: Vec<Option<String>> = (0..256)
        .map(|code| {
            differences[code]
                .clone()
                .or_else(|| base_names.and_then(|names| names[code]).map(str::to_owned))
                .or_else(|| program.as_ref().and_then(|names| names[code].clone()))
        })
        .collect();
    let by_name: Vec<Option<String>> = names.iter().map(|name| name.as_deref().and_then(glyphs::text_of)).collect();

    let codes: Vec<Read> = match family {
        Some(family) => {
            let table: Vec<Option<&str>> = (0..=255u8).map(|code| family.slot(code)).collect();
            if let Some(existing) = &existing {
                if !family.names_are_wrong() && agrees(existing, &table) {
                    return None;
                }
            }
            let names_sound = !family.names_are_wrong() && agrees(&by_name, &table);
            (0..256)
                .map(|code| {
                    let named = by_name[code].clone().filter(|_| names_sound);
                    match named.or_else(|| table[code].map(str::to_owned)) {
                        Some(text) => Read::Text(text),
                        None => Read::Unreadable,
                    }
                })
                .collect()
        }
        None => {
            if !own {
                // nothing of its own to say: pdf-extract's reading is all there is
                return None;
            }
            let nonsymbolic = descriptor
                .and_then(|descriptor| entry(document, descriptor, b"Flags"))
                .and_then(|flags| flags.as_i64().ok())
                .is_none_or(|flags| flags & NONSYMBOLIC != 0);
            // pdf-extract falls back on an encoding of its own only when the font has one it reads
            let has_fallback = encoding.is_some()
                || descriptor.is_some_and(|descriptor| entry(document, descriptor, b"FontFile").is_some());
            (0..256)
                .map(|code| match (&names[code], &by_name[code]) {
                    (_, Some(text)) => Read::Text(text.clone()),
                    (Some(_), None) => Read::Unreadable,
                    (None, None) if named_base => Read::Left,
                    (None, None) => match glyphs::STANDARD[code].and_then(glyphs::text_of) {
                        Some(text) if nonsymbolic => Read::Text(text),
                        _ if has_fallback => Read::Left,
                        _ => Read::Unreadable,
                    },
                })
                .collect()
        }
    };
    Some(codes)
}

/// Can what a font's map or names say be trusted, against its family's table? Not when, for most of the codes both
/// say something of, they read as a text encoding's letters, accents or quote marks where the table has another
/// symbol: what a math font read through a text encoding gives ("p" for "(", "´" for "−"). A map that differs from
/// the table only in how it writes a symbol ("∣" for "|", a combining hat for "^", "𝒜" for "A") is sound.
fn agrees(said: &[Option<String>], table: &[Option<&str>]) -> bool {
    let (mut both, mut garbled) = (0usize, 0usize);
    for (said, known) in said.iter().zip(table) {
        let (Some(said), Some(known)) = (said, known) else { continue };
        if known.contains(DROPPED) {
            continue;
        }
        both += 1;
        let differs = !said.nfkc().eq(known.nfkc());
        let text_like = said.chars().any(|c| c.is_alphabetic() || is_accent_or_quote(c));
        let known_symbol = !known.chars().any(char::is_alphabetic);
        if differs && text_like && known_symbol {
            garbled += 1;
        }
    }
    garbled * 2 <= both
}

/// A spacing accent or a quote mark of a text encoding: what a math symbol read through one often comes out as.
fn is_accent_or_quote(c: char) -> bool {
    matches!(c, '`' | '´' | '¨' | '¯' | '¸' | '˘'..='˝' | 'ˆ' | 'ˇ' | '‘'..='‟' | '‹' | '›' | '«' | '»')
}

/// A font's ToUnicode map, as text by one-byte code; codes of more than one byte are not a simple font's.
fn map_of(document: &Document, map: &Object) -> Vec<Option<String>> {
    let mut codes = vec![None; 256];
    let Ok(stream) = resolved(document, map).as_stream() else { return codes };
    let bytes = stream.decompressed_content().unwrap_or_else(|_| stream.content.clone());
    let Ok(Ok(parsed)) = catch_unwind(|| adobe_cmap_parser::get_unicode_map(&bytes)) else { return codes };
    for (code, utf16) in parsed {
        let Some(slot) = usize::try_from(code).ok().filter(|code| *code < 256) else { continue };
        let units: Vec<u16> =
            utf16.chunks(2).map(|pair| u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)])).collect();
        codes[slot] = String::from_utf16(&units).ok();
    }
    codes
}

/// The glyph names an `/Encoding` dictionary's `/Differences` give, by code.
fn differences_of(document: &Document, encoding: &Dictionary) -> Vec<Option<String>> {
    let mut names = vec![None; 256];
    let Some(Object::Array(differences)) = entry(document, encoding, b"Differences") else { return names };
    let mut code: Option<usize> = None;
    for item in differences {
        match resolved(document, item) {
            Object::Integer(at) => code = usize::try_from(*at).ok(),
            Object::Name(name) => {
                if let Some(at) = code.filter(|at| *at < 256) {
                    names[at] = Some(String::from_utf8_lossy(name).into_owned());
                }
                code = code.map(|at| at + 1);
            }
            _ => {}
        }
    }
    names
}

/// The glyph names the embedded font program gives each code: a Type 1 program's `/Encoding`, or a CFF program's
/// encoding and charset.
fn program_names(document: &Document, descriptor: &Dictionary) -> Option<Vec<Option<String>>> {
    let contents = |object: &Object| {
        let stream = object.as_stream().ok()?;
        Some(stream.decompressed_content().unwrap_or_else(|_| stream.content.clone()))
    };
    if let Some(bytes) = entry(document, descriptor, b"FontFile").and_then(contents) {
        return Some(type1_names(&bytes));
    }
    let file = entry(document, descriptor, b"FontFile3")?;
    let subtype = file.as_stream().ok()?.dict.get(b"Subtype").and_then(Object::as_name).ok()?;
    if subtype != b"Type1C" {
        return None;
    }
    let bytes = contents(file)?;
    let table = cff_parser::Table::parse(&bytes)?;
    Some(
        (0..=255u8)
            .map(|code| {
                let glyph = table.glyph_index(code)?;
                table.glyph_name(glyph).filter(|name| *name != ".notdef").map(str::to_owned)
            })
            .collect(),
    )
}

/// The encoding in the clear part of a Type 1 font program: `/Encoding StandardEncoding def`, or a 256-entry array
/// filled by `dup <code> /<name> put`.
fn type1_names(program: &[u8]) -> Vec<Option<String>> {
    let clear = program.windows(5).position(|window| window == b"eexec").map_or(program, |at| &program[..at]);
    let clear = String::from_utf8_lossy(clear);
    let Some((_, after)) = clear.split_once("/Encoding") else { return vec![None; 256] };
    if after.trim_start().starts_with("StandardEncoding") {
        return glyphs::STANDARD.iter().map(|name| name.map(str::to_owned)).collect();
    }
    let mut names = vec![None; 256];
    let words: Vec<&str> = after.split_whitespace().take_while(|word| *word != "readonly" && *word != "def").collect();
    for window in words.windows(4) {
        if let ["dup", code, name, "put"] = window
            && let (Ok(code), Some(name)) = (code.parse::<usize>(), name.strip_prefix('/'))
            && code < 256
        {
            names[code] = Some(name.to_owned());
        }
    }
    names
}

/// A ToUnicode CMap that reads each code as `codes` says (one that is [`Read::Left`] is not in it).
fn to_unicode(codes: &[Read]) -> Vec<u8> {
    let mut entries: Vec<String> = Vec::new();
    for (code, read) in codes.iter().enumerate() {
        let text = match read {
            Read::Left => continue,
            Read::Text(text) if !text.is_empty() => text.clone(),
            Read::Text(_) => DROPPED.to_string(),
            Read::Unreadable => UNREADABLE.to_string(),
        };
        let hex: String = text.encode_utf16().map(|unit| format!("{unit:04X}")).collect();
        entries.push(format!("<{code:02X}> <{hex}>"));
    }
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapName /Constellate-read def\n\
         /CMapType 2 def\n1 begincodespacerange\n<00> <FF>\nendcodespacerange\n",
    );
    // at most 100 entries in one block (PDF 1.7, 9.10.3)
    for block in entries.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n", block.len()));
        for line in block {
            cmap.push_str(line);
            cmap.push('\n');
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cmap.into_bytes()
}

/// PDFs made for tests, with fonts of the kinds this reads, so that no test needs a paper.
#[cfg(test)]
pub(crate) mod fixtures {
    /// A font of a test PDF: its dictionary (without `/Type /Font`), and the streams it refers to as `{0}`, `{1}`
    /// and so on, which are numbered after the fonts.
    pub(crate) struct Font<'a> {
        pub(crate) dictionary: &'a str,
        pub(crate) streams: &'a [&'a [u8]],
    }

    /// A one-page PDF that writes each of `lines` (a font's index and its string as hexadecimal codes) one below
    /// the other.
    pub(crate) fn pdf_with_fonts(fonts: &[Font<'_>], lines: &[(usize, &str)]) -> Vec<u8> {
        let mut content = String::from("BT 72 720 Td 14 TL\n");
        for (font, codes) in lines {
            content.push_str(&format!("/F{font} 10 Tf <{codes}> Tj T*\n"));
        }
        content.push_str("ET");
        pdf_with_content(fonts, &content)
    }

    /// A one-page PDF with these fonts (`/F0`, `/F1`, ...) and this content stream.
    pub(crate) fn pdf_with_content(fonts: &[Font<'_>], content: &str) -> Vec<u8> {
        pdf_with_page(fonts, "/MediaBox [0 0 612 792]", content)
    }

    /// [`pdf_with_content`] with the page's own entries (its boxes, a rotation) as `page`, and a one-pixel image
    /// it may draw as `/Im0`.
    pub(crate) fn pdf_with_page(fonts: &[Font<'_>], page: &str, content: &str) -> Vec<u8> {
        let first_font = 5;
        let streams_count: usize = fonts.iter().map(|font| font.streams.len()).sum();
        let image = first_font + fonts.len() + streams_count;
        let mut next_stream = first_font + fonts.len();
        let mut objects: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            format!(
                "<< /Type /Page /Parent 2 0 R {page} /Contents 4 0 R /Resources << /Font << {} >> /XObject << /Im0 {image} 0 R >> >> >>",
                (0..fonts.len()).map(|at| format!("/F{at} {} 0 R", first_font + at)).collect::<Vec<_>>().join(" ")
            )
            .into_bytes(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes(),
        ];
        let mut streams: Vec<Vec<u8>> = Vec::new();
        for font in fonts {
            let mut dictionary = font.dictionary.to_owned();
            for (at, stream) in font.streams.iter().enumerate() {
                dictionary = dictionary.replace(&format!("{{{at}}}"), &format!("{next_stream} 0 R"));
                let mut object = format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes();
                object.extend_from_slice(stream);
                object.extend_from_slice(b"\nendstream");
                streams.push(object);
                next_stream += 1;
            }
            let widths = vec!["500"; 256].join(" ");
            objects.push(
                format!("<< /Type /Font {dictionary} /FirstChar 0 /LastChar 255 /Widths [{widths}] >>").into_bytes(),
            );
        }
        objects.extend(streams);
        objects.push(
            b"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 \
              /Length 1 >>\nstream\n\x80\nendstream"
                .to_vec(),
        );
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (number, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend(format!("{} 0 obj\n", number + 1).bytes());
            pdf.extend_from_slice(object);
            pdf.extend(b"\nendobj\n");
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

    /// A ToUnicode map that reads each code of `pairs` as its text.
    pub(crate) fn to_unicode(pairs: &[(u8, &str)]) -> Vec<u8> {
        let codes: Vec<super::Read> = (0..=255u8)
            .map(|code| match pairs.iter().find(|(at, _)| *at == code) {
                Some((_, text)) => super::Read::Text((*text).to_owned()),
                None => super::Read::Left,
            })
            .collect();
        super::to_unicode(&codes)
    }

    /// The clear part of a Type 1 font program with these glyph names at these codes, and a few bytes standing for
    /// its encrypted part.
    pub(crate) fn type1_program(names: &[(u8, &str)]) -> Vec<u8> {
        let mut program = String::from(
            "%!PS-AdobeFont-1.0: TestMath 001.000\n/FontName /TestMath def\n/Encoding 256 array\n\
             0 1 255 {1 index exch /.notdef put} for\n",
        );
        for (code, name) in names {
            program.push_str(&format!("dup {code} /{name} put\n"));
        }
        program.push_str("readonly def\ncurrentfile eexec\n");
        let mut bytes = program.into_bytes();
        bytes.extend_from_slice(&[0x8f, 0x12, 0x34, 0x56]);
        bytes
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::fixtures::{Font, pdf_with_fonts, to_unicode, type1_program};
    use crate::text::pdf::{UNREADABLE, read_in_this_process};

    /// The lines of the one page a test PDF has, as the engine reads them.
    fn lines(fonts: &[Font<'_>], written: &[(usize, &str)]) -> Vec<String> {
        let pages = read_in_this_process(&pdf_with_fonts(fonts, written)).unwrap().pages;
        pages[0].lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_owned).collect()
    }

    /// Hexadecimal codes of the bytes of `text`.
    fn hex(text: &[u8]) -> String {
        text.iter().map(|byte| format!("{byte:02X}")).collect()
    }

    const PROSE: Font<'static> =
        Font { dictionary: "/Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding", streams: &[] };

    #[test]
    fn a_font_with_no_tounicode_is_read_by_its_differences_glyph_names() {
        // the codes of "pqrs" named as brackets and a relation: pdf-extract read uni2264 as the letter "r"
        let font = Font {
            dictionary: "/Subtype /Type1 /BaseFont /ABCDEF+SomeMath \
                         /Encoding << /Type /Encoding /Differences [112 /parenleft /parenright /uni2264 /summationdisplay] >>",
            streams: &[],
        };
        assert_eq!(lines(&[font], &[(0, &hex(b"pqrs"))]), ["()≤∑"]);
    }

    #[test]
    fn a_font_with_only_its_embedded_programs_encoding_is_read_by_the_programs_glyph_names() {
        let program = type1_program(&[(97, "uni2208"), (98, "minus"), (99, "summationtext")]);
        let font = Font {
            dictionary: "/Subtype /Type1 /BaseFont /ABCDEF+MathProgram \
                         /FontDescriptor << /Type /FontDescriptor /FontName /ABCDEF+MathProgram /Flags 4 /FontFile {0} >>",
            streams: &[&program],
        };
        assert_eq!(lines(&[font], &[(0, &hex(b"abc"))]), ["∈\u{2212}∑"]);
    }

    #[test]
    fn a_tex_math_font_with_nothing_of_its_own_is_read_by_its_familys_table_and_only_it() {
        let symbols = Font { dictionary: "/Subtype /Type1 /BaseFont /QWERTY+CMSY10", streams: &[] };
        let extension = Font { dictionary: "/Subtype /Type1 /BaseFont /CMEX10", streams: &[] };
        // "≤ − × ∈", then a display sum and a big left parenthesis; the same codes in Helvetica stay text
        let written = [(0, "14 00 02 32"), (1, "58 12"), (2, &*hex(b"p q r s"))];
        assert_eq!(
            lines(&[symbols, extension, PROSE], &written),
            ["≤\u{2212}×∈".to_owned(), "∑(".to_owned(), "p q r s".to_owned()]
        );
    }

    #[test]
    fn a_mathabx_font_whose_tounicode_names_text_letters_is_read_by_its_familys_table() {
        // as pdfTeX writes it: the Cork encoding's letters and accents at matha's slots
        let wrong = to_unicode(&[(0x70, "p"), (0x71, "q"), (0x72, "r"), (0x73, "s"), (0x01, "´"), (0xA4, "ď")]);
        let matha =
            Font { dictionary: "/Subtype /Type1 /BaseFont /OACKWY+TeX-matha10 /ToUnicode {0}", streams: &[&wrong] };
        // (k, u) ∈ [K] × [H − 1], the letters from an ordinary font with a sound map of its own
        let letters = to_unicode(&[(b'k', "k"), (b'u', "u"), (b'K', "K"), (b'H', "H"), (b'1', "1"), (b',', ",")]);
        let italic = Font {
            dictionary: "/Subtype /Type1 /BaseFont /ABCDEF+NimbusRomNo9L-Regu /ToUnicode {0}",
            streams: &[&letters],
        };
        let written = [
            (0, "70"),
            (1, &*hex(b"k,u")),
            (0, "71 50 72"),
            (1, "4B"),
            (0, "73 02 72"),
            (1, "48"),
            (0, "01"),
            (1, "31"),
            (0, "73 A4"),
        ];
        assert_eq!(lines(&[matha, italic], &written).concat(), "(k,u)∈[K]×[H\u{2212}1]≤");
    }

    #[test]
    fn a_math_font_with_a_sound_tounicode_and_ordinary_text_are_left_as_they_are() {
        let sound = to_unicode(&[(0x14, "≤"), (0x00, "\u{2212}"), (0x32, "∈"), (0x41, "𝒜")]);
        let symbols = Font { dictionary: "/Subtype /Type1 /BaseFont /CMSY10 /ToUnicode {0}", streams: &[&sound] };
        let prose = Font {
            dictionary: "/Subtype /Type1 /BaseFont /ABCDEF+NimbusRomNo9L-Regu \
                         /Encoding << /Differences [112 /p /q /r /s] >>",
            streams: &[],
        };
        let written = [(0, "14 41"), (1, &*hex(b"pqrs")), (2, &*hex(b"we use p, q, r and s"))];
        assert_eq!(lines(&[symbols, prose, PROSE], &written), ["≤𝒜", "pqrs", "we use p, q, r and s"]);
    }

    #[test]
    fn what_no_part_of_a_font_says_is_marked_unreadable_not_guessed() {
        let font = Font {
            dictionary: "/Subtype /Type1 /BaseFont /ABCDEF+Mystery /Encoding << /Differences [65 /g13 /g14 /g15] >>",
            streams: &[],
        };
        let read = lines(&[font, PROSE], &[(1, &hex(b"where")), (0, "41 42 43"), (1, &hex(b"holds"))]);
        assert_eq!(read.join(" "), format!("where {UNREADABLE} holds"), "one mark for the run of three");
        assert!(!read.concat().contains(['A', 'B', 'C']), "never the codes read as letters");
    }
}
