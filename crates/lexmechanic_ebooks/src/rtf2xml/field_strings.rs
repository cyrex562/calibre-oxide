//! Port of `old_src/src/calibre/ebooks/rtf2xml/field_strings.py`
//! (`FieldStrings`).
//!
//! Parses the text of a field-instruction group (the ` FIELDNAME
//! switches...` string Word puts inside `\*\fldinst { ... }`) into a
//! field-attribute string, dispatching on the field's name (`PAGE`,
//! `HYPERLINK`, `TOC`, `SYMBOL`, ...) via a ~65-entry name -> (handler,
//! base-name) table. Every handler is a pure function of the already
//! concatenated field-instruction text; nothing here is stateful
//! across calls, unlike Python's `FieldStrings` instance (whose
//! `__init__`-compiled regexes are plain constants here instead).
//!
//! Consumed by `fields_large.py`'s `__field_instruction_func` (that
//! module's own port is a separate, follow-up PR).
//!
//! # Return shape
//!
//! Every Python handler returns a 3-element list. Item 1 is `None` in
//! all ~30 of them -- dropped here. Item 0 is `None` everywhere except
//! [`SYMBOL`]'s handler, which returns the literal string `'Symbol'`;
//! the only real caller (`fields_large.py`) only ever checks
//! `== 'Symbol'`, so item 0 is collapsed into [`FieldInstruction::is_symbol`].
//!
//! # Preserved upstream quirks
//!
//! A few of the ~30 handlers have observable bugs in the original
//! Python, each documented at its port below rather than silently
//! "fixed": [`parse_num_format`] (always dead -- `match_group(1)`
//! calls a `Match` object instead of `.group(1)`), the
//! `INDEX_INSERT_LETTER_EXP` regex (a zero-width capture group makes
//! it practically unreachable), and [`index_func`]'s `\s` (index
//! sequence) branch (re-escapes a stale/possibly-undefined variable
//! whose result is then discarded).

use lazy_static::lazy_static;
use regex::Regex;
use thiserror::Error;

/// Errors [`process_string`] can return.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FieldStringsError {
    /// Port of `process_string`'s `run_level > 3`-gated
    /// `raise self.__bug_handler(msg)`, reached when `field_name`
    /// isn't a key in the field-instruction table. The diagnostic
    /// (`sys.stderr.write`) is printed unconditionally before this
    /// gate, matching Python -- see [`process_string`].
    #[error("no key for \"{field_name}\" \"{changed_string}\"")]
    NoKeyForField { field_name: String, changed_string: String },
}

pub type Result<T> = std::result::Result<T, FieldStringsError>;

/// Port of `process_string`'s 3-element return list -- see this
/// module's own docs for why item 1 is dropped and item 0 is
/// collapsed into `is_symbol`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInstruction {
    pub is_symbol: bool,
    pub content: String,
}

fn instr(content: String) -> FieldInstruction {
    FieldInstruction { is_symbol: false, content }
}

lazy_static! {
    static ref SYMBOL_NUM_EXP: Regex = Regex::new(r"SYMBOL (.*?) ").unwrap();
    static ref SYMBOL_FONT_EXP: Regex = Regex::new(r#"\\f "(.*?)""#).unwrap();
    static ref SYMBOL_SIZE_EXP: Regex = Regex::new(r"\\s (\d+)").unwrap();
    static ref DATE_EXP: Regex = Regex::new(r#"\\@\s+"(.*?)""#).unwrap();
    static ref NUM_TYPE_EXP: Regex = Regex::new(
        r"\\\*\s+(Arabic|alphabetic|ALPHABETIC|roman|ROMAN|Ordinal|CardText|OrdText|Hex|DollarText|Upper|Lower|FirstCap|Caps)"
    )
    .unwrap();
    static ref FORMAT_TEXT_EXP: Regex = Regex::new(r"\\\*\s+(Upper|Lower|FirstCap|Caps)").unwrap();
    static ref MERGE_FORMAT_EXP: Regex = Regex::new(r"\\\*\s+MERGEFORMAT").unwrap();
    static ref QUOTE_FIELD_EXP: Regex = Regex::new(r#"QUOTE\s+"(.*?)""#).unwrap();
    static ref TA_SHORT_FIELD_EXP: Regex = Regex::new(r#"\\s\s+"(.*?)""#).unwrap();
    static ref TA_LONG_FIELD_EXP: Regex = Regex::new(r#"\\l\s+"(.*?)""#).unwrap();
    static ref TA_CATEGORY_EXP: Regex = Regex::new(r"\\c\s+(\d+)").unwrap();
    static ref INDEX_INSERT_BLANK_LINE_EXP: Regex = Regex::new(r#"\\h\s+"""#).unwrap();
    /// Port of `__index_insert_letter_exp = re.compile(r'\\h\s{1,}"()"')`
    /// -- an apparent upstream typo (`"()"` rather than `"(.*?)"`): the
    /// capture group is zero-width, so this only matches when the
    /// quoted text is EMPTY, a case [`INDEX_INSERT_BLANK_LINE_EXP`]
    /// already catches first (and is checked first in `index_func`).
    /// Ported faithfully -- practically unreachable in both languages.
    static ref INDEX_INSERT_LETTER_EXP: Regex = Regex::new(r#"\\h\s+"()""#).unwrap();
    static ref INDEX_COLUMNS_EXP: Regex = Regex::new(r#"\\c\s+"(.*?)""#).unwrap();
    static ref BOOKMARK_EXP: Regex = Regex::new(r"\\b\s+(.*?)\s").unwrap();
    static ref D_SEPARATOR: Regex = Regex::new(r"\\d\s+(.*?)\s").unwrap();
    static ref E_SEPARATOR: Regex = Regex::new(r"\\e\s+(.*?)\s").unwrap();
    static ref L_SEPARATOR: Regex = Regex::new(r"\\l\s+(.*?)\s").unwrap();
    static ref P_SEPARATOR: Regex = Regex::new(r"\\p\s+(.*?)\s").unwrap();
    static ref INDEX_SEQUENCE: Regex = Regex::new(r"\\s\s+(.*?)\s").unwrap();
    static ref INDEX_ENTRY_TYPE_EXP: Regex = Regex::new(r#"\\f\s+"(.*?)""#).unwrap();
    static ref QUOTE_EXP: Regex = Regex::new(r#""(.*?)""#).unwrap();
    static ref FILTER_SWITCH: Regex = Regex::new(r"\\c\s+(.*?)\s").unwrap();
    /// Port of `__hyperlink_func`'s locally-reassigned `__link_switch`
    /// (the version compiled in `__init__`, `\\l\s{1,}(.*?)\s`, is
    /// dead -- always overwritten before its first use).
    static ref HYPERLINK_LINK_SWITCH: Regex = Regex::new(r#"\\l\s+"?(.*?)"?\s"#).unwrap();
}

fn number_dict(name: &str) -> Option<&'static str> {
    Some(match name {
        "Arabic" => "arabic",
        "alphabetic" => "alphabetic",
        "ALPHABETIC" => "capital-alphabetic",
        "roman" => "roman",
        "ROMAN" => "capital-roman",
        "Ordinal" => "ordinal",
        "CardText" => "cardinal-text",
        "OrdText" => "ordinal-text",
        "Hex" => "hexadecimal",
        "DollarText" => "dollar-text",
        "Upper" => "upper-case",
        "Lower" => "lower-case",
        "FirstCap" => "first-cap",
        "Caps" => "caps",
        _ => return None,
    })
}

fn text_format_dict(name: &str) -> Option<&'static str> {
    Some(match name {
        "Upper" => "upper",
        "Lower" => "lower",
        "FirstCap" => "first-cap",
        "Caps" => "caps",
        _ => return None,
    })
}

/// Port of `__parse_num_format`. Always `None`: the Python calls
/// `match_group(1)` -- invoking the `re.Match` object itself rather
/// than `.group(1)` -- which raises `TypeError` whenever
/// [`DATE_EXP`] actually matches. In practice this switch (`\@`) is a
/// date-field switch that essentially never appears in the number
/// fields that call this helper, so the crash is never hit; ported as
/// an always-`None` function rather than reproducing a panic in a
/// pure string-transform.
fn parse_num_format(_line: &str) -> Option<String> {
    None
}

/// Port of `__parse_num_type`.
fn parse_num_type(line: &str) -> Option<String> {
    let cap = NUM_TYPE_EXP.captures(line)?;
    let name = &cap[1];
    match number_dict(name) {
        Some(changed) => Some(changed.to_string()),
        None => {
            eprintln!("module is fields_string\nmethod is __parse_num_type\nno dictionary entry for {name}");
            None
        }
    }
}

fn default_inst_func(name: &str) -> FieldInstruction {
    instr(name.to_string())
}

fn no_switch_func(name: &str) -> FieldInstruction {
    instr(name.to_string())
}

fn equation_func(name: &str) -> FieldInstruction {
    instr(name.to_string())
}

/// Port of `__fall_back_func` (the second, `line`, parameter is
/// unused in the Python too).
fn fall_back_func(field_name: &str) -> FieldInstruction {
    instr(format!("{field_name}<update>none"))
}

fn num_type_and_format_func(field_name: &str, name: &str, line: &str) -> FieldInstruction {
    let mut s = name.to_string();
    if let Some(fmt) = parse_num_format(line) {
        s.push_str(&format!("<number-format>{fmt}"));
    }
    if let Some(t) = parse_num_type(line) {
        s.push_str(&format!("<number-type>{t}"));
    }
    if field_name == "QUOTE" {
        if let Some(cap) = QUOTE_FIELD_EXP.captures(line) {
            s.push_str(&format!("<argument>{}", &cap[1]));
        }
    }
    instr(s)
}

fn date_func(name: &str, line: &str) -> FieldInstruction {
    let mut s = name.to_string();
    if let Some(cap) = DATE_EXP.captures(line) {
        s.push_str(&format!("<date-format>{}", &cap[1]));
    }
    instr(s)
}

fn simple_info_func(name: &str, line: &str) -> FieldInstruction {
    let mut s = name.to_string();
    if let Some(cap) = FORMAT_TEXT_EXP.captures(line) {
        let n = &cap[1];
        match text_format_dict(n) {
            Some(changed) => s.push_str(&format!("<format>{changed}")),
            None => eprintln!(
                "module is fields_string\nmethod is __parse_num_type\nno dictionary entry for {n}"
            ),
        }
    }
    instr(s)
}

fn hyperlink_func(name: &str, line: &str) -> FieldInstruction {
    let mut s = name.to_string();
    if let Some(cap) = HYPERLINK_LINK_SWITCH.captures(line) {
        let link = cap[1].replace('"', "&quot;");
        s.push_str(&format!("<link>{link}"));
    }
    let stripped = HYPERLINK_LINK_SWITCH.replace(line, "");
    if let Some(cap) = QUOTE_EXP.captures(&stripped) {
        s.push_str(&format!("<argument>{}", &cap[1]));
    }
    if stripped.contains("\\m") {
        s.push_str("<html2-image-map>true");
    }
    if stripped.contains("\\n") {
        s.push_str("<new-window>true");
    }
    if stripped.contains("\\h") {
        s.push_str("<no-history>true");
    }
    instr(s)
}

fn include_text_func(name: &str, line: &str) -> FieldInstruction {
    let mut s = name.to_string();
    if let Some(cap) = FORMAT_TEXT_EXP.captures(line) {
        let n = &cap[1];
        match text_format_dict(n) {
            Some(changed) => s.push_str(&format!("<format>{changed}")),
            None => eprintln!(
                "module is fields_string\nmethod is __parse_num_type\nno dictionary entry for {n}"
            ),
        }
    }
    if let Some(cap) = FILTER_SWITCH.captures(line) {
        s.push_str(&format!("<filter>{}", &cap[1]));
    }
    let stripped = FILTER_SWITCH.replace(line, "");
    if let Some(cap) = QUOTE_EXP.captures(&stripped) {
        let arg = cap[1].replace('"', "&quot;");
        s.push_str(&format!("<argument>{arg}"));
    } else {
        eprintln!("Module is field_strings\nmethod is include_text_func\nno argument for include text");
    }
    if stripped.contains("\\!") {
        s.push_str("<no-field-update>true");
    }
    instr(s)
}

fn include_pict_func(name: &str, line: &str) -> FieldInstruction {
    let mut s = name.to_string();
    if let Some(cap) = FILTER_SWITCH.captures(line) {
        let arg = cap[1].replace('"', "&quot;");
        s.push_str(&format!("<filter>{arg}"));
    }
    let stripped = FILTER_SWITCH.replace(line, "");
    if let Some(cap) = QUOTE_EXP.captures(&stripped) {
        s.push_str(&format!("<argument>{}", &cap[1]));
    } else {
        eprintln!("Module is field_strings\nmethod is include_pict_func\nno argument for include pict");
    }
    if stripped.contains("\\d") {
        s.push_str("<external>true");
    }
    instr(s)
}

fn ref_func(name: &str, line: &str) -> FieldInstruction {
    let mut s = name.to_string();
    if let Some(cap) = FORMAT_TEXT_EXP.captures(line) {
        let n = &cap[1];
        match text_format_dict(n) {
            Some(changed) => s.push_str(&format!("<format>{changed}")),
            None => eprintln!(
                "module is fields_string\nmethod is __parse_num_type\nno dictionary entry for {n}"
            ),
        }
    }
    let stripped = MERGE_FORMAT_EXP.replace(line, "");
    for word in stripped.split_whitespace().skip(1) {
        if !word.starts_with('\\') {
            s.push_str(&format!("<bookmark>{word}"));
        }
    }
    if stripped.contains("\\f") {
        s.push_str("<include-note-number>true");
    }
    if stripped.contains("\\h") {
        s.push_str("<hyperlink>true");
    }
    if stripped.contains("\\n") {
        s.push_str("<insert-number>true");
    }
    if stripped.contains("\\r") {
        s.push_str("<insert-number-relative>true");
    }
    if stripped.contains("\\p") {
        s.push_str("<paragraph-relative-position>true");
    }
    if stripped.contains("\\t") {
        s.push_str("<suppress-non-delimeter>true");
    }
    if stripped.contains("\\w") {
        s.push_str("<insert-number-full>true");
    }
    instr(s)
}

fn toc_table_func(name: &str, line: &str) -> FieldInstruction {
    let mut s = name.to_string();
    if line.contains(r#"\c "Figure""#) {
        s = s.replace("table-of-contents", "table-of-figures");
    }
    instr(s)
}

/// Port of `__sequence_func`. `fields[1]` in the Python raises
/// `IndexError` on a malformed (fewer than 2 whitespace-separated
/// tokens) `SEQ` instruction; this port degrades to an empty label
/// instead of panicking.
fn sequence_func(name: &str, line: &str) -> FieldInstruction {
    let label = line.split_whitespace().nth(1).unwrap_or("");
    instr(format!("{name}<label>{label}"))
}

fn ta_func(name: &str, line: &str) -> FieldInstruction {
    let mut s = name.to_string();
    if let Some(cap) = TA_SHORT_FIELD_EXP.captures(line) {
        s.push_str(&format!("<short-field>{}", &cap[1]));
    }
    if let Some(cap) = TA_LONG_FIELD_EXP.captures(line) {
        s.push_str(&format!("<long-field>{}", &cap[1]));
    }
    if let Some(cap) = TA_CATEGORY_EXP.captures(line) {
        s.push_str(&format!("<category>{}", &cap[1]));
    }
    if line.contains("\\b") {
        s.push_str("<bold>true");
    }
    if line.contains("\\i") {
        s.push_str("<italics>true");
    }
    instr(s)
}

fn index_func(name: &str, line: &str) -> FieldInstruction {
    let mut s = name.to_string();
    if INDEX_INSERT_BLANK_LINE_EXP.is_match(line) {
        s.push_str("<insert-blank-line>true");
    } else if let Some(cap) = INDEX_INSERT_LETTER_EXP.captures(line) {
        s.push_str(&format!("<insert-letter>{}", &cap[1]));
    }
    if let Some(cap) = INDEX_COLUMNS_EXP.captures(line) {
        s.push_str(&format!("<number-of-columns>{}", &cap[1]));
    }
    if let Some(cap) = BOOKMARK_EXP.captures(line) {
        s.push_str(&format!("<use-bookmark>{}", &cap[1]));
    }
    if let Some(cap) = D_SEPARATOR.captures(line) {
        let sep = cap[1].replace('"', "&quot;");
        s.push_str(&format!("<sequence-separator>{sep}"));
    }
    if let Some(cap) = E_SEPARATOR.captures(line) {
        let sep = cap[1].replace('"', "&quot;");
        s.push_str(&format!("<page-separator>{sep}"));
    }
    if let Some(cap) = INDEX_SEQUENCE.captures(line) {
        // Upstream re-escapes a *different*, possibly-undefined
        // `separator` variable here (a genuine UnboundLocalError risk
        // in the original if neither the `\d` nor `\e` switch matched
        // above) but never uses the result -- `<use-sequence>` is
        // built from the unescaped capture regardless. Not
        // replicated: no observable effect on the non-crashing path,
        // and this port doesn't introduce panics into a pure
        // string-transform function.
        s.push_str(&format!("<use-sequence>{}", &cap[1]));
    }
    if let Some(cap) = INDEX_ENTRY_TYPE_EXP.captures(line) {
        s.push_str(&format!("<entry-type>{}", &cap[1]));
    }
    if let Some(cap) = P_SEPARATOR.captures(line) {
        s.push_str(&format!("<limit-to-letters>{}", &cap[1]));
    }
    if let Some(cap) = L_SEPARATOR.captures(line) {
        let sep = cap[1].replace('"', "&quot;");
        s.push_str(&format!("<multi-page-separator>{sep}"));
    }
    if line.contains("\\a") {
        s.push_str("<accented>true");
    }
    if line.contains("\\r") {
        s.push_str("<sub-entry-on-same-line>true");
    }
    if line.contains("\\t") {
        s.push_str("<enable-yomi-text>true");
    }
    instr(s)
}

fn page_ref_func(name: &str, line: &str) -> FieldInstruction {
    let mut s = name.to_string();
    if let Some(fmt) = parse_num_format(line) {
        s.push_str(&format!("<number-format>{fmt}"));
    }
    if let Some(t) = parse_num_type(line) {
        s.push_str(&format!("<number-type>{t}"));
    }
    let stripped = MERGE_FORMAT_EXP.replace(line, "");
    for word in stripped.split_whitespace().skip(1) {
        if !word.starts_with('\\') {
            s.push_str(&format!("<bookmark>{word}"));
        }
    }
    if stripped.contains("\\h") {
        s.push_str("<hyperlink>true");
    }
    if stripped.contains("\\p") {
        s.push_str("<paragraph-relative-position>true");
    }
    instr(s)
}

fn note_ref_func(name: &str, line: &str) -> FieldInstruction {
    let mut s = name.to_string();
    let stripped = MERGE_FORMAT_EXP.replace(line, "");
    for word in stripped.split_whitespace().skip(1) {
        if !word.starts_with('\\') {
            s.push_str(&format!("<bookmark>{word}"));
        }
    }
    if stripped.contains("\\h") {
        s.push_str("<hyperlink>true");
    }
    if stripped.contains("\\p") {
        s.push_str("<paragraph-relative-position>true");
    }
    if stripped.contains("\\f") {
        s.push_str("<include-note-number>true");
    }
    instr(s)
}

/// Port of `__symbol_func`. `int(num)`/`int(font_size)` in the Python
/// raise `ValueError` on a malformed capture; this port skips the
/// corresponding attribute instead of panicking.
fn symbol_func(line: &str) -> FieldInstruction {
    let mut num = String::new();
    let mut changed_line = String::new();
    if let Some(cap) = SYMBOL_NUM_EXP.captures(line) {
        if let Ok(n) = cap[1].trim().parse::<i64>() {
            num = format!("{n:X}");
        }
    }
    if let Some(cap) = SYMBOL_FONT_EXP.captures(line) {
        changed_line.push_str(&format!("cw<ci<font-style<nu<{}\n", &cap[1]));
    }
    if let Some(cap) = SYMBOL_SIZE_EXP.captures(line) {
        if let Ok(n) = cap[1].parse::<i64>() {
            // Port of Python's `f'{font_size:.2f}'` (formats the int
            // as a float with 2 decimal places) -- Rust's `{:.2}`
            // precision spec has no effect on integer `Display`, so
            // this goes through `f64` to get the same "12.00" shape.
            changed_line.push_str(&format!("cw<ci<font-size_<nu<{:.2}\n", n as f64));
        }
    }
    changed_line.push_str(&format!("tx<hx<__________<'{num}\n"));
    FieldInstruction { is_symbol: true, content: changed_line }
}

/// The `__field_instruction_dict` table: maps a field name to
/// (handler kind, base attribute name). Multiple field names sharing
/// one Python lambda-tuple (e.g. `AUTHOR`/`USERNAME` both
/// `__simple_info_func`/`'user-name'`) are collapsed into the same
/// match arm here, same as the Python dict literal.
fn field_entry(field_name: &str) -> Option<(&'static str, &'static str)> {
    Some(match field_name {
        "EDITTIME" => ("num_type_and_format", "editing-time"),
        "NUMCHARS" => ("num_type_and_format", "number-of-characters-in-doc"),
        "NUMPAGES" => ("num_type_and_format", "number-of-pages-in-doc"),
        "NUMWORDS" => ("num_type_and_format", "number-of-words-in-doc"),
        "REVNUM" => ("num_type_and_format", "revision-number"),
        "SECTIONPAGES" => ("num_type_and_format", "num-of-pages-in-section"),
        "SECTION" => ("num_type_and_format", "insert-section-number"),
        "QUOTE" => ("num_type_and_format", "quote"),
        "PAGE" | "page" => ("default_inst", "insert-page-number"),
        "CREATEDATE" | "PRINTDATE" => ("date", "insert-date"),
        "SAVEDATE" => ("date", "last-saved"),
        "TIME" => ("date", "insert-time"),
        "AUTHOR" | "USERNAME" => ("simple_info", "user-name"),
        "COMMENTS" => ("simple_info", "comments"),
        "FILENAME" | "filename" => ("simple_info", "file-name"),
        "KEYWORDS" => ("simple_info", "keywords"),
        "LASTSAVEDBY" => ("simple_info", "last-saved-by"),
        "SUBJECT" => ("simple_info", "subject"),
        "TEMPLATE" => ("simple_info", "based-on-template"),
        "TITLE" => ("simple_info", "document-title"),
        "USERADDRESS" => ("simple_info", "user-address"),
        "USERINITIALS" => ("simple_info", "user-initials"),
        "EQ" => ("equation", "equation"),
        "HYPERLINK" => ("hyperlink", "hyperlink"),
        "INCLUDEPICTURE" => ("include_pict", "include-picture"),
        "INCLUDETEXT" => ("include_text", "include-text-from-file"),
        "INDEX" => ("index", "index"),
        "NOTEREF" => ("note_ref", "reference-to-note"),
        "PAGEREF" => ("page_ref", "reference-to-page"),
        "REF" | "ref" => ("ref", "reference"),
        "SEQ" => ("sequence", "numbering-sequence"),
        "SYMBOL" => ("symbol", "symbol"),
        "TA" => ("ta", "anchor-for-table-of-authorities"),
        "TOA" => ("toc_table", "table-of-authorities"),
        "TOC" => ("toc_table", "table-of-contents"),
        "AUTONUMOUT" => ("no_switch", "auto-num-out?"),
        "COMPARE" => ("no_switch", "compare"),
        "DOCVARIABLE" => ("no_switch", "document-variable"),
        "GOTOBUTTON" => ("no_switch", "go-button"),
        "NEXT" => ("no_switch", "next"),
        "NEXTIF" => ("no_switch", "next-if"),
        "SKIPIF" => ("no_switch", "skip-if"),
        "IF" => ("no_switch", "if"),
        "MERGEFIELD" => ("no_switch", "merge-field"),
        "MERGEREC" => ("no_switch", "merge-record"),
        "MERGESEQ" => ("no_switch", "merge-sequence"),
        "PLACEHOLDER" => ("no_switch", "place-holder"),
        "PRIVATE" => ("no_switch", "private"),
        "RD" => ("no_switch", "referenced-document"),
        "SET" => ("no_switch", "set"),
        "ADVANCE" => ("default_inst", "advance"),
        "ASK" => ("default_inst", "prompt-user"),
        "AUTONUMLGL" => ("default_inst", "automatic-number"),
        "AUTONUM" => ("default_inst", "automatic-number"),
        "AUTOTEXTLIST" => ("default_inst", "auto-list-text"),
        "AUTOTEXT" => ("default_inst", "auto-text"),
        "BARCODE" => ("default_inst", "barcode"),
        "CONTACT" => ("default_inst", "contact"),
        "DATABASE" => ("default_inst", "database"),
        "DATE" | "date" => ("default_inst", "date"),
        "DOCPROPERTY" => ("default_inst", "document-property"),
        "FILESIZE" => ("default_inst", "file-size"),
        "FILLIN" => ("default_inst", "fill-in"),
        "INFO" => ("default_inst", "document-info"),
        "LINK" => ("default_inst", "link"),
        "PA" => ("default_inst", "page"),
        "PRINT" => ("default_inst", "print"),
        "STYLEREF" => ("default_inst", "style-reference"),
        "USERPROPERTY" => ("default_inst", "user-property"),
        "FORMCHECKBOX" => ("default_inst", "form-checkbox"),
        "FORMTEXT" => ("default_inst", "form-text"),
        "MACROBUTTON" => ("default_inst", "macro-button"),
        _ => return None,
    })
}

fn dispatch(kind: &str, field_name: &str, name: &str, line: &str) -> FieldInstruction {
    match kind {
        "default_inst" => default_inst_func(name),
        "no_switch" => no_switch_func(name),
        "equation" => equation_func(name),
        "num_type_and_format" => num_type_and_format_func(field_name, name, line),
        "date" => date_func(name, line),
        "simple_info" => simple_info_func(name, line),
        "hyperlink" => hyperlink_func(name, line),
        "include_text" => include_text_func(name, line),
        "include_pict" => include_pict_func(name, line),
        "ref" => ref_func(name, line),
        "toc_table" => toc_table_func(name, line),
        "sequence" => sequence_func(name, line),
        "ta" => ta_func(name, line),
        "index" => index_func(name, line),
        "page_ref" => page_ref_func(name, line),
        "note_ref" => note_ref_func(name, line),
        "symbol" => symbol_func(line),
        _ => unreachable!("field_entry only ever returns a kind handled above"),
    }
}

/// Port of `FieldStrings.process_string`. `my_string` is the
/// concatenation of raw intermediate-format lines (see
/// [`super::process_tokens`]'s module docs) making up one field
/// instruction; only its `tx<nu<__________<...` payload lines
/// contribute to the field-instruction text that gets parsed. The
/// `type` parameter Python's own signature takes is dropped -- it's
/// never read in the function body there either.
pub fn process_string(my_string: &str, run_level: u32) -> Result<FieldInstruction> {
    let mut changed_string = String::new();
    for line in my_string.split('\n') {
        if line.len() >= 2 && &line[..2] == "tx" {
            changed_string.push_str(if line.len() >= 17 { &line[17..] } else { "" });
        }
    }

    let field_name = changed_string.split_whitespace().next().unwrap_or("");
    match field_entry(field_name) {
        Some((kind, base_name)) => {
            let name = if MERGE_FORMAT_EXP.is_match(&changed_string) {
                format!("{base_name}<update>dynamic")
            } else {
                format!("{base_name}<update>static")
            };
            Ok(dispatch(kind, field_name, &name, &changed_string))
        }
        None => {
            eprintln!("no key for \"{field_name}\" \"{changed_string}\"");
            if run_level > 3 {
                Err(FieldStringsError::NoKeyForField {
                    field_name: field_name.to_string(),
                    changed_string,
                })
            } else {
                Ok(fall_back_func(field_name))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(text: &str) -> String {
        format!("tx<nu<__________<{text}\n")
    }

    #[test]
    fn default_inst_field_gets_static_update_by_default() {
        let out = process_string(&tx("PAGE"), 1).unwrap();
        assert!(!out.is_symbol);
        assert_eq!(out.content, "insert-page-number<update>static");
    }

    #[test]
    fn merge_format_switch_makes_the_update_dynamic() {
        let out = process_string(&tx(r"PAGE \* MERGEFORMAT"), 1).unwrap();
        assert_eq!(out.content, "insert-page-number<update>dynamic");
    }

    #[test]
    fn quote_field_extracts_number_type_and_argument() {
        let out = process_string(&tx(r#"QUOTE "hi" \* Arabic"#), 1).unwrap();
        assert!(out.content.contains("<number-type>arabic"), "{}", out.content);
        assert!(out.content.contains("<argument>hi"), "{}", out.content);
    }

    #[test]
    fn date_field_extracts_the_date_format_switch() {
        let out = process_string(&tx(r#"CREATEDATE \@ "dddd, MMMM d, yyyy""#), 1).unwrap();
        assert!(out.content.starts_with("insert-date<update>static"));
        assert!(out.content.contains("<date-format>dddd, MMMM d, yyyy"), "{}", out.content);
    }

    #[test]
    fn simple_info_field_extracts_a_text_format_switch() {
        let out = process_string(&tx(r"AUTHOR \* Upper"), 1).unwrap();
        assert!(out.content.contains("<format>upper"), "{}", out.content);
    }

    #[test]
    fn hyperlink_extracts_argument_sub_address_link_and_flags() {
        // The plain quoted string right after HYPERLINK is the link
        // target itself (-> <argument>); `\l "..."` is a sub-address
        // *within* that target (-> <link>).
        let out =
            process_string(&tx(r#"HYPERLINK "page.html" \l "section2" \n \h"#), 1).unwrap();
        assert!(out.content.contains("<argument>page.html"), "{}", out.content);
        assert!(out.content.contains("<link>section2"), "{}", out.content);
        assert!(out.content.contains("<new-window>true"), "{}", out.content);
        assert!(out.content.contains("<no-history>true"), "{}", out.content);
    }

    #[test]
    fn ref_field_collects_bookmark_words_and_switches() {
        let out = process_string(&tx(r"REF my_bookmark \h \p"), 1).unwrap();
        assert!(out.content.contains("<bookmark>my_bookmark"), "{}", out.content);
        assert!(out.content.contains("<hyperlink>true"), "{}", out.content);
        assert!(out.content.contains("<paragraph-relative-position>true"), "{}", out.content);
    }

    #[test]
    fn toc_field_switches_to_table_of_figures_when_figure_is_present() {
        let out = process_string(&tx(r#"TOC \c "Figure""#), 1).unwrap();
        assert!(out.content.contains("table-of-figures"), "{}", out.content);
        assert!(!out.content.contains("table-of-contents"), "{}", out.content);
    }

    #[test]
    fn toc_field_without_figure_switch_stays_table_of_contents() {
        let out = process_string(&tx(r#"TOC \o "1-3""#), 1).unwrap();
        assert!(out.content.starts_with("table-of-contents"), "{}", out.content);
    }

    #[test]
    fn sequence_field_extracts_the_label() {
        let out = process_string(&tx(r"SEQ Figure \* ARABIC"), 1).unwrap();
        assert!(out.content.contains("<label>Figure"), "{}", out.content);
    }

    #[test]
    fn ta_field_extracts_short_and_long_and_flags() {
        let out = process_string(&tx(r#"TA \s "short" \l "long" \b"#), 1).unwrap();
        assert!(out.content.contains("<short-field>short"), "{}", out.content);
        assert!(out.content.contains("<long-field>long"), "{}", out.content);
        assert!(out.content.contains("<bold>true"), "{}", out.content);
    }

    #[test]
    fn index_field_collects_bookmark_and_columns() {
        let out = process_string(&tx(r#"INDEX \c "2" \b my_bookmark "#), 1).unwrap();
        assert!(out.content.contains("<number-of-columns>2"), "{}", out.content);
        assert!(out.content.contains("<use-bookmark>my_bookmark"), "{}", out.content);
    }

    #[test]
    fn page_ref_field_extracts_bookmark_and_hyperlink_flag() {
        let out = process_string(&tx(r"PAGEREF _Toc440880424 \h"), 1).unwrap();
        assert!(out.content.contains("<bookmark>_Toc440880424"), "{}", out.content);
        assert!(out.content.contains("<hyperlink>true"), "{}", out.content);
    }

    #[test]
    fn symbol_field_is_flagged_as_a_symbol_with_hex_code_point() {
        let out = process_string(&tx(r#"SYMBOL 97 \f "Symbol" \s 12"#), 1).unwrap();
        assert!(out.is_symbol);
        assert!(out.content.contains("cw<ci<font-style<nu<Symbol\n"), "{}", out.content);
        assert!(out.content.contains("cw<ci<font-size_<nu<12.00\n"), "{}", out.content);
        assert!(out.content.contains("tx<hx<__________<'61\n"), "{}", out.content);
    }

    #[test]
    fn unknown_field_falls_back_at_low_run_level() {
        let out = process_string(&tx("NOTAREALFIELD"), 1).unwrap();
        assert_eq!(out.content, "NOTAREALFIELD<update>none");
    }

    #[test]
    fn unknown_field_errors_at_high_run_level() {
        let err = process_string(&tx("NOTAREALFIELD"), 4).unwrap_err();
        assert_eq!(
            err,
            FieldStringsError::NoKeyForField {
                field_name: "NOTAREALFIELD".to_string(),
                changed_string: "NOTAREALFIELD".to_string(),
            }
        );
    }
}
