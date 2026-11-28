//! Where on its page each figure and table of a paper is drawn, found by rules from what the PDF itself says, so
//! that the reader can show the figure as the paper shows it, under the caption the text already has.
//!
//! # What is read
//!
//! For every page: the lines of text with the box each takes (from the same reading as the page's text,
//! [`super::PageText`]); the boxes of the images the page draws (image XObjects, under the transformation matrix in
//! force where they are drawn, form XObjects followed); and the boxes of the vector drawing (each path that is
//! stroked or filled, with the matrix applied). All in the page's own coordinates: PDF points, origin at the
//! bottom left of the page's user space, as its crop box is written.
//!
//! # The rules, for each caption ("Figure 3.", "Fig. 2:", "Table 1.")
//!
//! 1. The caption is its first line and the lines right below it at its size.
//! 2. Its span across the page: the whole text width when it runs across the middle of a two-column page (or the
//!    page has one column), else its column.
//! 3. Above it and below it, up to the nearest line of body text in that span (or the page's edge), everything
//!    drawn inside the span is a candidate: images, paths, and lines of text that are not body text (the words
//!    inside a figure, a table's cells). The running header and footer, and the other captions, are not.
//! 4. On each side, the candidates are gathered outward from the caption while each is close to what was gathered
//!    before (less than [`GAP`] apart); the first must be close to the caption ([`FIRST_GAP`]).
//! 5. The figure is on the side that has something drawn: a picture needs an image or a path; a table may be text
//!    alone (three lines at least). With something on both sides, the nearer one; at equal distance, above for a
//!    figure and below for a table, as papers set them.
//! 6. Its rectangle is what was gathered, a little padded, within the crop box. Too small, or overlapping another
//!    figure's rectangle, and it is none: no rectangle is better than a wrong one ([`Figures::missed`] counts them).

use std::collections::HashMap;
use std::sync::LazyLock;

use pdf_extract::content::Content;
use pdf_extract::{Dictionary, Document, Object, ObjectId};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::text::pattern;

/// The most two neighbouring pieces of one figure may be apart, in points.
const GAP: f64 = 14.0;
/// The most the figure's nearest piece may be from its caption, in points.
const FIRST_GAP: f64 = 36.0;
/// What a figure's rectangle is padded by, in points.
const PAD: f64 = 2.0;
/// The smallest a figure's rectangle may be, in points.
const SMALLEST: (f64, f64) = (36.0, 24.0);
/// The share of a page's height at its top and bottom where its running header and footer are.
const EDGE: f64 = 0.08;
/// Form XObjects followed into, at most this deep: a real figure nests one or two.
const DEEPEST_FORM: usize = 8;
/// The most drawing operations read on one page: a page of a paper has thousands, a hostile one may have millions.
const MOST_OPERATIONS: usize = 2_000_000;

/// A rectangle in a page's own coordinates: PDF points, x to the right and y up from the origin of the page's user
/// space (the bottom left of its media box as the PDF writes it, which is most often 0, 0).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

// its numbers are always finite (a box no drawing reaches is never made one), so equal is an equivalence
impl Eq for Rect {}

impl Rect {
    fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    pub(crate) fn union(&self, other: &Rect) -> Rect {
        Rect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    fn intersection(&self, other: &Rect) -> Option<Rect> {
        let cut = Rect {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        };
        (cut.x0 < cut.x1 && cut.y0 < cut.y1).then_some(cut)
    }

    fn area(&self) -> f64 {
        self.width().max(0.0) * self.height().max(0.0)
    }

    /// Does it lie across `from..to` horizontally by at least half its own width?
    fn within_span(&self, from: f64, to: f64) -> bool {
        let overlap = self.x1.min(to) - self.x0.max(from);
        overlap > 0.0 && overlap >= self.width().max(0.01) * 0.5
    }

    fn finite(&self) -> bool {
        [self.x0, self.y0, self.x1, self.y1].iter().all(|value| value.is_finite())
    }
}

/// A line of a page's text and the box it takes.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub rect: Rect,
    /// The largest glyph's size in it, in points.
    pub size: f64,
}

/// What a page draws that is not text.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drawn {
    Image(Rect),
    Path(Rect),
}

impl Drawn {
    fn rect(&self) -> Rect {
        match self {
            Self::Image(rect) | Self::Path(rect) => *rect,
        }
    }
}

/// A figure or a table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FigureKind {
    Figure,
    Table,
}

/// A page's box and how it is turned, as a reader of the PDF needs them to draw part of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageBox {
    /// The crop box (the media box when there is none), in the page's own coordinates.
    pub crop: Rect,
    /// Degrees clockwise the page is turned when shown: 0, 90, 180 or 270.
    pub rotation: u32,
}

/// Where one figure or table is drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    /// As its caption begins it: "Figure 1", "Table 2", "Figure A3".
    pub label: String,
    pub kind: FigureKind,
    /// The PDF's page number, from 1.
    pub page: u32,
    pub rect: Rect,
    pub page_box: PageBox,
    /// The text of the lines inside its rectangle (the words drawn in a figure, a table's cells), for telling
    /// which of the passages it holds.
    pub inside: Vec<String>,
}

/// The figures of a paper, and how many captions got no rectangle.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Figures {
    pub placed: Vec<Placed>,
    pub missed: u32,
}

/// The first line of a caption, and what it labels.
static CAPTION: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^(?:(Figure|Fig\.)|(Table|Tab\.))\s*([A-Z]?\d{1,3})\s*[.:|]"));

/// The label a caption's first line gives, and whether it is a figure's or a table's.
pub fn caption_label(line: &str) -> Option<(String, FigureKind)> {
    let found = CAPTION.captures(line.trim_start())?;
    let kind = if found.get(1).is_some() { FigureKind::Figure } else { FigureKind::Table };
    let name = if kind == FigureKind::Figure { "Figure" } else { "Table" };
    Some((format!("{name} {}", &found[3]), kind))
}

/// The page box of the page `id`: its crop box (or media box), inherited from the page tree when the page has none,
/// and its rotation.
pub fn page_box(document: &Document, id: ObjectId) -> Option<PageBox> {
    let inherited = |key: &[u8]| -> Option<Object> {
        let mut node = document.get_dictionary(id).ok()?;
        for _ in 0..32 {
            if let Ok(found) = node.get(key) {
                return Some(document.dereference(found).map_or(found, |(_, found)| found).clone());
            }
            let parent = node.get(b"Parent").and_then(Object::as_reference).ok()?;
            node = document.get_dictionary(parent).ok()?;
        }
        None
    };
    let rect_of = |object: Object| -> Option<Rect> {
        let values: Vec<f64> = object.as_array().ok()?.iter().filter_map(number).collect();
        let [a, b, c, d] = values.as_slice() else { return None };
        Some(Rect { x0: a.min(*c), y0: b.min(*d), x1: a.max(*c), y1: b.max(*d) })
    };
    let crop = inherited(b"CropBox").and_then(rect_of).or_else(|| inherited(b"MediaBox").and_then(rect_of))?;
    let rotation = inherited(b"Rotate").and_then(|turn| turn.as_i64().ok()).unwrap_or(0).rem_euclid(360);
    let rotation = u32::try_from(rotation - rotation % 90).unwrap_or(0);
    Some(PageBox { crop, rotation })
}

fn number(object: &Object) -> Option<f64> {
    match object {
        Object::Integer(value) => Some(*value as f64),
        Object::Real(value) => Some(f64::from(*value)),
        _ => None,
    }
}

/// A matrix of PDF's, `[a b c d e f]`.
#[derive(Clone, Copy, Debug)]
struct Matrix([f64; 6]);

impl Matrix {
    const IDENTITY: Matrix = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    /// `self` applied after `inner`: what `cm` with `inner` does to the current matrix `self`.
    fn then(inner: &Matrix, outer: &Matrix) -> Matrix {
        let [a, b, c, d, e, f] = inner.0;
        let [p, q, r, s, t, u] = outer.0;
        Matrix([a * p + b * r, a * q + b * s, c * p + d * r, c * q + d * s, e * p + f * r + t, e * q + f * s + u])
    }

    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }

    fn of(operands: &[Object]) -> Option<Matrix> {
        let values: Vec<f64> = operands.iter().filter_map(number).collect();
        let values: [f64; 6] = values.try_into().ok()?;
        Some(Matrix(values))
    }
}

/// Where a path being built reaches, in page coordinates.
#[derive(Default)]
struct Reach(Option<Rect>);

impl Reach {
    fn add(&mut self, (x, y): (f64, f64)) {
        let point = Rect { x0: x, y0: y, x1: x, y1: y };
        self.0 = Some(self.0.map_or(point, |rect| rect.union(&point)));
    }
}

/// What the page `id` draws that is not text: its images and painted paths, in page coordinates.
fn drawn(document: &Document, id: ObjectId) -> Vec<Drawn> {
    let Ok(content) = document.get_page_content(id) else { return Vec::new() };
    let resources = page_resources(document, id);
    let mut found = Vec::new();
    let mut budget = MOST_OPERATIONS;
    walk(document, &content, &resources, Matrix::IDENTITY, 0, &mut budget, &mut found);
    found
}

/// The resources of a page, its own over those it inherits.
fn page_resources(document: &Document, id: ObjectId) -> Dictionary {
    let mut merged = Dictionary::new();
    let Ok((own, inherited)) = document.get_page_resources(id) else { return merged };
    for resources in inherited.iter().rev().filter_map(|id| document.get_dictionary(*id).ok()).chain(own) {
        for (key, value) in resources.iter() {
            merged.set(key.clone(), value.clone());
        }
    }
    merged
}

fn walk(
    document: &Document,
    content: &[u8],
    resources: &Dictionary,
    start: Matrix,
    depth: usize,
    budget: &mut usize,
    found: &mut Vec<Drawn>,
) {
    let Ok(content) = Content::decode(content) else { return };
    let mut matrix = start;
    let mut saved: Vec<Matrix> = Vec::new();
    let mut path = Reach::default();
    let mut point = (0.0, 0.0);
    let xobjects = resources
        .get(b"XObject")
        .ok()
        .and_then(|object| document.dereference(object).ok())
        .and_then(|(_, object)| object.as_dict().ok());
    for operation in &content.operations {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let operands = &operation.operands;
        let at = |index: usize| operands.get(index).and_then(number).unwrap_or(0.0);
        match operation.operator.as_str() {
            "q" => saved.push(matrix),
            "Q" => matrix = saved.pop().unwrap_or(start),
            "cm" => {
                if let Some(inner) = Matrix::of(operands) {
                    matrix = Matrix::then(&inner, &matrix);
                }
            }
            "m" | "l" => {
                point = (at(0), at(1));
                path.add(matrix.apply(point.0, point.1));
            }
            "c" => {
                for pair in [(0, 1), (2, 3), (4, 5)] {
                    path.add(matrix.apply(at(pair.0), at(pair.1)));
                }
                point = (at(4), at(5));
            }
            "v" | "y" => {
                for pair in [(0, 1), (2, 3)] {
                    path.add(matrix.apply(at(pair.0), at(pair.1)));
                }
                point = (at(2), at(3));
            }
            "re" => {
                let (x, y, w, h) = (at(0), at(1), at(2), at(3));
                for corner in [(x, y), (x + w, y), (x, y + h), (x + w, y + h)] {
                    path.add(matrix.apply(corner.0, corner.1));
                }
                point = (x, y);
            }
            "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                if let Some(rect) = path.0.take().filter(Rect::finite) {
                    found.push(Drawn::Path(rect));
                }
            }
            "n" => path.0 = None,
            "Do" => {
                let Some(name) = operands.first().and_then(|name| name.as_name().ok()) else { continue };
                let Some(xobject) = xobjects
                    .and_then(|xobjects| xobjects.get(name).ok())
                    .and_then(|object| document.dereference(object).ok())
                    .and_then(|(_, object)| object.as_stream().ok())
                else {
                    continue;
                };
                let subtype = xobject.dict.get(b"Subtype").and_then(Object::as_name).unwrap_or_default();
                if subtype == b"Image" {
                    let mut reach = Reach::default();
                    for corner in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                        reach.add(matrix.apply(corner.0, corner.1));
                    }
                    if let Some(rect) = reach.0.filter(Rect::finite) {
                        found.push(Drawn::Image(rect));
                    }
                } else if subtype == b"Form" && depth < DEEPEST_FORM {
                    let inner =
                        xobject.dict.get(b"Matrix").ok().and_then(|m| m.as_array().ok()).and_then(|m| Matrix::of(m));
                    let form_matrix = Matrix::then(&inner.unwrap_or(Matrix::IDENTITY), &matrix);
                    let form_resources = xobject
                        .dict
                        .get(b"Resources")
                        .ok()
                        .and_then(|object| document.dereference(object).ok())
                        .and_then(|(_, object)| object.as_dict().ok())
                        .unwrap_or(resources);
                    let bytes = xobject.decompressed_content().unwrap_or_else(|_| xobject.content.clone());
                    walk(document, &bytes, form_resources, form_matrix, depth + 1, budget, found);
                }
            }
            _ => {}
        }
    }
    let _ = point;
}

/// The size most of a paper's text is set in: its body text's.
fn body_size(lines: &[Vec<Line>]) -> f64 {
    let mut counted: HashMap<i64, usize> = HashMap::new();
    for line in lines.iter().flatten() {
        if line.size.is_finite() && line.size > 0.0 {
            *counted.entry((line.size * 2.0).round() as i64).or_default() += line.text.chars().count();
        }
    }
    counted.into_iter().max_by_key(|(size, count)| (*count, *size)).map_or(10.0, |(size, _)| size as f64 / 2.0)
}

/// Is a line running prose at the body's size, of some length: what a figure stops at?
fn is_body(line: &Line, body: f64, column: f64) -> bool {
    (line.size - body).abs() <= 0.6
        && line.rect.width() >= column * 0.4
        && crate::text::pdf::passages::prose_share(&line.text) >= 0.5
}

/// The figures of a paper, from each page's lines of text (in order, from page 1) and the document they are drawn
/// in.
pub fn find(document: &Document, lines: &[Vec<Line>]) -> Figures {
    let body = body_size(lines);
    let pages = document.get_pages();
    let mut figures = Figures::default();
    for (index, page_lines) in lines.iter().enumerate() {
        let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        if !page_lines.iter().any(|line| caption_label(&line.text).is_some()) {
            continue;
        }
        let Some((id, page_box)) = pages.get(&number).and_then(|id| Some((*id, page_box(document, *id)?))) else {
            continue;
        };
        let drawn = drawn(document, id);
        let (placed, missed) = on_page(page_lines, &drawn, page_box, body, number);
        figures.placed.extend(placed);
        figures.missed += missed;
    }
    figures
}

/// A piece of a page that may be part of a figure.
#[derive(Clone, Copy, Debug)]
struct Piece {
    rect: Rect,
    /// Drawn (an image or a path), not a line of text.
    drawn: bool,
}

/// The figures of one page.
fn on_page(lines: &[Line], drawn: &[Drawn], page_box: PageBox, body: f64, number: u32) -> (Vec<Placed>, u32) {
    let crop = page_box.crop;
    let (top_edge, bottom_edge) = (crop.y1 - crop.height() * EDGE, crop.y0 + crop.height() * EDGE);
    // the running header and footer: a long line of text, or a rule, at the page's top or bottom; a figure's own
    // labels and drawing may reach up there
    let in_margin = |piece: &Piece| {
        let rect = piece.rect;
        let at_edge = rect.y0 >= top_edge || rect.y1 <= bottom_edge;
        let header_like = if piece.drawn { rect.height() < 2.0 } else { rect.width() >= crop.width() * 0.35 };
        at_edge && header_like
    };
    let middle = (crop.x0 + crop.x1) / 2.0;
    let body_lines: Vec<&Line> = lines.iter().filter(|line| is_body(line, body, crop.width() * 0.4)).collect();
    let (text_left, text_right) = body_lines.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(left, right), line| {
        (left.min(line.rect.x0), right.max(line.rect.x1))
    });
    let (text_left, text_right) = if text_left < text_right {
        (text_left, text_right)
    } else {
        (crop.x0 + crop.width() * 0.1, crop.x1 - crop.width() * 0.1)
    };
    let two_columns = body_lines.iter().filter(|line| line.rect.x1 < middle + 5.0).count() >= 3
        && body_lines.iter().filter(|line| line.rect.x0 > middle - 5.0).count() >= 3;

    // the captions, each its first line and the lines right under it at its size
    let mut captions: Vec<(String, FigureKind, Rect)> = Vec::new();
    let mut in_caption = vec![false; lines.len()];
    for (at, line) in lines.iter().enumerate() {
        let Some((label, kind)) = caption_label(&line.text) else { continue };
        let mut rect = line.rect;
        in_caption[at] = true;
        let mut last = line;
        for (next_at, next) in lines.iter().enumerate().skip(at + 1).take(12) {
            let below = last.rect.y0 - next.rect.y1;
            let same_column = next.rect.x0 < rect.x1 && next.rect.x1 > rect.x0;
            if (next.size - line.size).abs() > 0.6 || !(-1.0..=line.size * 0.8).contains(&below) || !same_column {
                break;
            }
            if caption_label(&next.text).is_some() {
                break;
            }
            rect = rect.union(&next.rect);
            in_caption[next_at] = true;
            last = next;
        }
        captions.push((label, kind, rect));
    }

    let pieces: Vec<Piece> = drawn
        .iter()
        .map(|drawn| Piece { rect: drawn.rect(), drawn: true })
        .chain(
            lines
                .iter()
                .enumerate()
                .filter(|(at, line)| !in_caption[*at] && !is_body(line, body, crop.width() * 0.4))
                .map(|(_, line)| Piece { rect: line.rect, drawn: false }),
        )
        .filter(|piece| piece.rect.finite() && crop.intersection(&piece.rect).is_some())
        .filter(|piece| piece.rect.area() < crop.area() * 0.9 && !in_margin(piece))
        // what reaches the page's edge is the page's (a background, a bleed), not a figure set in the text
        .filter(|piece| {
            let rect = piece.rect;
            rect.x0 > crop.x0 + 1.0 && rect.x1 < crop.x1 - 1.0 && rect.y0 > crop.y0 + 1.0 && rect.y1 < crop.y1 - 1.0
        })
        .collect();

    let mut placed: Vec<(Placed, Rect)> = Vec::new();
    let mut missed = 0;
    for (label, kind, caption) in &captions {
        let full = !two_columns || (caption.x0 < middle - 20.0 && caption.x1 > middle + 20.0);
        let (from, to) = if !two_columns {
            // one column: the whole page's width (a figure may be wider than the lines of text)
            (crop.x0, crop.x1)
        } else if full {
            (text_left, text_right)
        } else if caption.x1 <= middle + 5.0 {
            (text_left, middle)
        } else {
            (middle, text_right)
        };
        let stops = |line: &&&Line| line.rect.within_span(from, to);
        // the nearest body line above and below the caption, in its span
        let ceiling = body_lines
            .iter()
            .filter(stops)
            .map(|line| line.rect.y0)
            .filter(|y| *y > caption.y1)
            .fold(crop.y1, f64::min);
        let floor = body_lines
            .iter()
            .filter(stops)
            .map(|line| line.rect.y1)
            .filter(|y| *y < caption.y0)
            .fold(crop.y0, f64::max);
        let others: Vec<&Rect> = captions.iter().map(|(_, _, rect)| rect).filter(|rect| *rect != caption).collect();
        let side = |above: bool| -> Option<(Rect, f64)> {
            let mut candidates: Vec<(f64, Piece)> = pieces
                .iter()
                // within the span, and no wider than it: a box across the whole page (a background, a clip) is not
                // part of one figure
                .filter(|piece| piece.rect.within_span(from, to) && piece.rect.width() <= (to - from) * 1.1 + 4.0)
                .filter(|piece| {
                    if above {
                        piece.rect.y0 >= caption.y1 - 1.0 && piece.rect.y1 <= ceiling + 1.0
                    } else {
                        piece.rect.y1 <= caption.y0 + 1.0 && piece.rect.y0 >= floor - 1.0
                    }
                })
                .filter(|piece| others.iter().all(|other| other.intersection(&piece.rect).is_none()))
                .map(|piece| {
                    let distance = if above { piece.rect.y0 - caption.y1 } else { caption.y0 - piece.rect.y1 };
                    (distance.max(0.0), *piece)
                })
                .collect();
            candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
            let nearest = candidates.first()?.0;
            if nearest > FIRST_GAP {
                return None;
            }
            let mut rect = candidates[0].1.rect;
            let (mut drawn, mut texts) = (0usize, 0usize);
            for (_, piece) in &candidates {
                let gap = if above { piece.rect.y0 - rect.y1 } else { rect.y0 - piece.rect.y1 };
                if gap > GAP {
                    break;
                }
                rect = rect.union(&piece.rect);
                if piece.drawn { drawn += 1 } else { texts += 1 }
            }
            let enough = drawn > 0 || (*kind == FigureKind::Table && texts >= 3);
            enough.then_some((rect, nearest))
        };
        let chosen = match (side(true), side(false)) {
            (Some(above), Some(below)) => {
                let prefer_above = match above.1.total_cmp(&below.1) {
                    std::cmp::Ordering::Less => true,
                    std::cmp::Ordering::Greater => false,
                    std::cmp::Ordering::Equal => *kind == FigureKind::Figure,
                };
                Some(if prefer_above { above.0 } else { below.0 })
            }
            (one, other) => one.or(other).map(|(rect, _)| rect),
        };
        let Some(rect) = chosen else {
            missed += 1;
            continue;
        };
        // padded, but never into its own caption
        let mut padded = Rect { x0: rect.x0 - PAD, y0: rect.y0 - PAD, x1: rect.x1 + PAD, y1: rect.y1 + PAD };
        if rect.y0 >= caption.y1 {
            padded.y0 = padded.y0.max(caption.y1);
        } else if rect.y1 <= caption.y0 {
            padded.y1 = padded.y1.min(caption.y0);
        }
        let Some(rect) =
            padded.intersection(&crop).filter(|rect| rect.width() >= SMALLEST.0 && rect.height() >= SMALLEST.1)
        else {
            missed += 1;
            continue;
        };
        let inside = lines
            .iter()
            .enumerate()
            .filter(|(at, line)| {
                !in_caption[*at]
                    && line.rect.intersection(&rect).is_some_and(|cut| cut.area() >= line.rect.area() * 0.5)
            })
            .map(|(_, line)| line.text.clone())
            .collect();
        placed.push((Placed { label: label.clone(), kind: *kind, page: number, rect, page_box, inside }, rect));
    }
    // two rectangles over one another: neither can be trusted
    let mut overlapping = vec![false; placed.len()];
    for one in 0..placed.len() {
        for other in one + 1..placed.len() {
            let (a, b) = (placed[one].1, placed[other].1);
            if a.intersection(&b).is_some_and(|cut| cut.area() > a.area().min(b.area()) * 0.2) {
                overlapping[one] = true;
                overlapping[other] = true;
            }
        }
    }
    let kept: Vec<Placed> = placed
        .into_iter()
        .zip(&overlapping)
        .filter_map(|((placed, _), overlaps)| (!overlaps).then_some(placed))
        .collect();
    missed += u32::try_from(overlapping.iter().filter(|overlaps| **overlaps).count()).unwrap_or(0);
    (kept, missed)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::text::pdf::fonts::fixtures::{Font, pdf_with_page};
    use crate::text::pdf::read_in_this_process;

    const HELVETICA: Font<'static> =
        Font { dictionary: "/Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding", streams: &[] };
    const LETTER: &str = "/MediaBox [0 0 612 792]";
    /// A line of running prose, long enough to be body text in a column.
    const PROSE: &str = "we study how the figures of a paper are found by rules";

    fn text(x: f64, y: f64, size: f64, words: &str) -> String {
        format!("BT /F0 {size} Tf {x} {y} Td ({words}) Tj ET\n")
    }

    /// Lines of prose at 10 points from `top` down, `count` of them, 12 points apart.
    fn prose(x: f64, top: f64, count: usize) -> String {
        (0..count).map(|at| text(x, top - 12.0 * at as f64, 10.0, PROSE)).collect()
    }

    fn image(x: f64, y: f64, width: f64, height: f64) -> String {
        format!("q {width} 0 0 {height} {x} {y} cm /Im0 Do Q\n")
    }

    fn filled(x: f64, y: f64, width: f64, height: f64) -> String {
        format!("{x} {y} {width} {height} re f\n")
    }

    fn figures_of(page: &str, content: &str) -> Figures {
        read_in_this_process(&pdf_with_page(&[HELVETICA], page, content)).unwrap().figures
    }

    fn near(rect: Rect, expected: [f64; 4]) {
        let got = [rect.x0, rect.y0, rect.x1, rect.y1];
        for (got, expected) in got.iter().zip(expected) {
            assert!((got - expected).abs() <= 4.0, "{rect:?} is not near {expected:?}");
        }
    }

    #[test]
    fn an_image_above_its_caption_is_its_figure() {
        let content = [
            prose(72.0, 700.0, 3),
            image(100.0, 450.0, 300.0, 190.0),
            text(72.0, 430.0, 9.0, "Figure 1. A picture of the method, drawn as an image."),
            prose(72.0, 400.0, 3),
        ]
        .concat();
        let figures = figures_of(LETTER, &content);
        assert_eq!(figures.missed, 0);
        let [figure] = figures.placed.as_slice() else { panic!("{figures:?}") };
        assert_eq!((figure.label.as_str(), figure.kind, figure.page), ("Figure 1", FigureKind::Figure, 1));
        near(figure.rect, [100.0, 450.0, 400.0, 640.0]);
        assert_eq!(figure.page_box, PageBox { crop: Rect { x0: 0.0, y0: 0.0, x1: 612.0, y1: 792.0 }, rotation: 0 });
    }

    #[test]
    fn vector_drawing_and_its_labels_above_a_caption_are_its_figure() {
        let content = [
            prose(72.0, 700.0, 3),
            filled(90.0, 480.0, 40.0, 120.0),
            filled(150.0, 480.0, 40.0, 80.0),
            filled(210.0, 480.0, 40.0, 150.0),
            text(90.0, 468.0, 7.0, "A2A"),
            text(150.0, 468.0, 7.0, "ACP"),
            text(210.0, 468.0, 7.0, "ANP"),
            text(72.0, 445.0, 9.0, "Figure 2: Prominent protocols, as bars."),
            prose(72.0, 415.0, 3),
        ]
        .concat();
        let figures = figures_of(LETTER, &content);
        let [figure] = figures.placed.as_slice() else { panic!("{figures:?}") };
        near(figure.rect, [90.0, 466.0, 250.0, 630.0]);
        let words: Vec<&str> = figure.inside.iter().flat_map(|line| line.split_whitespace()).collect();
        assert_eq!(words, ["A2A", "ACP", "ANP"], "the words drawn in it");
    }

    #[test]
    fn a_table_below_its_caption_is_its_table_rules_and_cells_alike() {
        let content = [
            prose(72.0, 700.0, 3),
            text(72.0, 650.0, 9.0, "Table 1. Accuracy of each model on each benchmark."),
            filled(72.0, 636.0, 300.0, 0.5),
            text(72.0, 624.0, 8.0, "Model    MMLU    GSM8K"),
            filled(72.0, 618.0, 300.0, 0.4),
            text(72.0, 606.0, 8.0, "Ours    71.2    80.4"),
            text(72.0, 594.0, 8.0, "Base    65.0    74.9"),
            filled(72.0, 588.0, 300.0, 0.5),
            prose(72.0, 560.0, 3),
        ]
        .concat();
        let figures = figures_of(LETTER, &content);
        let [table] = figures.placed.as_slice() else { panic!("{figures:?}") };
        assert_eq!((table.label.as_str(), table.kind), ("Table 1", FigureKind::Table));
        near(table.rect, [72.0, 588.0, 372.0, 637.0]);
    }

    #[test]
    fn a_figure_in_one_column_of_two_stays_in_its_column() {
        let left = 72.0;
        let right = 320.0;
        let short = "we read the columns of a page";
        let column = |x: f64, top: f64, count: usize| -> String {
            (0..count).map(|at| text(x, top - 12.0 * at as f64, 10.0, short)).collect()
        };
        let content = [
            column(left, 700.0, 6),
            image(80.0, 470.0, 200.0, 140.0),
            text(left, 450.0, 9.0, "Figure 3. In the left column."),
            column(left, 420.0, 6),
            column(right, 700.0, 36),
        ]
        .concat();
        let figures = figures_of(LETTER, &content);
        let [figure] = figures.placed.as_slice() else { panic!("{figures:?}") };
        near(figure.rect, [80.0, 470.0, 280.0, 610.0]);
        assert!(figure.rect.x1 < 306.0, "never into the other column");
    }

    #[test]
    fn sub_figures_side_by_side_under_one_caption_across_the_page_are_one_figure() {
        let short = "we read the columns of a page";
        let column = |x: f64, top: f64, count: usize| -> String {
            (0..count).map(|at| text(x, top - 12.0 * at as f64, 10.0, short)).collect()
        };
        let content = [
            column(72.0, 700.0, 4),
            column(320.0, 700.0, 4),
            image(80.0, 480.0, 200.0, 150.0),
            image(330.0, 480.0, 200.0, 150.0),
            text(
                72.0,
                460.0,
                9.0,
                "Figure 4. (a) The first part of the result, on the left. (b) The second part, on the right.",
            ),
            column(72.0, 430.0, 4),
            column(320.0, 430.0, 4),
        ]
        .concat();
        let figures = figures_of(LETTER, &content);
        let [figure] = figures.placed.as_slice() else { panic!("{figures:?}") };
        near(figure.rect, [80.0, 480.0, 530.0, 630.0]);
    }

    #[test]
    fn a_figure_at_the_top_of_a_page_with_its_caption_under_it() {
        let content = [
            image(72.0, 560.0, 460.0, 170.0),
            text(72.0, 540.0, 9.0, "Figure 5. At the top of the page, under the running header."),
            prose(72.0, 510.0, 10),
        ]
        .concat();
        let figures = figures_of(LETTER, &content);
        let [figure] = figures.placed.as_slice() else { panic!("{figures:?}") };
        near(figure.rect, [72.0, 560.0, 532.0, 730.0]);
    }

    #[test]
    fn a_caption_with_nothing_drawn_near_it_gets_no_rectangle_and_is_counted() {
        let content =
            [prose(72.0, 700.0, 3), text(72.0, 660.0, 9.0, "Figure 6. Nothing is drawn here."), prose(72.0, 640.0, 3)]
                .concat();
        let figures = figures_of(LETTER, &content);
        assert!(figures.placed.is_empty(), "{figures:?}");
        assert_eq!(figures.missed, 1);
    }

    #[test]
    fn the_page_box_and_its_rotation_are_said_as_the_pdf_writes_them() {
        let content = [
            prose(72.0, 700.0, 3),
            image(100.0, 450.0, 300.0, 190.0),
            text(72.0, 430.0, 9.0, "Figure 7. On a page turned a quarter."),
            prose(72.0, 400.0, 3),
        ]
        .concat();
        let figures = figures_of("/MediaBox [0 0 612 792] /CropBox [10 20 602 782] /Rotate 90", &content);
        let [figure] = figures.placed.as_slice() else { panic!("{figures:?}") };
        assert_eq!(figure.page_box, PageBox { crop: Rect { x0: 10.0, y0: 20.0, x1: 602.0, y1: 782.0 }, rotation: 90 });
        near(figure.rect, [100.0, 450.0, 400.0, 640.0]);
    }

    #[test]
    fn caption_labels_are_read_as_the_paper_writes_them() {
        assert_eq!(caption_label("Figure 1. Left: protocols"), Some(("Figure 1".into(), FigureKind::Figure)));
        assert_eq!(caption_label("Fig. 12: the loss"), Some(("Figure 12".into(), FigureKind::Figure)));
        assert_eq!(caption_label("Table A3. More results"), Some(("Table A3".into(), FigureKind::Table)));
        assert_eq!(caption_label("Figure 1 shows the loss"), None, "a sentence that names a figure");
    }
}
