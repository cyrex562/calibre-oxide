//! Port of `old_src/src/calibre/ebooks/rtf2xml/process_tokens.py`
//! (`ProcessTokens`).
//!
//! Consumes [`super::tokenize`]'s one-token-per-line output and
//! produces rtf2xml's bracket-tagged intermediate format: one output
//! line per meaningful input token, shaped
//! `<kind><<pre><<label><<subtype><<value>` with `<` as the field
//! delimiter (mirroring the module-level docstring in the Python:
//! `"delimited by \"<\" for main fields, and \">\" for sub fields"` --
//! the `>` sub-field delimiter shows up in a handful of `label` values
//! below, e.g. `"align_____<left"`, which is intentional and kept as a
//! literal part of the label rather than parsed further here).
//!
//! # Intermediate format shape
//!
//! Every emitted line is one of:
//!
//! - `cw<{pre}<{label}<{subtype}<{value}` -- a recognized RTF control
//!   word/symbol, resolved through [`dict_token`]. `pre` is a 2-letter
//!   category code (`ci` = character info, `pf` = paragraph
//!   formatting, `ls` = list, `bd`/`bt` = border/border-type, etc.);
//!   `label` is a fixed (often `_`-padded-to-10-chars) name distinct
//!   from the raw RTF keyword (e.g. RTF `\b` becomes label
//!   `bold______`); `subtype` is almost always `nu` (a handful of
//!   color/language paths use `en`); `value` is the resolved argument
//!   (booleans as `true`/`false`, distances divided down from
//!   twentieths/half-points, colors as 2-digit uppercase hex, etc).
//! - `ob<nu<open-brack<NNNN` / `cb<nu<clos-brack<NNNN` -- a real RTF
//!   group delimiter (`{`/`}`), NNNN a 4-digit zero-padded nesting
//!   sequence number consumed by [`super::check_brackets`].
//! - `tx<nu<__________<{text}` -- a run of plain text (the
//!   10-underscore field is a fixed placeholder, not padding for a
//!   variable name).
//! - `tx<ut<__________<{entity}` -- an already-XML-escaped entity
//!   reference (`&...;`, e.g. `&amp;` from [`super::tokenize`]'s own
//!   escaping, or `&#NNN;` from a resolved `\u` token) kept distinct
//!   from plain text so later passes can tell "this needs no further
//!   escaping" from "this might".
//! - `tx<hx<__________<'HH` -- a resolved `\'HH` hex byte (from
//!   [`super::tokenize`]'s `\mshex0HH` rewrite), `HH` uppercase.
//!
//! This shape (not the escaping/tokenizing rules that produce its
//! *inputs* -- that's `super::tokenize`'s job) is what every follow-up
//! rtf2xml issue's later passes (`add_brackets`, `check_brackets`,
//! and beyond) consume.

use std::collections::HashMap;

use lazy_static::lazy_static;
use regex::Regex;
use thiserror::Error;

use super::check_brackets::check_brackets;

/// Errors [`process_tokens`] can return. Splits the Python's two
/// distinct callables -- `exception_handler` (structural RTF-validity
/// failures) and `bug_handler` (internal "should not happen" failures,
/// only actually raised when `run_level > 3`; below that threshold the
/// Python silently degrades to a default value instead) -- into one
/// enum, documented per-variant.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProcessTokensError {
    /// Port of `"\nInvalid RTF: document doesn't start with {\n"`.
    #[error("\nInvalid RTF: document doesn't start with {{\n")]
    MissingOpeningBrace,
    /// Port of `"\nInvalid RTF: document doesn't start with \\rtf \n"`.
    #[error("\nInvalid RTF: document doesn't start with \\rtf \n")]
    MissingRtfKeyword,
    /// Port of `f'\nInvalid RTF: token "\\ " not valid.\nError at line {line_count}'`.
    #[error("\nInvalid RTF: token \"\\ \" not valid.\nError at line {0}")]
    InvalidBackslashSpaceToken(usize),
    /// Port of `'\nInvalid RTF: file appears to be empty.\n'`.
    #[error("\nInvalid RTF: file appears to be empty.\n")]
    EmptyFile,
    /// Port of the final `check_brackets.CheckBrackets` call raising
    /// `'\nInvalid RTF: document does not have matching brackets.\n'`.
    #[error("\nInvalid RTF: document does not have matching brackets.\n")]
    UnbalancedBrackets,
    /// Port of `bool_st_func`'s unconditional (not `run_level`-gated)
    /// `raise self.__bug_handler(msg)` for a boolean control word
    /// whose argument is neither absent, `''`, `'0'`, nor `'1'`.
    #[error("boolean should have some value module process tokens\ntoken is {token}\n'{num}'\n")]
    InvalidBoolean { token: String, num: String },
    /// Port of the `run_level > 3` gated `raise self.__bug_handler(msg)`
    /// in `divide_num`, `split_let_num`, `__list_type_func`, and
    /// `__language_func`. Below `run_level > 3` the Python silently
    /// degrades instead (0, the original token, `'Arabic'`, or `'not
    /// defined'` respectively) -- see each call site below for which
    /// default applies.
    #[error("{0}")]
    RunLevelError(String),
}

pub type Result<T> = std::result::Result<T, ProcessTokensError>;

// ---- Port of `ProcessTokens.initiate_token_dict`'s action dispatch ----

/// Identifies one of `ProcessTokens`'s bound-method "action" functions.
/// Rust has no direct equivalent to storing bound methods as dict
/// values, so [`dict_token`] stores this tag instead and
/// [`dispatch_action`] matches on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    MsHex,
    Ob,
    Cb,
    MsSub,
    DirectConv,
    Default,
    Colorz,
    ListType,
    Language,
    TwoPart,
    DivideBy2,
    DivideBy20,
    Text,
    Color,
    BoolSt,
    NoSupSub,
}

/// Port of `ProcessTokens.dict_token`: maps a (post `\`-stripped,
/// space-stripped, and -- for tokens with a trailing numeric argument
/// -- letters-only) RTF keyword to its category code, fixed output
/// label, and dispatch action. Mechanically generated from the live
/// Python dict literal via `ast.parse` (267 entries, transcribed by
/// script rather than by hand to eliminate manual-transcription risk
/// across a ~200-line dict) and spot-checked by hand against the
/// Python source in the tests below across every `pre` category.
fn dict_token() -> &'static HashMap<&'static str, (&'static str, &'static str, Action)> {
    lazy_static! {
        static ref DICT_TOKEN: HashMap<&'static str, (&'static str, &'static str, Action)> = {
            let mut m = HashMap::new();
            m.insert("mshex", ("nu", "__________", Action::MsHex));
            m.insert("{", ("nu", "{", Action::Ob));
            m.insert("}", ("nu", "}", Action::Cb));
            m.insert("ldblquote", ("mc", "ldblquote", Action::MsSub));
            m.insert("rdblquote", ("mc", "rdblquote", Action::MsSub));
            m.insert("rquote", ("mc", "rquote", Action::MsSub));
            m.insert("lquote", ("mc", "lquote", Action::MsSub));
            m.insert("emdash", ("mc", "emdash", Action::MsSub));
            m.insert("endash", ("mc", "endash", Action::MsSub));
            m.insert("bullet", ("mc", "bullet", Action::MsSub));
            m.insert("~", ("mc", "~", Action::MsSub));
            m.insert("tab", ("mc", "tab", Action::MsSub));
            m.insert("_", ("mc", "_", Action::MsSub));
            m.insert(";", ("mc", ";", Action::MsSub));
            m.insert("-", ("mc", "-", Action::MsSub));
            m.insert("line", ("mi", "hardline-break", Action::DirectConv));
            m.insert("*", ("ml", "asterisk__", Action::Default));
            m.insert(":", ("ml", "colon_____", Action::Default));
            m.insert("backslash", ("nu", "\\", Action::Text));
            m.insert("ob", ("nu", "{", Action::Text));
            m.insert("cb", ("nu", "}", Action::Text));
            m.insert("page", ("pf", "page-break", Action::Default));
            m.insert("par", ("pf", "par-end___", Action::Default));
            m.insert("pard", ("pf", "par-def___", Action::Default));
            m.insert("keepn", ("pf", "keep-w-nex", Action::BoolSt));
            m.insert("widctlpar", ("pf", "widow-cntl", Action::BoolSt));
            m.insert("adjustright", ("pf", "adjust-rgt", Action::BoolSt));
            m.insert("lang", ("pf", "language__", Action::Language));
            m.insert("ri", ("pf", "right-inde", Action::DivideBy20));
            m.insert("fi", ("pf", "fir-ln-ind", Action::DivideBy20));
            m.insert("li", ("pf", "left-inden", Action::DivideBy20));
            m.insert("sb", ("pf", "space-befo", Action::DivideBy20));
            m.insert("sa", ("pf", "space-afte", Action::DivideBy20));
            m.insert("sl", ("pf", "line-space", Action::DivideBy20));
            m.insert("deftab", ("pf", "default-ta", Action::DivideBy20));
            m.insert("ql", ("pf", "align_____<left", Action::TwoPart));
            m.insert("qc", ("pf", "align_____<cent", Action::TwoPart));
            m.insert("qj", ("pf", "align_____<just", Action::TwoPart));
            m.insert("qr", ("pf", "align_____<right", Action::TwoPart));
            m.insert("nowidctlpar", ("pf", "widow-cntr<false", Action::TwoPart));
            m.insert("tx", ("pf", "tab-stop__", Action::DivideBy20));
            m.insert("tb", ("pf", "tab-bar-st", Action::DivideBy20));
            m.insert("tqr", ("pf", "tab-right_", Action::Default));
            m.insert("tqdec", ("pf", "tab-dec___", Action::Default));
            m.insert("tqc", ("pf", "tab-center", Action::Default));
            m.insert("tlul", ("pf", "leader-und", Action::Default));
            m.insert("tlhyph", ("pf", "leader-hyp", Action::Default));
            m.insert("tldot", ("pf", "leader-dot", Action::Default));
            m.insert("stylesheet", ("ss", "style-shet", Action::Default));
            m.insert("sbasedon", ("ss", "based-on__", Action::Default));
            m.insert("snext", ("ss", "next-style", Action::Default));
            m.insert("cs", ("ss", "char-style", Action::Default));
            m.insert("s", ("ss", "para-style", Action::Default));
            m.insert("pict", ("gr", "picture___", Action::Default));
            m.insert("objclass", ("gr", "obj-class_", Action::Default));
            m.insert("macpict", ("gr", "mac-pic___", Action::Default));
            m.insert("sect", ("sc", "section___", Action::Default));
            m.insert("sectd", ("sc", "sect-defin", Action::Default));
            m.insert("endhere", ("sc", "sect-note_", Action::Default));
            m.insert("pntext", ("ls", "list-text_", Action::Default));
            m.insert("listtext", ("ls", "list-text_", Action::Default));
            m.insert("pn", ("ls", "list______", Action::Default));
            m.insert("pnseclvl", ("ls", "list-level", Action::Default));
            m.insert("pncard", ("ls", "list-cardi", Action::BoolSt));
            m.insert("pndec", ("ls", "list-decim", Action::BoolSt));
            m.insert("pnucltr", ("ls", "list-up-al", Action::BoolSt));
            m.insert("pnucrm", ("ls", "list-up-ro", Action::BoolSt));
            m.insert("pnord", ("ls", "list-ord__", Action::BoolSt));
            m.insert("pnordt", ("ls", "list-ordte", Action::BoolSt));
            m.insert("pnlvlblt", ("ls", "list-bulli", Action::BoolSt));
            m.insert("pnlvlbody", ("ls", "list-simpi", Action::BoolSt));
            m.insert("pnlvlcont", ("ls", "list-conti", Action::BoolSt));
            m.insert("pnhang", ("ls", "list-hang_", Action::BoolSt));
            m.insert("pntxtb", ("ls", "list-tebef", Action::BoolSt));
            m.insert("ilvl", ("ls", "list-level", Action::Default));
            m.insert("ls", ("ls", "list-id___", Action::Default));
            m.insert("pnstart", ("ls", "list-start", Action::Default));
            m.insert("itap", ("ls", "nest-level", Action::Default));
            m.insert("leveltext", ("ls", "level-text", Action::Default));
            m.insert("levelnumbers", ("ls", "level-numb", Action::Default));
            m.insert("list", ("ls", "list-in-tb", Action::Default));
            m.insert("listlevel", ("ls", "list-tb-le", Action::Default));
            m.insert("listname", ("ls", "list-name_", Action::Default));
            m.insert("listtemplateid", ("ls", "ls-tem-id_", Action::Default));
            m.insert("leveltemplateid", ("ls", "lv-tem-id_", Action::Default));
            m.insert("listhybrid", ("ls", "list-hybri", Action::Default));
            m.insert("levelstartat", ("ls", "level-star", Action::Default));
            m.insert("levelspace", ("ls", "level-spac", Action::DivideBy20));
            m.insert("levelindent", ("ls", "level-inde", Action::Default));
            m.insert("levelnfc", ("ls", "level-type", Action::ListType));
            m.insert("levelnfcn", ("ls", "level-type", Action::ListType));
            m.insert("listid", ("ls", "lis-tbl-id", Action::Default));
            m.insert("listoverride", ("ls", "lis-overid", Action::Default));
            m.insert("pnlvl", ("ls", "list-level", Action::Default));
            m.insert("rtf", ("ri", "rtf_______", Action::Default));
            m.insert("deff", ("ri", "deflt-font", Action::Default));
            m.insert("mac", ("ri", "macintosh_", Action::Default));
            m.insert("pc", ("ri", "pc________", Action::Default));
            m.insert("pca", ("ri", "pca_______", Action::Default));
            m.insert("ansi", ("ri", "ansi______", Action::Default));
            m.insert("ansicpg", ("ri", "ansi-codpg", Action::Default));
            m.insert("footnote", ("nt", "footnote__", Action::Default));
            m.insert("ftnalt", ("nt", "type______<endnote", Action::TwoPart));
            m.insert("tc", ("an", "toc_______", Action::Default));
            m.insert("bkmkstt", ("an", "book-mk-st", Action::Default));
            m.insert("bkmkstart", ("an", "book-mk-st", Action::Default));
            m.insert("bkmkend", ("an", "book-mk-en", Action::Default));
            m.insert("xe", ("an", "index-mark", Action::Default));
            m.insert("rxe", ("an", "place_____", Action::Default));
            m.insert("bxe", ("in", "index-bold", Action::Default));
            m.insert("ixe", ("in", "index-ital", Action::Default));
            m.insert("txe", ("in", "index-see_", Action::Default));
            m.insert("tcl", ("tc", "toc-level_", Action::Default));
            m.insert("tcn", ("tc", "toc-sup-nu", Action::Default));
            m.insert("field", ("fd", "field_____", Action::Default));
            m.insert("fldinst", ("fd", "field-inst", Action::Default));
            m.insert("fldrslt", ("fd", "field-rslt", Action::Default));
            m.insert("datafield", ("fd", "datafield_", Action::Default));
            m.insert("fonttbl", ("it", "font-table", Action::Default));
            m.insert("colortbl", ("it", "colr-table", Action::Default));
            m.insert("listoverridetable", ("it", "lovr-table", Action::Default));
            m.insert("listtable", ("it", "listtable_", Action::Default));
            m.insert("revtbl", ("it", "revi-table", Action::Default));
            m.insert("b", ("ci", "bold______", Action::BoolSt));
            m.insert("blue", ("ci", "blue______", Action::Color));
            m.insert("caps", ("ci", "caps______", Action::BoolSt));
            m.insert("cf", ("ci", "font-color", Action::Colorz));
            m.insert("chftn", ("ci", "footnot-mk", Action::BoolSt));
            m.insert("dn", ("ci", "font-down_", Action::DivideBy2));
            m.insert("embo", ("ci", "emboss____", Action::BoolSt));
            m.insert("f", ("ci", "font-style", Action::Default));
            m.insert("fs", ("ci", "font-size_", Action::DivideBy2));
            m.insert("green", ("ci", "green_____", Action::Color));
            m.insert("i", ("ci", "italics___", Action::BoolSt));
            m.insert("impr", ("ci", "engrave___", Action::BoolSt));
            m.insert("outl", ("ci", "outline___", Action::BoolSt));
            m.insert("plain", ("ci", "plain_____", Action::BoolSt));
            m.insert("red", ("ci", "red_______", Action::Color));
            m.insert("scaps", ("ci", "small-caps", Action::BoolSt));
            m.insert("shad", ("ci", "shadow____", Action::BoolSt));
            m.insert("strike", ("ci", "strike-thr", Action::BoolSt));
            m.insert("striked", ("ci", "dbl-strike", Action::BoolSt));
            m.insert("sub", ("ci", "subscript_", Action::BoolSt));
            m.insert("super", ("ci", "superscrip", Action::BoolSt));
            m.insert("nosupersub", ("ci", "no-su-supe", Action::NoSupSub));
            m.insert("up", ("ci", "font-up___", Action::DivideBy2));
            m.insert("v", ("ci", "hidden____", Action::Default));
            m.insert("ul", ("ci", "underlined<continous", Action::TwoPart));
            m.insert("uld", ("ci", "underlined<dotted", Action::TwoPart));
            m.insert("uldash", ("ci", "underlined<dash", Action::TwoPart));
            m.insert("uldashd", ("ci", "underlined<dash-dot", Action::TwoPart));
            m.insert(
                "uldashdd",
                ("ci", "underlined<dash-dot-dot", Action::TwoPart),
            );
            m.insert("uldb", ("ci", "underlined<double", Action::TwoPart));
            m.insert("ulhwave", ("ci", "underlined<heavy-wave", Action::TwoPart));
            m.insert("ulldash", ("ci", "underlined<long-dash", Action::TwoPart));
            m.insert("ulth", ("ci", "underlined<thich", Action::TwoPart));
            m.insert("ulthd", ("ci", "underlined<thick-dotted", Action::TwoPart));
            m.insert("ulthdash", ("ci", "underlined<thick-dash", Action::TwoPart));
            m.insert(
                "ulthdashd",
                ("ci", "underlined<thick-dash-dot", Action::TwoPart),
            );
            m.insert(
                "ulthdashdd",
                ("ci", "underlined<thick-dash-dot-dot", Action::TwoPart),
            );
            m.insert(
                "ulthldash",
                ("ci", "underlined<thick-long-dash", Action::TwoPart),
            );
            m.insert(
                "ululdbwave",
                ("ci", "underlined<double-wave", Action::TwoPart),
            );
            m.insert("ulw", ("ci", "underlined<word", Action::TwoPart));
            m.insert("ulwave", ("ci", "underlined<wave", Action::TwoPart));
            m.insert("ulnone", ("ci", "underlined<false", Action::TwoPart));
            m.insert("trowd", ("tb", "row-def___", Action::Default));
            m.insert("cell", ("tb", "cell______", Action::Default));
            m.insert("row", ("tb", "row_______", Action::Default));
            m.insert("intbl", ("tb", "in-table__", Action::Default));
            m.insert("cols", ("tb", "columns___", Action::Default));
            m.insert("trleft", ("tb", "row-pos-le", Action::DivideBy20));
            m.insert("cellx", ("tb", "cell-posit", Action::DivideBy20));
            m.insert("trhdr", ("tb", "row-header", Action::Default));
            m.insert("info", ("di", "doc-info__", Action::Default));
            m.insert("title", ("di", "title_____", Action::Default));
            m.insert("author", ("di", "author____", Action::Default));
            m.insert("operator", ("di", "operator__", Action::Default));
            m.insert("manager", ("di", "manager___", Action::Default));
            m.insert("company", ("di", "company___", Action::Default));
            m.insert("keywords", ("di", "keywords__", Action::Default));
            m.insert("category", ("di", "category__", Action::Default));
            m.insert("doccomm", ("di", "doc-notes_", Action::Default));
            m.insert("comment", ("di", "doc-notes_", Action::Default));
            m.insert("subject", ("di", "subject___", Action::Default));
            m.insert("creatim", ("di", "create-tim", Action::Default));
            m.insert("yr", ("di", "year______", Action::Default));
            m.insert("mo", ("di", "month_____", Action::Default));
            m.insert("dy", ("di", "day_______", Action::Default));
            m.insert("min", ("di", "minute____", Action::Default));
            m.insert("sec", ("di", "second____", Action::Default));
            m.insert("revtim", ("di", "revis-time", Action::Default));
            m.insert("edmins", ("di", "edit-time_", Action::Default));
            m.insert("printim", ("di", "print-time", Action::Default));
            m.insert("buptim", ("di", "backuptime", Action::Default));
            m.insert("nofwords", ("di", "num-of-wor", Action::Default));
            m.insert("nofchars", ("di", "num-of-chr", Action::Default));
            m.insert("nofcharsws", ("di", "numofchrws", Action::Default));
            m.insert("nofpages", ("di", "num-of-pag", Action::Default));
            m.insert("version", ("di", "version___", Action::Default));
            m.insert("vern", ("di", "intern-ver", Action::Default));
            m.insert("hlinkbase", ("di", "linkbase__", Action::Default));
            m.insert("id", ("di", "internalID", Action::Default));
            m.insert("headerf", ("hf", "head-first", Action::Default));
            m.insert("headerl", ("hf", "head-left_", Action::Default));
            m.insert("headerr", ("hf", "head-right", Action::Default));
            m.insert("footerf", ("hf", "foot-first", Action::Default));
            m.insert("footerl", ("hf", "foot-left_", Action::Default));
            m.insert("footerr", ("hf", "foot-right", Action::Default));
            m.insert("header", ("hf", "header____", Action::Default));
            m.insert("footer", ("hf", "footer____", Action::Default));
            m.insert("margl", ("pa", "margin-lef", Action::DivideBy20));
            m.insert("margr", ("pa", "margin-rig", Action::DivideBy20));
            m.insert("margb", ("pa", "margin-bot", Action::DivideBy20));
            m.insert("margt", ("pa", "margin-top", Action::DivideBy20));
            m.insert("gutter", ("pa", "gutter____", Action::DivideBy20));
            m.insert("paperw", ("pa", "paper-widt", Action::DivideBy20));
            m.insert("paperh", ("pa", "paper-hght", Action::DivideBy20));
            m.insert("annotation", ("an", "annotation", Action::Default));
            m.insert("trbrdrh", ("bd", "bor-t-r-hi", Action::Default));
            m.insert("trbrdrv", ("bd", "bor-t-r-vi", Action::Default));
            m.insert("trbrdrt", ("bd", "bor-t-r-to", Action::Default));
            m.insert("trbrdrl", ("bd", "bor-t-r-le", Action::Default));
            m.insert("trbrdrb", ("bd", "bor-t-r-bo", Action::Default));
            m.insert("trbrdrr", ("bd", "bor-t-r-ri", Action::Default));
            m.insert("clbrdrb", ("bd", "bor-cel-bo", Action::Default));
            m.insert("clbrdrt", ("bd", "bor-cel-to", Action::Default));
            m.insert("clbrdrl", ("bd", "bor-cel-le", Action::Default));
            m.insert("clbrdrr", ("bd", "bor-cel-ri", Action::Default));
            m.insert("brdrb", ("bd", "bor-par-bo", Action::Default));
            m.insert("brdrt", ("bd", "bor-par-to", Action::Default));
            m.insert("brdrl", ("bd", "bor-par-le", Action::Default));
            m.insert("brdrr", ("bd", "bor-par-ri", Action::Default));
            m.insert("box", ("bd", "bor-par-bx", Action::Default));
            m.insert("chbrdr", ("bd", "bor-par-bo", Action::Default));
            m.insert("brdrbtw", ("bd", "bor-for-ev", Action::Default));
            m.insert("brdrbar", ("bd", "bor-outsid", Action::Default));
            m.insert("brdrnone", ("bd", "bor-none__<false", Action::TwoPart));
            m.insert("brdrs", ("bt", "bdr-single", Action::Default));
            m.insert("brdrth", ("bt", "bdr-doubtb", Action::Default));
            m.insert("brdrsh", ("bt", "bdr-shadow", Action::Default));
            m.insert("brdrdb", ("bt", "bdr-double", Action::Default));
            m.insert("brdrdot", ("bt", "bdr-dotted", Action::Default));
            m.insert("brdrdash", ("bt", "bdr-dashed", Action::Default));
            m.insert("brdrhair", ("bt", "bdr-hair__", Action::Default));
            m.insert("brdrinset", ("bt", "bdr-inset_", Action::Default));
            m.insert("brdrdashsm", ("bt", "bdr-das-sm", Action::Default));
            m.insert("brdrdashd", ("bt", "bdr-dot-sm", Action::Default));
            m.insert("brdrdashdd", ("bt", "bdr-dot-do", Action::Default));
            m.insert("brdroutset", ("bt", "bdr-outset", Action::Default));
            m.insert("brdrtriple", ("bt", "bdr-trippl", Action::Default));
            m.insert("brdrtnthsg", ("bt", "bdr-thsm__", Action::Default));
            m.insert("brdrthtnsg", ("bt", "bdr-htsm__", Action::Default));
            m.insert("brdrtnthtnsg", ("bt", "bdr-hthsm_", Action::Default));
            m.insert("brdrtnthmg", ("bt", "bdr-thm___", Action::Default));
            m.insert("brdrthtnmg", ("bt", "bdr-htm___", Action::Default));
            m.insert("brdrtnthtnmg", ("bt", "bdr-hthm__", Action::Default));
            m.insert("brdrtnthlg", ("bt", "bdr-thl___", Action::Default));
            m.insert("brdrtnthtnlg", ("bt", "bdr-hthl__", Action::Default));
            m.insert("brdrwavy", ("bt", "bdr-wavy__", Action::Default));
            m.insert("brdrwavydb", ("bt", "bdr-d-wav_", Action::Default));
            m.insert("brdrdashdotstr", ("bt", "bdr-strip_", Action::Default));
            m.insert("brdremboss", ("bt", "bdr-embos_", Action::Default));
            m.insert("brdrengrave", ("bt", "bdr-engra_", Action::Default));
            m.insert("brdrframe", ("bt", "bdr-frame_", Action::Default));
            m.insert("brdrw", ("bt", "bdr-li-wid", Action::DivideBy20));
            m.insert("brsp", ("bt", "bdr-sp-wid", Action::DivideBy20));
            m.insert("brdrcf", ("bt", "bdr-color_", Action::Default));
            m
        };
    }
    &DICT_TOKEN
}

/// Port of `ProcessTokens.__number_type_dict` (48 entries, used by
/// `__list_type_func`/the `levelnfc`/`levelnfcn` RTF keywords).
fn number_type_dict() -> &'static HashMap<i64, &'static str> {
    lazy_static! {
        static ref NUMBER_TYPE_DICT: HashMap<i64, &'static str> = {
            let mut m = HashMap::new();
            m.insert(0, "Arabic");
            m.insert(1, "uppercase Roman numeral");
            m.insert(2, "lowercase Roman numeral");
            m.insert(3, "uppercase letter");
            m.insert(4, "lowercase letter");
            m.insert(5, "ordinal number");
            m.insert(6, "cardianl text number");
            m.insert(7, "ordinal text number");
            m.insert(10, "Kanji numbering without the digit character");
            m.insert(11, "Kanji numbering with the digit character");
            m.insert(1246, "phonetic Katakana characters in aiueo order");
            m.insert(1346, "phonetic katakana characters in iroha order");
            m.insert(14, "double byte character");
            m.insert(15, "single byte character");
            m.insert(16, "Kanji numbering 3");
            m.insert(17, "Kanji numbering 4");
            m.insert(18, "Circle numbering");
            m.insert(19, "double-byte Arabic numbering");
            m.insert(2046, "phonetic double-byte Katakana characters");
            m.insert(2146, "phonetic double-byte katakana characters");
            m.insert(22, "Arabic with leading zero");
            m.insert(23, "bullet");
            m.insert(24, "Korean numbering 2");
            m.insert(25, "Korean numbering 1");
            m.insert(26, "Chinese numbering 1");
            m.insert(27, "Chinese numbering 2");
            m.insert(28, "Chinese numbering 3");
            m.insert(29, "Chinese numbering 4");
            m.insert(30, "Chinese Zodiac numbering 1");
            m.insert(31, "Chinese Zodiac numbering 2");
            m.insert(32, "Chinese Zodiac numbering 3");
            m.insert(33, "Taiwanese double-byte numbering 1");
            m.insert(34, "Taiwanese double-byte numbering 2");
            m.insert(35, "Taiwanese double-byte numbering 3");
            m.insert(36, "Taiwanese double-byte numbering 4");
            m.insert(37, "Chinese double-byte numbering 1");
            m.insert(38, "Chinese double-byte numbering 2");
            m.insert(39, "Chinese double-byte numbering 3");
            m.insert(40, "Chinese double-byte numbering 4");
            m.insert(41, "Korean double-byte numbering 1");
            m.insert(42, "Korean double-byte numbering 2");
            m.insert(43, "Korean double-byte numbering 3");
            m.insert(44, "Korean double-byte numbering 4");
            m.insert(45, "Hebrew non-standard decimal");
            m.insert(46, "Arabic Alif Ba Tah");
            m.insert(47, "Hebrew Biblical standard");
            m.insert(48, "Arabic Abjad style");
            m.insert(255, "No number");
            m
        };
    }
    &NUMBER_TYPE_DICT
}

/// Port of `ProcessTokens.__language_dict` (179 entries, used by
/// `__language_func`/the `\lang` RTF keyword).
fn language_dict() -> &'static HashMap<i64, &'static str> {
    lazy_static! {
        static ref LANGUAGE_DICT: HashMap<i64, &'static str> = {
            let mut m = HashMap::new();
            m.insert(1078, "Afrikaans");
            m.insert(1052, "Albanian");
            m.insert(1025, "Arabic");
            m.insert(5121, "Arabic Algeria");
            m.insert(15361, "Arabic Bahrain");
            m.insert(3073, "Arabic Egypt");
            m.insert(1, "Arabic General");
            m.insert(2049, "Arabic Iraq");
            m.insert(11265, "Arabic Jordan");
            m.insert(13313, "Arabic Kuwait");
            m.insert(12289, "Arabic Lebanon");
            m.insert(4097, "Arabic Libya");
            m.insert(6145, "Arabic Morocco");
            m.insert(8193, "Arabic Oman");
            m.insert(16385, "Arabic Qatar");
            m.insert(10241, "Arabic Syria");
            m.insert(7169, "Arabic Tunisia");
            m.insert(14337, "Arabic U.A.E.");
            m.insert(9217, "Arabic Yemen");
            m.insert(1067, "Armenian");
            m.insert(1101, "Assamese");
            m.insert(2092, "Azeri Cyrillic");
            m.insert(1068, "Azeri Latin");
            m.insert(1069, "Basque");
            m.insert(1093, "Bengali");
            m.insert(4122, "Bosnia Herzegovina");
            m.insert(1026, "Bulgarian");
            m.insert(1109, "Burmese");
            m.insert(1059, "Byelorussian");
            m.insert(1027, "Catalan");
            m.insert(2052, "Chinese China");
            m.insert(4, "Chinese General");
            m.insert(3076, "Chinese Hong Kong");
            m.insert(4100, "Chinese Singapore");
            m.insert(1028, "Chinese Taiwan");
            m.insert(1050, "Croatian");
            m.insert(1029, "Czech");
            m.insert(1030, "Danish");
            m.insert(2067, "Dutch Belgium");
            m.insert(1043, "Dutch Standard");
            m.insert(3081, "English Australia");
            m.insert(10249, "English Belize");
            m.insert(2057, "English British");
            m.insert(4105, "English Canada");
            m.insert(9225, "English Caribbean");
            m.insert(9, "English General");
            m.insert(6153, "English Ireland");
            m.insert(8201, "English Jamaica");
            m.insert(5129, "English New Zealand");
            m.insert(13321, "English Philippines");
            m.insert(7177, "English South Africa");
            m.insert(11273, "English Trinidad");
            m.insert(1033, "English United States");
            m.insert(1061, "Estonian");
            m.insert(1080, "Faerose");
            m.insert(1065, "Farsi");
            m.insert(1035, "Finnish");
            m.insert(1036, "French");
            m.insert(2060, "French Belgium");
            m.insert(11276, "French Cameroon");
            m.insert(3084, "French Canada");
            m.insert(12300, "French Cote d'Ivoire");
            m.insert(5132, "French Luxembourg");
            m.insert(13324, "French Mali");
            m.insert(6156, "French Monaco");
            m.insert(8204, "French Reunion");
            m.insert(10252, "French Senegal");
            m.insert(4108, "French Swiss");
            m.insert(7180, "French West Indies");
            m.insert(9228, "French Democratic Republic of the Congo");
            m.insert(1122, "Frisian");
            m.insert(1084, "Gaelic");
            m.insert(2108, "Gaelic Ireland");
            m.insert(1110, "Galician");
            m.insert(1079, "Georgian");
            m.insert(1031, "German");
            m.insert(3079, "German Austrian");
            m.insert(5127, "German Liechtenstein");
            m.insert(4103, "German Luxembourg");
            m.insert(2055, "German Switzerland");
            m.insert(1032, "Greek");
            m.insert(1095, "Gujarati");
            m.insert(1037, "Hebrew");
            m.insert(1081, "Hindi");
            m.insert(1038, "Hungarian");
            m.insert(1039, "Icelandic");
            m.insert(1057, "Indonesian");
            m.insert(1040, "Italian");
            m.insert(2064, "Italian Switzerland");
            m.insert(1041, "Japanese");
            m.insert(1099, "Kannada");
            m.insert(1120, "Kashmiri");
            m.insert(2144, "Kashmiri India");
            m.insert(1087, "Kazakh");
            m.insert(1107, "Khmer");
            m.insert(1088, "Kirghiz");
            m.insert(1111, "Konkani");
            m.insert(1042, "Korean");
            m.insert(2066, "Korean Johab");
            m.insert(1108, "Lao");
            m.insert(1062, "Latvian");
            m.insert(1063, "Lithuanian");
            m.insert(2087, "Lithuanian Classic");
            m.insert(1086, "Malay");
            m.insert(2110, "Malay Brunei Darussalam");
            m.insert(1100, "Malayalam");
            m.insert(1082, "Maltese");
            m.insert(1112, "Manipuri");
            m.insert(1102, "Marathi");
            m.insert(1104, "Mongolian");
            m.insert(1121, "Nepali");
            m.insert(2145, "Nepali India");
            m.insert(1044, "Norwegian Bokmal");
            m.insert(2068, "Norwegian Nynorsk");
            m.insert(1096, "Oriya");
            m.insert(1045, "Polish");
            m.insert(1046, "Portuguese (Brazil)");
            m.insert(2070, "Portuguese (Portugal)");
            m.insert(1094, "Punjabi");
            m.insert(1047, "Rhaeto-Romanic");
            m.insert(1048, "Romanian");
            m.insert(2072, "Romanian Moldova");
            m.insert(1049, "Russian");
            m.insert(2073, "Russian Moldova");
            m.insert(1083, "Sami Lappish");
            m.insert(1103, "Sanskrit");
            m.insert(3098, "Serbian Cyrillic");
            m.insert(2074, "Serbian Latin");
            m.insert(1113, "Sindhi");
            m.insert(1051, "Slovak");
            m.insert(1060, "Slovenian");
            m.insert(1070, "Sorbian");
            m.insert(11274, "Spanish Argentina");
            m.insert(16394, "Spanish Bolivia");
            m.insert(13322, "Spanish Chile");
            m.insert(9226, "Spanish Colombia");
            m.insert(5130, "Spanish Costa Rica");
            m.insert(7178, "Spanish Dominican Republic");
            m.insert(12298, "Spanish Ecuador");
            m.insert(17418, "Spanish El Salvador");
            m.insert(4106, "Spanish Guatemala");
            m.insert(18442, "Spanish Honduras");
            m.insert(2058, "Spanish Mexico");
            m.insert(3082, "Spanish Modern");
            m.insert(19466, "Spanish Nicaragua");
            m.insert(6154, "Spanish Panama");
            m.insert(15370, "Spanish Paraguay");
            m.insert(10250, "Spanish Peru");
            m.insert(20490, "Spanish Puerto Rico");
            m.insert(1034, "Spanish Traditional");
            m.insert(14346, "Spanish Uruguay");
            m.insert(8202, "Spanish Venezuela");
            m.insert(1072, "Sutu");
            m.insert(1089, "Swahili");
            m.insert(1053, "Swedish");
            m.insert(2077, "Swedish Finland");
            m.insert(1064, "Tajik");
            m.insert(1097, "Tamil");
            m.insert(1092, "Tatar");
            m.insert(1098, "Telugu");
            m.insert(1054, "Thai");
            m.insert(1105, "Tibetan");
            m.insert(1073, "Tsonga");
            m.insert(1074, "Tswana");
            m.insert(1055, "Turkish");
            m.insert(1090, "Turkmen");
            m.insert(1058, "Ukranian");
            m.insert(1056, "Urdu");
            m.insert(2080, "Urdu India");
            m.insert(2115, "Uzbek Cyrillic");
            m.insert(1091, "Uzbek Latin");
            m.insert(1075, "Venda");
            m.insert(1066, "Vietnamese");
            m.insert(1106, "Welsh");
            m.insert(1076, "Xhosa");
            m.insert(1085, "Yiddish");
            m.insert(1077, "Zulu");
            m.insert(1024, "Unkown");
            m.insert(255, "Unkown");
            m
        };
    }
    &LANGUAGE_DICT
}

/// State threaded across a whole [`process_tokens`] call: port of
/// `self.__bracket_count` and `self.__return_code`.
struct ProcessState {
    bracket_count: u32,
    return_code: i64,
}

impl ProcessState {
    fn new() -> Self {
        ProcessState {
            bracket_count: 0,
            return_code: 0,
        }
    }
}

/// Port of `ob_func`.
fn ob_func(state: &mut ProcessState) -> String {
    state.bracket_count += 1;
    format!("ob<nu<open-brack<{:04}\n", state.bracket_count)
}

/// Port of `cb_func`.
fn cb_func(state: &mut ProcessState) -> String {
    let line = format!("cb<nu<clos-brack<{:04}\n", state.bracket_count);
    state.bracket_count = state.bracket_count.saturating_sub(1);
    line
}

/// Port of `__ms_hex_func`.
fn ms_hex_func(num: &str) -> String {
    let num = num.strip_prefix('0').unwrap_or(num).to_uppercase();
    format!("tx<hx<__________<'{num}\n")
}

/// Port of `ms_sub_func`.
fn ms_sub_func(original_token: &str) -> String {
    format!("tx<mc<__________<{original_token}\n")
}

/// Port of `direct_conv_func`.
fn direct_conv_func(original_token: &str) -> String {
    format!("mi<tg<empty_____<{original_token}\n")
}

/// Port of `default_func`.
fn default_func(pre: &str, label: &str, num: Option<&str>) -> String {
    let num = num.unwrap_or("true");
    format!("cw<{pre}<{label}<nu<{num}\n")
}

/// Port of `colorz_func`.
fn colorz_func(pre: &str, label: &str, num: Option<&str>) -> String {
    let num = num.unwrap_or("0");
    format!("cw<{pre}<{label}<nu<{num}\n")
}

/// Port of `text_func`.
fn text_func(label: &str) -> String {
    format!("tx<nu<__________<{label}\n")
}

/// Port of `two_part_func`: `label` here already carries the
/// dict-supplied `<`-joined `"name<value"` shape (e.g.
/// `"align_____<left"`), which this simply splits back apart.
fn two_part_func(pre: &str, label: &str) -> String {
    let mut parts = label.splitn(2, '<');
    let token = parts.next().unwrap_or(label);
    let num = parts.next().unwrap_or("");
    format!("cw<{pre}<{token}<nu<{num}\n")
}

/// Port of `divide_num` + `divide_by_2`/`divide_by_20`'s shared
/// numerator-extraction step: regex-searches `numerator` for its first
/// run of `[0-9.-]` characters (matching `re.search(r'[0-9.\\-]+',
/// numerator)`), divides by `denominator`, and formats to 2 decimal
/// places.
///
/// `numerator: None` (a bare control word with no digits at all, e.g.
/// a lone `\\deftab`) mirrors the Python's `except TypeError:` path
/// (`re.search(pattern, None)` raises `TypeError`, caught): below
/// `run_level > 3` this returns the bare string `"0"` -- *not*
/// `"0.00"` -- because Python's degrade path returns the literal `int`
/// `0` (not a formatted float string), which the caller's f-string
/// then stringifies as `"0"`. Verified against a live run of the
/// Python: `\deftab` (no digits) at the default `run_level` produces
/// `cw<pf<default-ta<nu<0`, and also bumps
/// `self.__return_code = max(self.__return_code, 5)` -- ported here
/// via `state.return_code`.
fn divide_num(
    state: &mut ProcessState,
    numerator: Option<&str>,
    denominator: u32,
    run_level: u32,
) -> Result<String> {
    lazy_static! {
        static ref NUMERIC_RUN: Regex = Regex::new(r"[0-9.\-]+").unwrap();
    }
    let parsed = numerator
        .and_then(|s| NUMERIC_RUN.find(s))
        .and_then(|m| m.as_str().parse::<f64>().ok());
    let Some(value) = parsed else {
        if run_level > 3 {
            return Err(ProcessTokensError::RunLevelError(format!(
                "No number to process?\nthis indicates that the token \\(\\li\\)should have a number and does not\nnumerator is\"{}\"\ndenominator is \"{denominator}\"\n",
                numerator.unwrap_or("None")
            )));
        }
        state.return_code = state.return_code.max(5);
        return Ok("0".to_string());
    };
    Ok(format!("{:.2}", value / f64::from(denominator)))
}

/// Port of `divide_by_2`.
fn divide_by_2(
    state: &mut ProcessState,
    pre: &str,
    label: &str,
    num: Option<&str>,
    run_level: u32,
) -> Result<String> {
    let value = divide_num(state, num, 2, run_level)?;
    Ok(format!("cw<{pre}<{label}<nu<{value}\n"))
}

/// Port of `divide_by_20`.
fn divide_by_20(
    state: &mut ProcessState,
    pre: &str,
    label: &str,
    num: Option<&str>,
    run_level: u32,
) -> Result<String> {
    let value = divide_num(state, num, 20, run_level)?;
    Ok(format!("cw<{pre}<{label}<nu<{value}\n"))
}

/// Port of `color_func`. `num` is required (matches the Python
/// indexing `num[-1]` unconditionally -- a `None` here would already
/// have crashed the live Python, so this treats it the same as an
/// empty string rather than introducing new leniency).
fn color_func(pre: &str, label: &str, num: Option<&str>) -> String {
    let num = num.unwrap_or("");
    let (num, third_field) = if let Some(stripped) = num.strip_suffix(';') {
        (stripped, "en")
    } else {
        (num, "nu")
    };
    let value = num.parse::<i64>().unwrap_or(0);
    let hex = format!("{value:X}");
    let hex = if hex.len() != 2 {
        format!("0{hex}")
    } else {
        hex
    };
    format!("cw<{pre}<{label}<{third_field}<{hex}\n")
}

/// Port of `bool_st_func`.
fn bool_st_func(pre: &str, label: &str, num: Option<&str>) -> Result<String> {
    match num {
        None | Some("") | Some("1") => Ok(format!("cw<{pre}<{label}<nu<true\n")),
        Some("0") => Ok(format!("cw<{pre}<{label}<nu<false\n")),
        Some(other) => Err(ProcessTokensError::InvalidBoolean {
            token: label.to_string(),
            num: other.to_string(),
        }),
    }
}

/// Port of `__no_sup_sub_func`.
fn no_sup_sub_func() -> String {
    "cw<ci<subscript_<nu<false\ncw<ci<superscrip<nu<false\n".to_string()
}

/// Port of `__list_type_func`.
fn list_type_func(pre: &str, label: &str, num: Option<&str>, run_level: u32) -> Result<String> {
    let type_name: &str = match num {
        None => "Arabic",
        Some(num_str) => match num_str.parse::<i64>() {
            // Port of `type = self.__number_type_dict.get(num); if type
            // is None: if run_level > 3: raise self.__bug_handler; type
            // = 'Arabic'`. The `run_level > 3` message the Python
            // builds, `'No type for "%s" in self.__number_type_dict\n'`,
            // is passed to `raise` completely unformatted -- the `%s`
            // is never substituted (no `%` or `.format()` applied) --
            // so the literal two-character text `%s` survives verbatim
            // into the raised message. Preserved as-is below.
            Ok(n) => match number_type_dict().get(&n).copied() {
                Some(name) => name,
                None => {
                    if run_level > 3 {
                        return Err(ProcessTokensError::RunLevelError(
                            "No type for \"%s\" in self.__number_type_dict\n".to_string(),
                        ));
                    }
                    "Arabic"
                }
            },
            Err(_) => {
                if run_level > 3 {
                    return Err(ProcessTokensError::RunLevelError(format!(
                        "Number \"{num_str}\" cannot be converted to integer\n"
                    )));
                }
                "Arabic"
            }
        },
    };
    Ok(format!("cw<{pre}<{label}<nu<{type_name}\n"))
}

/// Port of `__language_func`.
fn language_func(pre: &str, label: &str, num: Option<&str>, run_level: u32) -> Result<String> {
    lazy_static! {
        static ref DIGITS: Regex = Regex::new(r"[0-9]+").unwrap();
    }
    let num = num.unwrap_or("");
    let lang_name = DIGITS
        .find(num)
        .and_then(|m| m.as_str().parse::<i64>().ok())
        .and_then(|n| language_dict().get(&n).copied());
    let lang_name = match lang_name {
        Some(name) => name,
        None => {
            if run_level > 3 {
                return Err(ProcessTokensError::RunLevelError(format!(
                    "No entry for number \"{num}\""
                )));
            }
            "not defined"
        }
    };
    Ok(format!("cw<{pre}<{label}<nu<{lang_name}\n"))
}

/// Port of `split_let_num`: extracts the leading letters and trailing
/// remainder of a mixed alnum RTF keyword (e.g. `"fs24"` ->
/// `("fs", "24")`), via `re.search(r'([a-zA-Z]+)(.*)', token)` (an
/// unanchored search, not a match from position 0 -- kept unanchored
/// here too for parity, though real RTF keywords always start with
/// letters in practice).
fn split_let_num(token: &str, run_level: u32) -> Result<(String, Option<String>)> {
    lazy_static! {
        static ref NUM_EXP: Regex = Regex::new(r"([a-zA-Z]+)(.*)").unwrap();
    }
    let Some(caps) = NUM_EXP.captures(token) else {
        if run_level > 3 {
            return Err(ProcessTokensError::RunLevelError(format!(
                "token is '{token}' \n"
            )));
        }
        return Ok((token.to_string(), Some("0".to_string())));
    };
    let first = caps[1].to_string();
    let second = caps.get(2).map(|m| m.as_str()).unwrap_or("");
    if second.is_empty() {
        if run_level > 3 {
            return Err(ProcessTokensError::RunLevelError(format!(
                "token is '{token}' \n"
            )));
        }
        return Ok((first, Some("0".to_string())));
    }
    Ok((first, Some(second.to_string())))
}

fn dispatch_action(
    state: &mut ProcessState,
    action: Action,
    pre: &str,
    label: &str,
    original_token: &str,
    num: Option<&str>,
    run_level: u32,
) -> Result<String> {
    Ok(match action {
        Action::MsHex => ms_hex_func(num.unwrap_or("")),
        Action::Ob => ob_func(state),
        Action::Cb => cb_func(state),
        Action::MsSub => ms_sub_func(original_token),
        Action::DirectConv => direct_conv_func(original_token),
        Action::Default => default_func(pre, label, num),
        Action::Colorz => colorz_func(pre, label, num),
        Action::ListType => list_type_func(pre, label, num, run_level)?,
        Action::Language => language_func(pre, label, num, run_level)?,
        Action::TwoPart => two_part_func(pre, label),
        Action::DivideBy2 => divide_by_2(state, pre, label, num, run_level)?,
        Action::DivideBy20 => divide_by_20(state, pre, label, num, run_level)?,
        Action::Text => text_func(label),
        Action::Color => color_func(pre, label, num),
        Action::BoolSt => bool_st_func(pre, label, num)?,
        Action::NoSupSub => no_sup_sub_func(),
    })
}

/// Port of `process_cw`: resolves one `\`-prefixed token (leading `\`
/// already stripped by the caller's `token[:1] == '\\'` check in the
/// Python -- here `token` still includes it, and this strips it) into
/// its output line, or `None` if the keyword isn't in [`dict_token`]
/// (matching `dict_token.get(token, (None, None, None))` --
/// unrecognized control words are silently dropped, not an error).
fn process_cw(state: &mut ProcessState, token: &str, run_level: u32) -> Result<Option<String>> {
    const SPECIAL: &[&str] = &["*", ":", "}", "{", "~", "_", "-", ";"];

    let original_token = token;
    let stripped = token.strip_prefix('\\').unwrap_or(token);
    let no_spaces: String = stripped.chars().filter(|&c| c != ' ').collect();
    let only_alpha = !no_spaces.is_empty() && no_spaces.chars().all(|c| c.is_alphabetic());

    let (key, num): (String, Option<String>) =
        if !only_alpha && !SPECIAL.contains(&no_spaces.as_str()) {
            split_let_num(&no_spaces, run_level)?
        } else {
            (no_spaces, None)
        };

    let Some(&(pre, label, action)) = dict_token().get(key.as_str()) else {
        return Ok(None);
    };
    let line = dispatch_action(
        state,
        action,
        pre,
        label,
        original_token,
        num.as_deref(),
        run_level,
    )?;
    Ok(Some(line))
}

/// Port of the plain-text branch of `process_tokens`: splits `token`
/// on `(&.*?;)` (an already-XML-escaped entity reference) via the same
/// "keep the captured delimiter" technique as
/// [`super::tokenize::split_into_tokens`] (Rust's `regex` crate has no
/// direct `re.split`-with-capture-group equivalent), tagging entity
/// fields `tx<ut<...>` and everything else `tx<nu<...>`.
fn emit_text_fields(token: &str, out: &mut String) {
    lazy_static! {
        static ref ENTITY_EXP: Regex = Regex::new(r"&.*?;").unwrap();
    }
    let mut last_end = 0;
    let mut fields: Vec<&str> = Vec::new();
    for m in ENTITY_EXP.find_iter(token) {
        fields.push(&token[last_end..m.start()]);
        fields.push(m.as_str());
        last_end = m.end();
    }
    fields.push(&token[last_end..]);

    for field in fields {
        if field.is_empty() {
            continue;
        }
        if field.starts_with('&') {
            out.push_str("tx<ut<__________<");
        } else {
            out.push_str("tx<nu<__________<");
        }
        out.push_str(field);
        out.push('\n');
    }
}

/// Result of [`process_tokens`]: the intermediate-format text plus the
/// Python's `self.__return_code`, only ever raised above 0 (to 5) by
/// `divide_num`'s `run_level <= 3` degrade path -- see that function's
/// own doc comment, and the
/// `divide_num_degrades_silently_below_run_level_four` test below for
/// a verified example.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessTokensOutput {
    pub content: String,
    pub return_code: i64,
}

/// Port of `ProcessTokens.process_tokens` (the temp-file / `Copy` /
/// rename dance and the `os` module round trip around it are pipeline
/// plumbing, not ported here -- see [`super::copy`]). Takes the
/// one-token-per-line text [`super::tokenize::tokenize`] produces and
/// returns the bracket-tagged intermediate format described in this
/// module's own docs, or an error for structurally invalid RTF
/// (mirroring the Python's `exception_handler`-raised messages).
pub fn process_tokens(content: &str, run_level: u32) -> Result<ProcessTokensOutput> {
    let mut state = ProcessState::new();
    let mut out = String::new();
    let mut line_count = 0usize;

    for line in content.lines() {
        line_count += 1;
        let token = line;

        if line_count == 1 && token != "\\{" {
            return Err(ProcessTokensError::MissingOpeningBrace);
        } else if line_count == 2 && !token.starts_with("\\rtf") {
            return Err(ProcessTokensError::MissingRtfKeyword);
        }

        if token.contains("\\ ") {
            return Err(ProcessTokensError::InvalidBackslashSpaceToken(line_count));
        } else if let Some(stripped) = token.strip_prefix('\\') {
            let _ = stripped; // process_cw re-derives this itself
            if let Some(emitted) = process_cw(&mut state, token, run_level)? {
                out.push_str(&emitted);
            }
        } else {
            emit_text_fields(token, &mut out);
        }
    }

    if line_count == 0 {
        return Err(ProcessTokensError::EmptyFile);
    }

    let report = check_brackets(&out);
    if !report.balanced {
        return Err(ProcessTokensError::UnbalancedBrackets);
    }

    Ok(ProcessTokensOutput {
        content: out,
        return_code: state.return_code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every fixture below was cross-checked against a live run of the
    // real Python `ProcessTokens.process_tokens` (stubbing only its
    // two out-of-scope sibling-module dependencies,
    // `calibre.ptempfile.better_mktemp` and `calibre.ebooks.rtf2xml.copy`
    // -- both pure pipeline plumbing this port doesn't need) to guard
    // against hand-derived misreadings of the ~450 combined dict
    // entries and the action functions' formatting rules.

    fn ok_content(input: &str) -> String {
        process_tokens(input, 1).unwrap().content
    }

    // ---- structural validation ----

    #[test]
    fn rejects_a_document_not_starting_with_open_brace() {
        let err = process_tokens("not-a-brace\n\\rtf1\n\\}", 1).unwrap_err();
        assert_eq!(err, ProcessTokensError::MissingOpeningBrace);
    }

    #[test]
    fn rejects_a_document_missing_the_rtf_keyword_on_line_two() {
        let err = process_tokens("\\{\nnot-rtf\n\\}", 1).unwrap_err();
        assert_eq!(err, ProcessTokensError::MissingRtfKeyword);
    }

    #[test]
    fn rejects_an_empty_document() {
        let err = process_tokens("", 1).unwrap_err();
        assert_eq!(err, ProcessTokensError::EmptyFile);
    }

    #[test]
    fn rejects_a_literal_backslash_space_token() {
        let err = process_tokens("\\{\n\\rtf1\n\\ \n\\}", 1).unwrap_err();
        assert_eq!(err, ProcessTokensError::InvalidBackslashSpaceToken(3));
    }

    #[test]
    fn line_one_and_two_are_also_processed_as_ordinary_control_words() {
        // The opening `\{` and `\rtf1` lines pass the structural
        // checks *and then still flow through `process_cw` like any
        // other token -- verified: `run_tokenize`'s output for this
        // input starts with the `ob<nu<open-brack<0001` /
        // `cw<ri<rtf_______<nu<1` lines below, not just validation
        // with no emitted output for them.
        let out = ok_content("\\{\n\\rtf1\n\\}");
        assert_eq!(
            out,
            "ob<nu<open-brack<0001\ncw<ri<rtf_______<nu<1\ncb<nu<clos-brack<0001\n"
        );
    }

    #[test]
    fn rejects_unbalanced_brackets_via_the_final_check_brackets_pass() {
        let err = process_tokens("\\{\n\\rtf1\n\\{\n", 1).unwrap_err();
        assert_eq!(err, ProcessTokensError::UnbalancedBrackets);
    }

    // ---- default_func / bool_st_func / divide_by_2 / divide_by_20 ----

    #[test]
    fn par_with_no_argument_becomes_a_presence_marker() {
        let out = ok_content("\\{\n\\rtf1\n\\par\n\\}");
        assert!(out.contains("cw<pf<par-end___<nu<true\n"));
    }

    #[test]
    fn bold_without_argument_is_true_and_with_zero_is_false() {
        let on = ok_content("\\{\n\\rtf1\n\\b\n\\}");
        assert!(on.contains("cw<ci<bold______<nu<true\n"));
        let off = ok_content("\\{\n\\rtf1\n\\b0\n\\}");
        assert!(off.contains("cw<ci<bold______<nu<false\n"));
    }

    #[test]
    fn bool_st_func_errors_on_a_non_boolean_argument() {
        // `\b2` (a real "should never happen" input): unconditional
        // error regardless of run_level.
        let err = process_tokens("\\{\n\\rtf1\n\\b2\n\\}", 1).unwrap_err();
        assert!(matches!(err, ProcessTokensError::InvalidBoolean { .. }));
    }

    #[test]
    fn font_size_divides_by_two() {
        let out = ok_content("\\{\n\\rtf1\n\\fs24\n\\}");
        assert!(out.contains("cw<ci<font-size_<nu<12.00\n"));
    }

    #[test]
    fn left_indent_divides_by_twenty() {
        let out = ok_content("\\{\n\\rtf1\n\\li720\n\\}");
        assert!(out.contains("cw<pf<left-inden<nu<36.00\n"));
    }

    #[test]
    fn negative_first_line_indent_divides_correctly() {
        let out = ok_content("\\{\n\\rtf1\n\\fi-360\n\\}");
        assert!(out.contains("cw<pf<fir-ln-ind<nu<-18.00\n"));
    }

    // ---- color_func / colorz_func ----

    #[test]
    fn red_channel_becomes_two_digit_uppercase_hex() {
        let out = ok_content("\\{\n\\rtf1\n\\red255\n\\}");
        assert!(out.contains("cw<ci<red_______<nu<FF\n"));
    }

    #[test]
    fn trailing_semicolon_marks_the_color_table_terminator_subtype() {
        let out = ok_content("\\{\n\\rtf1\n\\red255;\n\\}");
        assert!(out.contains("cw<ci<red_______<en<FF\n"));
    }

    #[test]
    fn font_color_index_is_forwarded_without_hex_conversion() {
        // `cf` uses `colorz_func`, not `color_func`: the raw index is
        // passed straight through, no hex formatting.
        let out = ok_content("\\{\n\\rtf1\n\\cf3\n\\}");
        assert!(out.contains("cw<ci<font-color<nu<3\n"));
    }

    // ---- language_func / list_type_func ----

    #[test]
    fn language_code_resolves_to_a_readable_name() {
        let out = ok_content("\\{\n\\rtf1\n\\lang1033\n\\}");
        assert!(out.contains("cw<pf<language__<nu<English United States\n"));
    }

    #[test]
    fn list_number_type_zero_resolves_to_arabic() {
        let out = ok_content("\\{\n\\rtf1\n\\levelnfc0\n\\}");
        assert!(out.contains("cw<ls<level-type<nu<Arabic\n"));
    }

    #[test]
    fn list_number_type_with_no_argument_defaults_to_arabic() {
        let out = ok_content("\\{\n\\rtf1\n\\levelnfc\n\\}");
        assert!(out.contains("cw<ls<level-type<nu<Arabic\n"));
    }

    // ---- text / entity splitting ----

    #[test]
    fn plain_text_token_becomes_a_tx_nu_line() {
        let out = ok_content("\\{\n\\rtf1\nhello\n\\}");
        assert!(out.contains("tx<nu<__________<hello\n"));
    }

    #[test]
    fn entity_references_are_split_out_as_tx_ut_lines() {
        let out = ok_content("\\{\n\\rtf1\na&amp;b\n\\}");
        assert_eq!(
            out,
            "ob<nu<open-brack<0001\ncw<ri<rtf_______<nu<1\ntx<nu<__________<a\ntx<ut<__________<&amp;\ntx<nu<__________<b\ncb<nu<clos-brack<0001\n"
        );
    }

    // ---- escaped-literal-brace text_func tokens (`\ob`/`\cb`) ----

    #[test]
    fn escaped_literal_brace_tokens_become_plain_text_not_bracket_markers() {
        // `\ob`/`\cb` (tokenize.rs's rewrite of an *escaped* literal
        // brace character, as opposed to a real `\{`/`\}` group
        // delimiter) resolve via `text_func` to plain text lines and
        // do not touch the bracket counter.
        let out = ok_content("\\{\n\\rtf1\n\\ob\n\\cb\n\\}");
        assert_eq!(
            out,
            "ob<nu<open-brack<0001\ncw<ri<rtf_______<nu<1\ntx<nu<__________<{\ntx<nu<__________<}\ncb<nu<clos-brack<0001\n"
        );
    }

    // ---- unrecognized control words are silently dropped ----

    #[test]
    fn unrecognized_control_word_produces_no_output_line() {
        let out = ok_content("\\{\n\\rtf1\n\\notarealkeyword\n\\}");
        assert_eq!(
            out,
            "ob<nu<open-brack<0001\ncw<ri<rtf_______<nu<1\ncb<nu<clos-brack<0001\n"
        );
    }

    // ---- dict_token / number_type_dict / language_dict spot checks ----
    //
    // A representative sample across every `pre` category, cross-checked
    // by hand against `old_src/src/calibre/ebooks/rtf2xml/process_tokens.py`.

    #[test]
    fn dict_token_spot_checks_across_categories() {
        let dict = dict_token();
        assert_eq!(dict.get("b"), Some(&("ci", "bold______", Action::BoolSt)));
        assert_eq!(
            dict.get("par"),
            Some(&("pf", "par-end___", Action::Default))
        );
        assert_eq!(
            dict.get("pnlvlblt"),
            Some(&("ls", "list-bulli", Action::BoolSt))
        );
        assert_eq!(
            dict.get("brdrnone"),
            Some(&("bd", "bor-none__<false", Action::TwoPart))
        );
        assert_eq!(
            dict.get("brdrs"),
            Some(&("bt", "bdr-single", Action::Default))
        );
        assert_eq!(
            dict.get("fonttbl"),
            Some(&("it", "font-table", Action::Default))
        );
        assert_eq!(
            dict.get("trowd"),
            Some(&("tb", "row-def___", Action::Default))
        );
        assert_eq!(
            dict.get("author"),
            Some(&("di", "author____", Action::Default))
        );
        assert_eq!(
            dict.get("headerf"),
            Some(&("hf", "head-first", Action::Default))
        );
        assert_eq!(
            dict.get("margl"),
            Some(&("pa", "margin-lef", Action::DivideBy20))
        );
        assert_eq!(dict.get("xe"), Some(&("an", "index-mark", Action::Default)));
        assert_eq!(
            dict.get("ixe"),
            Some(&("in", "index-ital", Action::Default))
        );
        assert_eq!(
            dict.get("mshex"),
            Some(&("nu", "__________", Action::MsHex))
        );
        assert_eq!(dict.len(), 267);
    }

    #[test]
    fn number_type_dict_spot_checks() {
        let dict = number_type_dict();
        assert_eq!(dict.get(&0), Some(&"Arabic"));
        assert_eq!(dict.get(&23), Some(&"bullet"));
        assert_eq!(dict.get(&255), Some(&"No number"));
        assert_eq!(dict.len(), 48);
    }

    #[test]
    fn language_dict_spot_checks() {
        let dict = language_dict();
        assert_eq!(dict.get(&1033), Some(&"English United States"));
        assert_eq!(dict.get(&1036), Some(&"French"));
        assert_eq!(dict.get(&1024), Some(&"Unkown"));
        assert_eq!(dict.len(), 179);
    }

    // ---- run_level gating ----

    #[test]
    fn divide_num_degrades_silently_below_run_level_four() {
        // `\deftab` with no numeric suffix at all: `num` is `None`,
        // `divide_num` can't extract a numerator, and at the default
        // `run_level` (1) this degrades to the bare string `"0"`
        // (not `"0.00"` -- see `divide_num`'s own doc comment) and
        // bumps `return_code` to 5. Verified against a live run of
        // the Python: `run_tokenize`-equivalent output is
        // `'...cw<pf<default-ta<nu<0\n...'` with `rc == 5`.
        let result = process_tokens("\\{\n\\rtf1\n\\deftab\n\\}", 1).unwrap();
        assert!(result.content.contains("cw<pf<default-ta<nu<0\n"));
        assert_eq!(result.return_code, 5);
    }

    #[test]
    fn divide_num_errors_above_run_level_three() {
        let err = process_tokens("\\{\n\\rtf1\n\\deftab\n\\}", 4).unwrap_err();
        assert!(matches!(err, ProcessTokensError::RunLevelError(_)));
    }
}
