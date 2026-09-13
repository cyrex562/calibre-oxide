//! Port of `calibre.utils.fonts.sfnt.cff.dict_data` (issue #563, split
//! from #554/#65): the CFF DICT byte-code codec (`ByteCode`) and the
//! generic, TABLE-driven `Dict`/`TopDict`/`PrivateDict` structures
//! built on it.
//!
//! # No reflection: a static field-spec table instead
//!
//! Real upstream's `Dict.handle_operator`/`compile` dispatch by
//! string-building a method name (`getattr(self, 'arg_' + arg_type)`,
//! `getattr(self, 'encode_' + arg)`) and looking it up via Python's
//! object system. Rust has no equivalent reflection. This port encodes
//! the same information as a static `DictEntry` table (opcode, name,
//! [`ArgType`], numeric default) per real dict flavor
//! ([`TOP_DICT_SCHEMA`]/[`PRIVATE_DICT_SCHEMA`]), and
//! [`Dict::decompile`]/[`Dict::compile`] are ordinary `match`-based
//! functions over [`ArgType`] instead of dynamic dispatch. Same
//! observable behavior, no runtime name-based lookup.
//!
//! # `Operand::Int`/`Operand::Float`, not one `Num(f64)`
//!
//! A CFF DICT operand's on-the-wire *encoding* (compact 1-5 byte
//! integer forms vs. the nibble-packed real-number form) depends on
//! whether the value is a Python `int` or `float` at the call site
//! (`write_number`'s `isinstance(value, float)` check) -- not on its
//! numeric value. `7` and `7.0` are numerically equal but encode to
//! different bytes. Decoding preserves this: the small-int/short-int/
//! long-int byte forms always produce [`Operand::Int`]; the real-number
//! form always produces [`Operand::Float`] (the general Type2-charstring
//! fixed-1616 form doesn't apply to CFF DICTs at all -- see
//! [`OperandKind`]'s own doc). A single `f64`-only operand type would
//! silently lose this distinction and could re-encode a decompiled
//! value differently than the bytes it came from.
//!
//! # `arg_delta`'s real int/float promotion, preserved on both sides
//!
//! `arg_delta` accumulates a running sum (`current = current + v`);
//! Python's `int + float` promotes to `float` from that point on. This
//! port's [`decode_delta`] mirrors that exactly (an accumulator that
//! switches from `Operand::Int` to `Operand::Float` the first time a
//! `Float` input appears, and stays `Float` afterward), and
//! [`encode_delta`] (the reverse: re-diffing absolute values back to
//! deltas for `compile`) applies the same per-pair promotion rule.
//!
//! # Not ported: `SID`/table-reading integration
//!
//! `arg_SID`/`encode_SID` need a `strings` sequence indexable by SID
//! (decode) and a string->SID resolver (encode, real Python's own
//! `compile(self, strings)` where `strings` is actually a *callable*
//! there, confirmed by reading the call site: `val = strings(val)`).
//! Building and maintaining that CFF String INDEX (`table.py`'s own
//! `Strings`/`Index` classes) is issue #564's scope -- [`Dict::decompile`]/
//! [`Dict::compile`] take these as parameters (a slice, a closure)
//! rather than owning that machinery, so #564 can supply the real
//! implementation without this module needing to know about it.
//! `global_subrs` (real `decompile(self, strings, global_subrs, data)`'s
//! third parameter) is stored by real upstream but never read anywhere
//! in this file -- dropped from this port's signature; #564's own
//! future CharString-parsing code can carry it however it needs to
//! when it's actually built.

use crate::fonts::sfnt::errors::UnsupportedFont;

// ---------------------------------------------------------------------
// Operand / Value
// ---------------------------------------------------------------------

/// One decoded CFF DICT operand. See the module doc for why `Int`/
/// `Float` are distinct variants rather than one `f64`.
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Int(i64),
    Float(f64),
    /// A resolved string (`arg_SID`'s real upstream behavior: it
    /// resolves the SID to its string value immediately during
    /// decode, not lazily).
    Str(String),
}

impl Operand {
    fn as_f64(&self) -> Option<f64> {
        match self {
            Operand::Int(n) => Some(*n as f64),
            Operand::Float(f) => Some(*f),
            Operand::Str(_) => None,
        }
    }
}

/// A dict entry's value is always a flat list of operands: a `number`/
/// `SID` is a 1-element `Value`; `array`/`delta` are N-element; the two
/// real compound (tuple) argument shapes (`ROS`'s `(SID,SID,number)`,
/// `Private`'s `(number,number)`) are just N-element `Value`s whose
/// elements happen to be a mix of `Str`/`Int` -- this uniform
/// representation covers every real shape in both real tables without
/// needing a separate recursive/tagged tuple type.
pub type Value = Vec<Operand>;

// ---------------------------------------------------------------------
// ByteCode: the operand encoding
// ---------------------------------------------------------------------

/// Port of `real_nibbles`. Index 13 (nibble value `0xD`) is reserved/
/// unused by the spec; real upstream's own `read_real_number` would
/// raise an uncontrolled `TypeError` concatenating `None` if it were
/// ever seen (never happens for real font data) -- this port turns
/// that into a real, controlled [`UnsupportedFont`] error instead of
/// reproducing an incidental crash.
const REAL_NIBBLES: [Option<&str>; 15] = [
    Some("0"),
    Some("1"),
    Some("2"),
    Some("3"),
    Some("4"),
    Some("5"),
    Some("6"),
    Some("7"),
    Some("8"),
    Some("9"),
    Some("."),
    Some("E"),
    Some("E-"),
    None,
    Some("-"),
];

fn real_nibble_index(token: &str) -> Option<u8> {
    REAL_NIBBLES.iter().position(|n| *n == Some(token)).map(|i| i as u8)
}

/// Port of `ByteCode.read_real_number`.
fn read_real_number(data: &[u8], mut index: usize) -> Result<(f64, usize), UnsupportedFont> {
    let mut number = String::new();
    loop {
        let b = *data
            .get(index)
            .ok_or_else(|| UnsupportedFont("Truncated CFF DICT real number".to_string()))?;
        index += 1;
        let nibble0 = (b & 0xf0) >> 4;
        let nibble1 = b & 0x0f;
        if nibble0 == 0xf {
            break;
        }
        push_nibble(&mut number, nibble0)?;
        if nibble1 == 0xf {
            break;
        }
        push_nibble(&mut number, nibble1)?;
    }
    let value: f64 = number
        .parse()
        .map_err(|_| UnsupportedFont(format!("Invalid CFF DICT real number: {number:?}")))?;
    Ok((value, index))
}

fn push_nibble(number: &mut String, nibble: u8) -> Result<(), UnsupportedFont> {
    match REAL_NIBBLES[nibble as usize] {
        Some(s) => {
            number.push_str(s);
            Ok(())
        }
        None => Err(UnsupportedFont(format!("Reserved CFF DICT real-number nibble: {nibble:#x}"))),
    }
}

/// Port of `ByteCode.write_float`.
fn write_float(f: f64) -> Vec<u8> {
    let mut s = format!("{f}").to_uppercase();
    if let Some(rest) = s.strip_prefix("0.") {
        s = format!(".{rest}");
    } else if let Some(rest) = s.strip_prefix("-0.") {
        s = format!("-.{rest}");
    }
    let mut nibbles: Vec<u8> = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let (token, consumed) = if chars[i] == 'E' && chars.get(i + 1) == Some(&'-') {
            ("E-".to_string(), 2)
        } else {
            (chars[i].to_string(), 1)
        };
        i += consumed;
        if let Some(n) = real_nibble_index(&token) {
            nibbles.push(n);
        }
    }
    nibbles.push(0xf);
    if !nibbles.len().is_multiple_of(2) {
        nibbles.push(0xf);
    }
    let mut d = vec![30u8];
    for pair in nibbles.chunks(2) {
        d.push((pair[0] << 4) | pair[1]);
    }
    d
}

/// Port of `ByteCode.write_int`'s `encoding` parameter. Only `Cff` is
/// ever real reached from this module's own `compile()` (`write_number`'s
/// default); `T1` is kept for faithfulness to the shared upstream
/// `ByteCode` class (used elsewhere for Type1 fonts, not by this
/// module), and the implicit "neither" 2-byte short-int fallback is
/// upstream's own real behavior for that case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntEncoding {
    Cff,
    T1,
}

/// Port of `ByteCode.write_int`.
fn write_int(value: i64, encoding: IntEncoding) -> Vec<u8> {
    if (-107..=107).contains(&value) {
        vec![(value + 139) as u8]
    } else if (108..=1131).contains(&value) {
        let v = value - 108;
        vec![((v >> 8) + 247) as u8, (v & 0xFF) as u8]
    } else if (-1131..=-108).contains(&value) {
        let v = -value - 108;
        vec![((v >> 8) + 251) as u8, (v & 0xFF) as u8]
    } else {
        match encoding {
            IntEncoding::T1 => {
                let mut d = vec![255u8];
                d.extend_from_slice(&(value as i32).to_be_bytes());
                d
            }
            IntEncoding::Cff => {
                let mut d = vec![29u8];
                d.extend_from_slice(&(value as i32).to_be_bytes());
                d
            }
        }
    }
}

/// Port of `ByteCode.write_offset`.
fn write_offset(value: i64) -> Vec<u8> {
    let mut d = vec![29u8];
    d.extend_from_slice(&(value as i32).to_be_bytes());
    d
}

/// Port of `ByteCode.write_number`: dispatches on the operand's own
/// `Int`/`Float` tag (see the module doc), matching real upstream's
/// `isinstance(value, float)` check.
fn write_number(op: &Operand) -> Vec<u8> {
    match op {
        Operand::Int(n) => write_int(*n, IntEncoding::Cff),
        Operand::Float(f) => write_float(*f),
        Operand::Str(_) => Vec::new(),
    }
}

// ---------------------------------------------------------------------
// Dict schema: opcodes, argument shapes, defaults
// ---------------------------------------------------------------------

/// Port of a CFF DICT opcode: either a single byte, or the two-byte
/// `12 <n>` escape form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    Op(u8),
    Esc(u8),
}

/// The two real scalar argument kinds a compound (tuple) operator's
/// elements can be -- both real compound shapes (`ROS`, `Private`) are
/// built entirely from these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarArg {
    Number,
    Sid,
}

/// Port of each TABLE row's argument-type string (`'number'`, `'SID'`,
/// `'array'`, `'delta'`, or a tuple like `('SID','SID','number')`).
#[derive(Debug, Clone, Copy)]
pub enum ArgType {
    Number,
    Sid,
    Array,
    Delta,
    Tuple(&'static [ScalarArg]),
}

/// One row of a real `TABLE` tuple: `(opcode, name, argument_type, default)`.
#[derive(Debug, Clone, Copy)]
pub struct DictEntry {
    pub opcode: Opcode,
    pub name: &'static str,
    pub arg: ArgType,
    /// `None` matches real upstream's `default=None` (the field simply
    /// isn't written unless explicitly set to a non-`None` value).
    /// Real defaults are always numeric (never a string/SID) in both
    /// real tables, so this is `f64`, applied per-operand -- see
    /// [`Dict::compile`]'s equality check.
    pub default: Option<&'static [f64]>,
}

/// The real per-flavor schema: its field table plus the `FILTERED`/
/// `OFFSETS` name sets.
#[derive(Debug)]
pub struct DictSchema {
    pub table: &'static [DictEntry],
    /// Real upstream's `FILTERED`: fields that are decoded (so
    /// `decompile` must still handle their operators correctly) but
    /// never re-emitted by `compile`.
    pub filtered: &'static [&'static str],
    /// Real upstream's `OFFSETS`: fields whose number(s) must always be
    /// written via the 4-byte [`write_offset`] form, not the compact
    /// variable-length [`write_number`] form.
    pub offsets: &'static [&'static str],
}

use ArgType::{Array, Delta, Number, Sid, Tuple};

/// Port of `TopDict.TABLE`/`FILTERED`/`OFFSETS`.
pub static TOP_DICT_SCHEMA: DictSchema = DictSchema {
    table: &[
        DictEntry { opcode: Opcode::Esc(30), name: "ROS", arg: Tuple(&[ScalarArg::Sid, ScalarArg::Sid, ScalarArg::Number]), default: None },
        DictEntry { opcode: Opcode::Esc(20), name: "SyntheticBase", arg: Number, default: None },
        DictEntry { opcode: Opcode::Op(0), name: "version", arg: Sid, default: None },
        DictEntry { opcode: Opcode::Op(1), name: "Notice", arg: Sid, default: None },
        DictEntry { opcode: Opcode::Esc(0), name: "Copyright", arg: Sid, default: None },
        DictEntry { opcode: Opcode::Op(2), name: "FullName", arg: Sid, default: None },
        DictEntry { opcode: Opcode::Esc(38), name: "FontName", arg: Sid, default: None },
        DictEntry { opcode: Opcode::Op(3), name: "FamilyName", arg: Sid, default: None },
        DictEntry { opcode: Opcode::Op(4), name: "Weight", arg: Sid, default: None },
        DictEntry { opcode: Opcode::Esc(1), name: "isFixedPitch", arg: Number, default: Some(&[0.0]) },
        DictEntry { opcode: Opcode::Esc(2), name: "ItalicAngle", arg: Number, default: Some(&[0.0]) },
        DictEntry { opcode: Opcode::Esc(3), name: "UnderlinePosition", arg: Number, default: None },
        DictEntry { opcode: Opcode::Esc(4), name: "UnderlineThickness", arg: Number, default: Some(&[50.0]) },
        DictEntry { opcode: Opcode::Esc(5), name: "PaintType", arg: Number, default: Some(&[0.0]) },
        DictEntry { opcode: Opcode::Esc(6), name: "CharstringType", arg: Number, default: Some(&[2.0]) },
        DictEntry { opcode: Opcode::Esc(7), name: "FontMatrix", arg: Array, default: Some(&[0.001, 0.0, 0.0, 0.001, 0.0, 0.0]) },
        DictEntry { opcode: Opcode::Op(13), name: "UniqueID", arg: Number, default: None },
        DictEntry { opcode: Opcode::Op(5), name: "FontBBox", arg: Array, default: Some(&[0.0, 0.0, 0.0, 0.0]) },
        DictEntry { opcode: Opcode::Esc(8), name: "StrokeWidth", arg: Number, default: Some(&[0.0]) },
        DictEntry { opcode: Opcode::Op(14), name: "XUID", arg: Array, default: None },
        DictEntry { opcode: Opcode::Esc(21), name: "PostScript", arg: Sid, default: None },
        DictEntry { opcode: Opcode::Esc(22), name: "BaseFontName", arg: Sid, default: None },
        DictEntry { opcode: Opcode::Esc(23), name: "BaseFontBlend", arg: Delta, default: None },
        DictEntry { opcode: Opcode::Esc(31), name: "CIDFontVersion", arg: Number, default: Some(&[0.0]) },
        DictEntry { opcode: Opcode::Esc(32), name: "CIDFontRevision", arg: Number, default: Some(&[0.0]) },
        DictEntry { opcode: Opcode::Esc(33), name: "CIDFontType", arg: Number, default: Some(&[0.0]) },
        DictEntry { opcode: Opcode::Esc(34), name: "CIDCount", arg: Number, default: Some(&[8720.0]) },
        DictEntry { opcode: Opcode::Op(15), name: "charset", arg: Number, default: Some(&[0.0]) },
        DictEntry { opcode: Opcode::Esc(35), name: "UIDBase", arg: Number, default: None },
        DictEntry { opcode: Opcode::Op(16), name: "Encoding", arg: Number, default: Some(&[0.0]) },
        DictEntry { opcode: Opcode::Op(18), name: "Private", arg: Tuple(&[ScalarArg::Number, ScalarArg::Number]), default: None },
        DictEntry { opcode: Opcode::Esc(37), name: "FDSelect", arg: Number, default: None },
        DictEntry { opcode: Opcode::Esc(36), name: "FDArray", arg: Number, default: None },
        DictEntry { opcode: Opcode::Op(17), name: "CharStrings", arg: Number, default: None },
    ],
    filtered: &["ROS", "SyntheticBase", "UniqueID", "XUID", "CIDFontVersion", "CIDFontRevision", "CIDFontType", "CIDCount", "UIDBase", "FDSelect", "FDArray"],
    offsets: &["charset", "Encoding", "CharStrings", "Private"],
};

/// Port of `PrivateDict.TABLE`/`OFFSETS` (no `FILTERED`).
pub static PRIVATE_DICT_SCHEMA: DictSchema = DictSchema {
    table: &[
        DictEntry { opcode: Opcode::Op(6), name: "BlueValues", arg: Delta, default: None },
        DictEntry { opcode: Opcode::Op(7), name: "OtherBlues", arg: Delta, default: None },
        DictEntry { opcode: Opcode::Op(8), name: "FamilyBlues", arg: Delta, default: None },
        DictEntry { opcode: Opcode::Op(9), name: "FamilyOtherBlues", arg: Delta, default: None },
        DictEntry { opcode: Opcode::Esc(9), name: "BlueScale", arg: Number, default: Some(&[0.039625]) },
        DictEntry { opcode: Opcode::Esc(10), name: "BlueShift", arg: Number, default: Some(&[7.0]) },
        DictEntry { opcode: Opcode::Esc(11), name: "BlueFuzz", arg: Number, default: Some(&[1.0]) },
        DictEntry { opcode: Opcode::Op(10), name: "StdHW", arg: Number, default: None },
        DictEntry { opcode: Opcode::Op(11), name: "StdVW", arg: Number, default: None },
        DictEntry { opcode: Opcode::Esc(12), name: "StemSnapH", arg: Delta, default: None },
        DictEntry { opcode: Opcode::Esc(13), name: "StemSnapV", arg: Delta, default: None },
        DictEntry { opcode: Opcode::Esc(14), name: "ForceBold", arg: Number, default: Some(&[0.0]) },
        DictEntry { opcode: Opcode::Esc(15), name: "ForceBoldThreshold", arg: Number, default: None },
        DictEntry { opcode: Opcode::Esc(16), name: "lenIV", arg: Number, default: None },
        DictEntry { opcode: Opcode::Esc(17), name: "LanguageGroup", arg: Number, default: Some(&[0.0]) },
        DictEntry { opcode: Opcode::Esc(18), name: "ExpansionFactor", arg: Number, default: Some(&[0.06]) },
        DictEntry { opcode: Opcode::Esc(19), name: "initialRandomSeed", arg: Number, default: Some(&[0.0]) },
        DictEntry { opcode: Opcode::Op(20), name: "defaultWidthX", arg: Number, default: Some(&[0.0]) },
        DictEntry { opcode: Opcode::Op(21), name: "nominalWidthX", arg: Number, default: Some(&[0.0]) },
        DictEntry { opcode: Opcode::Op(19), name: "Subrs", arg: Number, default: None },
    ],
    filtered: &[],
    offsets: &["Subrs"],
};

// ---------------------------------------------------------------------
// Dict: decompile / compile
// ---------------------------------------------------------------------

/// Port of `Dict`/`TopDict`/`PrivateDict` (the schema selects which).
#[derive(Debug)]
pub struct Dict {
    schema: &'static DictSchema,
    values: std::collections::HashMap<&'static str, Value>,
}

impl Dict {
    pub fn new(schema: &'static DictSchema) -> Self {
        Dict { schema, values: std::collections::HashMap::new() }
    }

    pub fn top() -> Self {
        Self::new(&TOP_DICT_SCHEMA)
    }

    pub fn private() -> Self {
        Self::new(&PRIVATE_DICT_SCHEMA)
    }

    /// Port of `Dict.get` (dict's own real `.get`, not `safe_get`) --
    /// the value only if explicitly set, no default fallback.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(name)
    }

    /// Port of `Dict.safe_get`.
    pub fn safe_get(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.values.get(name) {
            return Some(v.clone());
        }
        self.entry(name).and_then(|e| e.default).map(|nums| nums.iter().map(|&n| Operand::Float(n)).collect())
    }

    fn entry(&self, name: &str) -> Option<&'static DictEntry> {
        self.schema.table.iter().find(|e| e.name == name)
    }

    fn entry_for_opcode(&self, opcode: Opcode) -> Option<&'static DictEntry> {
        self.schema.table.iter().find(|e| e.opcode == opcode)
    }

    /// Port of `Dict.decompile`. `strings` is a flat SID-indexable
    /// table (real upstream's `Strings`, #564's scope to build); see
    /// the module doc for why `global_subrs` isn't a parameter here.
    pub fn decompile(&mut self, strings: &[String], data: &[u8]) -> Result<(), UnsupportedFont> {
        let mut stack: Vec<Operand> = Vec::new();
        let mut index = 0usize;
        while index < data.len() {
            let b0 = data[index];
            index += 1;
            match cff_dict_operand_encoding(b0) {
                OperandKind::Operator => {
                    let opcode = if b0 == 12 {
                        let b1 = *data
                            .get(index)
                            .ok_or_else(|| UnsupportedFont("Truncated CFF DICT escape operator".to_string()))?;
                        index += 1;
                        Opcode::Esc(b1)
                    } else {
                        Opcode::Op(b0)
                    };
                    self.do_operator(opcode, &mut stack, strings)?;
                }
                OperandKind::Byte => stack.push(Operand::Int(b0 as i64 - 139)),
                OperandKind::SmallInt1 => {
                    let b1 = next_byte(data, &mut index)?;
                    stack.push(Operand::Int((b0 as i64 - 247) * 256 + b1 as i64 + 108));
                }
                OperandKind::SmallInt2 => {
                    let b1 = next_byte(data, &mut index)?;
                    stack.push(Operand::Int(-(b0 as i64 - 251) * 256 - b1 as i64 - 108));
                }
                OperandKind::ShortInt => {
                    let bytes = next_bytes::<2>(data, &mut index)?;
                    stack.push(Operand::Int(i16::from_be_bytes(bytes) as i64));
                }
                OperandKind::LongInt => {
                    let bytes = next_bytes::<4>(data, &mut index)?;
                    stack.push(Operand::Int(i32::from_be_bytes(bytes) as i64));
                }
                OperandKind::RealNumber => {
                    let (value, new_index) = read_real_number(data, index)?;
                    index = new_index;
                    stack.push(Operand::Float(value));
                }
                OperandKind::Reserved => {
                    return Err(UnsupportedFont(format!("Reserved CFF DICT operand byte: {b0:#x}")));
                }
            }
        }
        Ok(())
    }

    fn do_operator(&mut self, opcode: Opcode, stack: &mut Vec<Operand>, strings: &[String]) -> Result<(), UnsupportedFont> {
        let entry = self
            .entry_for_opcode(opcode)
            .ok_or_else(|| UnsupportedFont(format!("Unknown CFF DICT operator: {opcode:?}")))?;
        let value = match entry.arg {
            Number => vec![pop(stack)?],
            Sid => {
                let sid = pop_int(stack)?;
                vec![Operand::Str(resolve_sid(strings, sid)?)]
            }
            Array => std::mem::take(stack),
            Delta => decode_delta(std::mem::take(stack)),
            Tuple(scalars) => {
                let mut vals: Vec<Operand> = Vec::with_capacity(scalars.len());
                vals.resize(scalars.len(), Operand::Int(0));
                for i in (0..scalars.len()).rev() {
                    vals[i] = match scalars[i] {
                        ScalarArg::Number => pop(stack)?,
                        ScalarArg::Sid => {
                            let sid = pop_int(stack)?;
                            Operand::Str(resolve_sid(strings, sid)?)
                        }
                    };
                }
                vals
            }
        };
        self.values.insert(entry.name, value);
        Ok(())
    }

    /// Port of `Dict.compile`. `resolve_sid` is real upstream's own
    /// `strings` callable parameter (a string -> SID resolver; #564's
    /// scope to build the real String INDEX behind it).
    pub fn compile(&self, resolve_sid: &dyn Fn(&str) -> u16) -> Vec<u8> {
        let mut data = Vec::new();
        for entry in self.schema.table {
            if self.schema.filtered.contains(&entry.name) {
                continue;
            }
            let Some(val) = self.safe_get(entry.name) else { continue };
            if values_equal_default(&val, entry.default) {
                continue;
            }
            let is_offset = self.schema.offsets.contains(&entry.name);
            encode_value(&mut data, entry.arg, &val, is_offset, resolve_sid);
            match entry.opcode {
                Opcode::Op(b) => data.push(b),
                Opcode::Esc(b) => {
                    data.push(12);
                    data.push(b);
                }
            }
        }
        data
    }
}

fn next_byte(data: &[u8], index: &mut usize) -> Result<u8, UnsupportedFont> {
    let b = *data.get(*index).ok_or_else(|| UnsupportedFont("Truncated CFF DICT operand".to_string()))?;
    *index += 1;
    Ok(b)
}

fn next_bytes<const N: usize>(data: &[u8], index: &mut usize) -> Result<[u8; N], UnsupportedFont> {
    let slice = data
        .get(*index..*index + N)
        .ok_or_else(|| UnsupportedFont("Truncated CFF DICT operand".to_string()))?;
    *index += N;
    Ok(slice.try_into().unwrap())
}

fn pop(stack: &mut Vec<Operand>) -> Result<Operand, UnsupportedFont> {
    stack.pop().ok_or_else(|| UnsupportedFont("CFF DICT operator with too few operands".to_string()))
}

fn pop_int(stack: &mut Vec<Operand>) -> Result<i64, UnsupportedFont> {
    match pop(stack)? {
        Operand::Int(n) => Ok(n),
        Operand::Float(f) => Ok(f as i64),
        Operand::Str(_) => Err(UnsupportedFont("Expected a numeric SID operand".to_string())),
    }
}

fn resolve_sid(strings: &[String], sid: i64) -> Result<String, UnsupportedFont> {
    usize::try_from(sid)
        .ok()
        .and_then(|i| strings.get(i))
        .cloned()
        .ok_or_else(|| UnsupportedFont(format!("SID out of range: {sid}")))
}

/// Port of `arg_delta`'s real int/float promotion -- see the module doc.
fn decode_delta(input: Vec<Operand>) -> Vec<Operand> {
    let mut out = Vec::with_capacity(input.len());
    let mut current_i: i64 = 0;
    let mut current_f: f64 = 0.0;
    let mut is_float = false;
    for v in input {
        match v {
            Operand::Int(n) if !is_float => {
                current_i += n;
                out.push(Operand::Int(current_i));
            }
            other => {
                if !is_float {
                    current_f = current_i as f64;
                    is_float = true;
                }
                current_f += other.as_f64().unwrap_or(0.0);
                out.push(Operand::Float(current_f));
            }
        }
    }
    out
}

/// The reverse of [`decode_delta`], for `compile`.
fn encode_delta(val: &Value) -> Vec<Operand> {
    let mut out = Vec::with_capacity(val.len());
    let mut last = 0f64;
    let mut last_is_float = false;
    for v in val {
        let (cur, cur_is_float) = match v {
            Operand::Int(n) => (*n as f64, false),
            Operand::Float(f) => (*f, true),
            Operand::Str(_) => (0.0, false),
        };
        let diff = cur - last;
        if cur_is_float || last_is_float {
            out.push(Operand::Float(diff));
        } else {
            out.push(Operand::Int(diff as i64));
        }
        last = cur;
        last_is_float = cur_is_float;
    }
    out
}

fn values_equal_default(val: &Value, default: Option<&[f64]>) -> bool {
    match default {
        None => false,
        Some(defaults) => {
            val.len() == defaults.len()
                && val.iter().zip(defaults.iter()).all(|(v, d)| v.as_f64() == Some(*d))
        }
    }
}

fn encode_number(op: &Operand, is_offset: bool) -> Vec<u8> {
    if is_offset {
        let n = match op {
            Operand::Int(n) => *n,
            Operand::Float(f) => *f as i64,
            Operand::Str(_) => 0,
        };
        write_offset(n)
    } else {
        write_number(op)
    }
}

fn encode_value(out: &mut Vec<u8>, arg: ArgType, val: &Value, is_offset: bool, resolve_sid: &dyn Fn(&str) -> u16) {
    match arg {
        Number => out.extend(encode_number(&val[0], is_offset)),
        Sid => {
            if let Some(Operand::Str(s)) = val.first() {
                out.extend(write_int(resolve_sid(s) as i64, IntEncoding::Cff));
            }
        }
        Array => {
            for v in val {
                out.extend(encode_number(v, is_offset));
            }
        }
        Delta => {
            for v in encode_delta(val) {
                out.extend(encode_number(&v, is_offset));
            }
        }
        Tuple(scalars) => {
            for (v, scalar) in val.iter().zip(scalars) {
                match scalar {
                    ScalarArg::Number => out.extend(encode_number(v, is_offset)),
                    ScalarArg::Sid => {
                        if let Operand::Str(s) = v {
                            out.extend(write_int(resolve_sid(s) as i64, IntEncoding::Cff));
                        }
                    }
                }
            }
        }
    }
}

/// Port of `cff_dict_operand_encoding` (`t1_operand_encoding` extended
/// by `t2_operand_encoding` extended by the CFF-DICT-specific final
/// overrides), expressed as a function over byte ranges instead of a
/// 256-entry lookup table of method-name strings.
///
/// `t2_operand_encoding[255] = 'read_fixed_1616'`, but the CFF-DICT-
/// specific final layer immediately overrides it back to `'reserved'`
/// (`cff_dict_operand_encoding[255] = 'reserved'`) -- so byte `255`
/// always means [`OperandKind::Reserved`] in a DICT, and no
/// `Fixed1616` variant/handler exists here at all (it would be
/// genuinely unreachable dead code for this table specifically, even
/// though the general Type2-charstring encoding this DICT table is
/// layered on top of does use it elsewhere).
enum OperandKind {
    Operator,
    Byte,
    SmallInt1,
    SmallInt2,
    ShortInt,
    LongInt,
    RealNumber,
    Reserved,
}

fn cff_dict_operand_encoding(b0: u8) -> OperandKind {
    match b0 {
        0..=27 => OperandKind::Operator,
        28 => OperandKind::ShortInt,
        29 => OperandKind::LongInt,
        30 => OperandKind::RealNumber,
        31 => OperandKind::Operator,
        32..=246 => OperandKind::Byte,
        247..=250 => OperandKind::SmallInt1,
        251..=254 => OperandKind::SmallInt2,
        255 => OperandKind::Reserved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn write_int_covers_every_real_range() {
        assert_eq!(write_int(0, IntEncoding::Cff), vec![139]);
        assert_eq!(write_int(107, IntEncoding::Cff), vec![246]);
        assert_eq!(write_int(-107, IntEncoding::Cff), vec![32]);
        assert_eq!(write_int(108, IntEncoding::Cff), vec![247, 0]);
        assert_eq!(write_int(1131, IntEncoding::Cff), vec![250, 255]);
        assert_eq!(write_int(-108, IntEncoding::Cff), vec![251, 0]);
        assert_eq!(write_int(-1131, IntEncoding::Cff), vec![254, 255]);
        assert_eq!(write_int(1132, IntEncoding::Cff), vec![29, 0, 0, 4, 108]);
        assert_eq!(write_int(1132, IntEncoding::T1), vec![255, 0, 0, 4, 108]);
    }

    #[test]
    fn write_and_read_real_number_round_trip() {
        for value in [0.001_f64, -2.5, 7.0, 100.0, -0.06] {
            let bytes = write_float(value);
            assert_eq!(bytes[0], 30);
            let (decoded, consumed) = read_real_number(&bytes, 1).unwrap();
            assert_eq!(consumed, bytes.len());
            assert!((decoded - value).abs() < 1e-9, "value={value} decoded={decoded}");
        }
    }

    #[test]
    fn small_int_round_trips_via_full_decompile() {
        // Encode `version` (opcode 0, SID arg) with a small-int1 SID.
        let strings = vec![s(".notdef"), s("hello")];
        let data = write_int(1, IntEncoding::Cff).into_iter().chain([0u8]).collect::<Vec<u8>>();
        let mut d = Dict::top();
        d.decompile(&strings, &data).unwrap();
        assert_eq!(d.get("version"), Some(&vec![Operand::Str(s("hello"))]));
    }

    #[test]
    fn decompile_resolves_short_and_long_ints() {
        let strings: Vec<String> = vec![];
        // isFixedPitch (12 1) with a short-int operand (28-prefixed).
        let mut data = vec![28u8];
        data.extend_from_slice(&300i16.to_be_bytes());
        data.push(12);
        data.push(1);
        let mut d = Dict::top();
        d.decompile(&strings, &data).unwrap();
        assert_eq!(d.get("isFixedPitch"), Some(&vec![Operand::Int(300)]));
    }

    #[test]
    fn decompile_and_compile_a_private_tuple_round_trips() {
        // Private = (number, number) -- two small-ints then opcode 18.
        let strings: Vec<String> = vec![];
        let mut data = write_int(45, IntEncoding::Cff);
        data.extend(write_int(1000, IntEncoding::Cff));
        data.push(18);
        let mut d = Dict::top();
        d.decompile(&strings, &data).unwrap();
        assert_eq!(d.get("Private"), Some(&vec![Operand::Int(45), Operand::Int(1000)]));

        let out = d.compile(&|_| 0);
        // Private is in OFFSETS -> both numbers must be 4-byte forms (5 bytes each incl. opcode 29).
        assert_eq!(out.len(), 5 + 5 + 1);
        assert_eq!(out[out.len() - 1], 18);
    }

    #[test]
    fn delta_decode_and_encode_round_trip_all_integers() {
        // BlueValues (opcode 6, delta arg): three small-ints, cumulative.
        let strings: Vec<String> = vec![];
        let mut data = write_int(10, IntEncoding::Cff);
        data.extend(write_int(5, IntEncoding::Cff));
        data.extend(write_int(20, IntEncoding::Cff));
        data.push(6);
        let mut d = Dict::private();
        d.decompile(&strings, &data).unwrap();
        // Absolute values: 10, 15, 35.
        assert_eq!(d.get("BlueValues"), Some(&vec![Operand::Int(10), Operand::Int(15), Operand::Int(35)]));

        let out = d.compile(&|_| 0);
        assert_eq!(out, data, "re-encoding the same absolute values must reproduce the original relative-delta bytes");
    }

    #[test]
    fn delta_decode_promotes_to_float_once_a_float_operand_appears() {
        let deltas = vec![Operand::Int(5), Operand::Float(2.5), Operand::Int(1)];
        let out = decode_delta(deltas);
        assert_eq!(out[0], Operand::Int(5));
        assert_eq!(out[1], Operand::Float(7.5));
        assert_eq!(out[2], Operand::Float(8.5));
    }

    #[test]
    fn compile_omits_fields_left_at_their_default() {
        let d = Dict::top();
        let out = d.compile(&|_| 0);
        assert!(out.is_empty(), "an empty TopDict must compile to zero bytes");
    }

    #[test]
    fn compile_writes_a_non_default_scalar_and_resolves_its_sid() {
        let mut d = Dict::top();
        d.decompile(&[s("dummy"), s("1.0")], &{
            let mut data = write_int(1, IntEncoding::Cff); // version = SID 1 ("1.0")
            data.push(0);
            data
        })
        .unwrap();
        let out = d.compile(&|name| if name == "1.0" { 42 } else { 0 });
        // encode_SID(42) via write_int -> single byte 42+139=181, then opcode 0.
        assert_eq!(out, vec![181, 0]);
    }

    #[test]
    fn filtered_fields_are_decoded_but_never_recompiled() {
        // ROS is FILTERED: decode 2 SIDs + 1 number, opcode (12,30).
        let strings = vec![s("Adobe"), s("Identity")];
        let mut data = write_int(0, IntEncoding::Cff);
        data.extend(write_int(1, IntEncoding::Cff));
        data.extend(write_int(0, IntEncoding::Cff));
        data.push(12);
        data.push(30);
        let mut d = Dict::top();
        d.decompile(&strings, &data).unwrap();
        assert!(d.get("ROS").is_some());
        let out = d.compile(&|_| 0);
        assert!(out.is_empty(), "ROS must never be re-emitted by compile()");
    }
}
