//! Port of `old_src/src/calibre/ebooks/rtf2xml/hex_2_utf8.py` (`Hex2Utf8`).
//!
//! Resolves the `tx<hx<__________<'HH` hex-byte lines and (in the body
//! pass) `tx<mc<__________<'HH` lines emitted by [`super::process_tokens`]
//! (see that module's docs for the exact intermediate-format shapes) to
//! either plain text (`tx<nu<__________<...`) or an already-resolved XML
//! entity (`tx<ut<__________<...`), by looking each hex byte up in a
//! character map built from [`super::char_set`]/[`super::get_char_map`].
//!
//! # Two call sites, one object, two configurations
//!
//! In the real pipeline (`ParseRtf.py`) a single `Hex2Utf8` object is
//! used twice:
//!
//! 1. Checkpoint `hex_2_utf_preamble` -- constructed with
//!    `area_to_convert='preamble'`, then `convert_hex_2_utf8()` is
//!    called. Only resolves hex bytes up through the `\body-open_`
//!    marker; everything from there to end of file passes through
//!    untouched (see [`AreaToConvert::Preamble`] / [`convert_preamble`]).
//! 2. Much later, after many other passes: `update_values(file=...,
//!    area_to_convert='body', ..., symbol=1, wingdings=1, dingbats=1)`
//!    reconfigures the *same* object, then `convert_hex_2_utf8()` is
//!    called again -- this time resolving hex bytes for the whole file,
//!    with font-aware dictionary switching (`Symbol`/`Wingdings`/`Zapf
//!    Dingbats`), caps handling, and non-hex text/entity passthrough
//!    (see [`AreaToConvert::Body`] / [`convert_body`]).
//!
//! [`Hex2Utf8`] models that shared, reconfigurable object: [`Hex2Utf8::new`]
//! is the preamble-pass constructor, [`Hex2Utf8::update_values`] mutates it
//! in place for the body pass exactly as the Python method does, and
//! [`Hex2Utf8::convert_hex_2_utf8`] dispatches on whichever
//! [`AreaToConvert`] is currently configured. Only call site 1's caller is
//! in scope for this issue, but both configurations (and both
//! `convert_hex_2_utf8` code paths) are fully ported and tested.
//!
//! Matching [`super::check_brackets`]'s convention (and the crate-level
//! module docs' note on this issue's passes generally), this operates on
//! `&str` intermediate-format content in and an owned `String` out --
//! the temp-file/[`super::copy`]/rename plumbing around each pass in the
//! real pipeline is not ported.
//!
//! # Deliberately not modeled: dead/inert Python state
//!
//! - **`in_file`/`char_file`/`copy`/`temp_dir`/`bug_handler`/
//!   `invalid_rtf_handler` constructor arguments**: out-of-scope file/
//!   debug-copy plumbing, per this module's and [`super::check_brackets`]'s
//!   convention. `char_file` doubly so: `__initiate_values` unconditionally
//!   *overwrites* `self.__char_file` with `io.StringIO(char_set)` before
//!   ever reading it, so whatever the constructor was given is discarded
//!   without ever being used for anything.
//! - **The constructor's `convert_caps` argument**: `Hex2Utf8.__init__`
//!   accepts it but then unconditionally sets `self.__convert_caps = 0`
//!   two lines later, discarding it -- verified by inspection of the
//!   Python (`self.__convert_caps = 0` follows the assignment-free
//!   parameter list with no branch on `convert_caps` anywhere in
//!   `__init__`). The real (`update_values`-driven) `convert_caps`
//!   configuration is exposed on [`Hex2Utf8::update_values`] instead,
//!   which does honor its own `convert_caps` argument.
//! - **The constructor's/`update_values`'s `symbol`/`wingdings`/`dingbats`/
//!   `caps` arguments, on the *constructor* specifically**: even though
//!   `__init__` does store whatever `symbol`/`wingdings`/`dingbats` it's
//!   given (unlike `convert_caps`), those flags only gate whether
//!   `__initiate_values` *builds* the corresponding font dictionaries --
//!   never whether they get *used*, which is gated entirely by
//!   `convert_symbol`/`convert_wingdings`/`convert_zapf`. Since those are
//!   always hardcoded to `0` straight out of the constructor (previous
//!   bullet), nothing reachable through a freshly-constructed object can
//!   ever select a font dictionary regardless of `symbol`/`wingdings`/
//!   `dingbats`, making them fully inert at construction time. (The real
//!   preamble call site in `ParseRtf.py` doesn't even pass them.)
//!   [`Hex2Utf8::new`] therefore omits them; [`Hex2Utf8::update_values`]
//!   (where they *do* matter) takes its own `symbol`/`wingdings`/
//!   `dingbats` arguments.
//! - **`caps` on `update_values` too**: stored into `self.__caps` and
//!   never read again anywhere in the file (grep-verified against the
//!   whole module) -- a write-only field both call sites populate for no
//!   observable effect. Not modeled at all. (Not to be confused with the
//!   *body pass's own* caps-list stack, `self.__caps_list`/
//!   `self.__in_caps`-driving `mi<mk<caps______`/`mi<mk<caps-end__`
//!   markers, which is very much live -- see [`State::caps_list`].)
//! - **`self.__in_caps`/`self.__special_fonts_found`**: set in
//!   `__initiate_values`/`__start_caps_func`/`__end_special_font_func` but
//!   never *read* anywhere in the file -- write-only dead state.
//! - **`__start_special_font_func_old`, `__end_special_font_func`,
//!   `__start_caps_func_old`**: never referenced by any of the three
//!   token-dispatch dictionaries (`__preamble_state_dict`,
//!   `__body_state_dict`, `__in_body_dict`), so they are unreachable dead
//!   code -- and `__start_special_font_func_old`/`__end_special_font_func`
//!   are additionally broken on their own terms (they call `.append`/
//!   `.pop` on `self.__current_dict`, which is always a `dict`, not a
//!   `list`, so even a hypothetical direct call would raise
//!   `AttributeError`). Not ported.
//! - **The "no state found" `sys.stderr.write` calls** in
//!   `__convert_preamble`/`__convert_body`: guarded by `action is None`,
//!   which can never happen -- the outer dispatch key is always
//!   `self.__state`, which only ever holds `'preamble'`/`'body'`, both of
//!   which are always-present keys in the relevant dispatch dict. (They're
//!   also two-argument `sys.stderr.write(msg, state)` calls, which would
//!   themselves raise `TypeError` -- `write` takes one argument -- making
//!   this simultaneously unreachable *and* buggy on the hypothetical path
//!   that would reach it.) Modeled here with an explicit two-state model
//!   ([`convert_preamble`]'s `in_preamble: bool`; [`convert_body`] has no
//!   state transition at all -- see next bullet) that has no "unknown
//!   state" case to begin with.
//!
//! # Preserved quirk: `__preamble_for_body_func` is unreachable
//!
//! `__convert_body` unconditionally sets `self.__state = 'body'` once,
//! before its line loop, and nothing inside the body-pass call graph ever
//! reassigns `self.__state` again. `__body_state_dict['preamble']`
//! (`__preamble_for_body_func`) can therefore never be selected in
//! practice -- every line of a body pass runs through
//! `__body_for_body_func` (`__in_body_dict`) only. [`convert_body`]
//! reflects this directly: it has no state field and always calls the
//! [`in_body_dict`]-equivalent dispatch, matching real behavior rather
//! than reproducing an unreachable branch.
//!
//! # Preserved quirk: font/caps marker lines vanish from body-pass output
//!
//! `__start_font_func`, `__end_font_func`, `__start_caps_func`, and
//! `__end_caps_func` update dictionary/caps-stack state but -- unlike
//! every other handler in `__in_body_dict` (e.g. `__found_body_func`,
//! which still calls `self.__write_obj.write(line)`) -- never write their
//! own line to output. The `mi<mk<font______`/`mi<mk<font-end__`/
//! `mi<mk<caps______`/`mi<mk<caps-end__` marker lines are consumed and do
//! not appear in a body pass's output. Verified in
//! `body_font_and_caps_markers_are_dropped_from_output` below.
//!
//! # Preserved quirk: hex bytes with no dictionary entry and a small code
//! point are silently dropped
//!
//! `__hex_text_func`'s not-found branch only ever writes its
//! `mi<tg<empty-att_<udef_symbol<num>...<description>not-in-table`
//! diagnostic line (and, above `run_level` 4, raises) when the
//! (quote-stripped) hex value parses to *more* than 10 -- for hex values
//! `<= 10` (i.e. control codes `'00`..`'0A`) that aren't in the active
//! dictionary, nothing is written at all: the token is silently dropped
//! from the output stream. Verified in
//! `hex_byte_with_small_missing_code_point_is_silently_dropped` below.
//!
//! # Preserved quirk: caps-uppercasing skips `Symbol`/`Wingdings`/`Zapf Dingbats` runs
//!
//! Every caps-uppercasing check (`__hex_text_func`, `__text_func`,
//! `__utf_to_caps_func`) additionally requires the active font not be one
//! of the three special fonts -- glyph code points in those fonts are not
//! letters, so uppercasing them would corrupt them. See
//! [`State::is_special_font`].

use std::collections::HashMap;

use thiserror::Error;

use super::get_char_map::{get_char_map, MapNotFoundError};

/// Port of the `area_to_convert` flag. Python validates this at runtime
/// (`if area_to_convert not in ('preamble', 'body'): raise
/// self.__bug_handler(...)`) since it's just a string; modeling it as an
/// enum instead makes the invalid case a compile-time impossibility, so
/// that runtime check/error has no Rust equivalent here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaToConvert {
    Preamble,
    Body,
}

/// Port of the two genuinely reachable `raise self.__bug_handler(msg)`/
/// error paths in this file (see the module docs for the ones that are
/// dead code and thus have no Rust equivalent).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Hex2Utf8Error {
    /// Port of `GetCharMap.get_char_map`'s raise, reachable here only if
    /// `default_char_map` names a codepage/map absent from
    /// [`super::char_set::CHAR_SET`] (the other map names this module
    /// requests are fixed literals verified present by
    /// [`super::char_set`]'s own tests).
    #[error(transparent)]
    MapNotFound(#[from] MapNotFoundError),
    /// Port of `__hex_text_func`'s `run_level > 4` gated raise: a hex
    /// byte with no dictionary entry and a (quote-stripped) parsed value
    /// greater than 10.
    #[error("Character \"&#x{0};\" does not appear to be valid (or is a control character)\n")]
    InvalidHexChar(String),
}

/// Port of `Hex2Utf8`'s reconfigurable object state -- the fields
/// `update_values` can change for the body pass. See the module docs for
/// why `char_file`/`copy`/`temp_dir`/the constructor's `convert_caps`/
/// `symbol`/`wingdings`/`dingbats`/`caps` are not represented.
#[derive(Debug, Clone)]
pub struct Hex2Utf8 {
    area_to_convert: AreaToConvert,
    /// Port of `self.__default_char_map`: the codepage/map name used for
    /// the "upper 128" half of the default dictionary (e.g.
    /// `"ansicpg1252"`). Set once at construction and never touched by
    /// `update_values` -- both the preamble and body passes share the
    /// same default-encoding table determined once, upstream, by
    /// `DefaultEncoding.find_default_encoding()`.
    default_char_map: String,
    symbol: bool,
    wingdings: bool,
    dingbats: bool,
    convert_caps: bool,
    convert_symbol: bool,
    convert_wingdings: bool,
    convert_zapf: bool,
    run_level: u32,
}

impl Hex2Utf8 {
    /// Port of `Hex2Utf8.__init__` as actually exercised by the in-scope
    /// preamble call site (`ParseRtf.py`'s `hex_2_utf_preamble`
    /// checkpoint), which passes only `area_to_convert`, `default_char_map`,
    /// and `run_level` -- every other Python constructor parameter is
    /// either out-of-scope plumbing or provably inert at construction time
    /// (see the module docs).
    pub fn new(
        area_to_convert: AreaToConvert,
        default_char_map: impl Into<String>,
        run_level: u32,
    ) -> Self {
        Self {
            area_to_convert,
            default_char_map: default_char_map.into(),
            symbol: false,
            wingdings: false,
            dingbats: false,
            // Port of `self.__convert_caps = 0` (and the analogous
            // hardcoded-0 lines for the other three `convert_*` flags)
            // in `__init__` -- always off regardless of any
            // (unmodeled) constructor argument. See module docs.
            convert_caps: false,
            convert_symbol: false,
            convert_wingdings: false,
            convert_zapf: false,
            run_level,
        }
    }

    /// Port of `Hex2Utf8.update_values`, reconfiguring an existing object
    /// for the body pass. Unlike the constructor, every `convert_*` flag
    /// here is honored as given (this is the only place they can ever
    /// become `true`), and `symbol`/`wingdings`/`dingbats` now actually
    /// matter (they gate which font dictionaries [`Hex2Utf8::convert_hex_2_utf8`]
    /// builds). `default_char_map` is deliberately left unchanged --
    /// Python's `update_values` doesn't take it either. The Python's dead
    /// `caps` parameter (see module docs) is not represented.
    #[allow(clippy::too_many_arguments)] // mirrors Hex2Utf8.update_values's own parameter list
    pub fn update_values(
        &mut self,
        area_to_convert: AreaToConvert,
        convert_caps: bool,
        convert_symbol: bool,
        convert_wingdings: bool,
        convert_zapf: bool,
        symbol: bool,
        wingdings: bool,
        dingbats: bool,
    ) {
        self.area_to_convert = area_to_convert;
        self.symbol = symbol;
        self.wingdings = wingdings;
        self.dingbats = dingbats;
        self.convert_caps = convert_caps;
        self.convert_symbol = convert_symbol;
        self.convert_wingdings = convert_wingdings;
        self.convert_zapf = convert_zapf;
    }

    /// Port of `Hex2Utf8.convert_hex_2_utf8` (+ `__initiate_values`,
    /// inlined into [`State::new`]): builds the character-map state fresh
    /// (matching `__initiate_values` being called anew on every
    /// invocation) and dispatches on the currently configured
    /// [`AreaToConvert`].
    pub fn convert_hex_2_utf8(&self, content: &str) -> Result<String, Hex2Utf8Error> {
        let mut state = State::new(self)?;
        match self.area_to_convert {
            AreaToConvert::Preamble => Ok(convert_preamble(self, &mut state, content)?),
            AreaToConvert::Body => Ok(convert_body(self, &mut state, content)?),
        }
    }
}

/// Port of `self.__current_dict_name`, which (verified by inspection: it
/// is only ever assigned one of these four literals across the whole
/// file) only ever holds one of these four values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FontDict {
    Default,
    Symbol,
    Wingdings,
    ZapfDingbats,
}

/// Port of `Hex2Utf8`'s per-call conversion state (`__def_dict`,
/// `__symbol_dict`, ..., `__current_dict`/`__current_dict_name`,
/// `__caps_list`, `__font_list`), threaded explicitly here instead of as
/// `self` fields, matching this crate's established convention (see
/// `super::delete_info::State`).
struct State {
    def_dict: HashMap<String, String>,
    symbol_dict: Option<HashMap<String, String>>,
    wingdings_dict: Option<HashMap<String, String>>,
    dingbats_dict: Option<HashMap<String, String>>,
    caps_uni_dict: HashMap<String, String>,
    current_dict_name: FontDict,
    /// Port of `self.__caps_list`, a stack initialized to `['false']`.
    caps_list: Vec<String>,
    /// Port of `self.__font_list`, a stack initialized to `['not-defined']`.
    font_list: Vec<String>,
}

impl State {
    /// Port of `Hex2Utf8.__initiate_values`.
    fn new(cfg: &Hex2Utf8) -> Result<Self, Hex2Utf8Error> {
        let mut def_dict = get_char_map(&cfg.default_char_map)?;
        def_dict.extend(get_char_map("bottom_128")?);
        def_dict.extend(get_char_map("ms_standard")?);

        let symbol_dict = if cfg.symbol {
            let mut d = get_char_map("SYMBOL")?;
            d.extend(get_char_map("ms_symbol")?);
            Some(d)
        } else {
            None
        };
        let wingdings_dict = if cfg.wingdings {
            let mut d = get_char_map("wingdings")?;
            d.extend(get_char_map("ms_wingdings")?);
            Some(d)
        } else {
            None
        };
        let dingbats_dict = if cfg.dingbats {
            let mut d = get_char_map("dingbats")?;
            d.extend(get_char_map("ms_dingbats")?);
            Some(d)
        } else {
            None
        };
        let caps_uni_dict = get_char_map("caps_uni")?;

        Ok(State {
            def_dict,
            symbol_dict,
            wingdings_dict,
            dingbats_dict,
            caps_uni_dict,
            current_dict_name: FontDict::Default,
            caps_list: vec!["false".to_string()],
            font_list: vec!["not-defined".to_string()],
        })
    }

    /// Port of dereferencing `self.__current_dict`. Falls back to the
    /// default dictionary if the special-font dictionary named by
    /// `current_dict_name` was never built (`symbol`/`wingdings`/
    /// `dingbats` false) -- a state only reachable via API misuse (real
    /// callers always pass the data-loading flag and the corresponding
    /// `convert_*` flag together), where the Python would instead raise
    /// `AttributeError` (`self.__symbol_dict` was never assigned). Per
    /// this crate's no-panic-equivalent-to-an-unreachable-crash
    /// convention (see `check_encoding.rs`'s module docs for the same
    /// reasoning), this degrades gracefully instead.
    fn current_dict(&self) -> &HashMap<String, String> {
        match self.current_dict_name {
            FontDict::Default => &self.def_dict,
            FontDict::Symbol => self.symbol_dict.as_ref().unwrap_or(&self.def_dict),
            FontDict::Wingdings => self.wingdings_dict.as_ref().unwrap_or(&self.def_dict),
            FontDict::ZapfDingbats => self.dingbats_dict.as_ref().unwrap_or(&self.def_dict),
        }
    }

    /// Port of the `font not in ('Symbol', 'Wingdings', 'Zapf Dingbats')`
    /// check repeated in `__hex_text_func`/`__text_func`/
    /// `__utf_to_caps_func`.
    fn is_special_font(&self) -> bool {
        self.current_dict_name != FontDict::Default
    }

    /// Port of the identical dictionary-selection `if`/`elif`/`else` chain
    /// duplicated in `__start_font_func` and `__end_font_func`.
    fn select_dict_for_face(&mut self, cfg: &Hex2Utf8, face: &str) {
        self.current_dict_name = match face {
            "Symbol" if cfg.convert_symbol => FontDict::Symbol,
            "Wingdings" if cfg.convert_wingdings => FontDict::Wingdings,
            "Zapf Dingbats" if cfg.convert_zapf => FontDict::ZapfDingbats,
            _ => FontDict::Default,
        };
    }
}

/// Port of `line[:16]`, used throughout rtf2xml's intermediate-format
/// passes to read a line's fixed-width label prefix. Matches
/// `super::check_brackets`/`super::delete_info`'s own helper.
fn token_info(line: &str) -> &str {
    if line.len() >= 16 {
        &line[..16]
    } else {
        line
    }
}

/// Port of `line[17:-1]` applied to a `str::lines()` line (trailing `\n`
/// already stripped, so only the leading 17-byte `label<` prefix needs
/// dropping here) -- the value field following any of this file's
/// 16-char labels plus their `<` delimiter.
fn value_after_label(line: &str) -> &str {
    if line.len() >= 17 {
        &line[17..]
    } else {
        ""
    }
}

/// Port of `Hex2Utf8.__hex_text_func`, shared by the preamble pass
/// (`tx<hx<__________`) and the body pass (`tx<hx<__________` and
/// `tx<mc<__________`, both routed here -- see `__in_body_dict`).
fn hex_text_func(
    cfg: &Hex2Utf8,
    st: &mut State,
    line: &str,
    out: &mut String,
) -> Result<(), Hex2Utf8Error> {
    let hex_num = value_after_label(line);
    if let Some(converted) = st.current_dict().get(hex_num) {
        let mut converted = converted.clone();
        let caps_active = cfg.convert_caps
            && st.caps_list.last().map(String::as_str) == Some("true")
            && !st.is_special_font();
        if converted.starts_with('&') {
            if caps_active {
                converted = utf_token_to_caps(st, &converted);
            }
            out.push_str("tx<ut<__________<");
            out.push_str(&converted);
            out.push('\n');
        } else {
            if caps_active {
                converted = converted.to_uppercase();
            }
            out.push_str("tx<nu<__________<");
            out.push_str(&converted);
            out.push('\n');
        }
        return Ok(());
    }

    // Not found: port of the `else` branch.
    let token = hex_num.replace('\'', "");
    let the_num: u32 = if token.is_empty() {
        0
    } else {
        // Port of `int(token, 16)`. A parse failure here would be an
        // uncaught `ValueError` in the Python; not expected on real
        // `'HH` hex-byte input from `super::process_tokens`, so this
        // defensively degrades to 0 rather than panicking (matching
        // this crate's no-panic convention).
        u32::from_str_radix(&token, 16).unwrap_or(0)
    };
    if the_num > 10 {
        out.push_str("mi<tg<empty-att_<udef_symbol<num>");
        out.push_str(hex_num);
        out.push_str("<description>not-in-table\n");
        if cfg.run_level > 4 {
            return Err(Hex2Utf8Error::InvalidHexChar(token));
        }
    }
    // the_num <= 10: silently dropped, matching Python -- see module docs.
    Ok(())
}

/// Port of `Hex2Utf8.__utf_token_to_caps_func`.
fn utf_token_to_caps(st: &State, char_entity: &str) -> String {
    if char_entity.len() < 3 {
        // Port of `char_entity[3:]` on a shorter-than-3-byte string,
        // which in Python just yields `''` rather than raising.
        return char_entity.to_string();
    }
    let hex_num = &char_entity[3..];
    let normalized = match hex_num.len() {
        3 => format!("00{hex_num}"),
        4 => format!("0{hex_num}"),
        _ => hex_num.to_string(),
    };
    let new_char_entity = format!("&#x{normalized}");
    match st.caps_uni_dict.get(&new_char_entity) {
        Some(v) if !v.is_empty() => v.clone(),
        _ => char_entity.to_string(),
    }
}

/// Port of `Hex2Utf8.__found_body_func`. In the preamble pass this
/// transitions preamble->body state; in the body pass `self.__state` is
/// already (and only ever) `'body'`, so the assignment is a no-op there
/// -- either way the line is written through unchanged.
fn write_through(line: &str, out: &mut String) {
    out.push_str(line);
    out.push('\n');
}

/// Port of `Hex2Utf8.__convert_preamble` (+ `__preamble_func`,
/// `__found_body_func`, `__body_func`, inlined).
fn convert_preamble(
    cfg: &Hex2Utf8,
    st: &mut State,
    content: &str,
) -> Result<String, Hex2Utf8Error> {
    let mut out = String::new();
    let mut in_preamble = true;
    for line in content.lines() {
        if in_preamble {
            match token_info(line) {
                "mi<mk<body-open_" => {
                    in_preamble = false;
                    write_through(line, &mut out);
                }
                "tx<hx<__________" => hex_text_func(cfg, st, line, &mut out)?,
                _ => write_through(line, &mut out),
            }
        } else {
            // Port of `__body_func`: once the body has started, the
            // preamble pass no longer converts anything -- every
            // remaining line passes through verbatim.
            write_through(line, &mut out);
        }
    }
    Ok(out)
}

/// Port of `Hex2Utf8.__convert_body` (+ `__body_for_body_func`,
/// `__in_body_dict`, inlined). `__preamble_for_body_func` is not
/// represented -- see the module docs' "unreachable" quirk.
fn convert_body(cfg: &Hex2Utf8, st: &mut State, content: &str) -> Result<String, Hex2Utf8Error> {
    let mut out = String::new();
    for line in content.lines() {
        match token_info(line) {
            "mi<mk<body-open_" => write_through(line, &mut out),
            "tx<ut<__________" => utf_to_caps_func(cfg, st, line, &mut out),
            "tx<hx<__________" | "tx<mc<__________" => hex_text_func(cfg, st, line, &mut out)?,
            "tx<nu<__________" => text_func(cfg, st, line, &mut out),
            "mi<mk<font______" => start_font_func(cfg, st, line),
            "mi<mk<caps______" => start_caps_func(st, line),
            "mi<mk<font-end__" => end_font_func(cfg, st),
            "mi<mk<caps-end__" => end_caps_func(st),
            _ => write_through(line, &mut out),
        }
    }
    Ok(out)
}

/// Port of `Hex2Utf8.__utf_to_caps_func`.
fn utf_to_caps_func(cfg: &Hex2Utf8, st: &mut State, line: &str, out: &mut String) {
    let mut utf_text = value_after_label(line).to_string();
    if cfg.convert_caps && st.caps_list.last().map(String::as_str) == Some("true") {
        utf_text = utf_token_to_caps(st, &utf_text);
    }
    out.push_str("tx<ut<__________<");
    out.push_str(&utf_text);
    out.push('\n');
}

/// Port of `Hex2Utf8.__text_func`.
fn text_func(cfg: &Hex2Utf8, st: &mut State, line: &str, out: &mut String) {
    let text = value_after_label(line);
    if st.is_special_font() {
        let mut result = String::new();
        for letter in text.chars() {
            // Port of `hex(ord(letter))[2:].upper()` prefixed with `'`
            // (Python's `hex()`/`{:X}` both produce unpadded uppercase
            // hex digits, so no manual zero-padding is needed here).
            let hex_num = format!("'{:X}", letter as u32);
            match st.current_dict().get(&hex_num) {
                Some(v) => result.push_str(v),
                None => {
                    eprintln!("module is hex_2_ut8\nmethod is __text_func");
                    eprintln!("no hex value for \"{hex_num}\"");
                }
            }
        }
        out.push_str("tx<nu<__________<");
        out.push_str(&result);
        out.push('\n');
    } else {
        let mut text = text.to_string();
        if st.caps_list.last().map(String::as_str) == Some("true") && cfg.convert_caps {
            text = text.to_uppercase();
        }
        out.push_str("tx<nu<__________<");
        out.push_str(&text);
        out.push('\n');
    }
}

/// Port of `Hex2Utf8.__start_font_func`. Note: does not write `line` to
/// output -- see module docs.
fn start_font_func(cfg: &Hex2Utf8, st: &mut State, line: &str) {
    let face = value_after_label(line).to_string();
    st.font_list.push(face.clone());
    st.select_dict_for_face(cfg, &face);
}

/// Port of `Hex2Utf8.__end_font_func`. Note: does not write to output --
/// see module docs.
fn end_font_func(cfg: &Hex2Utf8, st: &mut State) {
    if st.font_list.len() > 1 {
        st.font_list.pop();
    } else {
        eprintln!("module is hex_2_utf8");
        eprintln!("method is end_font_func");
        eprintln!("self.__font_list should be greater than one?");
    }
    let face = st
        .font_list
        .last()
        .cloned()
        .unwrap_or_else(|| "not-defined".to_string());
    st.select_dict_for_face(cfg, &face);
}

/// Port of `Hex2Utf8.__start_caps_func`. Note: does not write to output
/// -- see module docs. (`self.__in_caps = 1` is dead state, not
/// represented -- see module docs.)
fn start_caps_func(st: &mut State, line: &str) {
    let value = value_after_label(line).to_string();
    st.caps_list.push(value);
}

/// Port of `Hex2Utf8.__end_caps_func`. Note: does not write to output --
/// see module docs.
fn end_caps_func(st: &mut State) {
    if st.caps_list.len() > 1 {
        st.caps_list.pop();
    } else {
        eprintln!(
            "Module is hex_2_utf8\nmethod is __end_caps_func\ncaps list should be more than one?"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preamble_obj() -> Hex2Utf8 {
        Hex2Utf8::new(AreaToConvert::Preamble, "ansicpg1252", 1)
    }

    fn body_obj(run_level: u32) -> Hex2Utf8 {
        let mut obj = Hex2Utf8::new(AreaToConvert::Preamble, "ansicpg1252", run_level);
        obj.update_values(
            AreaToConvert::Body,
            /* convert_caps */ true,
            /* convert_symbol */ true,
            /* convert_wingdings */ true,
            /* convert_zapf */ true,
            /* symbol */ true,
            /* wingdings */ true,
            /* dingbats */ true,
        );
        obj
    }

    fn lines(v: &[&str]) -> String {
        v.join("\n") + "\n"
    }

    // ---- preamble mode ----

    #[test]
    fn preamble_converts_plain_ascii_hex_byte() {
        // bottom_128's '41 -> "A" (plain text, not an entity).
        let content = "tx<hx<__________<'41\n";
        let out = preamble_obj().convert_hex_2_utf8(content).unwrap();
        assert_eq!(out, "tx<nu<__________<A\n");
    }

    #[test]
    fn preamble_converts_codepage_hex_byte_to_entity() {
        // ansicpg1252's 'C0 -> "&#x00C0;" (an entity: starts with '&').
        let content = "tx<hx<__________<'C0\n";
        let out = preamble_obj().convert_hex_2_utf8(content).unwrap();
        assert_eq!(out, "tx<ut<__________<&#x00C0;\n");
    }

    #[test]
    fn preamble_stops_converting_after_body_open_marker() {
        let content = lines(&[
            "tx<hx<__________<'41",
            "mi<mk<body-open_<nu<true",
            // this would convert to "B" if still in preamble mode, but
            // should now pass through completely unchanged.
            "tx<hx<__________<'42",
        ]);
        let out = preamble_obj().convert_hex_2_utf8(&content).unwrap();
        assert_eq!(
            out,
            lines(&[
                "tx<nu<__________<A",
                "mi<mk<body-open_<nu<true",
                "tx<hx<__________<'42",
            ])
        );
    }

    #[test]
    fn preamble_passes_non_hex_lines_through_unchanged() {
        let content = "cw<ci<bold______<nu<true\n";
        let out = preamble_obj().convert_hex_2_utf8(content).unwrap();
        assert_eq!(out, content);
    }

    #[test]
    fn hex_byte_with_small_missing_code_point_is_silently_dropped() {
        // '01 (SOH, the_num=1) isn't in ansicpg1252+bottom_128+ms_standard
        // (only in `not_unicode`, which this file never loads) and
        // the_num <= 10, so nothing at all is written for it.
        let content = "tx<hx<__________<'01\n";
        let out = preamble_obj().convert_hex_2_utf8(content).unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn hex_byte_with_large_missing_code_point_emits_diagnostic_below_run_level_five() {
        // 'ansicpg1252 has a genuine gap at '81 (the_num=0x81=129 > 10).
        let content = "tx<hx<__________<'81\n";
        let out = preamble_obj().convert_hex_2_utf8(content).unwrap();
        assert_eq!(
            out,
            "mi<tg<empty-att_<udef_symbol<num>'81<description>not-in-table\n"
        );
    }

    #[test]
    fn hex_byte_with_large_missing_code_point_raises_above_run_level_four() {
        let content = "tx<hx<__________<'81\n";
        let obj = Hex2Utf8::new(AreaToConvert::Preamble, "ansicpg1252", 5);
        let err = obj.convert_hex_2_utf8(content).unwrap_err();
        assert!(matches!(err, Hex2Utf8Error::InvalidHexChar(t) if t == "81"));
    }

    #[test]
    fn unknown_default_char_map_errors() {
        let obj = Hex2Utf8::new(AreaToConvert::Preamble, "does-not-exist", 1);
        let err = obj
            .convert_hex_2_utf8("tx<hx<__________<'41\n")
            .unwrap_err();
        assert!(matches!(err, Hex2Utf8Error::MapNotFound(_)));
    }

    // ---- body mode (post-update_values) ----

    #[test]
    fn body_converts_hex_and_mc_tokens_the_same_way() {
        let content = lines(&["tx<hx<__________<'41", "tx<mc<__________<'41"]);
        let out = body_obj(1).convert_hex_2_utf8(&content).unwrap();
        assert_eq!(out, lines(&["tx<nu<__________<A", "tx<nu<__________<A"]));
    }

    #[test]
    fn body_font_and_caps_markers_are_dropped_from_output() {
        let content = lines(&[
            "mi<mk<font______<Arial",
            "tx<nu<__________<hi",
            "mi<mk<font-end__<nu<true",
            "mi<mk<caps______<true",
            "tx<nu<__________<lo",
            "mi<mk<caps-end__<nu<true",
        ]);
        let out = body_obj(1).convert_hex_2_utf8(&content).unwrap();
        // The `caps______`/`caps-end__` markers themselves never appear
        // in the output, and caps starts only *after* "hi" is written,
        // so only "lo" ends up uppercased.
        assert_eq!(out, lines(&["tx<nu<__________<hi", "tx<nu<__________<LO"]));
    }

    #[test]
    fn body_switches_to_symbol_dictionary_between_font_markers() {
        // SYMBOL's '22 -> "&#x2200;" (FOR ALL), a different entity than
        // the default dictionary would ever resolve '22 to.
        let content = lines(&[
            "mi<mk<font______<Symbol",
            "tx<hx<__________<'22",
            "mi<mk<font-end__<nu<true",
        ]);
        let out = body_obj(1).convert_hex_2_utf8(&content).unwrap();
        assert_eq!(out, "tx<ut<__________<&#x2200;\n");
    }

    #[test]
    fn body_end_font_func_restores_previous_font() {
        let content = lines(&[
            "mi<mk<font______<Symbol",
            "mi<mk<font______<Arial",
            "tx<hx<__________<'41",
            "mi<mk<font-end__<nu<true",
            // back to Symbol now.
            "tx<hx<__________<'22",
            "mi<mk<font-end__<nu<true",
        ]);
        let out = body_obj(1).convert_hex_2_utf8(&content).unwrap();
        assert_eq!(
            out,
            lines(&["tx<nu<__________<A", "tx<ut<__________<&#x2200;"])
        );
    }

    #[test]
    fn body_symbol_font_text_run_is_converted_char_by_char() {
        // dingbats' '20 -> "" (empty replacement) and '21 -> "&#x2701;".
        let content = lines(&[
            "mi<mk<font______<Zapf Dingbats",
            "tx<nu<__________<\u{20}\u{21}",
        ]);
        let out = body_obj(1).convert_hex_2_utf8(&content).unwrap();
        assert_eq!(out, "tx<nu<__________<&#x2701;\n");
    }

    #[test]
    fn body_caps_active_uppercases_plain_text() {
        let content = lines(&["mi<mk<caps______<true", "tx<nu<__________<shout"]);
        let out = body_obj(1).convert_hex_2_utf8(&content).unwrap();
        assert_eq!(out, "tx<nu<__________<SHOUT\n");
    }

    #[test]
    fn body_caps_inactive_by_default_leaves_text_unchanged() {
        let content = "tx<nu<__________<quiet\n";
        let out = body_obj(1).convert_hex_2_utf8(content).unwrap();
        assert_eq!(out, content);
    }

    #[test]
    fn body_caps_active_uppercases_hex_resolved_plain_text() {
        // bottom_128's '61 -> "a" (lowercase plain text, not an entity),
        // so caps-uppercasing should kick in via the `.to_uppercase()`
        // branch of `hex_text_func`.
        let content = lines(&["mi<mk<caps______<true", "tx<hx<__________<'61"]);
        let out = body_obj(1).convert_hex_2_utf8(&content).unwrap();
        assert_eq!(out, "tx<nu<__________<A\n");
    }

    #[test]
    fn body_caps_active_converts_utf_entity_to_caps_equivalent() {
        // caps_uni maps "&#x0161;" (LATIN SMALL LETTER S WITH CARON) to
        // "&#x0160;" (its capital equivalent).
        let content = lines(&["mi<mk<caps______<true", "tx<ut<__________<&#x0161;"]);
        let out = body_obj(1).convert_hex_2_utf8(&content).unwrap();
        assert_eq!(out, "tx<ut<__________<&#x0160;\n");
    }

    #[test]
    fn body_caps_active_leaves_utf_entity_unchanged_when_no_caps_equivalent() {
        // An entity absent from caps_uni_dict is returned unchanged.
        let content = lines(&["mi<mk<caps______<true", "tx<ut<__________<&#x1234;"]);
        let out = body_obj(1).convert_hex_2_utf8(&content).unwrap();
        assert_eq!(out, "tx<ut<__________<&#x1234;\n");
    }

    #[test]
    fn body_caps_skips_special_fonts() {
        // Even with caps active, Symbol-font text/hex runs are never
        // uppercased (they're glyph code points, not letters). SYMBOL's
        // '61 -> "&#x03B1;" (GREEK SMALL LETTER ALPHA) -- if caps
        // uppercasing wrongly applied, this would instead run through
        // `utf_token_to_caps` and (having no caps_uni entry) come out
        // unchanged anyway, so the real proof is that it's tagged `ut`
        // (an entity) rather than `nu`, confirming `hex_text_func` never
        // took the special-cased-away uppercase branch.
        let content = lines(&[
            "mi<mk<caps______<true",
            "mi<mk<font______<Symbol",
            "tx<hx<__________<'61",
        ]);
        let out = body_obj(1).convert_hex_2_utf8(&content).unwrap();
        assert_eq!(out, "tx<ut<__________<&#x03B1;\n");
    }

    #[test]
    fn body_without_symbol_flag_falls_back_to_default_dict_instead_of_panicking() {
        // API-misuse case: convert_symbol true but symbol=false, so no
        // Symbol dictionary was ever built. Rather than panicking (the
        // Python would AttributeError), this degrades to the default
        // dictionary. See State::current_dict's doc comment.
        let mut obj = Hex2Utf8::new(AreaToConvert::Preamble, "ansicpg1252", 1);
        obj.update_values(
            AreaToConvert::Body,
            false,
            true,
            false,
            false,
            /* symbol */ false,
            false,
            false,
        );
        let content = lines(&["mi<mk<font______<Symbol", "tx<hx<__________<'41"]);
        let out = obj.convert_hex_2_utf8(&content).unwrap();
        assert_eq!(out, "tx<nu<__________<A\n");
    }

    #[test]
    fn body_end_font_func_underflow_emits_diagnostic_and_does_not_panic() {
        let content = "mi<mk<font-end__<nu<true\n";
        let out = body_obj(1).convert_hex_2_utf8(content).unwrap();
        // No output line for the marker itself either way.
        assert_eq!(out, "");
    }

    #[test]
    fn body_end_caps_func_underflow_emits_diagnostic_and_does_not_panic() {
        let content = "mi<mk<caps-end__<nu<true\n";
        let out = body_obj(1).convert_hex_2_utf8(content).unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn body_open_marker_passes_through() {
        let content = "mi<mk<body-open_<nu<true\n";
        let out = body_obj(1).convert_hex_2_utf8(content).unwrap();
        assert_eq!(out, content);
    }

    #[test]
    fn body_default_char_map_persists_from_construction_through_update_values() {
        // Constructed with "ansicpg1252"; update_values doesn't take a
        // new default_char_map, so 'C0 should still resolve via it.
        let content = "tx<hx<__________<'C0\n";
        let out = body_obj(1).convert_hex_2_utf8(content).unwrap();
        assert_eq!(out, "tx<ut<__________<&#x00C0;\n");
    }
}
