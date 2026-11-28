//! What each slot of TeX's math fonts holds, for the fonts whose own names for their glyphs say nothing (or say
//! something wrong): the last way a math font's text is read, and only for a font whose name says it is one of
//! these families ([`family`]), never for ordinary text.
//!
//! The tables come from the TeX font encodings themselves:
//!
//! - OMS (math symbols: `cmsy`, and the fonts made to its layout: `txsy`, `pxsy`, `ntxsy`, MathTime's `mtsy`):
//!   Knuth, *Computer Modern Typefaces* (cmsy), as LaTeX declares it in `omsenc.def` and `fontmath.ltx`.
//! - OML (math italic: `cmmi`, `txmi`, `pxmi`, `ntxmi`, `mtmi`): Knuth's cmmi, `omlenc.def` and `fontmath.ltx`.
//! - OMX (math extension: `cmex`, `txex`, `pxex`, `ntxexx`, `mtex`): Knuth's cmex, `omxenc.def` and `fontmath.ltx`.
//! - mathabx (`matha`, `mathb`, `mathx`, embedded as `TeX-matha10` and so on): Anthony Phan's `mathabx.dcl`
//!   (version of May 18, 2005), which declares each symbol's slot, and for `mathx` its `mathxdrv.mf` and
//!   `mathacnt.mf`, which place the larger sizes of each delimiter, operator and wide accent. These fonts are the reason this exists: their Type 1 files
//!   name every glyph after the letter or accent of the Cork (T1) text encoding at the same slot ("p" for the left
//!   parenthesis, "dcaron" for less-or-equal), so a ToUnicode map made from those names, as pdfTeX makes one,
//!   turns every formula into accented letters.
//!
//! A slot that holds one piece of a large delimiter built of several (the middle or bottom of a tall brace) is
//! [`DROPPED`]: the delimiter is read once, from its top piece. A slot not in a table is not known, and what is drawn
//! there is unreadable, not guessed.

/// A glyph that is a drawn part of another one, left out of the text.
pub(super) const DROPPED: &str = "\u{FDD1}";

/// A family of TeX math fonts, each with the table of what its slots hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Family {
    Symbols,
    Italic,
    Extension,
    MathA,
    MathB,
    MathX,
}

impl Family {
    /// What slot `code` of a font of this family holds, when known.
    pub(super) fn slot(self, code: u8) -> Option<&'static str> {
        let table = match self {
            Self::Symbols => OMS,
            Self::Italic => OML,
            Self::Extension => OMX,
            Self::MathA => MATHA,
            Self::MathB => MATHB,
            Self::MathX => MATHX,
        };
        table.binary_search_by_key(&code, |(slot, _)| *slot).ok().map(|at| table[at].1)
    }

    /// Do the font's own glyph names (and so a ToUnicode map made from them) say nothing true of what it draws?
    /// The mathabx fonts name their glyphs after a text encoding's letters.
    pub(super) fn names_are_wrong(self) -> bool {
        matches!(self, Self::MathA | Self::MathB | Self::MathX)
    }
}

/// The family of the font named `base` (its `BaseFont`, with or without the subset tag "ABCDEF+"), if it is one of
/// TeX's math fonts whose layout is known. Sizes and weights follow the stem: `cmsy10`, `cmbsy7`, `TeX-matha12`.
pub(super) fn family(base: &str) -> Option<Family> {
    let name = base.split_once('+').map_or(base, |(_, name)| name);
    let stem = name.strip_prefix("TeX-").unwrap_or(name).to_ascii_lowercase();
    let stem = stem.trim_end_matches(|c: char| c.is_ascii_digit() || c == '-' || c == '_');
    let starts = |prefixes: &[&str]| prefixes.iter().any(|prefix| stem.starts_with(prefix));
    // the longer names first: "cmmib" is italic, "cmbsy" symbols, "ntxexx" extension
    if starts(&["matha"]) {
        Some(Family::MathA)
    } else if starts(&["mathb"]) {
        Some(Family::MathB)
    } else if starts(&["mathx"]) {
        Some(Family::MathX)
    } else if starts(&[
        "cmsy",
        "cmbsy",
        "lmsy",
        "lmbsy",
        "lmmathsymbols",
        "txsy",
        "txbsy",
        "pxsy",
        "pxbsy",
        "ntxsy",
        "ntxbsy",
        "npxsy",
        "npxbsy",
        "mtsy",
    ]) {
        Some(Family::Symbols)
    } else if starts(&[
        "cmmi",
        "lmmi",
        "lmmathitalic",
        "txmi",
        "txbmi",
        "pxmi",
        "pxbmi",
        "ntxmi",
        "ntxbmi",
        "npxmi",
        "npxbmi",
        "mtmi",
    ]) {
        Some(Family::Italic)
    } else if starts(&[
        "cmex",
        "lmex",
        "lmmathextension",
        "txex",
        "txbex",
        "pxex",
        "pxbex",
        "ntxex",
        "ntxbex",
        "npxex",
        "npxbex",
        "mtex",
    ]) {
        Some(Family::Extension)
    } else {
        None
    }
}

/// OMS, the math symbols (cmsy). The calligraphic capitals are read as plain capitals, as a reader would type them.
#[rustfmt::skip]
const OMS: &[(u8, &str)] = &[
    (0x00, "\u{2212}"), (0x01, "·"), (0x02, "×"), (0x03, "∗"), (0x04, "÷"), (0x05, "⋄"), (0x06, "±"), (0x07, "∓"),
    (0x08, "⊕"), (0x09, "⊖"), (0x0A, "⊗"), (0x0B, "⊘"), (0x0C, "⊙"), (0x0D, "◯"), (0x0E, "∘"), (0x0F, "∙"),
    (0x10, "≍"), (0x11, "≡"), (0x12, "⊆"), (0x13, "⊇"), (0x14, "≤"), (0x15, "≥"), (0x16, "⪯"), (0x17, "⪰"),
    (0x18, "∼"), (0x19, "≈"), (0x1A, "⊂"), (0x1B, "⊃"), (0x1C, "≪"), (0x1D, "≫"), (0x1E, "≺"), (0x1F, "≻"),
    (0x20, "←"), (0x21, "→"), (0x22, "↑"), (0x23, "↓"), (0x24, "↔"), (0x25, "↗"), (0x26, "↘"), (0x27, "≃"),
    (0x28, "⇐"), (0x29, "⇒"), (0x2A, "⇑"), (0x2B, "⇓"), (0x2C, "⇔"), (0x2D, "↖"), (0x2E, "↙"), (0x2F, "∝"),
    (0x30, "′"), (0x31, "∞"), (0x32, "∈"), (0x33, "∋"), (0x34, "△"), (0x35, "▽"), (0x36, "/"), (0x37, DROPPED),
    (0x38, "∀"), (0x39, "∃"), (0x3A, "¬"), (0x3B, "∅"), (0x3C, "ℜ"), (0x3D, "ℑ"), (0x3E, "⊤"), (0x3F, "⊥"),
    (0x40, "ℵ"), (0x41, "A"), (0x42, "B"), (0x43, "C"), (0x44, "D"), (0x45, "E"), (0x46, "F"), (0x47, "G"),
    (0x48, "H"), (0x49, "I"), (0x4A, "J"), (0x4B, "K"), (0x4C, "L"), (0x4D, "M"), (0x4E, "N"), (0x4F, "O"),
    (0x50, "P"), (0x51, "Q"), (0x52, "R"), (0x53, "S"), (0x54, "T"), (0x55, "U"), (0x56, "V"), (0x57, "W"),
    (0x58, "X"), (0x59, "Y"), (0x5A, "Z"), (0x5B, "∪"), (0x5C, "∩"), (0x5D, "⊎"), (0x5E, "∧"), (0x5F, "∨"),
    (0x60, "⊢"), (0x61, "⊣"), (0x62, "⌊"), (0x63, "⌋"), (0x64, "⌈"), (0x65, "⌉"), (0x66, "{"), (0x67, "}"),
    (0x68, "⟨"), (0x69, "⟩"), (0x6A, "|"), (0x6B, "‖"), (0x6C, "↕"), (0x6D, "⇕"), (0x6E, "\\"), (0x6F, "≀"),
    (0x70, "√"), (0x71, "⨿"), (0x72, "∇"), (0x73, "∫"), (0x74, "⊔"), (0x75, "⊓"), (0x76, "⊑"), (0x77, "⊒"),
    (0x78, "§"), (0x79, "†"), (0x7A, "‡"), (0x7B, "¶"), (0x7C, "♣"), (0x7D, "♢"), (0x7E, "♡"), (0x7F, "♠"),
];

/// OML, math italic (cmmi). The old-style digits are digits; the hooks and the tie, parts of other symbols, are
/// dropped.
#[rustfmt::skip]
const OML: &[(u8, &str)] = &[
    (0x00, "Γ"), (0x01, "Δ"), (0x02, "Θ"), (0x03, "Λ"), (0x04, "Ξ"), (0x05, "Π"), (0x06, "Σ"), (0x07, "Υ"),
    (0x08, "Φ"), (0x09, "Ψ"), (0x0A, "Ω"), (0x0B, "α"), (0x0C, "β"), (0x0D, "γ"), (0x0E, "δ"), (0x0F, "ϵ"),
    (0x10, "ζ"), (0x11, "η"), (0x12, "θ"), (0x13, "ι"), (0x14, "κ"), (0x15, "λ"), (0x16, "μ"), (0x17, "ν"),
    (0x18, "ξ"), (0x19, "π"), (0x1A, "ρ"), (0x1B, "σ"), (0x1C, "τ"), (0x1D, "υ"), (0x1E, "ϕ"), (0x1F, "χ"),
    (0x20, "ψ"), (0x21, "ω"), (0x22, "ε"), (0x23, "ϑ"), (0x24, "ϖ"), (0x25, "ϱ"), (0x26, "ς"), (0x27, "φ"),
    (0x28, "↼"), (0x29, "↽"), (0x2A, "⇀"), (0x2B, "⇁"), (0x2C, DROPPED), (0x2D, DROPPED), (0x2E, "▹"), (0x2F, "◃"),
    (0x30, "0"), (0x31, "1"), (0x32, "2"), (0x33, "3"), (0x34, "4"), (0x35, "5"), (0x36, "6"), (0x37, "7"),
    (0x38, "8"), (0x39, "9"), (0x3A, "."), (0x3B, ","), (0x3C, "<"), (0x3D, "/"), (0x3E, ">"), (0x3F, "⋆"),
    (0x40, "∂"), (0x41, "A"), (0x42, "B"), (0x43, "C"), (0x44, "D"), (0x45, "E"), (0x46, "F"), (0x47, "G"),
    (0x48, "H"), (0x49, "I"), (0x4A, "J"), (0x4B, "K"), (0x4C, "L"), (0x4D, "M"), (0x4E, "N"), (0x4F, "O"),
    (0x50, "P"), (0x51, "Q"), (0x52, "R"), (0x53, "S"), (0x54, "T"), (0x55, "U"), (0x56, "V"), (0x57, "W"),
    (0x58, "X"), (0x59, "Y"), (0x5A, "Z"), (0x5B, "♭"), (0x5C, "♮"), (0x5D, "♯"), (0x5E, "⌣"), (0x5F, "⌢"),
    (0x60, "ℓ"), (0x61, "a"), (0x62, "b"), (0x63, "c"), (0x64, "d"), (0x65, "e"), (0x66, "f"), (0x67, "g"),
    (0x68, "h"), (0x69, "i"), (0x6A, "j"), (0x6B, "k"), (0x6C, "l"), (0x6D, "m"), (0x6E, "n"), (0x6F, "o"),
    (0x70, "p"), (0x71, "q"), (0x72, "r"), (0x73, "s"), (0x74, "t"), (0x75, "u"), (0x76, "v"), (0x77, "w"),
    (0x78, "x"), (0x79, "y"), (0x7A, "z"), (0x7B, "ı"), (0x7C, "ȷ"), (0x7D, "℘"), (0x7E, DROPPED), (0x7F, DROPPED),
];

/// OMX, the math extension (cmex): delimiters in four sizes and as pieces, big operators in text and display size,
/// wide accents and radicals. A tall delimiter's top piece reads as the delimiter, its other pieces are dropped.
#[rustfmt::skip]
const OMX: &[(u8, &str)] = &[
    (0x00, "("), (0x01, ")"), (0x02, "["), (0x03, "]"), (0x04, "⌊"), (0x05, "⌋"), (0x06, "⌈"), (0x07, "⌉"),
    (0x08, "{"), (0x09, "}"), (0x0A, "⟨"), (0x0B, "⟩"), (0x0C, "|"), (0x0D, "‖"), (0x0E, "/"), (0x0F, "\\"),
    (0x10, "("), (0x11, ")"), (0x12, "("), (0x13, ")"), (0x14, "["), (0x15, "]"), (0x16, "⌊"), (0x17, "⌋"),
    (0x18, "⌈"), (0x19, "⌉"), (0x1A, "{"), (0x1B, "}"), (0x1C, "⟨"), (0x1D, "⟩"), (0x1E, "/"), (0x1F, "\\"),
    (0x20, "("), (0x21, ")"), (0x22, "["), (0x23, "]"), (0x24, "⌊"), (0x25, "⌋"), (0x26, "⌈"), (0x27, "⌉"),
    (0x28, "{"), (0x29, "}"), (0x2A, "⟨"), (0x2B, "⟩"), (0x2C, "/"), (0x2D, "\\"), (0x2E, "/"), (0x2F, "\\"),
    (0x30, "("), (0x31, ")"), (0x32, "["), (0x33, "]"), (0x34, DROPPED), (0x35, DROPPED), (0x36, DROPPED),
    (0x37, DROPPED), (0x38, "{"), (0x39, "}"), (0x3A, DROPPED), (0x3B, DROPPED), (0x3C, DROPPED), (0x3D, DROPPED),
    (0x3E, DROPPED), (0x3F, DROPPED), (0x40, DROPPED), (0x41, DROPPED), (0x42, DROPPED), (0x43, DROPPED),
    (0x44, "⟨"), (0x45, "⟩"), (0x46, "⨆"), (0x47, "⨆"), (0x48, "∮"), (0x49, "∮"), (0x4A, "⨀"), (0x4B, "⨀"),
    (0x4C, "⨁"), (0x4D, "⨁"), (0x4E, "⨂"), (0x4F, "⨂"), (0x50, "∑"), (0x51, "∏"), (0x52, "∫"), (0x53, "⋃"),
    (0x54, "⋂"), (0x55, "⨄"), (0x56, "⋀"), (0x57, "⋁"), (0x58, "∑"), (0x59, "∏"), (0x5A, "∫"), (0x5B, "⋃"),
    (0x5C, "⋂"), (0x5D, "⨄"), (0x5E, "⋀"), (0x5F, "⋁"), (0x60, "∐"), (0x61, "∐"), (0x62, "^"), (0x63, "^"),
    (0x64, "^"), (0x65, "~"), (0x66, "~"), (0x67, "~"), (0x68, "["), (0x69, "]"), (0x6A, "⌊"), (0x6B, "⌋"),
    (0x6C, "⌈"), (0x6D, "⌉"), (0x6E, "{"), (0x6F, "}"), (0x70, "√"), (0x71, "√"), (0x72, "√"), (0x73, "√"),
    (0x74, "√"), (0x75, DROPPED), (0x76, DROPPED), (0x77, DROPPED), (0x78, "↑"), (0x79, "↓"), (0x7A, DROPPED),
    (0x7B, DROPPED), (0x7C, DROPPED), (0x7D, DROPPED), (0x7E, "⇑"), (0x7F, "⇓"),
];

// mathabx, made from mathabx.dcl, mathxdrv.mf and mathacnt.mf (see above)
/// matha: operators, relations, arrows, the small delimiters.
#[rustfmt::skip]
const MATHA: &[(u8, &str)] = &[
    (0x00, "+"), (0x01, "\u{2212}"), (0x02, "×"), (0x03, "÷"), (0x04, "·"), (0x05, "∘"), (0x06, "∗"),
    (0x07, "⊛"), (0x08, "±"), (0x09, "∓"), (0x0A, "⋉"), (0x0B, "⋊"), (0x0C, "⋄"), (0x0D, "∙"), (0x0E, "⋆"),
    (0x0F, "✶"), (0x10, "="), (0x11, "≡"), (0x12, "∼"), (0x13, "≈"), (0x14, "≃"), (0x15, "≅"), (0x16, "≍"),
    (0x17, "∣"), (0x18, "≠"), (0x19, "≢"), (0x1A, "≁"), (0x1B, "≉"), (0x1C, "≄"), (0x1D, "≇"), (0x1E, "≭"),
    (0x1F, "∤"), (0x20, "¬"), (0x21, "≪"), (0x22, "≫"), (0x23, "#"), (0x24, "⊢"), (0x25, "⊣"), (0x26, "⊬"),
    (0x28, "⊨"), (0x2A, "⊭"), (0x2C, "⊩"), (0x2E, "⊮"), (0x30, "°"), (0x31, "′"), (0x32, "″"), (0x33, "‴"),
    (0x34, "⁗"), (0x35, "♭"), (0x36, "♮"), (0x37, "♯"), (0x38, "∞"), (0x39, "∝"), (0x3A, "†"), (0x3B, "‡"),
    (0x3C, "∑"), (0x3D, "∏"), (0x3E, "⨿"), (0x3F, "√"), (0x40, "∀"), (0x41, "∁"), (0x42, "∂"), (0x44, "∃"),
    (0x45, "∄"), (0x46, "Ⅎ"), (0x47, "⅁"), (0x48, "∅"), (0x49, "⌀"), (0x4A, "⊤"), (0x4B, "⊥"), (0x4E, "⋏"),
    (0x4F, "⋎"), (0x50, "∈"), (0x51, "∋"), (0x52, "∉"), (0x53, "∌"), (0x54, "∉"), (0x55, "∌"), (0x58, "∩"),
    (0x59, "∪"), (0x5A, "⊎"), (0x5B, "⊓"), (0x5C, "⊔"), (0x5E, "∧"), (0x5F, "∨"), (0x60, "⊕"), (0x61, "⊖"),
    (0x62, "⊗"), (0x63, "⨸"), (0x64, "⊙"), (0x65, "⊚"), (0x66, "⊛"), (0x6D, "⊘"), (0x6E, "⦸"), (0x70, "("),
    (0x71, ")"), (0x72, "["), (0x73, "]"), (0x74, "{"), (0x75, "}"), (0x76, "⟦"), (0x77, "⟧"), (0x78, "⟨"),
    (0x79, "⟩"), (0x7A, "∖"), (0x7B, "/"), (0x7C, "|"), (0x7D, "‖"), (0x7E, "⦀"), (0x7F, "¬"), (0x80, "⊂"),
    (0x81, "⊃"), (0x82, "⊄"), (0x83, "⊅"), (0x84, "⊆"), (0x85, "⊇"), (0x86, "⊈"), (0x87, "⊉"), (0x88, "⊊"),
    (0x89, "⊋"), (0x8A, "⊊"), (0x8B, "⊋"), (0x8C, "⫅"), (0x8D, "⫆"), (0x90, "⫋"), (0x91, "⫌"), (0x92, "⫋"),
    (0x93, "⫌"), (0x94, "⋐"), (0x95, "⋑"), (0x98, "◁"), (0x99, "▷"), (0x9A, "⋪"), (0x9B, "⋫"), (0x9C, "⊴"),
    (0x9D, "⊵"), (0x9E, "⋬"), (0x9F, "⋭"), (0xA0, "<"), (0xA1, ">"), (0xA2, "≮"), (0xA3, "≯"), (0xA4, "≤"),
    (0xA5, "≥"), (0xA6, "≰"), (0xA7, "≱"), (0xA8, "⩽"), (0xA9, "⩾"), (0xAC, "⪇"), (0xAD, "⪈"), (0xAE, "≦"),
    (0xAF, "≧"), (0xB2, "≨"), (0xB3, "≩"), (0xB6, "⪕"), (0xB7, "⪖"), (0xBA, "≶"), (0xBB, "≷"), (0xBC, "⋚"),
    (0xBD, "⋛"), (0xBE, "⪋"), (0xBF, "⪌"), (0xC0, "≲"), (0xC1, "≳"), (0xC2, "≴"), (0xC3, "≵"), (0xC4, "⋦"),
    (0xC5, "⋧"), (0xC6, "⪅"), (0xC7, "⪆"), (0xCA, "⪉"), (0xCB, "⪊"), (0xCC, "⋖"), (0xCD, "⋗"), (0xCE, "⋘"),
    (0xCF, "⋙"), (0xD0, "←"), (0xD1, "→"), (0xD2, "↑"), (0xD3, "↓"), (0xD4, "↖"), (0xD5, "↗"), (0xD6, "↙"),
    (0xD7, "↘"), (0xD8, "↔"), (0xD9, "↕"), (0xDA, "↚"), (0xDB, "↛"), (0xDC, "↮"), (0xDD, "\u{2212}"),
    (0xE0, "↼"), (0xE1, "⇀"), (0xE2, "↽"), (0xE3, "⇁"), (0xE4, "↿"), (0xE5, "⇃"), (0xE6, "↾"), (0xE7, "⇂"),
    (0xE8, "⇋"), (0xE9, "⇌"), (0xF0, "⇐"), (0xF1, "⇒"), (0xF2, "⇑"), (0xF3, "⇓"), (0xF4, "⇔"), (0xF5, "⇕"),
    (0xF6, "⇍"), (0xF7, "⇏"), (0xF8, "⇎"), (0xF9, "="),
];
/// mathb: more operators and relations, small accents, the corners, ceilings and floors.
#[rustfmt::skip]
const MATHB: &[(u8, &str)] = &[
    (0x00, "∔"), (0x01, "∸"), (0x05, "□"), (0x06, "∗"), (0x08, "⊕"), (0x0B, "⋇"), (0x0C, "◆"), (0x0D, "▪"),
    (0x0E, "★"), (0x12, "≑"), (0x13, "≓"), (0x14, "≒"), (0x15, "≔"), (0x16, "≕"), (0x17, "≏"), (0x19, "≎"),
    (0x1A, "≗"), (0x1B, "≖"), (0x1C, "≜"), (0x20, "≬"), (0x21, "⌣"), (0x22, "⌢"), (0x24, "⋋"), (0x25, "⋌"),
    (0x26, "⋔"), (0x27, "⋈"), (0x28, "⊫"), (0x2A, "⊯"), (0x2C, "⊪"), (0x36, "∴"), (0x37, "∵"), (0x38, "˚"),
    (0x39, "˙"), (0x3A, "¨"), (0x3D, "∠"), (0x3E, "∡"), (0x3F, "∢"), (0x58, "⊼"), (0x59, "⊻"), (0x5A, "⩞"),
    (0x5C, "⋒"), (0x5D, "⋓"), (0x60, "⊞"), (0x61, "⊟"), (0x62, "⊠"), (0x64, "⊡"), (0x6D, "⧄"), (0x6E, "⧅"),
    (0x70, "⟮"), (0x71, "⟯"), (0x72, "⌈"), (0x73, "⌉"), (0x74, "⌊"), (0x75, "⌋"), (0x78, "⌜"), (0x79, "⌝"),
    (0x7A, "⌞"), (0x7B, "⌟"), (0x7E, "|"), (0x7F, "¬"), (0x80, "⊏"), (0x81, "⊐"), (0x84, "⊑"), (0x85, "⊒"),
    (0x86, "⋢"), (0x87, "⋣"), (0x88, "⋤"), (0x89, "⋥"), (0x98, "▵"), (0x99, "▿"), (0x9A, "◃"), (0x9B, "▹"),
    (0x9C, "▴"), (0x9D, "▾"), (0x9E, "◂"), (0x9F, "▸"), (0xA0, "≺"), (0xA1, "≻"), (0xA2, "⊀"), (0xA3, "⊁"),
    (0xA4, "≼"), (0xA5, "≽"), (0xA6, "⋠"), (0xA7, "⋡"), (0xA8, "⪯"), (0xA9, "⪰"), (0xAC, "⪱"), (0xAD, "⪲"),
    (0xB6, "⋞"), (0xB7, "⋟"), (0xC0, "≾"), (0xC1, "≿"), (0xC4, "⋨"), (0xC5, "⋩"), (0xC6, "⪷"), (0xC7, "⪸"),
    (0xCA, "⪹"), (0xCB, "⪺"), (0xD0, "⇇"), (0xD1, "⇉"), (0xD2, "⇈"), (0xD3, "⇊"), (0xD4, "⇆"), (0xD5, "⇄"),
    (0xE4, "╱"), (0xE5, "╲"), (0xE8, "↰"), (0xE9, "↱"), (0xEC, "↫"), (0xED, "↬"), (0xF0, "↶"), (0xF1, "↷"),
    (0xF6, "↺"), (0xF7, "↻"), (0xF8, "⇜"), (0xF9, "⇝"), (0xFA, "↭"),
];
/// mathx: delimiters in their larger sizes (`mathxdrv.mf`), wide accents (`mathacnt.mf`), big operators and
/// radicals.
#[rustfmt::skip]
const MATHX: &[(u8, &str)] = &[
    (0x00, "("), (0x01, "("), (0x02, "("), (0x03, "("), (0x04, "("), (0x05, DROPPED), (0x06, DROPPED),
    (0x07, DROPPED), (0x08, ")"), (0x09, ")"), (0x0A, ")"), (0x0B, ")"), (0x0C, ")"), (0x0D, DROPPED),
    (0x0E, DROPPED), (0x0F, DROPPED), (0x10, "["), (0x11, "["), (0x12, "["), (0x13, "["), (0x14, "["),
    (0x15, DROPPED), (0x16, DROPPED), (0x17, DROPPED), (0x18, "]"), (0x19, "]"), (0x1A, "]"),
    (0x1B, "]"), (0x1C, "]"), (0x1D, DROPPED), (0x1E, DROPPED), (0x1F, DROPPED), (0x20, "{"),
    (0x21, "{"), (0x22, "{"), (0x23, "{"), (0x24, "{"), (0x25, DROPPED), (0x26, DROPPED),
    (0x27, DROPPED), (0x28, "}"), (0x29, "}"), (0x2A, "}"), (0x2B, "}"), (0x2C, "}"), (0x2D, DROPPED),
    (0x2E, DROPPED), (0x2F, DROPPED), (0x30, "⟦"), (0x31, "⟦"), (0x32, "⟦"), (0x33, "⟦"), (0x34, "⟦"),
    (0x35, DROPPED), (0x36, DROPPED), (0x37, DROPPED), (0x38, "⟧"), (0x39, "⟧"), (0x3A, "⟧"),
    (0x3B, "⟧"), (0x3C, "⟧"), (0x3D, DROPPED), (0x3E, DROPPED), (0x3F, DROPPED), (0x40, "⟨"),
    (0x41, "⟨"), (0x42, "⟨"), (0x43, "⟨"), (0x44, "⟩"), (0x45, "⟩"), (0x46, "⟩"), (0x47, "⟩"), (0x48, "\\"),
    (0x49, "\\"), (0x4A, "\\"), (0x4B, "\\"), (0x4C, "/"), (0x4D, "/"), (0x4E, "/"), (0x4F, "/"), (0x50, "⌈"),
    (0x51, "⌈"), (0x52, "⌈"), (0x53, "⌈"), (0x54, "⌉"), (0x55, "⌉"), (0x56, "⌉"), (0x57, "⌉"), (0x58, "⌊"),
    (0x59, "⌊"), (0x5A, "⌊"), (0x5B, "⌊"), (0x5C, "⌋"), (0x5D, "⌋"), (0x5E, "⌋"), (0x5F, "⌋"), (0x60, "√"),
    (0x61, "√"), (0x62, "√"), (0x63, "√"), (0x64, "√"), (0x65, "√"), (0x66, DROPPED), (0x67, DROPPED),
    (0x68, DROPPED), (0x69, DROPPED), (0x6A, DROPPED), (0x6B, DROPPED), (0x6C, DROPPED),
    (0x6D, DROPPED), (0x6E, DROPPED), (0x6F, DROPPED), (0x70, "^"), (0x71, "ˇ"), (0x72, "~"),
    (0x73, "¯"), (0x74, "→"), (0x75, "⌒"), (0x76, "⟮"), (0x77, "⟯"), (0x78, "^"), (0x79, "^"), (0x7A, "^"),
    (0x7B, "^"), (0x7C, "ˇ"), (0x7D, "ˇ"), (0x7E, "ˇ"), (0x7F, "ˇ"), (0x80, "~"), (0x81, "~"), (0x82, "~"),
    (0x83, "~"), (0x84, "¯"), (0x85, "¯"), (0x86, "¯"), (0x87, "¯"), (0x88, "→"), (0x89, "→"), (0x8A, "→"),
    (0x8B, "→"), (0x8C, "⌒"), (0x8D, "⌒"), (0x8E, "⌒"), (0x8F, "⌒"), (0x90, "+"), (0x91, "×"), (0x93, "⋂"),
    (0x94, "⋃"), (0x95, "⨄"), (0x96, "⨅"), (0x97, "⨆"), (0x99, "⋀"), (0x9A, "⋁"), (0x9B, "⋀"), (0x9C, "⋁"),
    (0x9D, DROPPED), (0x9E, DROPPED), (0x9F, DROPPED), (0xA0, "+"), (0xA1, "×"), (0xA3, "⋂"),
    (0xA4, "⋃"), (0xA5, "⨄"), (0xA6, "⨅"), (0xA7, "⨆"), (0xA9, "⋀"), (0xAA, "⋁"), (0xAB, "⋀"), (0xAC, "⋁"),
    (0xAD, DROPPED), (0xAE, DROPPED), (0xAF, DROPPED), (0xB0, "∑"), (0xB1, "∏"), (0xB2, "∐"),
    (0xB3, "∫"), (0xB4, "∬"), (0xB5, "∭"), (0xB6, "∮"), (0xB7, "∯"), (0xB8, "∑"), (0xB9, "∏"), (0xBA, "∐"),
    (0xBB, "∫"), (0xBC, "∬"), (0xBD, "∭"), (0xBE, "∮"), (0xBF, "∯"), (0xC0, "⨁"), (0xC1, "⊖"), (0xC2, "⨂"),
    (0xC4, "⨀"), (0xCD, "⊘"), (0xD0, "⊞"), (0xD1, "⊟"), (0xD2, "⊠"), (0xD4, "⊡"), (0xE0, "⨁"), (0xE1, "⊖"),
    (0xE2, "⨂"), (0xE4, "⨀"), (0xED, "⊘"), (0xF0, "⊞"), (0xF1, "⊟"), (0xF2, "⊠"), (0xF4, "⊡"),
];
