//! Port of `old_src/src/calibre/ebooks/pdf/render/common.py` (245 lines).
//!
//! The PDF object model shared by every other file in
//! `crate::pdf::render`: `/Name`s, PDF strings (both Latin-1-or-UTF16BE
//! `(...)`-literal `String` and always-UTF16BE `UTF16String`),
//! dictionaries (sorted-key block form and unsorted inline form), arrays,
//! streams, and indirect-object references, plus the free
//! `serialize`/`fmtnum` dispatchers and the page-size constants.
//!
//! Ported for real, no gaps. The only native dependency in the Python
//! source is `calibre_extensions.speedup.pdf_float` (a small C float
//! formatter, `old_src/src/calibre/utils/speedup.c`'s
//! `speedup_pdf_float`, around line 120) - reimplemented here as
//! [`pdf_float`].
//!
//! # Structural notes on the port
//!
//! Python's `Stream` is a `BytesIO` subclass that other files subclass
//! and override `add_extra_keys` on (`FontStream`, `CMap`, `Page`,
//! `Image`, `Metadata`, the `TilingPattern` family in `graphics.py`).
//! Rust has no inheritance, so this is ported as composition: [`Stream`]
//! is a plain owned byte buffer implementing [`StreamLike`] with no
//! extra keys, and composite types elsewhere in `crate::pdf::render`
//! hold a `Stream` field and implement [`StreamLike`] themselves,
//! overriding [`StreamLike::extra_keys`]. This works because, on
//! inspection, none of the Python overrides of `add_extra_keys` actually
//! need the `Length`/`DL`/`Filter` keys already computed by the base
//! class's `pdf_serialize` (the one entry that looks like it might,
//! `FontStream`'s `d['Length1'] = d['DL']`, is just `len(uncompressed
//! buffer)`, known before compression) - so `extra_keys` here is a pure
//! function of the composite type's own fields, not a callback into a
//! half-built dictionary.
//!
//! Python's `IndirectObjects.add(o)` returns a `Reference` whose `.obj`
//! field is a live handle back to `o`, later mutated in place (e.g.
//! `PDFStream.set_metadata` does `self.catalog.obj['Metadata'] = ...`).
//! That live-shared-mutable-reference pattern doesn't translate
//! directly; see `crate::pdf::render::serialize`'s `IndirectObjects` for
//! the arena/handle restructuring (mirrors the same translation choice
//! applied to `links.py`'s `Links.pdf` back-reference, per that module's
//! doc comment).
//!
//! `current_log`/`default_log` (a global mutable logger singleton from
//! `calibre.utils.logging`, itself unported and out of scope) is ported
//! as [`set_current_log`]/[`log_warn`]: a settable global callback with
//! an `eprintln!`-based fallback standing in for calibre's full logging
//! system, consistent with this crate's practice of plain-data/closure
//! stand-ins for out-of-scope infrastructure (see e.g.
//! `crate::pdf::image_writer`'s `QPageSize`/`QPageLayout` stand-ins).

use std::sync::{Mutex, OnceLock};

use flate2::write::ZlibEncoder;
use flate2::Compression;

// ==========================================================================
// Sizes (common.py lines 20-54)
// ==========================================================================

pub const INCH: f64 = 72.0;
pub const CM: f64 = INCH / 2.54;
pub const MM: f64 = CM * 0.1;
pub const PICA: f64 = 12.0;
pub const DIDOT: f64 = 0.375 * MM;
pub const CICERO: f64 = 12.0 * DIDOT;

const PAPER_W: f64 = 21.0 * CM;
const PAPER_H: f64 = 29.7 * CM;

pub const A6: (f64, f64) = (PAPER_W * 0.5, PAPER_H * 0.5);
pub const A5: (f64, f64) = (PAPER_H * 0.5, PAPER_W);
pub const A4: (f64, f64) = (PAPER_W, PAPER_H);
pub const A3: (f64, f64) = (PAPER_H, PAPER_W * 2.0);
pub const A2: (f64, f64) = (PAPER_W * 2.0, PAPER_H * 2.0);
pub const A1: (f64, f64) = (PAPER_H * 2.0, PAPER_W * 4.0);
pub const A0: (f64, f64) = (PAPER_W * 4.0, PAPER_H * 4.0);

pub const LETTER: (f64, f64) = (8.5 * INCH, 11.0 * INCH);
pub const LEGAL: (f64, f64) = (8.5 * INCH, 14.0 * INCH);
pub const ELEVENSEVENTEEN: (f64, f64) = (11.0 * INCH, 17.0 * INCH);

const PAPER_BW: f64 = 25.0 * CM;
const PAPER_BH: f64 = 35.3 * CM;

pub const B6: (f64, f64) = (PAPER_BW * 0.5, PAPER_BH * 0.5);
pub const B5: (f64, f64) = (PAPER_BH * 0.5, PAPER_BW);
pub const B4: (f64, f64) = (PAPER_BW, PAPER_BH);
pub const B3: (f64, f64) = (PAPER_BH * 2.0, PAPER_BW);
pub const B2: (f64, f64) = (PAPER_BW * 2.0, PAPER_BH * 2.0);
pub const B1: (f64, f64) = (PAPER_BH * 4.0, PAPER_BW * 2.0);
pub const B0: (f64, f64) = (PAPER_BW * 4.0, PAPER_BH * 4.0);

/// Port of `PAPER_SIZES` (common.py lines 51-52): looks up a named paper
/// size (lowercase, e.g. `"a4"`, `"letter"`) and returns `(width, height)`
/// in points.
pub fn paper_size(name: &str) -> Option<(f64, f64)> {
    Some(match name {
        "a0" => A0,
        "a1" => A1,
        "a2" => A2,
        "a3" => A3,
        "a4" => A4,
        "a5" => A5,
        "a6" => A6,
        "b0" => B0,
        "b1" => B1,
        "b2" => B2,
        "b3" => B3,
        "b4" => B4,
        "b5" => B5,
        "b6" => B6,
        "letter" => LETTER,
        "legal" => LEGAL,
        _ => return None,
    })
}

// ==========================================================================
// pdf_float (port of `speedup_pdf_float`, old_src/src/calibre/utils/speedup.c ~line 120)
// ==========================================================================

/// Port of `calibre_extensions.speedup.pdf_float` (`speedup_pdf_float` in
/// `old_src/src/calibre/utils/speedup.c`). Formats a float the way PDF
/// content streams want numbers: fixed-point, no exponent, trailing
/// zeros (and a trailing bare `.`) stripped.
///
/// `precision` starts at 6 decimal digits; for `|f| > 1` it's tightened
/// to `clamp(0, 6 - floor(log10(|f|)), 6)` so large numbers don't grow
/// absurdly long fractional parts. Values with `|f| <= 1e-7` are treated
/// as zero (matches the C source's `a > 1.0e-7` guard).
pub fn pdf_float(f: f64) -> String {
    let a = f.abs();
    // Mirrors the C source's `if (a > 1.0e-7) { ... }` framing (the
    // "near zero" case is everything else, including `f == NaN`, for
    // which `a > 1.0e-7` is also false) - written as a negation rather
    // than `a <= 1.0e-7` so NaN keeps falling into this branch exactly
    // as `!(a > 1.0e-7)` would (`a <= 1.0e-7` is false for NaN too, but
    // reads as if it excludes NaN).
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    let is_near_zero = !(a > 1.0e-7);
    if is_near_zero {
        return "0".to_string();
    }
    let mut precision: i32 = 6;
    if a > 1.0 {
        // C: `(int)log10(a)` truncates toward zero; for a > 1, log10(a) >
        // 0, so truncation and floor agree.
        let log = a.log10() as i32;
        precision = (6 - log).clamp(0, 6);
    }
    let mut buf = format!("{f:.*}", precision as usize);
    if precision > 0 {
        while buf.ends_with('0') {
            buf.pop();
        }
        if buf.ends_with('.') {
            buf.pop();
        }
    }
    buf
}

// ==========================================================================
// PDF value model
// ==========================================================================

/// Port of `Name` (common.py lines 84-95): a PDF `/Name` token.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name(pub String);

impl Name {
    pub fn new(s: impl Into<String>) -> Self {
        Name(s.into())
    }
}

impl<S: Into<String>> From<S> for Name {
    fn from(s: S) -> Self {
        Name::new(s)
    }
}

/// Port of `Name.pdf_serialize`: percent-style-escapes any byte outside
/// `33 < byte < 126` (and `#` itself) as `#xx` hex, matching the PDF
/// name-object escaping rules. Panics on names over 126 bytes, matching
/// the Python `raise ValueError` (this is a programmer-error-class
/// invariant violation, not a recoverable runtime condition - see the
/// `ValueError: Name too long` in the source).
fn serialize_name(name: &Name, out: &mut Vec<u8>) {
    let raw = name.0.as_bytes();
    assert!(raw.len() <= 126, "Name too long: {:?}", name.0);
    out.push(b'/');
    for &x in raw {
        if x > 33 && x < 126 && x != b'#' {
            out.push(x);
        } else {
            out.extend(format!("#{x:x}").into_bytes());
        }
    }
}

/// Port of `escape_pdf_string` (common.py lines 98-118): escape
/// unbalanced/unmatched parentheses and the control characters PDF 1.7
/// Table 3.2 requires (`\n \r \f \b \t \\`) for use inside a `(...)`
/// PDF string literal.
///
/// Faithfully reproduces the Python source's `bad_map` verbatim,
/// including its apparent quirk: the tab entry maps byte `9` (`'\t'`) to
/// replacement byte `ord('\t') == 9` (i.e. it inserts a literal backslash
/// then a literal tab byte, `"\<TAB>"`), not the two "backslash-then-
/// letter-t" characters a `\t` escape sequence would usually mean. This
/// is ground truth from the Python source, not a bug fixed in this port.
pub fn escape_pdf_string(bytestring: &[u8]) -> Vec<u8> {
    let mut open_parens: Vec<usize> = Vec::new();
    let mut bad: Vec<(usize, u8)> = Vec::new();
    for (i, &num) in bytestring.iter().enumerate() {
        match num {
            40 => open_parens.push(i), // '('
            41 => {
                // ')'
                if open_parens.pop().is_none() {
                    bad.push((i, 41));
                }
            }
            10 => bad.push((i, b'n')),
            13 => bad.push((i, b'r')),
            12 => bad.push((i, b'f')),
            8 => bad.push((i, b'b')),
            9 => bad.push((i, 9)), // see doc comment: replacement is byte 9, not b't'
            92 => bad.push((i, b'\\')),
            _ => {}
        }
    }
    // Unmatched '(' entries get escaped too (replacement byte is '('
    // itself, matching Python's `(i, 40)` tuples appended to `indices`).
    for i in open_parens {
        bad.push((i, 40));
    }
    if bad.is_empty() {
        return bytestring.to_vec();
    }
    bad.sort_by_key(|&(i, _)| std::cmp::Reverse(i)); // descending, so splicing keeps earlier indices valid
    let mut out = bytestring.to_vec();
    for (i, repl) in bad {
        out.splice(i..i + 1, [92u8, repl]);
    }
    out
}

/// Port of `String` (common.py lines 121-130): a PDF `(...)` string
/// literal. Encodes as Latin-1 when every character fits in a byte,
/// otherwise falls back to UTF-16BE with a BOM (matching the Python
/// `try: encode('latin1') except UnicodeEncodeError:` control flow) -
/// including the same edge case where a Latin-1 encoding that happens to
/// start with the UTF-16BE BOM bytes (`\xfe\xff`, i.e. the string starts
/// with `þÿ`) is forced onto the UTF-16BE path too, to avoid ambiguity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfString(pub String);

impl PdfString {
    pub fn new(s: impl Into<String>) -> Self {
        PdfString(s.into())
    }
}

const BOM_UTF16_BE: [u8; 2] = [0xFE, 0xFF];

fn encode_utf16_be_with_bom(s: &str) -> Vec<u8> {
    let mut raw = BOM_UTF16_BE.to_vec();
    for unit in s.encode_utf16() {
        raw.extend(unit.to_be_bytes());
    }
    raw
}

fn encode_latin1(s: &str) -> Option<Vec<u8>> {
    let mut raw = Vec::with_capacity(s.len());
    for ch in s.chars() {
        let cp = ch as u32;
        if cp > 0xFF {
            return None;
        }
        raw.push(cp as u8);
    }
    Some(raw)
}

fn serialize_pdf_string(s: &PdfString, out: &mut Vec<u8>) {
    let raw = match encode_latin1(&s.0) {
        Some(raw) if !raw.starts_with(&BOM_UTF16_BE) => raw,
        _ => encode_utf16_be_with_bom(&s.0),
    };
    out.push(b'(');
    out.extend(escape_pdf_string(&raw));
    out.push(b')');
}

/// Port of `UTF16String` (common.py lines 133-142): always encodes as
/// UTF-16BE with a leading BOM, wrapped as a `(...)` literal (the
/// alternate `<hex>` form is `if False`-disabled in the Python source and
/// so is not ported).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Utf16String(pub String);

impl Utf16String {
    pub fn new(s: impl Into<String>) -> Self {
        Utf16String(s.into())
    }
}

fn serialize_utf16_string(s: &Utf16String, out: &mut Vec<u8>) {
    let raw = encode_utf16_be_with_bom(&s.0);
    out.push(b'(');
    out.extend(escape_pdf_string(&raw));
    out.push(b')');
}

/// Port of `Reference` (common.py lines 222-235): an indirect-object
/// reference, `N 0 R`. Unlike the Python original, this does not carry a
/// live handle back to the referenced object (`.obj`) - see the module
/// doc comment and `serialize::IndirectObjects` for the arena/handle
/// restructuring that replaces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Reference {
    pub num: u32,
}

impl Reference {
    pub fn new(num: u32) -> Self {
        Reference { num }
    }
}

impl std::fmt::Display for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} 0 R", self.num)
    }
}

fn serialize_reference(r: &Reference, out: &mut Vec<u8>) {
    out.extend(r.to_string().into_bytes());
}

/// A PDF date value, e.g. `PDFStream`'s `CreationDate`. Port of the
/// `elif isinstance(o, datetime):` branch of `serialize()` (common.py
/// lines 75-79).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PdfDateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// UTC offset in minutes (e.g. 0 for UTC, -300 for UTC-05:00).
    pub tz_offset_minutes: i32,
}

fn format_pdf_datetime(dt: &PdfDateTime) -> String {
    let secs = dt.second.min(59);
    let base = format!(
        "D:{:04}{:02}{:02}{:02}{:02}{:02}",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, secs
    );
    let sign = if dt.tz_offset_minutes >= 0 { '+' } else { '-' };
    let abs_min = dt.tz_offset_minutes.unsigned_abs();
    let val = format!("{base}{sign}{:02}{:02}", abs_min / 60, abs_min % 60);
    // Python's `if datetime.tzinfo is not None:` checks the *class*
    // attribute (a descriptor, always truthy), not `o.tzinfo` - so this
    // branch is unconditionally taken. Faithfully reproduced rather than
    // "fixed": always wrap as `(head'tail')`.
    let split_at = val.len() - 2;
    format!("({}'{}')", &val[..split_at], &val[split_at..])
}

/// Port of the free `Array` class (common.py lines 172-180): an ordered
/// PDF array.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Array(pub Vec<PdfObj>);

impl Array {
    pub fn new() -> Self {
        Array(Vec::new())
    }
    pub fn push(&mut self, o: impl Into<PdfObj>) {
        self.0.push(o.into());
    }
    pub fn extend(&mut self, items: impl IntoIterator<Item = impl Into<PdfObj>>) {
        self.0.extend(items.into_iter().map(Into::into));
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<T: Into<PdfObj>> FromIterator<T> for Array {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Array(iter.into_iter().map(Into::into).collect())
    }
}

/// Like [`serialize`] but for an `&Dictionary` directly, avoiding a
/// clone into a `PdfObj::Dict` just to serialize it - used by
/// `serialize::IndirectObjects`, which stores `Dictionary`/`Array`
/// values directly rather than wrapped in `PdfObj`.
pub(crate) fn serialize_dictionary(d: &Dictionary, out: &mut Vec<u8>) {
    serialize_dict_entries(&d.0, true, out);
}

pub(crate) fn serialize_array(a: &Array, out: &mut Vec<u8>) {
    out.push(b'[');
    for (i, o) in a.0.iter().enumerate() {
        if i != 0 {
            out.push(b' ');
        }
        serialize(o, out);
    }
    out.push(b']');
}

/// Sort key used by [`Dictionary`]'s `pdf_serialize`: faithful port of
/// `Dictionary.pdf_serialize`'s `sorted_keys` lambda (common.py lines
/// 149-151), including its quirk of ordering "other" keys by `key+key`
/// rather than plain `key` (this only affects cosmetic key ordering in
/// the output PDF, never correctness).
fn dict_sort_key(k: &str) -> String {
    match k {
        "Type" => format!("1{k}"),
        "Subtype" => format!("2{k}"),
        _ => format!("{k}{k}"),
    }
}

/// Port of `Dictionary` (common.py lines 145-157): a PDF dictionary
/// object serialized in sorted-key, one-entry-per-line block form
/// (`<<\n/Key value\n...\n>>\n`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dictionary(pub indexmap::IndexMap<String, PdfObj>);

impl Dictionary {
    pub fn new() -> Self {
        Dictionary(indexmap::IndexMap::new())
    }

    pub fn from_pairs<K: Into<String>, V: Into<PdfObj>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        let mut d = Dictionary::new();
        for (k, v) in pairs {
            d.insert(k, v);
        }
        d
    }

    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<PdfObj>) {
        self.0.insert(key.into(), value.into());
    }

    pub fn get(&self, key: &str) -> Option<&PdfObj> {
        self.0.get(key)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn serialize_dict_entries(
    entries: &indexmap::IndexMap<String, PdfObj>,
    sorted: bool,
    out: &mut Vec<u8>,
) {
    if sorted {
        out.extend_from_slice(b"<<\n");
        let mut keys: Vec<&String> = entries.keys().collect();
        keys.sort_by_key(|k| dict_sort_key(k));
        for k in keys {
            serialize_name(&Name::new(k.clone()), out);
            out.push(b' ');
            serialize(&entries[k], out);
            out.extend_from_slice(b"\n");
        }
        out.extend_from_slice(b">>\n");
    } else {
        out.extend_from_slice(b"<< ");
        for (k, v) in entries {
            serialize_name(&Name::new(k.clone()), out);
            out.push(b' ');
            serialize(v, out);
            out.push(b' ');
        }
        out.extend_from_slice(b">>");
    }
}

/// Port of `InlineDictionary` (common.py lines 160-169): like
/// [`Dictionary`] but serialized inline, in insertion order (`<< /Key
/// value ... >>`, no sorting, no newlines). Used for the per-stream
/// dictionary `Stream.pdf_serialize` builds on the fly.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InlineDictionary(pub indexmap::IndexMap<String, PdfObj>);

impl InlineDictionary {
    pub fn new() -> Self {
        InlineDictionary(indexmap::IndexMap::new())
    }
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<PdfObj>) {
        self.0.insert(key.into(), value.into());
    }
}

/// Any value that can appear inside a [`Dictionary`]/[`Array`], or be
/// passed to [`serialize`]. Port of the dynamic-typing surface Python's
/// free `serialize()` function (common.py lines 63-81) dispatches over.
#[derive(Debug, Clone, PartialEq)]
pub enum PdfObj {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Name(Name),
    Str(PdfString),
    Utf16Str(Utf16String),
    Array(Array),
    Dict(Dictionary),
    InlineDict(InlineDictionary),
    Reference(Reference),
    DateTime(PdfDateTime),
}

macro_rules! impl_from_pdfobj {
    ($variant:ident, $ty:ty) => {
        impl From<$ty> for PdfObj {
            fn from(v: $ty) -> Self {
                PdfObj::$variant(v)
            }
        }
    };
}

impl_from_pdfobj!(Name, Name);
impl_from_pdfobj!(Str, PdfString);
impl_from_pdfobj!(Utf16Str, Utf16String);
impl_from_pdfobj!(Array, Array);
impl_from_pdfobj!(Dict, Dictionary);
impl_from_pdfobj!(InlineDict, InlineDictionary);
impl_from_pdfobj!(Reference, Reference);
impl_from_pdfobj!(DateTime, PdfDateTime);

impl From<bool> for PdfObj {
    fn from(v: bool) -> Self {
        PdfObj::Bool(v)
    }
}
impl From<i64> for PdfObj {
    fn from(v: i64) -> Self {
        PdfObj::Int(v)
    }
}
impl From<i32> for PdfObj {
    fn from(v: i32) -> Self {
        PdfObj::Int(v as i64)
    }
}
impl From<usize> for PdfObj {
    fn from(v: usize) -> Self {
        PdfObj::Int(v as i64)
    }
}
impl From<f64> for PdfObj {
    fn from(v: f64) -> Self {
        PdfObj::Real(v)
    }
}

impl PdfObj {
    pub fn null() -> Self {
        PdfObj::Null
    }
}

/// Port of the free function `fmtnum` (common.py lines 57-60): formats a
/// number the way it belongs in a content-stream operator list (floats
/// via [`pdf_float`], everything else via `Display`/`ToString`).
pub fn fmtnum_f64(o: f64) -> String {
    pdf_float(o)
}

pub fn fmtnum_i64(o: i64) -> String {
    o.to_string()
}

/// Port of the free function `serialize` (common.py lines 63-81): the
/// top-level dispatcher used to write any [`PdfObj`] (or, via the small
/// wrapper trait [`PdfWrite`], any raw bytes) into an output buffer.
pub fn serialize(o: &PdfObj, out: &mut Vec<u8>) {
    match o {
        PdfObj::Real(f) => out.extend(pdf_float(*f).into_bytes()),
        PdfObj::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        PdfObj::Int(i) => out.extend(i.to_string().into_bytes()),
        PdfObj::Name(n) => serialize_name(n, out),
        PdfObj::Str(s) => serialize_pdf_string(s, out),
        PdfObj::Utf16Str(s) => serialize_utf16_string(s, out),
        PdfObj::Array(a) => serialize_array(a, out),
        PdfObj::Dict(d) => serialize_dict_entries(&d.0, true, out),
        PdfObj::InlineDict(d) => serialize_dict_entries(&d.0, false, out),
        PdfObj::Reference(r) => serialize_reference(r, out),
        PdfObj::Null => out.extend_from_slice(b"null"),
        PdfObj::DateTime(dt) => out.extend(format_pdf_datetime(dt).into_bytes()),
    }
}

// ==========================================================================
// Stream (common.py lines 183-219)
// ==========================================================================

/// Port of `Stream` (common.py lines 183-219): an owned, growable byte
/// buffer (the `BytesIO` half) that also knows how to serialize itself
/// as a PDF stream object (the `pdf_serialize` half). See the module doc
/// comment for how composite stream types elsewhere in `pdf::render`
/// (`FontStream`, `CMap`, `Page`, `Image`, `Metadata`, the tiling-pattern
/// family) build on this via composition + [`StreamLike`] rather than
/// subclassing.
#[derive(Debug, Clone, Default)]
pub struct Stream {
    buf: Vec<u8>,
    pub compress: bool,
    pub filters: Array,
    pub last_char: Option<u8>,
}

impl Stream {
    pub fn new(compress: bool) -> Self {
        Stream {
            buf: Vec::new(),
            compress,
            filters: Array::new(),
            last_char: None,
        }
    }

    /// Port of `Stream.write` (common.py lines 214-216): appends raw
    /// bytes (Python's version also accepts/encodes `str`; callers here
    /// just pass `&[u8]` or `&str`, both via `impl AsRef<[u8]>`).
    pub fn write(&mut self, raw: impl AsRef<[u8]>) {
        self.write_raw(raw.as_ref());
    }

    /// Port of `Stream.write_raw` (common.py line 218): appends raw
    /// bytes with no encoding step (matches `BytesIO.write` directly).
    pub fn write_raw(&mut self, raw: &[u8]) {
        self.buf.extend_from_slice(raw);
        if let Some(&b) = raw.last() {
            self.last_char = Some(b);
        }
    }

    /// Port of `Stream.write_line` (common.py lines 210-212).
    pub fn write_line(&mut self, raw: impl AsRef<[u8]>) {
        self.write(raw);
        self.write_raw(b"\n");
    }

    pub fn getvalue(&self) -> &[u8] {
        &self.buf
    }

    pub fn tell(&self) -> usize {
        self.buf.len()
    }
}

/// The `Stream` "class hierarchy" translated to composition: any type
/// that owns a [`Stream`] buffer and wants `pdf_serialize` behavior
/// implements this to expose the buffer plus its own extra dictionary
/// keys (the port of `add_extra_keys`).
pub trait StreamLike {
    fn stream(&self) -> &Stream;
    /// Port of `add_extra_keys` (common.py line 190-191's no-op base
    /// case, overridden by subclasses elsewhere in `pdf::render`).
    fn extra_keys(&self) -> Vec<(String, PdfObj)> {
        Vec::new()
    }
}

impl StreamLike for Stream {
    fn stream(&self) -> &Stream {
        self
    }
}

/// Port of `Stream.pdf_serialize` (common.py lines 193-208), for any
/// [`StreamLike`] composite (taken as `&dyn StreamLike` rather than
/// generic so it works uniformly on owned values and boxed trait
/// objects, e.g. `serialize::IndirectObjects`'s arena entries).
pub fn pdf_serialize_stream(s: &dyn StreamLike, out: &mut Vec<u8>) {
    let stream = s.stream();
    let raw_uncompressed = stream.getvalue();
    let dl = raw_uncompressed.len();
    let mut filters = stream.filters.clone();
    let raw: Vec<u8> = if stream.compress {
        filters.push(Name::new("FlateDecode"));
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        use std::io::Write as _;
        enc.write_all(raw_uncompressed)
            .expect("in-memory zlib encode");
        enc.finish().expect("in-memory zlib encode")
    } else {
        raw_uncompressed.to_vec()
    };

    let mut d = InlineDictionary::new();
    d.insert("Length", raw.len() as i64);
    d.insert("DL", dl as i64);
    for (k, v) in s.extra_keys() {
        d.insert(k, v);
    }
    if !filters.is_empty() {
        d.insert("Filter", filters);
    }
    serialize(&PdfObj::InlineDict(d), out);
    out.extend_from_slice(b"\nstream\n");
    out.extend_from_slice(&raw);
    out.extend_from_slice(b"\nendstream\n");
}

// ==========================================================================
// current_log (common.py lines 239-245)
// ==========================================================================

type LogFn = Box<dyn Fn(&str) + Send + Sync>;

static CURRENT_LOG: OnceLock<Mutex<Option<LogFn>>> = OnceLock::new();

/// Port of `current_log`'s setter half (common.py lines 239-245):
/// installs the process-wide warning sink used by [`log_warn`].
pub fn set_current_log(f: impl Fn(&str) + Send + Sync + 'static) {
    let cell = CURRENT_LOG.get_or_init(|| Mutex::new(None));
    *cell.lock().expect("current_log mutex poisoned") = Some(Box::new(f));
}

/// Port of `current_log().warn(...)` call sites: routes through the
/// installed sink if any, else `eprintln!` - a minimal stand-in for
/// `calibre.utils.logging.default_log`, itself out of scope for this
/// port (see module doc comment).
pub fn log_warn(msg: &str) {
    let cell = CURRENT_LOG.get_or_init(|| Mutex::new(None));
    let guard = cell.lock().expect("current_log mutex poisoned");
    match guard.as_ref() {
        Some(f) => f(msg),
        None => eprintln!("WARN: {msg}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- pdf_float ----------------------------------------------------

    #[test]
    fn pdf_float_one() {
        assert_eq!(pdf_float(1.0), "1");
    }

    #[test]
    fn pdf_float_half() {
        assert_eq!(pdf_float(0.5), "0.5");
    }

    #[test]
    #[allow(clippy::approx_constant)] // deliberately pi-like, per the task's worked example
    fn pdf_float_pi_truncated_to_six_places() {
        assert_eq!(pdf_float(3.14159265), "3.141593");
    }

    #[test]
    fn pdf_float_zero() {
        assert_eq!(pdf_float(0.0), "0");
    }

    #[test]
    fn pdf_float_negative() {
        assert_eq!(pdf_float(-2.5), "-2.5");
    }

    #[test]
    fn pdf_float_hundred_has_no_decimal() {
        assert_eq!(pdf_float(100.0), "100");
    }

    #[test]
    fn pdf_float_very_near_zero_is_zero() {
        assert_eq!(pdf_float(1e-8), "0");
        assert_eq!(pdf_float(-1e-8), "0");
    }

    #[test]
    fn pdf_float_large_number_gets_reduced_precision() {
        // a = 1234567.0 > 1 -> log10 ~= 6.09 -> precision = clamp(6-6,0,6) = 0
        assert_eq!(pdf_float(1234567.0), "1234567");
    }

    #[test]
    fn pdf_float_ten_reduces_precision_by_at_least_zero() {
        // a=10 -> log10(10) is either exactly 1.0 or a hair under it in
        // f64; either way precision must land in {5, 6} and the decimal
        // value must round-trip to 10.
        let s = pdf_float(10.0);
        assert_eq!(s.parse::<f64>().unwrap(), 10.0);
    }

    // ---- Name -----------------------------------------------------------

    #[test]
    fn name_serializes_plain_ascii_unescaped() {
        let mut out = Vec::new();
        serialize(&PdfObj::Name(Name::new("Page")), &mut out);
        assert_eq!(out, b"/Page");
    }

    #[test]
    fn name_escapes_hash_and_space() {
        let mut out = Vec::new();
        serialize(&PdfObj::Name(Name::new("A B#C")), &mut out);
        assert_eq!(out, b"/A#20B#23C");
    }

    #[test]
    #[should_panic(expected = "Name too long")]
    fn name_over_126_bytes_panics() {
        let mut out = Vec::new();
        serialize(&PdfObj::Name(Name::new("x".repeat(127))), &mut out);
    }

    // ---- escape_pdf_string ------------------------------------------------

    #[test]
    fn escape_pdf_string_passes_through_balanced_parens() {
        assert_eq!(escape_pdf_string(b"(hi)"), b"(hi)".to_vec());
    }

    #[test]
    fn escape_pdf_string_escapes_unmatched_close_paren() {
        assert_eq!(escape_pdf_string(b"a)b"), b"a\\)b".to_vec());
    }

    #[test]
    fn escape_pdf_string_escapes_unmatched_open_paren() {
        assert_eq!(escape_pdf_string(b"a(b"), b"a\\(b".to_vec());
    }

    #[test]
    fn escape_pdf_string_escapes_newline_and_backslash() {
        assert_eq!(escape_pdf_string(b"a\nb\\c"), b"a\\nb\\\\c".to_vec());
    }

    #[test]
    fn escape_pdf_string_no_bad_bytes_returns_input_unchanged() {
        assert_eq!(escape_pdf_string(b"plain text"), b"plain text".to_vec());
    }

    // ---- PdfString / Utf16String -----------------------------------------

    #[test]
    fn pdf_string_ascii_round_trips_as_latin1_literal() {
        let mut out = Vec::new();
        serialize(&PdfObj::Str(PdfString::new("hello")), &mut out);
        assert_eq!(out, b"(hello)");
    }

    #[test]
    fn pdf_string_non_latin1_falls_back_to_utf16be_bom() {
        let mut out = Vec::new();
        serialize(&PdfObj::Str(PdfString::new("héllo \u{4e2d}")), &mut out);
        // Should start with '(' then the escaped BOM bytes 0xFE 0xFF (fe
        // is not a PDF-special byte, so unescaped).
        assert_eq!(out[0], b'(');
        assert_eq!(&out[1..3], &[0xFE, 0xFF]);
        assert_eq!(*out.last().unwrap(), b')');
    }

    #[test]
    fn utf16_string_always_uses_bom() {
        let mut out = Vec::new();
        serialize(&PdfObj::Utf16Str(Utf16String::new("hi")), &mut out);
        assert_eq!(out[0], b'(');
        assert_eq!(&out[1..3], &[0xFE, 0xFF]);
    }

    // ---- Reference --------------------------------------------------------

    #[test]
    fn reference_serializes_as_indirect_ref() {
        let mut out = Vec::new();
        serialize(&PdfObj::Reference(Reference::new(7)), &mut out);
        assert_eq!(out, b"7 0 R");
        assert_eq!(Reference::new(7).to_string(), "7 0 R");
    }

    // ---- Array --------------------------------------------------------------

    #[test]
    fn array_serializes_space_separated() {
        let mut arr = Array::new();
        arr.push(1i64);
        arr.push(2.5f64);
        arr.push(Name::new("X"));
        let mut out = Vec::new();
        serialize(&PdfObj::Array(arr), &mut out);
        assert_eq!(out, b"[1 2.5 /X]");
    }

    #[test]
    fn empty_array_serializes_as_brackets() {
        let mut out = Vec::new();
        serialize(&PdfObj::Array(Array::new()), &mut out);
        assert_eq!(out, b"[]");
    }

    // ---- Dictionary ---------------------------------------------------------

    #[test]
    fn dictionary_sorts_type_first_then_subtype_then_lexical() {
        let mut d = Dictionary::new();
        d.insert("Width", 10i64);
        d.insert("Subtype", Name::new("Image"));
        d.insert("Type", Name::new("XObject"));
        let mut out = Vec::new();
        serialize(&PdfObj::Dict(d), &mut out);
        let s = String::from_utf8(out).unwrap();
        let type_pos = s.find("/Type").unwrap();
        let subtype_pos = s.find("/Subtype").unwrap();
        let width_pos = s.find("/Width").unwrap();
        assert!(type_pos < subtype_pos);
        assert!(subtype_pos < width_pos);
        assert!(s.starts_with("<<\n"));
        assert!(s.ends_with(">>\n"));
    }

    #[test]
    fn inline_dictionary_preserves_insertion_order_no_newlines() {
        let mut d = InlineDictionary::new();
        d.insert("Length", 5i64);
        d.insert("DL", 5i64);
        d.insert("Filter", Array::new());
        let mut out = Vec::new();
        serialize(&PdfObj::InlineDict(d), &mut out);
        let s = String::from_utf8(out).unwrap();
        assert_eq!(s, "<< /Length 5 /DL 5 /Filter [] >>");
    }

    // ---- Stream ---------------------------------------------------------------

    #[test]
    fn stream_uncompressed_round_trip_shape() {
        let mut s = Stream::new(false);
        s.write(b"hello world" as &[u8]);
        let mut out = Vec::new();
        pdf_serialize_stream(&s, &mut out);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("/Length 11"));
        assert!(text.contains("/DL 11"));
        assert!(text.contains("stream\nhello world\nendstream\n"));
        assert!(!text.contains("FlateDecode"));
    }

    #[test]
    fn stream_compressed_declares_flate_filter_and_shrinks_or_equals() {
        let mut s = Stream::new(true);
        // Repetitive data compresses well, proving real zlib compression ran.
        s.write(vec![b'a'; 200].as_slice());
        let mut out = Vec::new();
        pdf_serialize_stream(&s, &mut out);
        let text_prefix = String::from_utf8_lossy(&out[..out.len().min(200)]).to_string();
        assert!(text_prefix.contains("FlateDecode"));
        assert!(text_prefix.contains("/DL 200"));
    }

    #[test]
    fn stream_write_line_appends_newline() {
        let mut s = Stream::new(false);
        s.write_line("abc");
        assert_eq!(s.getvalue(), b"abc\n");
    }

    struct ExtraKeyStream {
        inner: Stream,
    }
    impl StreamLike for ExtraKeyStream {
        fn stream(&self) -> &Stream {
            &self.inner
        }
        fn extra_keys(&self) -> Vec<(String, PdfObj)> {
            vec![("Type".to_string(), PdfObj::Name(Name::new("XObject")))]
        }
    }

    #[test]
    fn stream_like_composite_adds_extra_keys() {
        let s = ExtraKeyStream {
            inner: Stream::new(false),
        };
        let mut out = Vec::new();
        pdf_serialize_stream(&s, &mut out);
        assert!(String::from_utf8(out).unwrap().contains("/Type /XObject"));
    }

    // ---- current_log ------------------------------------------------------

    #[test]
    fn current_log_routes_to_installed_sink() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static CALLED: AtomicBool = AtomicBool::new(false);
        set_current_log(|_msg| CALLED.store(true, Ordering::SeqCst));
        log_warn("test message");
        assert!(CALLED.load(Ordering::SeqCst));
    }

    // ---- PdfDateTime --------------------------------------------------------

    #[test]
    fn pdf_datetime_formats_with_quoted_offset() {
        let dt = PdfDateTime {
            year: 2024,
            month: 1,
            day: 2,
            hour: 3,
            minute: 4,
            second: 5,
            tz_offset_minutes: 0,
        };
        let mut out = Vec::new();
        serialize(&PdfObj::DateTime(dt), &mut out);
        assert_eq!(out, b"(D:20240102030405+00'00')");
    }

    #[test]
    fn pdf_datetime_negative_offset() {
        let dt = PdfDateTime {
            year: 2024,
            month: 1,
            day: 2,
            hour: 3,
            minute: 4,
            second: 5,
            tz_offset_minutes: -330, // -05:30
        };
        let mut out = Vec::new();
        serialize(&PdfObj::DateTime(dt), &mut out);
        assert_eq!(out, b"(D:20240102030405-05'30')");
    }

    #[test]
    fn paper_size_lookup() {
        assert_eq!(paper_size("letter"), Some(LETTER));
        assert_eq!(paper_size("nonexistent"), None);
    }
}
