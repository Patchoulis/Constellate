//! What a glyph's name says it is: the Adobe Glyph List's names, the `uniXXXX` and `uXXXX` names, and the names
//! TeX's fonts give their glyphs that the list does not have. Also the standard encoding of Type 1 fonts, the last
//! resort for a text font that says nothing of its own.
//!
//! [`AGLFN`] is the Adobe Glyph List For New Fonts, version 1.7 (https://github.com/adobe-type-tools/agl-aglfn),
//! made into a table; its notice is in `docs/licenses/third-party-notices.md`:
//!
//! > Copyright 2002-2019 Adobe (http://www.adobe.com/). Redistribution and use in source and binary forms, with or
//! > without modification, are permitted provided that the following conditions are met: Redistributions of source
//! > code must retain the above copyright notice, this list of conditions and the following disclaimer.
//! > Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
//! > following disclaimer in the documentation and/or other materials provided with the distribution. Neither the
//! > name of Adobe nor the names of its contributors may be used to endorse or promote products derived from this
//! > software without specific prior written permission. THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND
//! > CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
//! > WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE
//! > COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
//! > CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
//! > DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
//! > CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF
//! > THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
//!
//! [`STANDARD`], [`WIN_ANSI`] and [`MAC_ROMAN`] are the encodings of PDF 1.7, Annex D.2, by glyph name.

/// The glyph names of the base encoding `/Encoding` names, when it is one of those a font may name.
pub(super) fn named_encoding(name: &[u8]) -> Option<&'static [Option<&'static str>; 256]> {
    match name {
        b"StandardEncoding" => Some(&STANDARD),
        b"WinAnsiEncoding" => Some(&WIN_ANSI),
        b"MacRomanEncoding" => Some(&MAC_ROMAN),
        _ => None,
    }
}

/// What the glyph named `name` is, as text; `None` when its name does not say.
///
/// A name is first looked up whole; then as `uniXXXX` (one or more four-digit groups) or `uXXXX` to `uXXXXXX`
/// (Adobe Glyph List specification); then as one of TeX's names; then without a suffix after a full stop
/// ("a.sc", "one.oldstyle") or as a ligature of components ("f_f_i").
pub(super) fn text_of(name: &str) -> Option<String> {
    if let Some(found) = whole(name) {
        return Some(found);
    }
    let base = name.split('.').next().unwrap_or(name);
    if base != name && !base.is_empty() {
        return whole(base);
    }
    if base.contains('_') {
        return base.split('_').map(whole).collect::<Option<String>>();
    }
    None
}

fn whole(name: &str) -> Option<String> {
    if let Ok(at) = AGLFN.binary_search_by_key(&name, |(glyph, _)| glyph) {
        return Some(AGLFN[at].1.to_string());
    }
    if let Some(tex) = TEX.iter().find(|(glyph, _)| *glyph == name) {
        return Some(tex.1.to_owned());
    }
    let hex = |digits: &str| u32::from_str_radix(digits, 16).ok().and_then(char::from_u32);
    let upper_hex = |digits: &str| digits.chars().all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c));
    if let Some(groups) = name.strip_prefix("uni")
        && !groups.is_empty()
        && groups.len() % 4 == 0
        && upper_hex(groups)
    {
        return (0..groups.len()).step_by(4).map(|at| hex(&groups[at..at + 4])).collect();
    }
    if let Some(digits) = name.strip_prefix('u')
        && (4..=6).contains(&digits.len())
        && upper_hex(digits)
    {
        return hex(digits).map(String::from);
    }
    None
}

/// Names TeX's fonts (Computer Modern, AMS, txfonts and newtx) give glyphs that the Adobe list lacks or reads
/// otherwise. A size suffix (`big`, `Big`, `bigg`, `Bigg`, `text`, `display`) is part of the name in those fonts.
#[rustfmt::skip]
const TEX: &[(&str, &str)] = &[
    ("alpha1", "α"), ("epsilon1", "ε"), ("theta1", "ϑ"), ("pi1", "ϖ"), ("rho1", "ϱ"), ("sigma1", "ς"), ("phi1", "φ"),
    ("dotlessj", "ȷ"), ("ff", "ff"), ("ffi", "ffi"), ("ffl", "ffl"), ("fi", "fi"), ("fl", "fl"), ("minus", "\u{2212}"),
    ("periodcentered", "·"), ("asteriskmath", "∗"), ("diamondmath", "⋄"), ("circleplus", "⊕"), ("circleminus", "⊖"),
    ("circlemultiply", "⊗"), ("circledivide", "⊘"), ("circledot", "⊙"), ("openbullet", "∘"), ("equivasymptotic", "≍"),
    ("equivalence", "≡"), ("reflexsubset", "⊆"), ("reflexsuperset", "⊇"), ("precedesequal", "⪯"),
    ("followsequal", "⪰"), ("similar", "∼"), ("approxequal", "≈"), ("propersubset", "⊂"), ("propersuperset", "⊃"),
    ("lessmuch", "≪"), ("greatermuch", "≫"), ("precedes", "≺"), ("follows", "≻"), ("arrowboth", "↔"),
    ("arrownortheast", "↗"), ("arrowsoutheast", "↘"), ("similarequal", "≃"), ("arrowdblboth", "⇔"),
    ("arrownorthwest", "↖"), ("arrowsouthwest", "↙"), ("proportional", "∝"), ("prime", "′"), ("owner", "∋"),
    ("triangleinv", "▽"), ("negationslash", "/"), ("mapsto", "\u{FDD1}"), ("emptyset", "∅"), ("Rfractur", "ℜ"),
    ("Ifractur", "ℑ"), ("latticetop", "⊤"), ("unionmulti", "⊎"), ("turnstileleft", "⊢"), ("turnstileright", "⊣"),
    ("floorleft", "⌊"), ("floorright", "⌋"), ("ceilingleft", "⌈"), ("ceilingright", "⌉"), ("angbracketleft", "⟨"),
    ("angbracketright", "⟩"), ("bardbl", "‖"), ("arrowbothv", "↕"), ("arrowdblbothv", "⇕"), ("wreathproduct", "≀"),
    ("coproduct", "⨿"), ("unionsq", "⊔"), ("intersectionsq", "⊓"), ("subsetsqequal", "⊑"), ("supersetsqequal", "⊒"),
    ("lscript", "ℓ"), ("weierstrass", "℘"), ("slurbelow", "⌣"), ("slurabove", "⌢"), ("harpoonleftup", "↼"),
    ("harpoonleftdown", "↽"), ("harpoonrightup", "⇀"), ("harpoonrightdown", "⇁"), ("triangleright", "▹"),
    ("triangleleft", "◃"), ("star", "⋆"), ("flat", "♭"), ("natural", "♮"), ("sharp", "♯"), ("tie", "\u{FDD1}"),
    ("vector", "\u{FDD1}"), ("compwordmark", "\u{FDD1}"), ("visiblespace", "␣"),
    ("parenleftbig", "("), ("parenrightbig", ")"), ("parenleftBig", "("), ("parenrightBig", ")"),
    ("parenleftbigg", "("), ("parenrightbigg", ")"), ("parenleftBigg", "("), ("parenrightBigg", ")"),
    ("bracketleftbig", "["), ("bracketrightbig", "]"), ("bracketleftBig", "["), ("bracketrightBig", "]"),
    ("bracketleftbigg", "["), ("bracketrightbigg", "]"), ("bracketleftBigg", "["), ("bracketrightBigg", "]"),
    ("braceleftbig", "{"), ("bracerightbig", "}"), ("braceleftBig", "{"), ("bracerightBig", "}"),
    ("braceleftbigg", "{"), ("bracerightbigg", "}"), ("braceleftBigg", "{"), ("bracerightBigg", "}"),
    ("summationtext", "∑"), ("summationdisplay", "∑"), ("producttext", "∏"), ("productdisplay", "∏"),
    ("integraltext", "∫"), ("integraldisplay", "∫"), ("uniontext", "⋃"), ("uniondisplay", "⋃"),
    ("intersectiontext", "⋂"), ("intersectiondisplay", "⋂"), ("logicalandtext", "⋀"), ("logicalanddisplay", "⋀"),
    ("logicalortext", "⋁"), ("logicalordisplay", "⋁"), ("coproducttext", "∐"), ("coproductdisplay", "∐"),
    ("circleplustext", "⨁"), ("circleplusdisplay", "⨁"), ("circlemultiplytext", "⨂"),
    ("circlemultiplydisplay", "⨂"), ("circledottext", "⨀"), ("circledotdisplay", "⨀"), ("contintegraltext", "∮"),
    ("contintegraldisplay", "∮"), ("radicalbig", "√"), ("radicalBig", "√"), ("radicalbigg", "√"),
    ("radicalBigg", "√"), ("radicalbt", "√"), ("radicalvertex", "\u{FDD1}"), ("radicaltp", "\u{FDD1}"),
    // cmex's wide accents, drawn over what follows them: their spacing forms
    ("hatwide", "^"), ("hatwider", "^"), ("hatwidest", "^"), ("tildewide", "~"), ("tildewider", "~"),
    ("tildewidest", "~"),
    // the AMS symbol fonts (msam, msbm)
    ("followsorcurly", "≽"), ("precedesorcurly", "≼"), ("lessorequalslant", "⩽"), ("greaterorequalslant", "⩾"),
    ("lessorsimilar", "≲"), ("greaterorsimilar", "≳"), ("lessorgreater", "≶"), ("greaterorless", "≷"),
    ("defines", "≜"), ("square", "□"), ("squaresolid", "■"), ("checkmark", "✓"), ("measuredangle", "∡"),
    ("because", "∵"), ("dotplus", "∔"), ("lessornotequal", "≨"), ("greaterornotequal", "≩"),
    ("notlessequal", "≰"), ("notgreaterequal", "≱"), ("notless", "≮"), ("notgreater", "≯"),
];

#[rustfmt::skip]
pub(super) const AGLFN: &[(&str, char)] = &[
    ("A", 'A'), ("AE", 'Æ'), ("AEacute", 'Ǽ'), ("Aacute", 'Á'), ("Abreve", 'Ă'), ("Acircumflex", 'Â'),
    ("Adieresis", 'Ä'), ("Agrave", 'À'), ("Alpha", 'Α'), ("Alphatonos", 'Ά'), ("Amacron", 'Ā'), ("Aogonek", 'Ą'),
    ("Aring", 'Å'), ("Aringacute", 'Ǻ'), ("Atilde", 'Ã'), ("B", 'B'), ("Beta", 'Β'), ("C", 'C'), ("Cacute", 'Ć'),
    ("Ccaron", 'Č'), ("Ccedilla", 'Ç'), ("Ccircumflex", 'Ĉ'), ("Cdotaccent", 'Ċ'), ("Chi", 'Χ'), ("D", 'D'),
    ("Dcaron", 'Ď'), ("Dcroat", 'Đ'), ("Delta", '∆'), ("E", 'E'), ("Eacute", 'É'), ("Ebreve", 'Ĕ'),
    ("Ecaron", 'Ě'), ("Ecircumflex", 'Ê'), ("Edieresis", 'Ë'), ("Edotaccent", 'Ė'), ("Egrave", 'È'),
    ("Emacron", 'Ē'), ("Eng", 'Ŋ'), ("Eogonek", 'Ę'), ("Epsilon", 'Ε'), ("Epsilontonos", 'Έ'), ("Eta", 'Η'),
    ("Etatonos", 'Ή'), ("Eth", 'Ð'), ("Euro", '€'), ("F", 'F'), ("G", 'G'), ("Gamma", 'Γ'), ("Gbreve", 'Ğ'),
    ("Gcaron", 'Ǧ'), ("Gcircumflex", 'Ĝ'), ("Gdotaccent", 'Ġ'), ("H", 'H'), ("H18533", '●'), ("H18543", '▪'),
    ("H18551", '▫'), ("H22073", '□'), ("Hbar", 'Ħ'), ("Hcircumflex", 'Ĥ'), ("I", 'I'), ("IJ", 'Ĳ'),
    ("Iacute", 'Í'), ("Ibreve", 'Ĭ'), ("Icircumflex", 'Î'), ("Idieresis", 'Ï'), ("Idotaccent", 'İ'),
    ("Ifraktur", 'ℑ'), ("Igrave", 'Ì'), ("Imacron", 'Ī'), ("Iogonek", 'Į'), ("Iota", 'Ι'), ("Iotadieresis", 'Ϊ'),
    ("Iotatonos", 'Ί'), ("Itilde", 'Ĩ'), ("J", 'J'), ("Jcircumflex", 'Ĵ'), ("K", 'K'), ("Kappa", 'Κ'), ("L", 'L'),
    ("Lacute", 'Ĺ'), ("Lambda", 'Λ'), ("Lcaron", 'Ľ'), ("Ldot", 'Ŀ'), ("Lslash", 'Ł'), ("M", 'M'), ("Mu", 'Μ'),
    ("N", 'N'), ("Nacute", 'Ń'), ("Ncaron", 'Ň'), ("Ntilde", 'Ñ'), ("Nu", 'Ν'), ("O", 'O'), ("OE", 'Œ'),
    ("Oacute", 'Ó'), ("Obreve", 'Ŏ'), ("Ocircumflex", 'Ô'), ("Odieresis", 'Ö'), ("Ograve", 'Ò'), ("Ohorn", 'Ơ'),
    ("Ohungarumlaut", 'Ő'), ("Omacron", 'Ō'), ("Omega", 'Ω'), ("Omegatonos", 'Ώ'), ("Omicron", 'Ο'),
    ("Omicrontonos", 'Ό'), ("Oslash", 'Ø'), ("Oslashacute", 'Ǿ'), ("Otilde", 'Õ'), ("P", 'P'), ("Phi", 'Φ'),
    ("Pi", 'Π'), ("Psi", 'Ψ'), ("Q", 'Q'), ("R", 'R'), ("Racute", 'Ŕ'), ("Rcaron", 'Ř'), ("Rfraktur", 'ℜ'),
    ("Rho", 'Ρ'), ("S", 'S'), ("SF010000", '┌'), ("SF020000", '└'), ("SF030000", '┐'), ("SF040000", '┘'),
    ("SF050000", '┼'), ("SF060000", '┬'), ("SF070000", '┴'), ("SF080000", '├'), ("SF090000", '┤'),
    ("SF100000", '─'), ("SF110000", '│'), ("SF190000", '╡'), ("SF200000", '╢'), ("SF210000", '╖'),
    ("SF220000", '╕'), ("SF230000", '╣'), ("SF240000", '║'), ("SF250000", '╗'), ("SF260000", '╝'),
    ("SF270000", '╜'), ("SF280000", '╛'), ("SF360000", '╞'), ("SF370000", '╟'), ("SF380000", '╚'),
    ("SF390000", '╔'), ("SF400000", '╩'), ("SF410000", '╦'), ("SF420000", '╠'), ("SF430000", '═'),
    ("SF440000", '╬'), ("SF450000", '╧'), ("SF460000", '╨'), ("SF470000", '╤'), ("SF480000", '╥'),
    ("SF490000", '╙'), ("SF500000", '╘'), ("SF510000", '╒'), ("SF520000", '╓'), ("SF530000", '╫'),
    ("SF540000", '╪'), ("Sacute", 'Ś'), ("Scaron", 'Š'), ("Scedilla", 'Ş'), ("Scircumflex", 'Ŝ'), ("Sigma", 'Σ'),
    ("T", 'T'), ("Tau", 'Τ'), ("Tbar", 'Ŧ'), ("Tcaron", 'Ť'), ("Theta", 'Θ'), ("Thorn", 'Þ'), ("U", 'U'),
    ("Uacute", 'Ú'), ("Ubreve", 'Ŭ'), ("Ucircumflex", 'Û'), ("Udieresis", 'Ü'), ("Ugrave", 'Ù'), ("Uhorn", 'Ư'),
    ("Uhungarumlaut", 'Ű'), ("Umacron", 'Ū'), ("Uogonek", 'Ų'), ("Upsilon", 'Υ'), ("Upsilon1", 'ϒ'),
    ("Upsilondieresis", 'Ϋ'), ("Upsilontonos", 'Ύ'), ("Uring", 'Ů'), ("Utilde", 'Ũ'), ("V", 'V'), ("W", 'W'),
    ("Wacute", 'Ẃ'), ("Wcircumflex", 'Ŵ'), ("Wdieresis", 'Ẅ'), ("Wgrave", 'Ẁ'), ("X", 'X'), ("Xi", 'Ξ'),
    ("Y", 'Y'), ("Yacute", 'Ý'), ("Ycircumflex", 'Ŷ'), ("Ydieresis", 'Ÿ'), ("Ygrave", 'Ỳ'), ("Z", 'Z'),
    ("Zacute", 'Ź'), ("Zcaron", 'Ž'), ("Zdotaccent", 'Ż'), ("Zeta", 'Ζ'), ("a", 'a'), ("aacute", 'á'),
    ("abreve", 'ă'), ("acircumflex", 'â'), ("acute", '´'), ("acutecomb", '́'), ("adieresis", 'ä'), ("ae", 'æ'),
    ("aeacute", 'ǽ'), ("agrave", 'à'), ("aleph", 'ℵ'), ("alpha", 'α'), ("alphatonos", 'ά'), ("amacron", 'ā'),
    ("ampersand", '&'), ("angle", '∠'), ("angleleft", '〈'), ("angleright", '〉'), ("anoteleia", '·'),
    ("aogonek", 'ą'), ("approxequal", '≈'), ("aring", 'å'), ("aringacute", 'ǻ'), ("arrowboth", '↔'),
    ("arrowdblboth", '⇔'), ("arrowdbldown", '⇓'), ("arrowdblleft", '⇐'), ("arrowdblright", '⇒'),
    ("arrowdblup", '⇑'), ("arrowdown", '↓'), ("arrowleft", '←'), ("arrowright", '→'), ("arrowup", '↑'),
    ("arrowupdn", '↕'), ("arrowupdnbse", '↨'), ("asciicircum", '^'), ("asciitilde", '~'), ("asterisk", '*'),
    ("asteriskmath", '∗'), ("at", '@'), ("atilde", 'ã'), ("b", 'b'), ("backslash", '\\'), ("bar", '|'),
    ("beta", 'β'), ("block", '█'), ("braceleft", '{'), ("braceright", '}'), ("bracketleft", '['),
    ("bracketright", ']'), ("breve", '˘'), ("brokenbar", '¦'), ("bullet", '•'), ("c", 'c'), ("cacute", 'ć'),
    ("caron", 'ˇ'), ("carriagereturn", '↵'), ("ccaron", 'č'), ("ccedilla", 'ç'), ("ccircumflex", 'ĉ'),
    ("cdotaccent", 'ċ'), ("cedilla", '¸'), ("cent", '¢'), ("chi", 'χ'), ("circle", '○'), ("circlemultiply", '⊗'),
    ("circleplus", '⊕'), ("circumflex", 'ˆ'), ("club", '♣'), ("colon", ':'), ("colonmonetary", '₡'),
    ("comma", ','), ("congruent", '≅'), ("copyright", '©'), ("currency", '¤'), ("d", 'd'), ("dagger", '†'),
    ("daggerdbl", '‡'), ("dcaron", 'ď'), ("dcroat", 'đ'), ("degree", '°'), ("delta", 'δ'), ("diamond", '♦'),
    ("dieresis", '¨'), ("dieresistonos", '΅'), ("divide", '÷'), ("dkshade", '▓'), ("dnblock", '▄'),
    ("dollar", '$'), ("dong", '₫'), ("dotaccent", '˙'), ("dotbelowcomb", '̣'), ("dotlessi", 'ı'), ("dotmath", '⋅'),
    ("e", 'e'), ("eacute", 'é'), ("ebreve", 'ĕ'), ("ecaron", 'ě'), ("ecircumflex", 'ê'), ("edieresis", 'ë'),
    ("edotaccent", 'ė'), ("egrave", 'è'), ("eight", '8'), ("element", '∈'), ("ellipsis", '…'), ("emacron", 'ē'),
    ("emdash", '—'), ("emptyset", '∅'), ("endash", '–'), ("eng", 'ŋ'), ("eogonek", 'ę'), ("epsilon", 'ε'),
    ("epsilontonos", 'έ'), ("equal", '='), ("equivalence", '≡'), ("estimated", '℮'), ("eta", 'η'),
    ("etatonos", 'ή'), ("eth", 'ð'), ("exclam", '!'), ("exclamdbl", '‼'), ("exclamdown", '¡'),
    ("existential", '∃'), ("f", 'f'), ("female", '♀'), ("figuredash", '‒'), ("filledbox", '■'),
    ("filledrect", '▬'), ("five", '5'), ("fiveeighths", '⅝'), ("florin", 'ƒ'), ("four", '4'), ("fraction", '⁄'),
    ("franc", '₣'), ("g", 'g'), ("gamma", 'γ'), ("gbreve", 'ğ'), ("gcaron", 'ǧ'), ("gcircumflex", 'ĝ'),
    ("gdotaccent", 'ġ'), ("germandbls", 'ß'), ("gradient", '∇'), ("grave", '`'), ("gravecomb", '̀'),
    ("greater", '>'), ("greaterequal", '≥'), ("guillemotleft", '«'), ("guillemotright", '»'),
    ("guilsinglleft", '‹'), ("guilsinglright", '›'), ("h", 'h'), ("hbar", 'ħ'), ("hcircumflex", 'ĥ'),
    ("heart", '♥'), ("hookabovecomb", '̉'), ("house", '⌂'), ("hungarumlaut", '˝'), ("hyphen", '-'), ("i", 'i'),
    ("iacute", 'í'), ("ibreve", 'ĭ'), ("icircumflex", 'î'), ("idieresis", 'ï'), ("igrave", 'ì'), ("ij", 'ĳ'),
    ("imacron", 'ī'), ("infinity", '∞'), ("integral", '∫'), ("integralbt", '⌡'), ("integraltp", '⌠'),
    ("intersection", '∩'), ("invbullet", '◘'), ("invcircle", '◙'), ("invsmileface", '☻'), ("iogonek", 'į'),
    ("iota", 'ι'), ("iotadieresis", 'ϊ'), ("iotadieresistonos", 'ΐ'), ("iotatonos", 'ί'), ("itilde", 'ĩ'),
    ("j", 'j'), ("jcircumflex", 'ĵ'), ("k", 'k'), ("kappa", 'κ'), ("kgreenlandic", 'ĸ'), ("l", 'l'),
    ("lacute", 'ĺ'), ("lambda", 'λ'), ("lcaron", 'ľ'), ("ldot", 'ŀ'), ("less", '<'), ("lessequal", '≤'),
    ("lfblock", '▌'), ("lira", '₤'), ("logicaland", '∧'), ("logicalnot", '¬'), ("logicalor", '∨'), ("longs", 'ſ'),
    ("lozenge", '◊'), ("lslash", 'ł'), ("ltshade", '░'), ("m", 'm'), ("macron", '¯'), ("male", '♂'),
    ("minus", '\u{2212}'), ("minute", '′'), ("mu", 'µ'), ("multiply", '×'), ("musicalnote", '♪'),
    ("musicalnotedbl", '♫'), ("n", 'n'), ("nacute", 'ń'), ("napostrophe", 'ŉ'), ("ncaron", 'ň'), ("nine", '9'),
    ("notelement", '∉'), ("notequal", '≠'), ("notsubset", '⊄'), ("ntilde", 'ñ'), ("nu", 'ν'), ("numbersign", '#'),
    ("o", 'o'), ("oacute", 'ó'), ("obreve", 'ŏ'), ("ocircumflex", 'ô'), ("odieresis", 'ö'), ("oe", 'œ'),
    ("ogonek", '˛'), ("ograve", 'ò'), ("ohorn", 'ơ'), ("ohungarumlaut", 'ő'), ("omacron", 'ō'), ("omega", 'ω'),
    ("omega1", 'ϖ'), ("omegatonos", 'ώ'), ("omicron", 'ο'), ("omicrontonos", 'ό'), ("one", '1'),
    ("onedotenleader", '․'), ("oneeighth", '⅛'), ("onehalf", '½'), ("onequarter", '¼'), ("onethird", '⅓'),
    ("openbullet", '◦'), ("ordfeminine", 'ª'), ("ordmasculine", 'º'), ("orthogonal", '∟'), ("oslash", 'ø'),
    ("oslashacute", 'ǿ'), ("otilde", 'õ'), ("p", 'p'), ("paragraph", '¶'), ("parenleft", '('), ("parenright", ')'),
    ("partialdiff", '∂'), ("percent", '%'), ("period", '.'), ("periodcentered", '·'), ("perpendicular", '⊥'),
    ("perthousand", '‰'), ("peseta", '₧'), ("phi", 'φ'), ("phi1", 'ϕ'), ("pi", 'π'), ("plus", '+'),
    ("plusminus", '±'), ("prescription", '℞'), ("product", '∏'), ("propersubset", '⊂'), ("propersuperset", '⊃'),
    ("proportional", '∝'), ("psi", 'ψ'), ("q", 'q'), ("question", '?'), ("questiondown", '¿'), ("quotedbl", '\"'),
    ("quotedblbase", '„'), ("quotedblleft", '“'), ("quotedblright", '”'), ("quoteleft", '‘'),
    ("quotereversed", '‛'), ("quoteright", '’'), ("quotesinglbase", '‚'), ("quotesingle", '\''), ("r", 'r'),
    ("racute", 'ŕ'), ("radical", '√'), ("rcaron", 'ř'), ("reflexsubset", '⊆'), ("reflexsuperset", '⊇'),
    ("registered", '®'), ("revlogicalnot", '⌐'), ("rho", 'ρ'), ("ring", '˚'), ("rtblock", '▐'), ("s", 's'),
    ("sacute", 'ś'), ("scaron", 'š'), ("scedilla", 'ş'), ("scircumflex", 'ŝ'), ("second", '″'), ("section", '§'),
    ("semicolon", ';'), ("seven", '7'), ("seveneighths", '⅞'), ("shade", '▒'), ("sigma", 'σ'), ("sigma1", 'ς'),
    ("similar", '∼'), ("six", '6'), ("slash", '/'), ("smileface", '☺'), ("space", ' '), ("spade", '♠'),
    ("sterling", '£'), ("suchthat", '∋'), ("summation", '∑'), ("sun", '☼'), ("t", 't'), ("tau", 'τ'),
    ("tbar", 'ŧ'), ("tcaron", 'ť'), ("therefore", '∴'), ("theta", 'θ'), ("theta1", 'ϑ'), ("thorn", 'þ'),
    ("three", '3'), ("threeeighths", '⅜'), ("threequarters", '¾'), ("tilde", '˜'), ("tildecomb", '̃'),
    ("tonos", '΄'), ("trademark", '™'), ("triagdn", '▼'), ("triaglf", '◄'), ("triagrt", '►'), ("triagup", '▲'),
    ("two", '2'), ("twodotenleader", '‥'), ("twothirds", '⅔'), ("u", 'u'), ("uacute", 'ú'), ("ubreve", 'ŭ'),
    ("ucircumflex", 'û'), ("udieresis", 'ü'), ("ugrave", 'ù'), ("uhorn", 'ư'), ("uhungarumlaut", 'ű'),
    ("umacron", 'ū'), ("underscore", '_'), ("underscoredbl", '‗'), ("union", '∪'), ("universal", '∀'),
    ("uogonek", 'ų'), ("upblock", '▀'), ("upsilon", 'υ'), ("upsilondieresis", 'ϋ'), ("upsilondieresistonos", 'ΰ'),
    ("upsilontonos", 'ύ'), ("uring", 'ů'), ("utilde", 'ũ'), ("v", 'v'), ("w", 'w'), ("wacute", 'ẃ'),
    ("wcircumflex", 'ŵ'), ("wdieresis", 'ẅ'), ("weierstrass", '℘'), ("wgrave", 'ẁ'), ("x", 'x'), ("xi", 'ξ'),
    ("y", 'y'), ("yacute", 'ý'), ("ycircumflex", 'ŷ'), ("ydieresis", 'ÿ'), ("yen", '¥'), ("ygrave", 'ỳ'),
    ("z", 'z'), ("zacute", 'ź'), ("zcaron", 'ž'), ("zdotaccent", 'ż'), ("zero", '0'), ("zeta", 'ζ'),
];

#[rustfmt::skip]
pub(super) const STANDARD: [Option<&str>; 256] = [
    None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None, None, None, None, None, None, None, Some("space"),
    Some("exclam"), Some("quotedbl"), Some("numbersign"), Some("dollar"), Some("percent"), Some("ampersand"),
    Some("quoteright"), Some("parenleft"), Some("parenright"), Some("asterisk"), Some("plus"), Some("comma"),
    Some("hyphen"), Some("period"), Some("slash"), Some("zero"), Some("one"), Some("two"), Some("three"),
    Some("four"), Some("five"), Some("six"), Some("seven"), Some("eight"), Some("nine"), Some("colon"),
    Some("semicolon"), Some("less"), Some("equal"), Some("greater"), Some("question"), Some("at"), Some("A"),
    Some("B"), Some("C"), Some("D"), Some("E"), Some("F"), Some("G"), Some("H"), Some("I"), Some("J"), Some("K"),
    Some("L"), Some("M"), Some("N"), Some("O"), Some("P"), Some("Q"), Some("R"), Some("S"), Some("T"), Some("U"),
    Some("V"), Some("W"), Some("X"), Some("Y"), Some("Z"), Some("bracketleft"), Some("backslash"),
    Some("bracketright"), Some("asciicircum"), Some("underscore"), Some("quoteleft"), Some("a"), Some("b"),
    Some("c"), Some("d"), Some("e"), Some("f"), Some("g"), Some("h"), Some("i"), Some("j"), Some("k"), Some("l"),
    Some("m"), Some("n"), Some("o"), Some("p"), Some("q"), Some("r"), Some("s"), Some("t"), Some("u"), Some("v"),
    Some("w"), Some("x"), Some("y"), Some("z"), Some("braceleft"), Some("bar"), Some("braceright"),
    Some("asciitilde"), None, None, None, None, None, None, None, None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None,
    None, Some("exclamdown"), Some("cent"), Some("sterling"), Some("fraction"), Some("yen"), Some("florin"),
    Some("section"), Some("currency"), Some("quotesingle"), Some("quotedblleft"), Some("guillemotleft"),
    Some("guilsinglleft"), Some("guilsinglright"), Some("fi"), Some("fl"), None, Some("endash"), Some("dagger"),
    Some("daggerdbl"), Some("periodcentered"), None, Some("paragraph"), Some("bullet"), Some("quotesinglbase"),
    Some("quotedblbase"), Some("quotedblright"), Some("guillemotright"), Some("ellipsis"), Some("perthousand"),
    None, Some("questiondown"), None, Some("grave"), Some("acute"), Some("circumflex"), Some("tilde"),
    Some("macron"), Some("breve"), Some("dotaccent"), Some("dieresis"), None, Some("ring"), Some("cedilla"), None,
    Some("hungarumlaut"), Some("ogonek"), Some("caron"), Some("emdash"), None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None, None, Some("AE"), None, Some("ordfeminine"), None, None, None,
    None, Some("Lslash"), Some("Oslash"), Some("OE"), Some("ordmasculine"), None, None, None, None, None,
    Some("ae"), None, None, None, Some("dotlessi"), None, None, Some("lslash"), Some("oslash"), Some("oe"),
    Some("germandbls"), None, None, None, None,
];

#[rustfmt::skip]
pub(super) const WIN_ANSI: [Option<&str>; 256] = [
    None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None, None, None, None, None, None, None, Some("space"),
    Some("exclam"), Some("quotedbl"), Some("numbersign"), Some("dollar"), Some("percent"), Some("ampersand"),
    Some("quotesingle"), Some("parenleft"), Some("parenright"), Some("asterisk"), Some("plus"), Some("comma"),
    Some("hyphen"), Some("period"), Some("slash"), Some("zero"), Some("one"), Some("two"), Some("three"),
    Some("four"), Some("five"), Some("six"), Some("seven"), Some("eight"), Some("nine"), Some("colon"),
    Some("semicolon"), Some("less"), Some("equal"), Some("greater"), Some("question"), Some("at"), Some("A"),
    Some("B"), Some("C"), Some("D"), Some("E"), Some("F"), Some("G"), Some("H"), Some("I"), Some("J"), Some("K"),
    Some("L"), Some("M"), Some("N"), Some("O"), Some("P"), Some("Q"), Some("R"), Some("S"), Some("T"), Some("U"),
    Some("V"), Some("W"), Some("X"), Some("Y"), Some("Z"), Some("bracketleft"), Some("backslash"),
    Some("bracketright"), Some("asciicircum"), Some("underscore"), Some("grave"), Some("a"), Some("b"), Some("c"),
    Some("d"), Some("e"), Some("f"), Some("g"), Some("h"), Some("i"), Some("j"), Some("k"), Some("l"), Some("m"),
    Some("n"), Some("o"), Some("p"), Some("q"), Some("r"), Some("s"), Some("t"), Some("u"), Some("v"), Some("w"),
    Some("x"), Some("y"), Some("z"), Some("braceleft"), Some("bar"), Some("braceright"), Some("asciitilde"),
    Some("bullet"), Some("Euro"), Some("bullet"), Some("quotesinglbase"), Some("florin"), Some("quotedblbase"),
    Some("ellipsis"), Some("dagger"), Some("daggerdbl"), Some("circumflex"), Some("perthousand"), Some("Scaron"),
    Some("guilsinglleft"), Some("OE"), Some("bullet"), Some("Zcaron"), Some("bullet"), Some("bullet"),
    Some("quoteleft"), Some("quoteright"), Some("quotedblleft"), Some("quotedblright"), Some("bullet"),
    Some("endash"), Some("emdash"), Some("tilde"), Some("trademark"), Some("scaron"), Some("guilsinglright"),
    Some("oe"), Some("bullet"), Some("zcaron"), Some("Ydieresis"), Some("space"), Some("exclamdown"), Some("cent"),
    Some("sterling"), Some("currency"), Some("yen"), Some("brokenbar"), Some("section"), Some("dieresis"),
    Some("copyright"), Some("ordfeminine"), Some("guillemotleft"), Some("logicalnot"), Some("hyphen"),
    Some("registered"), Some("macron"), Some("degree"), Some("plusminus"), Some("twosuperior"),
    Some("threesuperior"), Some("acute"), Some("mu"), Some("paragraph"), Some("periodcentered"), Some("cedilla"),
    Some("onesuperior"), Some("ordmasculine"), Some("guillemotright"), Some("onequarter"), Some("onehalf"),
    Some("threequarters"), Some("questiondown"), Some("Agrave"), Some("Aacute"), Some("Acircumflex"),
    Some("Atilde"), Some("Adieresis"), Some("Aring"), Some("AE"), Some("Ccedilla"), Some("Egrave"), Some("Eacute"),
    Some("Ecircumflex"), Some("Edieresis"), Some("Igrave"), Some("Iacute"), Some("Icircumflex"), Some("Idieresis"),
    Some("Eth"), Some("Ntilde"), Some("Ograve"), Some("Oacute"), Some("Ocircumflex"), Some("Otilde"),
    Some("Odieresis"), Some("multiply"), Some("Oslash"), Some("Ugrave"), Some("Uacute"), Some("Ucircumflex"),
    Some("Udieresis"), Some("Yacute"), Some("Thorn"), Some("germandbls"), Some("agrave"), Some("aacute"),
    Some("acircumflex"), Some("atilde"), Some("adieresis"), Some("aring"), Some("ae"), Some("ccedilla"),
    Some("egrave"), Some("eacute"), Some("ecircumflex"), Some("edieresis"), Some("igrave"), Some("iacute"),
    Some("icircumflex"), Some("idieresis"), Some("eth"), Some("ntilde"), Some("ograve"), Some("oacute"),
    Some("ocircumflex"), Some("otilde"), Some("odieresis"), Some("divide"), Some("oslash"), Some("ugrave"),
    Some("uacute"), Some("ucircumflex"), Some("udieresis"), Some("yacute"), Some("thorn"), Some("ydieresis"),
];

#[rustfmt::skip]
pub(super) const MAC_ROMAN: [Option<&str>; 256] = [
    None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None, None, None, None, None, None, None, Some("space"),
    Some("exclam"), Some("quotedbl"), Some("numbersign"), Some("dollar"), Some("percent"), Some("ampersand"),
    Some("quotesingle"), Some("parenleft"), Some("parenright"), Some("asterisk"), Some("plus"), Some("comma"),
    Some("hyphen"), Some("period"), Some("slash"), Some("zero"), Some("one"), Some("two"), Some("three"),
    Some("four"), Some("five"), Some("six"), Some("seven"), Some("eight"), Some("nine"), Some("colon"),
    Some("semicolon"), Some("less"), Some("equal"), Some("greater"), Some("question"), Some("at"), Some("A"),
    Some("B"), Some("C"), Some("D"), Some("E"), Some("F"), Some("G"), Some("H"), Some("I"), Some("J"), Some("K"),
    Some("L"), Some("M"), Some("N"), Some("O"), Some("P"), Some("Q"), Some("R"), Some("S"), Some("T"), Some("U"),
    Some("V"), Some("W"), Some("X"), Some("Y"), Some("Z"), Some("bracketleft"), Some("backslash"),
    Some("bracketright"), Some("asciicircum"), Some("underscore"), Some("grave"), Some("a"), Some("b"), Some("c"),
    Some("d"), Some("e"), Some("f"), Some("g"), Some("h"), Some("i"), Some("j"), Some("k"), Some("l"), Some("m"),
    Some("n"), Some("o"), Some("p"), Some("q"), Some("r"), Some("s"), Some("t"), Some("u"), Some("v"), Some("w"),
    Some("x"), Some("y"), Some("z"), Some("braceleft"), Some("bar"), Some("braceright"), Some("asciitilde"), None,
    Some("Adieresis"), Some("Aring"), Some("Ccedilla"), Some("Eacute"), Some("Ntilde"), Some("Odieresis"),
    Some("Udieresis"), Some("aacute"), Some("agrave"), Some("acircumflex"), Some("adieresis"), Some("atilde"),
    Some("aring"), Some("ccedilla"), Some("eacute"), Some("egrave"), Some("ecircumflex"), Some("edieresis"),
    Some("iacute"), Some("igrave"), Some("icircumflex"), Some("idieresis"), Some("ntilde"), Some("oacute"),
    Some("ograve"), Some("ocircumflex"), Some("odieresis"), Some("otilde"), Some("uacute"), Some("ugrave"),
    Some("ucircumflex"), Some("udieresis"), Some("dagger"), Some("degree"), Some("cent"), Some("sterling"),
    Some("section"), Some("bullet"), Some("paragraph"), Some("germandbls"), Some("registered"), Some("copyright"),
    Some("trademark"), Some("acute"), Some("dieresis"), Some("notequal"), Some("AE"), Some("Oslash"),
    Some("infinity"), Some("plusminus"), Some("lessequal"), Some("greaterequal"), Some("yen"), Some("mu"),
    Some("partialdiff"), Some("summation"), Some("product"), Some("pi"), Some("integral"), Some("ordfeminine"),
    Some("ordmasculine"), Some("Omega"), Some("ae"), Some("oslash"), Some("questiondown"), Some("exclamdown"),
    Some("logicalnot"), Some("radical"), Some("florin"), Some("approxequal"), Some("Delta"), Some("guillemotleft"),
    Some("guillemotright"), Some("ellipsis"), Some("space"), Some("Agrave"), Some("Atilde"), Some("Otilde"),
    Some("OE"), Some("oe"), Some("endash"), Some("emdash"), Some("quotedblleft"), Some("quotedblright"),
    Some("quoteleft"), Some("quoteright"), Some("divide"), Some("lozenge"), Some("ydieresis"), Some("Ydieresis"),
    Some("fraction"), Some("currency"), Some("guilsinglleft"), Some("guilsinglright"), Some("fi"), Some("fl"),
    Some("daggerdbl"), Some("periodcentered"), Some("quotesinglbase"), Some("quotedblbase"), Some("perthousand"),
    Some("Acircumflex"), Some("Ecircumflex"), Some("Aacute"), Some("Edieresis"), Some("Egrave"), Some("Iacute"),
    Some("Icircumflex"), Some("Idieresis"), Some("Igrave"), Some("Oacute"), Some("Ocircumflex"), Some("apple"),
    Some("Ograve"), Some("Uacute"), Some("Ucircumflex"), Some("Ugrave"), Some("dotlessi"), Some("circumflex"),
    Some("tilde"), Some("macron"), Some("breve"), Some("dotaccent"), Some("ring"), Some("cedilla"),
    Some("hungarumlaut"), Some("ogonek"), Some("caron"),
];
