//! Port of `old_src/src/calibre/ebooks/textile/functions.py` -- PyTextile,
//! a Textile-markup-to-HTML converter vendored into calibre.
//!
//! # `regex` vs. `fancy_regex`
//!
//! PyTextile's grammar leans hard on regex lookaround (`(?=...)`,
//! `(?!...)`, `(?<=...)`, `(?<!...)`) and, in two places
//! ([`has_raw_text`]/`hasRawText` and [`Textile::do_p_br`]/`doPBr`), a
//! backreference (`\1`) -- neither of which the plain `regex` crate
//! (already a dependency of this crate) supports at all. This module
//! adds `fancy-regex` (a backtracking engine with Python-compatible
//! lookaround/backreference support) as a dependency and uses it only
//! for the patterns that actually need it:
//!
//! - `HLGN` (the alignment-token grammar used throughout via `A`/`C`)
//!   and everything built from it: [`pba`](Textile::pba)'s alignment
//!   lookup, `table`/`fTable`, `lists`/`fList`, `block`, `links`,
//!   `span`, `image`.
//! - [`has_raw_text`] and [`Textile::do_p_br`]: both match a tag and
//!   its matching close tag via `<(TAG)...>...</\1>` -- a real
//!   backreference.
//! - [`Textile::do_br`]/`doBr`: negative lookbehind for `<br>`/`<br
//!   />` plus a negative lookahead.
//! - [`Textile::get_refs`]/`getRefs`: lookbehind for start-of-string or
//!   whitespace, lookahead for whitespace or end-of-string.
//! - two of the twelve `glyph_defaults` rules (dimension-sign and
//!   3+-uppercase-"caps" detection): both use a lookahead to avoid
//!   consuming the character that follows the match.
//!
//! Everything else (the `macro_defaults` table, `pba`'s non-alignment
//! sub-patterns, `doSpecial`/`code`/`fCode`/`fPre`, `footnoteRef`,
//! `glyphs`/`macros_only`'s tag-splitting, the ten other
//! `glyph_defaults` rules) uses the plain `regex` crate, per this
//! crate's usual preference for it when lookaround isn't needed.
//!
//! # A genuine upstream bug, preserved
//!
//! [`Textile::pba`]'s restricted-mode early return for a `lang`
//! attribute is, in the Python:
//!
//! ```python
//! if self.restricted:
//!     if lang:
//!         return ' lang="%s"'
//!     else:
//!         return ''
//! ```
//!
//! That's a literal, unsubstituted `%s` -- it should read `% lang` (or
//! be an f-string), but doesn't. Verified against a live run of the
//! actual Python (`Textile(restricted=True).pba('[fr]')` really does
//! return the literal string `' lang="%s"'`, not `' lang="fr"'`). This
//! is preserved as-is in [`Textile::pba`] rather than "fixed", with a
//! test (`pba_restricted_lang_bug_is_preserved`) pinning the buggy
//! behavior, since this port needs to match calibre's actual
//! observable output, not a corrected reading of it.

use std::collections::HashMap;

use fancy_regex::Regex as FancyRegex;
use lazy_static::lazy_static;
use regex::Regex;
use uuid::Uuid;

use calibre_utils::smartypants::smarty_pants;

// ===========================================================================
// Class-level constants (ported from `Textile`'s class attributes)
// ===========================================================================

/// Port of `Textile.hlgn`: horizontal-alignment token grammar. Needs
/// `fancy_regex` wherever it's used (directly, or transitively via `A`
/// or `C`) -- it has three lookarounds of its own.
const HLGN: &str = r"(?:\<(?!>)|(?<!<)\>|\<\>|\=|[()]+(?! ))";
/// Port of `Textile.vlgn`: vertical-alignment token grammar. No
/// lookaround; safe with the plain `regex` crate.
const VLGN: &str = r"[\-^~]";
const CLAS: &str = r"(?:\([^)]+\))";
const LNGE: &str = r"(?:\[[^\]]+\])";
const STYL: &str = r"(?:\{[^}]+\})";
const CSPN: &str = r"(?:\\\d+)";
const RSPN: &str = r"(?:\/\d+)";

/// Port of `Textile.pnct`.
const PNCT: &str = r##"[-!"#$%&()*+,/:;<=>?@\'\[\\\]\.^_`{|}~]"##;
/// Port of `Textile.urlch`. Dead upstream: defined but never referenced
/// by any method in `functions.py` (only its commented-out predecessor
/// line above it in the Python hints at what it might once have been
/// used for). Ported anyway for fidelity with the class's constant set.
#[allow(dead_code)]
const URLCH: &str = r##"[\w"$\-_.+*\'(),";\/?:@=&%#{}|\\^~\[\]`]"##;

/// Port of `Textile.url_schemes`.
const URL_SCHEMES: &[&str] = &["http", "https", "ftp", "mailto"];

/// Port of `Textile.btag` (full grammar) and `Textile.btag_lite`
/// (`lite` mode).
const BTAG: &[&str] = &["bq", "bc", "notextile", "pre", "h[1-6]", r"fn\d+", "p"];
const BTAG_LITE: &[&str] = &["bq", "bc", "p"];

lazy_static! {
    /// Port of `Textile.a = fr'(?:{hlgn}|{vlgn})*'`.
    static ref A: String = format!("(?:{HLGN}|{VLGN})*");
    /// Port of `Textile.s = fr'(?:{cspn}|{rspn})*'`.
    static ref S: String = format!("(?:{CSPN}|{RSPN})*");
    /// Port of `Textile.c = r'(?:{})*'.format('|'.join([clas, styl, lnge, hlgn]))`.
    static ref C: String = format!("(?:{CLAS}|{STYL}|{LNGE}|{HLGN})*");
}

// ===========================================================================
// `macro_defaults` / `glyph_defaults`: the class-level substitution tables
// ===========================================================================

/// A single `(pattern, replacement)` rule, using either the plain
/// `regex` crate or `fancy_regex` depending on whether the pattern
/// needs lookaround. `replacement` uses `$1`/`$2`-style group-ref
/// syntax (the Rust equivalent of Python's `\1`/`\2`) where the
/// original Python replacement referenced a group.
enum SubRule {
    Plain(Regex, &'static str),
    Fancy(FancyRegex, &'static str),
}

impl SubRule {
    fn apply(&self, text: &str) -> String {
        match self {
            SubRule::Plain(re, repl) => re.replace_all(text, *repl).into_owned(),
            SubRule::Fancy(re, repl) => re.replace_all(text, *repl).into_owned(),
        }
    }
}

fn apply_rules(rules: &[SubRule], text: &str) -> String {
    let mut text = text.to_string();
    for rule in rules {
        text = rule.apply(&text);
    }
    text
}

macro_rules! plain_rule {
    ($pattern:expr, $replacement:expr) => {
        SubRule::Plain(Regex::new($pattern).expect("static regex"), $replacement)
    };
}

macro_rules! fancy_rule {
    ($pattern:expr, $replacement:expr) => {
        SubRule::Fancy(
            FancyRegex::new($pattern).expect("static regex"),
            $replacement,
        )
    };
}

lazy_static! {
    /// Port of `Textile.macro_defaults`. None of these need lookaround
    /// or backreferences (they're plain literal-or-alternation
    /// patterns matching `{...}` escape tokens), so all are plain
    /// `regex`. Replacements are always literal HTML entities (no
    /// group refs).
    static ref MACRO_DEFAULTS: Vec<SubRule> = vec![
        plain_rule!(r"\{(c\||\|c)\}",     "&#162;"),  // cent
        plain_rule!(r"\{(L-|-L)\}",       "&#163;"),  // pound
        plain_rule!(r"\{(Y=|=Y)\}",       "&#165;"),  // yen
        plain_rule!(r"\{\(c\)\}",         "&#169;"),  // copyright
        plain_rule!(r"\{\(r\)\}",         "&#174;"),  // registered
        plain_rule!(r"\{(\+_|_\+)\}",     "&#177;"),  // plus-minus
        plain_rule!(r"\{1/4\}",           "&#188;"),  // quarter
        plain_rule!(r"\{1/2\}",           "&#189;"),  // half
        plain_rule!(r"\{3/4\}",           "&#190;"),  // three-quarter
        plain_rule!(r"\{(A`|`A)\}",       "&#192;"),  // A-acute
        plain_rule!(r#"\{(A'|'A)\}"#,     "&#193;"),  // A-grave
        plain_rule!(r"\{(A\^|\^A)\}",     "&#194;"),  // A-circumflex
        plain_rule!(r"\{(A~|~A)\}",       "&#195;"),  // A-tilde
        plain_rule!(r#"\{(A"|"A)\}"#,     "&#196;"),  // A-diaeresis
        plain_rule!(r"\{(Ao|oA)\}",       "&#197;"),  // A-ring
        plain_rule!(r"\{(AE)\}",          "&#198;"),  // AE
        plain_rule!(r"\{(C,|,C)\}",       "&#199;"),  // C-cedilla
        plain_rule!(r"\{(E`|`E)\}",       "&#200;"),  // E-acute
        plain_rule!(r#"\{(E'|'E)\}"#,     "&#201;"),  // E-grave
        plain_rule!(r"\{(E\^|\^E)\}",     "&#202;"),  // E-circumflex
        plain_rule!(r#"\{(E"|"E)\}"#,     "&#203;"),  // E-diaeresis
        plain_rule!(r"\{(I`|`I)\}",       "&#204;"),  // I-acute
        plain_rule!(r#"\{(I'|'I)\}"#,     "&#205;"),  // I-grave
        plain_rule!(r"\{(I\^|\^I)\}",     "&#206;"),  // I-circumflex
        plain_rule!(r#"\{(I"|"I)\}"#,     "&#207;"),  // I-diaeresis
        plain_rule!(r"\{(D-|-D)\}",       "&#208;"),  // ETH
        plain_rule!(r"\{(N~|~N)\}",       "&#209;"),  // N-tilde
        plain_rule!(r"\{(O`|`O)\}",       "&#210;"),  // O-acute
        plain_rule!(r#"\{(O'|'O)\}"#,     "&#211;"),  // O-grave
        plain_rule!(r"\{(O\^|\^O)\}",     "&#212;"),  // O-circumflex
        plain_rule!(r"\{(O~|~O)\}",       "&#213;"),  // O-tilde
        plain_rule!(r#"\{(O"|"O)\}"#,     "&#214;"),  // O-diaeresis
        plain_rule!(r"\{x\}",             "&#215;"),  // dimension
        plain_rule!(r"\{(O\/|\/O)\}",     "&#216;"),  // O-slash
        plain_rule!(r"\{(U`|`U)\}",       "&#217;"),  // U-acute
        plain_rule!(r#"\{(U'|'U)\}"#,     "&#218;"),  // U-grave
        plain_rule!(r"\{(U\^|\^U)\}",     "&#219;"),  // U-circumflex
        plain_rule!(r#"\{(U"|"U)\}"#,     "&#220;"),  // U-diaeresis
        plain_rule!(r#"\{(Y'|'Y)\}"#,     "&#221;"),  // Y-grave
        plain_rule!(r"\{sz\}",            "&szlig;"), // sharp-s
        plain_rule!(r"\{(a`|`a)\}",       "&#224;"),  // a-grave
        plain_rule!(r#"\{(a'|'a)\}"#,     "&#225;"),  // a-acute
        plain_rule!(r"\{(a\^|\^a)\}",     "&#226;"),  // a-circumflex
        plain_rule!(r"\{(a~|~a)\}",       "&#227;"),  // a-tilde
        plain_rule!(r#"\{(a"|"a)\}"#,     "&#228;"),  // a-diaeresis
        plain_rule!(r"\{(ao|oa)\}",       "&#229;"),  // a-ring
        plain_rule!(r"\{ae\}",            "&#230;"),  // ae
        plain_rule!(r"\{(c,|,c)\}",       "&#231;"),  // c-cedilla
        plain_rule!(r"\{(e`|`e)\}",       "&#232;"),  // e-grave
        plain_rule!(r#"\{(e'|'e)\}"#,     "&#233;"),  // e-acute
        plain_rule!(r"\{(e\^|\^e)\}",     "&#234;"),  // e-circumflex
        plain_rule!(r#"\{(e"|"e)\}"#,     "&#235;"),  // e-diaeresis
        plain_rule!(r"\{(i`|`i)\}",       "&#236;"),  // i-grave
        plain_rule!(r#"\{(i'|'i)\}"#,     "&#237;"),  // i-acute
        plain_rule!(r"\{(i\^|\^i)\}",     "&#238;"),  // i-circumflex
        plain_rule!(r#"\{(i"|"i)\}"#,     "&#239;"),  // i-diaeresis
        plain_rule!(r"\{(d-|-d)\}",       "&#240;"),  // eth
        plain_rule!(r"\{(n~|~n)\}",       "&#241;"),  // n-tilde
        plain_rule!(r"\{(o`|`o)\}",       "&#242;"),  // o-grave
        plain_rule!(r#"\{(o'|'o)\}"#,     "&#243;"),  // o-acute
        plain_rule!(r"\{(o\^|\^o)\}",     "&#244;"),  // o-circumflex
        plain_rule!(r"\{(o~|~o)\}",       "&#245;"),  // o-tilde
        plain_rule!(r#"\{(o"|"o)\}"#,     "&#246;"),  // o-diaeresis
        plain_rule!(r"\{(o\/|\/o)\}",     "&#248;"),  // o-stroke
        plain_rule!(r"\{(u`|`u)\}",       "&#249;"),  // u-grave
        plain_rule!(r#"\{(u'|'u)\}"#,     "&#250;"),  // u-acute
        plain_rule!(r"\{(u\^|\^u)\}",     "&#251;"),  // u-circumflex
        plain_rule!(r#"\{(u"|"u)\}"#,     "&#252;"),  // u-diaeresis
        plain_rule!(r#"\{(y'|'y)\}"#,     "&#253;"),  // y-acute
        plain_rule!(r#"\{(y"|"y)\}"#,     "&#255;"),  // y-diaeresis

        plain_rule!(r"\{(C\x{2c7}|\x{2c7}C)\}", "&#268;"), // C-caron
        plain_rule!(r"\{(c\x{2c7}|\x{2c7}c)\}", "&#269;"), // c-caron
        plain_rule!(r"\{(D\x{2c7}|\x{2c7}D)\}", "&#270;"), // D-caron
        plain_rule!(r"\{(d\x{2c7}|\x{2c7}d)\}", "&#271;"), // d-caron
        plain_rule!(r"\{(E\x{2c7}|\x{2c7}E)\}", "&#282;"), // E-caron
        plain_rule!(r"\{(e\x{2c7}|\x{2c7}e)\}", "&#283;"), // e-caron
        plain_rule!(r#"\{(L'|'L)\}"#,     "&#313;"),  // L-acute
        plain_rule!(r#"\{(l'|'l)\}"#,     "&#314;"),  // l-acute
        plain_rule!(r"\{(L\x{2c7}|\x{2c7}L)\}", "&#317;"), // L-caron
        plain_rule!(r"\{(l\x{2c7}|\x{2c7}l)\}", "&#318;"), // l-caron
        plain_rule!(r"\{(N\x{2c7}|\x{2c7}N)\}", "&#327;"), // N-caron
        plain_rule!(r"\{(n\x{2c7}|\x{2c7}n)\}", "&#328;"), // n-caron

        plain_rule!(r"\{OE\}",            "&#338;"),  // OE
        plain_rule!(r"\{oe\}",            "&#339;"),  // oe

        plain_rule!(r#"\{(R'|'R)\}"#,     "&#340;"),  // R-acute
        plain_rule!(r#"\{(r'|'r)\}"#,     "&#341;"),  // r-acute
        plain_rule!(r"\{(R\x{2c7}|\x{2c7}R)\}", "&#344;"), // R-caron
        plain_rule!(r"\{(r\x{2c7}|\x{2c7}r)\}", "&#345;"), // r-caron

        plain_rule!(r"\{(S\^|\^S)\}",     "&#348;"),  // S-circumflex
        plain_rule!(r"\{(s\^|\^s)\}",     "&#349;"),  // s-circumflex

        plain_rule!(r"\{(S\x{2c7}|\x{2c7}S)\}", "&#352;"), // S-caron
        plain_rule!(r"\{(s\x{2c7}|\x{2c7}s)\}", "&#353;"), // s-caron
        plain_rule!(r"\{(T\x{2c7}|\x{2c7}T)\}", "&#356;"), // T-caron
        plain_rule!(r"\{(t\x{2c7}|\x{2c7}t)\}", "&#357;"), // t-caron
        plain_rule!(r"\{(U\x{b0}|\x{b0}U)\}",   "&#366;"), // U-ring
        plain_rule!(r"\{(u\x{b0}|\x{b0}u)\}",   "&#367;"), // u-ring
        plain_rule!(r"\{(Z\x{2c7}|\x{2c7}Z)\}", "&#381;"), // Z-caron
        plain_rule!(r"\{(z\x{2c7}|\x{2c7}z)\}", "&#382;"), // z-caron

        plain_rule!(r"\{\*\}",            "&#8226;"), // bullet
        plain_rule!(r"\{Fr\}",            "&#8355;"), // Franc
        plain_rule!(r"\{(L=|=L)\}",       "&#8356;"), // Lira
        plain_rule!(r"\{Rs\}",            "&#8360;"), // Rupee
        plain_rule!(r"\{(C=|=C)\}",       "&#8364;"), // euro
        plain_rule!(r"\{tm\}",            "&#8482;"), // trademark
        plain_rule!(r"\{spades?\}",       "&#9824;"), // spade
        plain_rule!(r"\{clubs?\}",        "&#9827;"), // club
        plain_rule!(r"\{hearts?\}",       "&#9829;"), // heart
        plain_rule!(r"\{diam(onds?|s)\}", "&#9830;"), // diamond
        plain_rule!(r#"\{"\}"#,           "&#34;"),   // double-quote
        plain_rule!(r"\{'\}",             "&#39;"),   // single-quote
        fancy_rule_utf8_apostrophe_closing(),          // closing-single-quote - apostrophe
        fancy_rule_utf8_apostrophe_opening(),          // opening-single-quote
        fancy_rule_utf8_dquote_closing(),              // closing-double-quote
        fancy_rule_utf8_dquote_opening(),              // opening-double-quote
    ];
}

// The last four `macro_defaults` entries embed literal Unicode smart-quote
// characters alongside a `'` / `"` alternative -- none need lookaround, but
// building them via small helper functions (rather than inline in the
// `vec![]` above) keeps the raw-string quoting manageable.
fn fancy_rule_utf8_apostrophe_closing() -> SubRule {
    plain_rule!(r"\{(\x{2019}|'/|/')\}", "&#8217;")
}
fn fancy_rule_utf8_apostrophe_opening() -> SubRule {
    plain_rule!(r"\{(\x{2018}|\\'|'\\)\}", "&#8216;")
}
fn fancy_rule_utf8_dquote_closing() -> SubRule {
    plain_rule!(r#"\{(\x{201d}|"/|/")\}"#, "&#8221;")
}
fn fancy_rule_utf8_dquote_opening() -> SubRule {
    plain_rule!(r#"\{(\x{201c}|\\"|"\\)\}"#, "&#8220;")
}

lazy_static! {
    /// Port of `Textile.glyph_defaults`. Two entries (dimension-sign,
    /// 3+-uppercase-caps) need a lookahead and use `fancy_regex`; the
    /// other ten are plain `regex`.
    static ref GLYPH_DEFAULTS: Vec<SubRule> = vec![
        fancy_rule!(r#"(\d+'?"?)( ?)x( ?)(?=\d+)"#, "$1$2&#215;$3"), // dimension sign
        plain_rule!(r#"(?i)(\d+)'(\s)"#, "$1&#8242;$2"), // prime
        plain_rule!(r#"(?i)(\d+)"(\s)"#, "$1&#8243;$2"), // prime-double
        plain_rule!(r"\b([A-Z][A-Z0-9]{2,})\b(?:\(([^)]*)\))", r#"<acronym title="$2">$1</acronym>"#), // 3+ uppercase acronym
        fancy_rule!(r"\b([A-Z][A-Z'\-]+[A-Z])(?=[\s.,\)>])", r#"<span class="caps">$1</span>"#), // 3+ uppercase
        plain_rule!(r"\b(\s{0,1})?\.{3}", "$1&#8230;"), // ellipsis
        plain_rule!(r"(?m)^[\*_-]{3,}$", "<hr />"), // <hr> scene-break
        plain_rule!(r"(^|[^-])--([^-]|$)", "$1&#8212;$2"), // em dash
        plain_rule!(r"\s-(?:\s|$)", " &#8211; "), // en dash
        // NB: the Python source writes these as `[([]` / `[])]`
        // (unescaped `[`/leading `]` inside a character class), which
        // Python's `re` accepts (POSIX bracket-expression convention:
        // `]` right after `[` is a literal, and `[` needs no escaping
        // inside a class). Rust's `regex` crate needs both escaped --
        // an unescaped `[` inside a class starts a nested
        // class/set-operation, and `]` always closes the class unless
        // escaped -- so this uses `[(\[]` / `[)\]]` instead; the
        // matched character set (`(`/`[`, and `)`/`]`) is identical.
        plain_rule!(r"(?i)\b( ?)[(\[]TM[)\]]", "$1&#8482;"), // trademark
        plain_rule!(r"(?i)\b( ?)[(\[]R[)\]]", "$1&#174;"), // registered
        plain_rule!(r"(?i)\b( ?)[(\[]C[)\]]", "$1&#169;"), // copyright
    ];
}

// ===========================================================================
// Small precompiled patterns used by individual methods
// ===========================================================================

lazy_static! {
    // -- `pba` --
    static ref RE_COLSPAN: Regex = Regex::new(r"\\(\d+)").expect("static regex");
    static ref RE_ROWSPAN: Regex = Regex::new(r"/(\d+)").expect("static regex");
    static ref RE_VALIGN: Regex = Regex::new(&format!("({VLGN})")).expect("static regex");
    static ref RE_STYLE_ATT: Regex = Regex::new(r"\{([^}]*)\}").expect("static regex");
    static ref RE_LANG_ATT: Regex = Regex::new(r"\[([^\]]+)\]").expect("static regex");
    static ref RE_CLASS_ATT: Regex = Regex::new(r"\(([^()]+)\)").expect("static regex");
    static ref RE_PAD_LEFT: Regex = Regex::new(r"([(]+)").expect("static regex");
    static ref RE_PAD_RIGHT: Regex = Regex::new(r"([)]+)").expect("static regex");
    static ref RE_HALIGN: FancyRegex = FancyRegex::new(&format!("({HLGN})")).expect("static regex");
    static ref RE_ID_SPLIT: Regex = Regex::new(r"^(.*)#(.*)$").expect("static regex");

    // -- `hasRawText` --
    static ref RE_RAW_TEXT_BLOCKS: FancyRegex = FancyRegex::new(
        r"(?s)<(p|blockquote|div|form|table|ul|ol|pre|h\d)[^>]*?>.*</\1>"
    ).expect("static regex");
    static ref RE_RAW_TEXT_VOID: Regex =
        Regex::new(r"<(hr|br)[^>]*?/>").expect("static regex");

    // -- `doPBr` / `doBr` --
    static ref RE_P_BR: FancyRegex =
        FancyRegex::new(r"(?s)<(p)([^>]*?)>(.*)(</\1>)").expect("static regex");
    static ref RE_BR_HTML: FancyRegex =
        FancyRegex::new(r"(.+)(?:(?<!<br>)|(?<!<br />))\n(?![#*\s|])").expect("static regex");

    // -- `footnoteRef` / `footnoteID` --
    static ref RE_FOOTNOTE_REF: Regex =
        Regex::new(r"\b\[([0-9]+)\](\s)?").expect("static regex");

    // -- `glyphs` / `macros_only` --
    static ref RE_TRAILING_QUOTE: Regex = Regex::new("\"\\z").expect("static regex");
    static ref RE_TAG_SPLIT: Regex = Regex::new(r"(<.*?>)").expect("static regex");
    static ref RE_ANY_TAG: Regex = Regex::new(r"<.*>").expect("static regex");
    static ref RE_HAS_MACRO: Regex = Regex::new(r"\{.+?\}").expect("static regex");

    // -- `getRefs` --
    static ref RE_GET_REFS: FancyRegex = FancyRegex::new(
        r"(?:(?<=^)|(?<=\s))\[(.+)\]((?:http(?:s?)://|/)\S+)(?=\s|$)"
    ).expect("static regex");

    // -- `table` / `fTable` --
    static ref RE_TABLE: FancyRegex = FancyRegex::new(&format!(
        r"(?sm)^(?:table(_?{s}{a}{c})\. ?\n)?^({a}{c}\.? ?\|.*\|)\n\n",
        s = *S, a = *A, c = *C
    )).expect("static regex");
    static ref RE_TABLE_ROW: FancyRegex = FancyRegex::new(&format!(
        r"^({a}{c}\. )(.*)", a = *A, c = *C
    )).expect("static regex");
    static ref RE_TABLE_CELL: FancyRegex = FancyRegex::new(&format!(
        r"^(_?{s}{a}{c}\. )(.*)", s = *S, a = *A, c = *C
    )).expect("static regex");
    static ref RE_CELL_HEADER: Regex = Regex::new(r"^_").expect("static regex");

    // -- `lists` / `fList` --
    static ref RE_LISTS: FancyRegex = FancyRegex::new(&format!(
        r"(?sm)^([#*]+{c} .*)$(?![^#*])", c = *C
    )).expect("static regex");
    static ref RE_FLIST: FancyRegex = FancyRegex::new(&format!(
        r"(?s)^([#*]+)({a}{c}) (.*)$", a = *A, c = *C
    )).expect("static regex");
    static ref RE_FLIST_NEXT: Regex = Regex::new(r"^([#*]+)\s.*").expect("static regex");
    static ref RE_LIST_TYPE: Regex = Regex::new(r"^#+").expect("static regex");

    // -- `fBlock` --
    static ref RE_FOOTNOTE_TAG: Regex = Regex::new(r"fn(\d+)").expect("static regex");
    static ref RE_HEADER_TAG: Regex = Regex::new(r"h([1-6])").expect("static regex");
    static ref RE_LEADING_SPACE: Regex = Regex::new(r"^\s").expect("static regex");

    // -- `links` --
    static ref PUNCT_ESCAPED_FOR_LINKS: String =
        regex::escape(r##"!"#$%&'*+,-./:;=?@\^_`|~"##);
    static ref RE_LINKS: FancyRegex = FancyRegex::new(&LINKS_PATTERN_TEMPLATE
        .replace("__PUNCT__", &PUNCT_ESCAPED_FOR_LINKS)
        .replace("__C__", &C)
    ).expect("static regex");

    // -- `span` --
    static ref SPAN_QTAGS: Vec<(&'static str, &'static str, FancyRegex)> = {
        // (raw qtag regex-escaped form, html tag name, compiled pattern)
        let entries: &[(&str, &str)] = &[
            (r"\*\*", "b"),
            (r"\*", "strong"),
            (r"\?\?", "cite"),
            (r"\-", "del"),
            ("__", "i"),
            ("_", "em"),
            ("%", "span"),
            (r"\+", "ins"),
            ("~", "sub"),
            (r"\^", "sup"),
        ];
        entries
            .iter()
            .map(|&(qtag, tag)| {
                let pattern = SPAN_PATTERN_TEMPLATE
                    .replace("__QTAG__", qtag)
                    .replace("__C__", &C)
                    .replace("__PNCT__", SPAN_LOCAL_PNCT)
                    .replace("__PNCT_CLASS__", PNCT);
                (qtag, tag, FancyRegex::new(&pattern).expect("static regex"))
            })
            .collect()
    };

    // -- `image` --
    static ref RE_IMAGE: FancyRegex = FancyRegex::new(&IMAGE_PATTERN_TEMPLATE
        .replace("__C__", &C)
    ).expect("static regex");
}

/// Port of the `links()` method's verbose (`re.X`) pattern, with `__C__`
/// standing in for `self.c` and `__PUNCT__` for `re.escape(punct)` --
/// see [`RE_LINKS`]. Ported to `(?x)` mode directly (`fancy_regex`
/// supports it) rather than manually stripping whitespace/comments, to
/// stay as close to the Python source as possible.
const LINKS_PATTERN_TEMPLATE: &str = r#"(?x)
    (?P<pre>    [\s\[{(]|[__PUNCT__]   )?
    "                          # start
    (?P<atts>   __C__       )
    (?P<text>   [^"]+?   )
    \s?
    (?:   \(([^)]+?)\)(?=")   )?     # $title
    ":
    (?P<url>    (?:ftp|https?)? (?: :// )? [-A-Za-z0-9+&@#/?=~_()|!:,.;]*[-A-Za-z0-9+&@#/=~_()|]   )
    (?P<post>   [^\w\/;]*?   )
    (?=<|\s|$)
"#;

/// Port of `span()`'s per-`qtag` verbose pattern. `__QTAG__` appears
/// four times (matching the Python's four uses of the `{qtag}`
/// f-string interpolation per iteration).
///
/// `span()`'s Python has *two* different things both loosely called
/// "punct": a short local `pnct = ".,\"'?!;:"` (bare characters, no
/// brackets -- used inside other character classes and as `[pnct]*`)
/// and the *class-level* `self.pnct` (a complete, already-bracketed
/// `[...]` character class -- used standalone at the very end with a
/// `{1,2}` quantifier applied to the whole class). `__PNCT__` here is
/// the former ([`SPAN_LOCAL_PNCT`]); `__PNCT_CLASS__` is the latter
/// ([`PNCT`]). Conflating the two (substituting the bracketed `PNCT`
/// into `__PNCT__`'s bracket-nested positions) was an earlier bug in
/// this port: it broke the single-character qtags (`*`, `_`) while
/// leaving the double-character ones (`**`, `__`) looking like they
/// still worked, caught by `bold_and_italic_variants`'s test against
/// live Python output.
const SPAN_PATTERN_TEMPLATE: &str = r#"(?x)
    (?:^|(?<=[\s>__PNCT__\(])|\[|([\]}]))
    (__QTAG__)(?!__QTAG__)
    (__C__)
    (?::(\S+))?
    ([^\s__QTAG__]+|\S[^__QTAG__\n]*[^\s__QTAG__\n])
    ([__PNCT__]*)
    __QTAG__
    (?:$|([\]}])|(?=__PNCT_CLASS__{1,2}|\s))
"#;

/// Port of `span()`'s local `pnct = ".,\"'?!;:"` (distinct from the
/// class-level `self.pnct`/[`PNCT`] -- see [`SPAN_PATTERN_TEMPLATE`]'s
/// docs).
const SPAN_LOCAL_PNCT: &str = ".,\"'?!;:";

/// Port of `image()`'s verbose pattern.
const IMAGE_PATTERN_TEMPLATE: &str = r#"(?x)
    (?:[\[{])?          # pre
    \!                 # opening !
    (__C__)               # optional style,class atts
    (?:\. )?           # optional dot-space
    ([^\s(!]+)         # presume this is the src
    \s?                # optional space
    (?:\(([^\)]+)\))?  # optional title
    \!                 # closing
    (?::(\S+))?        # optional href
    (?:[\]}]|(?=\s|$)) # lookahead: space or end of string
"#;

// ===========================================================================
// The `Textile` struct
// ===========================================================================

/// Port of the `Textile` class.
pub struct Textile {
    pub restricted: bool,
    pub lite: bool,
    pub noimage: bool,
    /// Port of `self.get_sizes`: whether [`Textile::f_image`] should
    /// attempt to fetch remote images to backfill `width`/`height`
    /// (see [`getimagesize`]). Defaults to `false`, matching Python
    /// (nothing in `__init__` or the free functions ever sets it to
    /// `True` -- it's dead-by-default upstream too).
    pub get_sizes: bool,
    /// Port of `self.fn` (renamed: `fn` is a Rust keyword). Footnote
    /// number -> generated UUID id.
    footnotes: HashMap<String, String>,
    /// Port of `self.urlrefs`.
    urlrefs: HashMap<String, String>,
    /// Port of `self.shelf`.
    shelf: HashMap<String, String>,
    /// Port of `self.rel`.
    rel: String,
    /// Port of `self.html_type`.
    html_type: String,
}

impl Default for Textile {
    fn default() -> Self {
        Self::new(false, false, false)
    }
}

impl Textile {
    /// Port of `Textile.__init__`.
    pub fn new(restricted: bool, lite: bool, noimage: bool) -> Self {
        Textile {
            restricted,
            lite,
            noimage,
            get_sizes: false,
            footnotes: HashMap::new(),
            urlrefs: HashMap::new(),
            shelf: HashMap::new(),
            rel: String::new(),
            html_type: "xhtml".to_string(),
        }
    }

    /// Port of `Textile.textile`: the main entry point. `rel` mirrors
    /// Python's `Optional[str]` parameter (`None` -> no `rel=...` is
    /// added to links).
    pub fn textile(
        &mut self,
        text: &str,
        rel: Option<&str>,
        head_offset: i32,
        html_type: &str,
    ) -> String {
        self.html_type = html_type.to_string();

        let mut text = normalize_newlines(text);

        if self.restricted {
            text = Textile::encode_html(&text, false);
        }

        if let Some(rel) = rel {
            self.rel = format!(" rel=\"{rel}\"");
        }

        text = self.get_refs(&text);
        text = self.block(&text, head_offset);
        text = self.retrieve(&text);
        text = smarty_pants(&text, "q");

        text
    }

    // -----------------------------------------------------------------
    // `pba`
    // -----------------------------------------------------------------

    /// Port of `Textile.pba`: parse block attributes (alignment,
    /// class, style, language, colspan/rowspan).
    ///
    /// See the module docs for the genuine upstream bug preserved in
    /// the `self.restricted && lang` branch (a literal, unsubstituted
    /// `%s`).
    pub fn pba(&self, input: &str, element: Option<&str>) -> String {
        let mut style: Vec<String> = Vec::new();
        let mut aclass = String::new();
        let mut lang = String::new();
        let mut colspan = String::new();
        let mut rowspan = String::new();
        let mut id = String::new();

        if input.is_empty() {
            return String::new();
        }

        let mut matched = input.to_string();

        if element == Some("td") {
            if let Some(m) = RE_COLSPAN.captures(&matched) {
                colspan = m[1].to_string();
            }
            if let Some(m) = RE_ROWSPAN.captures(&matched) {
                rowspan = m[1].to_string();
            }
        }

        if matches!(element, Some("td") | Some("tr")) {
            if let Some(m) = RE_VALIGN.captures(&matched) {
                style.push(format!("vertical-align:{};", Textile::v_align(&m[1])));
            }
        }

        // NB: Python's `matched.replace(m.group(0), '')` (no count arg)
        // replaces *every* occurrence of the matched substring, not just
        // the one just matched -- `String::replace` (not `replacen`)
        // mirrors that exactly.
        if let Some(m) = RE_STYLE_ATT.captures(&matched) {
            let whole = m[0].to_string();
            style.push(format!("{};", m[1].trim_end_matches(';')));
            matched = matched.replace(&whole, "");
        }

        if let Some(m) = RE_LANG_ATT.captures(&matched) {
            let whole = m[0].to_string();
            lang = m[1].to_string();
            matched = matched.replace(&whole, "");
        }

        if let Some(m) = RE_CLASS_ATT.captures(&matched) {
            let whole = m[0].to_string();
            aclass = m[1].to_string();
            matched = matched.replace(&whole, "");
        }

        if let Some(m) = RE_PAD_LEFT.captures(&matched) {
            let whole = m[0].to_string();
            style.push(format!("padding-left:{}em;", m[1].len()));
            matched = matched.replace(&whole, "");
        }

        if let Some(m) = RE_PAD_RIGHT.captures(&matched) {
            let whole = m[0].to_string();
            style.push(format!("padding-right:{}em;", m[1].len()));
            matched = matched.replace(&whole, "");
        }

        if let Ok(Some(m)) = RE_HALIGN.captures(matched.as_str()) {
            style.push(format!("text-align:{};", Textile::h_align(&m[1])));
        }

        let aclass_snapshot = aclass.clone();
        if let Some(m) = RE_ID_SPLIT.captures(&aclass_snapshot) {
            id = m[2].to_string();
            aclass = m[1].to_string();
        }

        if self.restricted {
            // Genuine upstream bug, preserved: this is a literal,
            // unsubstituted `%s`, not `f' lang="{lang}"'`. See the
            // module docs.
            return if !lang.is_empty() {
                " lang=\"%s\"".to_string()
            } else {
                String::new()
            };
        }

        let mut result = String::new();
        if !style.is_empty() {
            result.push_str(&format!(" style=\"{}\"", style.join("")));
        }
        if !aclass.is_empty() {
            result.push_str(&format!(" class=\"{aclass}\""));
        }
        if !lang.is_empty() {
            result.push_str(&format!(" lang=\"{lang}\""));
        }
        if !id.is_empty() {
            result.push_str(&format!(" id=\"{id}\""));
        }
        if !colspan.is_empty() {
            result.push_str(&format!(" colspan=\"{colspan}\""));
        }
        if !rowspan.is_empty() {
            result.push_str(&format!(" rowspan=\"{rowspan}\""));
        }
        result
    }

    // -----------------------------------------------------------------
    // `hasRawText`
    // -----------------------------------------------------------------

    /// Port of `Textile.hasRawText`: does `text` have text not already
    /// enclosed by a block tag?
    pub fn has_raw_text(text: &str) -> bool {
        has_raw_text(text)
    }

    // -----------------------------------------------------------------
    // `table` / `fTable`
    // -----------------------------------------------------------------

    /// Port of `Textile.table`.
    pub fn table(&mut self, text: &str) -> String {
        let text = format!("{text}\n\n");
        sub_fancy_mut(self, &RE_TABLE, &text, |t, caps| t.f_table(caps))
    }

    /// Port of `Textile.fTable`.
    fn f_table(&mut self, caps: &fancy_regex::Captures<'_, str>) -> String {
        let tatts = self.pba(caps.get(1).map(|m| m.as_str()).unwrap_or(""), Some("table"));
        let body = caps.get(2).map(|m| m.as_str()).unwrap_or("");

        let mut rows: Vec<String> = Vec::new();
        for row in body.split('\n').filter(|r| !r.is_empty()) {
            let row_trimmed_start = row.trim_start();
            let (ratts, row_content) = match RE_TABLE_ROW.captures(row_trimmed_start).ok().flatten()
            {
                Some(rm) => {
                    let ratts = self.pba(&rm[1], Some("tr"));
                    (ratts, rm[2].to_string())
                }
                None => (String::new(), row.to_string()),
            };

            let mut cells: Vec<String> = Vec::new();
            let parts: Vec<&str> = row_content.split('|').collect();
            let inner = if parts.len() >= 2 {
                &parts[1..parts.len() - 1]
            } else {
                &[][..]
            };
            for &cell in inner {
                let ctyp = if RE_CELL_HEADER.is_match(cell) {
                    "h"
                } else {
                    "d"
                };
                let (catts, cell_content) = match RE_TABLE_CELL.captures(cell).ok().flatten() {
                    Some(cm) => (self.pba(&cm[1], Some("td")), cm[2].to_string()),
                    None => (String::new(), cell.to_string()),
                };
                let spanned = self.span(&cell_content);
                let cell_html = self.graf(&spanned);
                cells.push(format!("\t\t\t<t{ctyp}{catts}>{cell_html}</t{ctyp}>"));
            }
            rows.push(format!(
                "\t\t<tr{}>\n{}\n\t\t</tr>",
                ratts,
                cells.join("\n")
            ));
        }

        format!("\t<table{}>\n{}\n\t</table>\n\n", tatts, rows.join("\n"))
    }

    // -----------------------------------------------------------------
    // `lists` / `fList` / `lT`
    // -----------------------------------------------------------------

    /// Port of `Textile.lists`.
    pub fn lists(&mut self, text: &str) -> String {
        sub_fancy_mut(self, &RE_LISTS, text, |t, caps| t.f_list(caps))
    }

    /// Port of `Textile.fList`.
    fn f_list(&mut self, caps: &fancy_regex::Captures<'_, str>) -> String {
        let whole = caps.get(0).map(|m| m.as_str()).unwrap_or("");
        let lines: Vec<&str> = whole.split('\n').collect();
        let mut result: Vec<String> = Vec::new();
        let mut open_lists: Vec<String> = Vec::new();

        for (i, &line) in lines.iter().enumerate() {
            let nextline = lines.get(i + 1).copied().unwrap_or("");

            let Some(m) = RE_FLIST.captures(line).ok().flatten() else {
                result.push(line.to_string());
                continue;
            };
            let tl = m[1].to_string();
            let atts = m[2].to_string();
            let content = m[3].to_string();

            let nl = RE_FLIST_NEXT
                .captures(nextline)
                .map(|nm| nm[1].to_string())
                .unwrap_or_default();

            let mut line_out = if !open_lists.contains(&tl) {
                open_lists.push(tl.clone());
                let pba = self.pba(&atts, None);
                let grafed = self.graf(&content);
                format!("\t<{}l{}>\n\t\t<li>{}", Textile::l_t(&tl), pba, grafed)
            } else {
                format!("\t\t<li>{}", self.graf(&content))
            };

            if nl.len() <= tl.len() {
                line_out.push_str("</li>");
            }
            for k in open_lists.clone().into_iter().rev() {
                if k.len() > nl.len() {
                    line_out.push_str(&format!("\n\t</{}l>", Textile::l_t(&k)));
                    if k.len() > 1 {
                        line_out.push_str("</li>");
                    }
                    open_lists.retain(|x| x != &k);
                }
            }

            result.push(line_out);
        }

        result.join("\n")
    }

    /// Port of `Textile.lT`.
    fn l_t(input: &str) -> &'static str {
        if RE_LIST_TYPE.is_match(input) {
            "o"
        } else {
            "u"
        }
    }

    // -----------------------------------------------------------------
    // `doPBr` / `doBr`
    // -----------------------------------------------------------------

    /// Port of `Textile.doPBr`.
    fn do_p_br(&self, input: &str) -> String {
        sub_fancy(&RE_P_BR, input, |caps| self.do_br(caps))
    }

    /// Port of `Textile.doBr`.
    fn do_br(&self, caps: &fancy_regex::Captures<'_, str>) -> String {
        let tag = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let attrs = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        let content = caps.get(3).map(|m| m.as_str()).unwrap_or("");
        let close = caps.get(4).map(|m| m.as_str()).unwrap_or("");

        let br_tag = if self.html_type == "html" {
            "<br>"
        } else {
            "<br />"
        };
        let replacement = format!("$1{br_tag}");
        let new_content = RE_BR_HTML
            .replace_all(content, replacement.as_str())
            .into_owned();

        format!("<{tag}{attrs}>{new_content}{close}")
    }

    // -----------------------------------------------------------------
    // `block` / `fBlock`
    // -----------------------------------------------------------------

    /// Port of `Textile.block`.
    pub fn block(&mut self, text: &str, head_offset: i32) -> String {
        let btags: &[&str] = if !self.lite { BTAG } else { BTAG_LITE };
        let tre = btags.join("|");
        let block_pattern = FancyRegex::new(&format!(
            r"(?s)^({tre})({a}{c})\.(\.?)(?::(\S+))? (.*)$",
            a = *A,
            c = *C
        ))
        .expect("dynamic block regex");

        let lines: Vec<&str> = text.split("\n\n").collect();

        let mut tag = "p".to_string();
        let mut atts = String::new();
        let mut cite = String::new();
        let mut ext = String::new();
        // Port of the Python's `c1` loop-local: `fBlock`'s 5th return
        // value, carried across iterations so a later match (or the
        // end of the loop) can close a still-open extended block.
        let mut c1 = String::new();

        let mut out: Vec<String> = Vec::new();
        let mut anon = false;

        for line in lines {
            let mut line_out;
            if let Ok(Some(m)) = block_pattern.captures(line) {
                if !ext.is_empty() {
                    if let Some(last) = out.pop() {
                        out.push(last + &c1);
                    }
                }

                tag = m[1].to_string();
                atts = m[2].to_string();
                ext = m
                    .get(3)
                    .map(|mm| mm.as_str().to_string())
                    .unwrap_or_default();
                cite = m
                    .get(4)
                    .map(|mm| mm.as_str().to_string())
                    .unwrap_or_default();
                let graf_text = m[5].to_string();

                if let Some(hm) = RE_HEADER_TAG.captures(&tag) {
                    let level: i32 = hm[1].parse().unwrap_or(1);
                    let new_level = (level + head_offset).clamp(1, 6);
                    tag = format!("h{new_level}");
                }

                let (o1, o2, content, c2, new_c1) =
                    self.f_block(&tag, &atts, &ext, &cite, &graf_text);
                c1 = new_c1;

                line_out = if !ext.is_empty() {
                    format!("{o1}{o2}{content}{c2}")
                } else {
                    format!("{o1}{o2}{content}{c2}{c1}")
                };
            } else {
                anon = true;
                if !ext.is_empty() || !RE_LEADING_SPACE.is_match(line) {
                    let (_o1, o2, content, c2, new_c1) =
                        self.f_block(&tag, &atts, &ext, &cite, line);
                    c1 = new_c1;
                    line_out = if tag == "p" && !Textile::has_raw_text(&content) {
                        content
                    } else {
                        format!("{o2}{content}{c2}")
                    };
                } else {
                    line_out = self.graf(line);
                }
            }

            line_out = self.do_p_br(&line_out);
            if self.html_type == "xhtml" {
                line_out = line_out.replace("<br>", "<br />");
            }

            if !ext.is_empty() && anon {
                if let Some(last) = out.pop() {
                    out.push(format!("{last}\n{line_out}"));
                } else {
                    out.push(line_out);
                }
            } else {
                out.push(line_out);
            }

            if ext.is_empty() {
                tag = "p".to_string();
                atts = String::new();
                cite = String::new();
            }
        }

        if !ext.is_empty() {
            if let Some(last) = out.pop() {
                out.push(last + &c1);
            }
        }

        out.join("\n\n")
    }

    /// Port of `Textile.fBlock`.
    fn f_block(
        &mut self,
        tag: &str,
        atts: &str,
        ext: &str,
        cite: &str,
        content: &str,
    ) -> (String, String, String, String, String) {
        let mut atts = self.pba(atts, None);
        let mut tag = tag.to_string();
        let cite = cite.to_string();
        let mut content = content.to_string();
        // `o1`/`c1` genuinely do keep their initial empty value for the
        // catch-all (plain-tag) match arm below, matching Python's `o1
        // = o2 = c2 = c1 = ''` init -- but `o2`/`c2` are unconditionally
        // overwritten by every arm, so their initial value is dead;
        // `#[allow(unused_assignments)]` documents that as intentional
        // rather than a bug.
        let mut o1 = String::new();
        #[allow(unused_assignments)]
        let mut o2 = String::new();
        #[allow(unused_assignments)]
        let mut c2 = String::new();
        let mut c1 = String::new();
        let _ = ext;

        if let Some(m) = RE_FOOTNOTE_TAG.captures(&tag) {
            let num = m[1].to_string();
            tag = "p".to_string();
            let fnid = self
                .footnotes
                .get(&num)
                .cloned()
                .unwrap_or_else(|| num.clone());
            atts = format!("{atts} id=\"fn{fnid}\"");
            if !atts.contains("class=") {
                atts.push_str(" class=\"footnote\"");
            }
            content = format!("<sup>{num}</sup>{content}");
        }

        match tag.as_str() {
            "bq" => {
                let checked = self.check_refs(&cite);
                let cite_attr = if !checked.is_empty() {
                    format!(" cite=\"{checked}\"")
                } else {
                    String::new()
                };
                o1 = format!("\t<blockquote{cite_attr}{atts}>\n");
                o2 = format!("\t\t<p{atts}>");
                c2 = "</p>".to_string();
                c1 = "\n\t</blockquote>".to_string();
            }
            "bc" => {
                o1 = format!("<pre{atts}>");
                o2 = format!("<code{atts}>");
                c2 = "</code>".to_string();
                c1 = "</pre>".to_string();
                let encoded =
                    Textile::encode_html(&format!("{}\n", content.trim_end_matches('\n')), true);
                content = self.shelve(&encoded);
            }
            "notextile" => {
                content = self.shelve(&content);
                o1 = String::new();
                o2 = String::new();
                c1 = String::new();
                c2 = String::new();
            }
            "pre" => {
                let encoded =
                    Textile::encode_html(&format!("{}\n", content.trim_end_matches('\n')), true);
                content = self.shelve(&encoded);
                o1 = format!("<pre{atts}>");
                o2 = String::new();
                c2 = String::new();
                c1 = "</pre>".to_string();
            }
            _ => {
                o2 = format!("\t<{tag}{atts}>");
                c2 = format!("</{tag}>");
            }
        }

        content = self.graf(&content);
        (o1, o2, content, c2, c1)
    }

    // -----------------------------------------------------------------
    // `footnoteRef` / `footnoteID`
    // -----------------------------------------------------------------

    /// Port of `Textile.footnoteRef`.
    fn footnote_ref(&mut self, text: &str) -> String {
        sub_plain_mut(self, &RE_FOOTNOTE_REF, text, |t, caps| t.footnote_id(caps))
    }

    /// Port of `Textile.footnoteID`.
    fn footnote_id(&mut self, caps: &regex::Captures) -> String {
        let id = caps[1].to_string();
        let t = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        let fnid = self
            .footnotes
            .entry(id.clone())
            .or_insert_with(|| Uuid::new_v4().to_string())
            .clone();
        format!("<sup class=\"footnote\"><a href=\"#fn{fnid}\">{id}</a></sup>{t}")
    }

    // -----------------------------------------------------------------
    // `glyphs` / `macros_only`
    // -----------------------------------------------------------------

    /// Port of `Textile.glyphs`. Python's `re.split(r'(<.*?>)', text)`
    /// keeps the captured tag delimiters interleaved into the result
    /// (unlike `regex::Regex::split`, which always discards captures),
    /// so this uses [`split_keep_tags`] rather than `RE_TAG_SPLIT.split`
    /// directly.
    fn glyphs(&self, text: &str) -> String {
        let text = RE_TRAILING_QUOTE.replace(text, "\" ").into_owned();

        let mut result = String::new();
        for piece in split_keep_tags(&text) {
            match piece {
                TagPiece::Tag(t) => result.push_str(t),
                TagPiece::Text(t) => {
                    if RE_HAS_MACRO.is_match(t) {
                        result.push_str(&apply_rules_ref(&COMBINED_RULES, t));
                    } else {
                        result.push_str(&apply_rules(&GLYPH_DEFAULTS, t));
                    }
                }
            }
        }
        result
    }

    /// Port of `Textile.macros_only`.
    fn macros_only(&self, text: &str) -> String {
        let text = RE_TRAILING_QUOTE.replace(text, "\" ").into_owned();

        let mut result = String::new();
        for piece in split_keep_tags(&text) {
            match piece {
                TagPiece::Tag(t) => result.push_str(t),
                TagPiece::Text(t) => {
                    if RE_HAS_MACRO.is_match(t) {
                        result.push_str(&apply_rules(&MACRO_DEFAULTS, t));
                    } else {
                        result.push_str(t);
                    }
                }
            }
        }
        result
    }

    // -----------------------------------------------------------------
    // `vAlign` / `hAlign`
    // -----------------------------------------------------------------

    /// Port of `Textile.vAlign`.
    fn v_align(input: &str) -> &'static str {
        match input {
            "^" => "top",
            "-" => "middle",
            "~" => "bottom",
            _ => "",
        }
    }

    /// Port of `Textile.hAlign`.
    fn h_align(input: &str) -> &'static str {
        match input {
            "<" => "left",
            "=" => "center",
            ">" => "right",
            "<>" => "justify",
            _ => "",
        }
    }

    // -----------------------------------------------------------------
    // `getRefs` / `refs` / `checkRefs` / `isRelURL` / `relURL`
    // -----------------------------------------------------------------

    /// Port of `Textile.getRefs`.
    fn get_refs(&mut self, text: &str) -> String {
        sub_fancy_mut(self, &RE_GET_REFS, text, |t, caps| t.refs(caps))
    }

    /// Port of `Textile.refs`.
    fn refs(&mut self, caps: &fancy_regex::Captures<'_, str>) -> String {
        let flag = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let url = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        self.urlrefs.insert(flag.to_string(), url.to_string());
        String::new()
    }

    /// Port of `Textile.checkRefs`.
    fn check_refs(&self, url: &str) -> String {
        self.urlrefs
            .get(url)
            .cloned()
            .unwrap_or_else(|| url.to_string())
    }

    /// Port of `Textile.isRelURL`.
    fn is_rel_url(url: &str) -> bool {
        match url::Url::parse(url) {
            Ok(_) => false,
            Err(_) => {
                // A relative URL has no scheme/netloc. `url::Url::parse`
                // fails for such input (it isn't a full URL), matching
                // Python's `urlparse` returning empty scheme/netloc.
                true
            }
        }
    }

    /// Port of `Textile.relURL`.
    fn rel_url(&self, url: &str) -> String {
        let scheme = url::Url::parse(url).ok().map(|u| u.scheme().to_string());
        if self.restricted {
            if let Some(scheme) = &scheme {
                if !URL_SCHEMES.contains(&scheme.as_str()) {
                    return "#".to_string();
                }
            }
        }
        url.to_string()
    }

    // -----------------------------------------------------------------
    // `shelve` / `retrieve`
    // -----------------------------------------------------------------

    /// Port of `Textile.shelve`.
    fn shelve(&mut self, text: &str) -> String {
        let id = format!("{}c", Uuid::new_v4());
        self.shelf.insert(id.clone(), text.to_string());
        id
    }

    /// Port of `Textile.retrieve`.
    fn retrieve(&self, text: &str) -> String {
        let mut text = text.to_string();
        loop {
            let old = text.clone();
            for (k, v) in self.shelf.iter() {
                text = text.replace(k.as_str(), v.as_str());
            }
            if text == old {
                break;
            }
        }
        text
    }

    // -----------------------------------------------------------------
    // `encode_html`
    // -----------------------------------------------------------------

    /// Port of `Textile.encode_html`.
    fn encode_html(text: &str, quotes: bool) -> String {
        let mut text = text.replace('&', "&#38;");
        text = text.replace('<', "&#60;");
        text = text.replace('>', "&#62;");
        if quotes {
            text = text.replace('\'', "&#39;");
            text = text.replace('"', "&#34;");
        }
        text
    }

    // -----------------------------------------------------------------
    // `graf`
    // -----------------------------------------------------------------

    /// Port of `Textile.graf`.
    fn graf(&mut self, text: &str) -> String {
        let mut text = text.to_string();
        if !self.lite {
            text = self.no_textile(&text);
            text = self.code(&text);
        }

        text = self.links(&text);

        if !self.noimage {
            text = self.image(&text);
        }

        if !self.lite {
            text = self.lists(&text);
            text = self.table(&text);
        }

        text = self.span(&text);
        text = self.footnote_ref(&text);
        text = self.glyphs(&text);

        text.trim_end_matches('\n').to_string()
    }

    // -----------------------------------------------------------------
    // `links` / `fLink`
    // -----------------------------------------------------------------

    /// Port of `Textile.links`.
    fn links(&mut self, text: &str) -> String {
        let text = self.macros_only(text);
        sub_fancy_mut(self, &RE_LINKS, &text, |t, caps| t.f_link(caps))
    }

    /// Port of `Textile.fLink`.
    fn f_link(&mut self, caps: &fancy_regex::Captures<'_, str>) -> String {
        let pre = caps.name("pre").map(|m| m.as_str()).unwrap_or("");
        let atts = caps.name("atts").map(|m| m.as_str()).unwrap_or("");
        let text = caps.name("text").map(|m| m.as_str()).unwrap_or("");
        let title = caps.get(4).map(|m| m.as_str());
        let mut url = caps
            .name("url")
            .map(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        let mut post = caps
            .name("post")
            .map(|m| m.as_str())
            .unwrap_or("")
            .to_string();

        // assume ) at the end of the url is not actually part of the url
        // unless the url also contains a (
        if url.ends_with(')') && !url.contains('(') {
            let last = url.pop().unwrap();
            post = format!("{last}{post}");
        }

        url = self.check_refs(&url);

        let mut atts = self.pba(atts, None);
        if let Some(title) = title {
            atts.push_str(&format!(" title=\"{}\"", Textile::encode_html(title, true)));
        }

        let mut text = text.to_string();
        if !self.noimage {
            text = self.image(&text);
        }
        text = self.span(&text);
        text = self.glyphs(&text);

        url = self.rel_url(&url);
        let out = format!(
            "<a href=\"{}\"{atts}{}>{text}</a>",
            Textile::encode_html(&url, true),
            self.rel
        );
        let shelved = self.shelve(&out);
        format!("{pre}{shelved}{post}")
    }

    // -----------------------------------------------------------------
    // `span` / `fSpan`
    // -----------------------------------------------------------------

    /// Port of `Textile.span`.
    fn span(&mut self, text: &str) -> String {
        let mut text = text.to_string();
        for (_, tag, re) in SPAN_QTAGS.iter() {
            text = sub_fancy_mut(self, re, &text, |t, caps| t.f_span(tag, caps));
        }
        text
    }

    /// Port of `Textile.fSpan`.
    fn f_span(&mut self, tag: &str, caps: &fancy_regex::Captures<'_, str>) -> String {
        let atts = caps.get(3).map(|m| m.as_str()).unwrap_or("");
        let cite = caps.get(4).map(|m| m.as_str());
        let content = caps.get(5).map(|m| m.as_str()).unwrap_or("");
        let end_punct = caps.get(6).map(|m| m.as_str()).unwrap_or("");
        let close_bracket = caps.get(7).map(|m| m.as_str()).unwrap_or("");

        let mut atts = self.pba(atts, None);
        if let Some(cite) = cite {
            atts.push_str(&format!("cite=\"{cite}\""));
        }

        let content = self.span(content);

        format!("<{tag}{atts}>{content}{end_punct}</{tag}>{close_bracket}")
    }

    // -----------------------------------------------------------------
    // `image` / `fImage`
    // -----------------------------------------------------------------

    /// Port of `Textile.image`.
    fn image(&mut self, text: &str) -> String {
        sub_fancy_mut(self, &RE_IMAGE, text, |t, caps| t.f_image(caps))
    }

    /// Port of `Textile.fImage`.
    fn f_image(&mut self, caps: &fancy_regex::Captures<'_, str>) -> String {
        let atts = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let url_in = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        let title = caps.get(3).map(|m| m.as_str());
        let href = caps.get(4).map(|m| m.as_str());

        let mut atts = self.pba(atts, None);
        if let Some(title) = title {
            atts.push_str(&format!(" title=\"{title}\" alt=\"{title}\""));
        } else {
            atts.push_str(" alt=\"\"");
        }

        if !Textile::is_rel_url(url_in) && self.get_sizes {
            if let Some(size) = getimagesize(url_in) {
                atts.push_str(&format!(" {size}"));
            }
        }

        let href = href.map(|h| self.check_refs(h));

        let mut url = self.check_refs(url_in);
        url = self.rel_url(&url);

        let mut out = String::new();
        if let Some(href) = &href {
            out.push_str(&format!("<a href=\"{href}\" class=\"img\">"));
        }
        if self.html_type == "html" {
            out.push_str(&format!("<img src=\"{url}\"{atts}>"));
        } else {
            out.push_str(&format!("<img src=\"{url}\"{atts} />"));
        }
        if href.is_some() {
            out.push_str("</a>");
        }
        out
    }

    // -----------------------------------------------------------------
    // `code` / `fCode` / `fPre` / `doSpecial` / `fSpecial` / `noTextile`
    // / `fTextile`
    // -----------------------------------------------------------------

    /// Port of `Textile.code`.
    fn code(&mut self, text: &str) -> String {
        let text = self.do_special(text, "<code>", "</code>", Textile::f_code);
        let text = self.do_special(&text, "@", "@", Textile::f_code);
        self.do_special(&text, "<pre>", "</pre>", Textile::f_pre)
    }

    /// Port of `Textile.fCode`.
    fn f_code(t: &mut Textile, before: &str, text: &str, after: &str) -> String {
        let text = if !t.restricted {
            Textile::encode_html(text, true)
        } else {
            text.to_string()
        };
        format!(
            "{before}{}{after}",
            t.shelve(&format!("<code>{text}</code>"))
        )
    }

    /// Port of `Textile.fPre`.
    fn f_pre(t: &mut Textile, before: &str, text: &str, after: &str) -> String {
        let text = if !t.restricted {
            Textile::encode_html(text, true)
        } else {
            text.to_string()
        };
        format!("{before}<pre>{}</pre>{after}", t.shelve(&text))
    }

    /// Port of `Textile.doSpecial`.
    fn do_special(
        &mut self,
        text: &str,
        start: &str,
        end: &str,
        handler: fn(&mut Textile, &str, &str, &str) -> String,
    ) -> String {
        let pattern = format!(
            r"(?ms)(^|\s|[\[({{>]){}(.*?){}(\s|$|[\])}}])?",
            regex::escape(start),
            regex::escape(end)
        );
        let re = Regex::new(&pattern).expect("dynamic doSpecial regex");
        sub_plain_mut(self, &re, text, |t, caps| {
            let before = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let inner = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let after = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            handler(t, before, inner, after)
        })
    }

    /// Port of `Textile.fSpecial`: `doSpecial`'s default `method`
    /// (`method=None` -> `method = self.fSpecial`). Dead code upstream
    /// too -- every actual call site (`code()`, `noTextile()`) passes
    /// an explicit `method` (`fCode`/`fPre`/`fTextile`), so this
    /// default is never reached in the Python either. Ported anyway
    /// for completeness/fidelity with the class's public shape.
    #[allow(dead_code)]
    fn f_special(t: &mut Textile, before: &str, text: &str, after: &str) -> String {
        let encoded = Textile::encode_html(text, true);
        format!("{before}{}{after}", t.shelve(&encoded))
    }

    /// Port of `Textile.noTextile`.
    fn no_textile(&mut self, text: &str) -> String {
        let text = self.do_special(text, "<notextile>", "</notextile>", Textile::f_textile);
        self.do_special(&text, "==", "==", Textile::f_textile)
    }

    /// Port of `Textile.fTextile`.
    fn f_textile(t: &mut Textile, before: &str, text: &str, after: &str) -> String {
        format!("{before}{}{after}", t.shelve(text))
    }
}

// ===========================================================================
// Free helper functions
// ===========================================================================

/// Port of `_normalize_newlines`.
fn normalize_newlines(text: &str) -> String {
    lazy_static! {
        static ref RE_CRLF: Regex = Regex::new(r"\r\n").expect("static regex");
        static ref RE_MANY_NL: Regex = Regex::new(r"\n{3,}").expect("static regex");
        static ref RE_BLANK_LINE: Regex = Regex::new(r"\n\s*\n").expect("static regex");
        static ref RE_TRAILING_DQUOTE: Regex = Regex::new("\"\\z").expect("static regex");
    }
    let out = RE_CRLF.replace_all(text, "\n");
    let out = RE_MANY_NL.replace_all(&out, "\n\n");
    let out = RE_BLANK_LINE.replace_all(&out, "\n\n");
    let out = RE_TRAILING_DQUOTE.replace(&out, "\" ");
    out.into_owned()
}

/// Port of `hasRawText`'s free logic (`Textile.hasRawText` doesn't use
/// `self`, so this crate exposes it via `Textile::has_raw_text`, a
/// `Self`-less associated function, backed by this free function).
fn has_raw_text(text: &str) -> bool {
    let r = RE_RAW_TEXT_BLOCKS
        .replace_all(text.trim(), "")
        .trim()
        .to_string();
    let r = RE_RAW_TEXT_VOID.replace_all(&r, "").into_owned();
    !r.is_empty()
}

enum TagPiece<'a> {
    Tag(&'a str),
    Text(&'a str),
}

/// `glyphs`/`macros_only` need Python's `re.split(r'(<.*?>)', text)`,
/// which -- because the pattern has a capture group -- interleaves the
/// captured delimiters back into the result (unlike Rust's
/// `Regex::split`, which always discards them). This reimplements that
/// specific capturing-split behavior directly via `find_iter`.
fn split_keep_tags(text: &str) -> Vec<TagPiece<'_>> {
    let mut pieces = Vec::new();
    let mut last_end = 0;
    for m in RE_TAG_SPLIT.find_iter(text) {
        if m.start() > last_end {
            pieces.push(TagPiece::Text(&text[last_end..m.start()]));
        }
        pieces.push(TagPiece::Tag(m.as_str()));
        last_end = m.end();
    }
    if last_end < text.len() {
        pieces.push(TagPiece::Text(&text[last_end..]));
    }
    pieces
}

lazy_static! {
    /// Port of `rules = self.macro_defaults + self.glyph_defaults` in
    /// `glyphs()`, precombined once rather than reallocated per call.
    static ref COMBINED_RULES: Vec<&'static SubRule> = {
        let mut v: Vec<&'static SubRule> = Vec::new();
        for r in MACRO_DEFAULTS.iter() {
            v.push(r);
        }
        for r in GLYPH_DEFAULTS.iter() {
            v.push(r);
        }
        v
    };
}

/// `apply_rules` takes `&[SubRule]`; `COMBINED_RULES` is `Vec<&SubRule>`
/// for lifetime reasons (it borrows from two other `lazy_static`s), so
/// this small adapter bridges the two without cloning any `Regex`.
impl SubRule {
    // (kept adjacent to the `apply` impl above; see there)
}

/// Applies a `Vec<&SubRule>` (see [`COMBINED_RULES`]) in order.
fn apply_rules_ref(rules: &[&SubRule], text: &str) -> String {
    let mut text = text.to_string();
    for rule in rules {
        text = rule.apply(&text);
    }
    text
}

/// Replaces all non-overlapping matches of `re` in `text`, computing
/// each replacement via `handler(owner, captures)`. This is the
/// mutation-friendly equivalent of Python's `pattern.sub(self.fXxx,
/// text)`: `fancy_regex::Regex::replace_all`'s `Replacer` trait can't
/// cleanly express a closure that both captures `&mut self` *and* is
/// called by the regex engine's internals, so this iterates
/// `captures_iter` manually and rebuilds the string by hand instead.
/// `owner` is threaded through explicitly (rather than captured by the
/// closure) so the closure can reborrow it on every match without
/// fighting the borrow checker over repeated exclusive captures.
///
/// Errors from `fancy_regex` (only possible via its backtracking
/// step-count guard, `RuntimeError::BacktrackLimitExceeded`) are
/// treated as "no match at this position" -- Python's `re` has no
/// equivalent failure mode, so this is unreachable for any input this
/// port's tests exercise, but it avoids a panic on pathological input.
fn sub_fancy_mut(
    owner: &mut Textile,
    re: &FancyRegex,
    text: &str,
    mut handler: impl FnMut(&mut Textile, &fancy_regex::Captures<'_, str>) -> String,
) -> String {
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    for caps in re.captures_iter(text).flatten() {
        let m = match caps.get(0) {
            Some(m) => m,
            None => continue,
        };
        result.push_str(&text[last_end..m.start()]);
        result.push_str(&handler(owner, &caps));
        last_end = m.end();
    }
    result.push_str(&text[last_end..]);
    result
}

/// Non-mutating counterpart of [`sub_fancy_mut`], for the one case
/// ([`Textile::do_p_br`]) whose handler ([`Textile::do_br`]) only
/// reads `self` (a plain, freely-copyable shared reference the closure
/// can capture directly, so no `owner` threading is needed).
fn sub_fancy(
    re: &FancyRegex,
    text: &str,
    mut handler: impl FnMut(&fancy_regex::Captures<'_, str>) -> String,
) -> String {
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    for caps in re.captures_iter(text).flatten() {
        let m = match caps.get(0) {
            Some(m) => m,
            None => continue,
        };
        result.push_str(&text[last_end..m.start()]);
        result.push_str(&handler(&caps));
        last_end = m.end();
    }
    result.push_str(&text[last_end..]);
    result
}

/// Plain-`regex` counterpart of [`sub_fancy_mut`].
fn sub_plain_mut(
    owner: &mut Textile,
    re: &Regex,
    text: &str,
    mut handler: impl FnMut(&mut Textile, &regex::Captures) -> String,
) -> String {
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    for caps in re.captures_iter(text) {
        let m = caps.get(0).expect("group 0 always matches");
        result.push_str(&text[last_end..m.start()]);
        result.push_str(&handler(owner, &caps));
        last_end = m.end();
    }
    result.push_str(&text[last_end..]);
    result
}

/// Port of `getimagesize`.
///
/// Fetches `url` synchronously with `reqwest`'s blocking client
/// (matching this crate's established convention -- see
/// `oeb::polish::download`), buffering the response in the same 1024-
/// byte chunks Python's `ImageFile.Parser.feed()` loop used, and after
/// each chunk attempts to decode just the image *header* from
/// whatever's been buffered so far via `image::io::Reader`. This
/// reproduces Python's early-exit optimization (stop downloading once
/// the dimensions are known, rather than waiting for the whole file) --
/// most image headers fit in the first chunk or two, so for a typical
/// remote image this returns after only a few KB.
pub fn getimagesize(url: &str) -> Option<String> {
    use std::io::Read as _;

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .ok()?;
    let mut resp = client.get(url).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = resp.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);

        if let Ok(reader) = image::io::Reader::new(std::io::Cursor::new(&buf)).with_guessed_format()
        {
            if let Ok((w, h)) = reader.into_dimensions() {
                return Some(format!("width=\"{w}\" height=\"{h}\""));
            }
        }
    }
    None
}

/// Port of the free `textile()` function.
pub fn textile(text: &str, head_offset: i32, html_type: &str) -> String {
    Textile::default().textile(text, None, head_offset, html_type)
}

/// Port of `textile_restricted()`.
pub fn textile_restricted(text: &str, lite: bool, noimage: bool, html_type: &str) -> String {
    Textile::new(true, lite, noimage).textile(text, Some("nofollow"), 0, html_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_paragraph() {
        let out = textile("h1. Header\n\nSome *bold* and _italic_ text.", 0, "xhtml");
        assert_eq!(
            out,
            "\t<h1>Header</h1>\n\n\t<p>Some <strong>bold</strong> and <em>italic</em> text.</p>"
        );
    }

    // The following cases were all cross-checked against a live run of
    // the actual Python (`old_src/src/calibre/ebooks/textile/functions.py`,
    // loaded standalone -- see this port's scratch loader script) via:
    //
    //   textile(src)                     # unrestricted cases
    //   textile_restricted(src)          # restricted cases
    //
    // and the expected strings below are transcribed verbatim from that
    // output, not hand-guessed.

    #[test]
    fn multiple_headers() {
        assert_eq!(
            textile("h1. Top\n\nh2. Sub\n\nh3. Subsub", 0, "xhtml"),
            "\t<h1>Top</h1>\n\n\t<h2>Sub</h2>\n\n\t<h3>Subsub</h3>"
        );
    }

    #[test]
    fn bold_and_italic_variants() {
        assert_eq!(
            textile(
                "This is *bold*, this is _italic_, this is **b** and __i__.",
                0,
                "xhtml"
            ),
            "\t<p>This is <strong>bold</strong>, this is <em>italic</em>, this is <b>b</b> and <i>i</i>.</p>"
        );
    }

    #[test]
    fn link_basic() {
        assert_eq!(
            textile(
                "I searched \"Google\":http://google.com/ for it.",
                0,
                "xhtml"
            ),
            "\t<p>I searched <a href=\"http://google.com/\">Google</a> for it.</p>"
        );
    }

    #[test]
    fn link_with_trailing_quoted_text() {
        assert_eq!(
            textile(
                "Check \"Google\":http://google.com/ \"yes\" out.",
                0,
                "xhtml"
            ),
            "\t<p>Check <a href=\"http://google.com/\">Google</a> &#8220;yes&#8221; out.</p>"
        );
    }

    #[test]
    fn image_basic() {
        assert_eq!(
            textile("!/imgs/myphoto.jpg!", 0, "xhtml"),
            "\t<p><img src=\"/imgs/myphoto.jpg\" alt=\"\" /></p>"
        );
    }

    #[test]
    fn image_with_link() {
        assert_eq!(
            textile("!/imgs/myphoto.jpg!:http://jsamsa.com", 0, "xhtml"),
            "\t<p><a href=\"http://jsamsa.com\" class=\"img\"><img src=\"/imgs/myphoto.jpg\" alt=\"\" /></a></p>"
        );
    }

    #[test]
    fn unordered_list() {
        assert_eq!(
            textile("* one\n* two\n* three", 0, "xhtml"),
            "\t<ul>\n\t\t<li>one</li>\n\t\t<li>two</li>\n\t\t<li>three</li>\n\t</ul>"
        );
    }

    #[test]
    fn ordered_list() {
        assert_eq!(
            textile("# one\n# two\n# three", 0, "xhtml"),
            "\t<ol>\n\t\t<li>one</li>\n\t\t<li>two</li>\n\t\t<li>three</li>\n\t</ol>"
        );
    }

    #[test]
    fn nested_list() {
        assert_eq!(
            textile("* one\n** subone\n** subtwo\n* two", 0, "xhtml"),
            "\t<ul>\n\t\t<li>one\n\t<ul>\n\t\t<li>subone</li>\n\t\t<li>subtwo</li>\n\t</ul></li>\n\t\t<li>two</li>\n\t</ul>"
        );
    }

    #[test]
    fn simple_table() {
        assert_eq!(
            textile("|one|two|three|\n|a|b|c|", 0, "xhtml"),
            "\t<table>\n\t\t<tr>\n\t\t\t<td>one</td>\n\t\t\t<td>two</td>\n\t\t\t<td>three</td>\n\t\t</tr>\n\t\t<tr>\n\t\t\t<td>a</td>\n\t\t\t<td>b</td>\n\t\t\t<td>c</td>\n\t\t</tr>\n\t</table>"
        );
    }

    #[test]
    fn table_with_header_row() {
        assert_eq!(
            textile("|_.Name|_.Age|\n|Alice|30|\n|Bob|25|", 0, "xhtml"),
            "\t<table>\n\t\t<tr>\n\t\t\t<th>_.Name</th>\n\t\t\t<th>_.Age</th>\n\t\t</tr>\n\t\t<tr>\n\t\t\t<td>Alice</td>\n\t\t\t<td>30</td>\n\t\t</tr>\n\t\t<tr>\n\t\t\t<td>Bob</td>\n\t\t\t<td>25</td>\n\t\t</tr>\n\t</table>"
        );
    }

    #[test]
    fn blockquote_basic() {
        assert_eq!(
            textile("bq. This is a quoted block of text.", 0, "xhtml"),
            "\t<blockquote>\n\t\t<p>This is a quoted block of text.</p>\n\t</blockquote>"
        );
    }

    #[test]
    fn blockquote_with_cite() {
        assert_eq!(
            textile("bq.:http://example.com Cited quote here.", 0, "xhtml"),
            "\t<blockquote cite=\"http://example.com\">\n\t\t<p>Cited quote here.</p>\n\t</blockquote>"
        );
    }

    #[test]
    fn footnote_links_id_between_ref_and_definition() {
        // The generated footnote id is a random UUID (`Textile::shelve`
        // and `footnoteID` both use `Uuid::new_v4`), so this checks
        // structure/consistency rather than an exact string match --
        // matching the Python's own behavior, which also generates a
        // fresh random UUID every run (verified live: the id changes
        // between runs, but is always the same value in both the `<sup
        // class="footnote">` back-reference and the `<p id="fn...">`
        // definition it points at).
        let out = textile(
            "This has a footnote[1].\n\nfn1. This is the footnote text.",
            0,
            "xhtml",
        );
        assert!(out.contains("<sup class=\"footnote\"><a href=\"#fn"));
        assert!(out.contains("\">1</a></sup>"));
        assert!(out.contains("<p id=\"fn"));
        assert!(out.contains("\" class=\"footnote\"><sup>1</sup>This is the footnote text.</p>"));

        // Extract the href target and the id it should match.
        let href_start = out.find("href=\"#fn").unwrap() + "href=\"#fn".len();
        let href_end = out[href_start..].find('"').unwrap() + href_start;
        let href_id = &out[href_start..href_end];

        let id_start = out.find("<p id=\"fn").unwrap() + "<p id=\"fn".len();
        let id_end = out[id_start..].find('"').unwrap() + id_start;
        let def_id = &out[id_start..id_end];

        assert_eq!(href_id, def_id);
    }

    #[test]
    fn inline_code() {
        assert_eq!(
            textile("Here is @some code@ inline.", 0, "xhtml"),
            "\t<p>Here is <code>some code</code> inline.</p>"
        );
    }

    #[test]
    fn pre_block() {
        assert_eq!(
            textile("pre. this is\npreformatted text", 0, "xhtml"),
            "<pre>this is\npreformatted text\n</pre>"
        );
    }

    #[test]
    fn block_class_and_id_attrs() {
        assert_eq!(
            textile("p(myclass#myid). Hello world", 0, "xhtml"),
            "\t<p class=\"myclass\" id=\"myid\">Hello world</p>"
        );
    }

    #[test]
    fn span_with_class_and_nested_formatting() {
        assert_eq!(
            textile(
                "Hello %(bob)span *strong* and **bold**% goodbye",
                0,
                "xhtml"
            ),
            "\t<p>Hello <span class=\"bob\">span <strong>strong</strong> and <b>bold</b></span> goodbye</p>"
        );
    }

    #[test]
    fn acronym_glyph() {
        assert_eq!(
            textile(
                "The W3C(World Wide Web Consortium) validated it.",
                0,
                "xhtml"
            ),
            "\t<p>The <acronym title=\"World Wide Web Consortium\">W3C</acronym> validated it.</p>"
        );
    }

    #[test]
    fn caps_glyph() {
        assert_eq!(
            textile("The NASA launched a rocket.", 0, "xhtml"),
            "\t<p>The <span class=\"caps\">NASA</span> launched a rocket.</p>"
        );
    }

    #[test]
    fn em_dash_glyph() {
        assert_eq!(
            textile("Something -- else entirely.", 0, "xhtml"),
            "\t<p>Something &#8212; else entirely.</p>"
        );
    }

    #[test]
    fn ellipsis_glyph() {
        assert_eq!(
            textile("Wait for it ...", 0, "xhtml"),
            "\t<p>Wait for it &#8230;</p>"
        );
    }

    #[test]
    fn apostrophe_via_smartypants() {
        assert_eq!(
            textile("It's a beautiful day, isn't it?", 0, "xhtml"),
            "\t<p>It&#8217;s a beautiful day, isn&#8217;t it?</p>"
        );
    }

    #[test]
    fn curly_quotes_via_smartypants() {
        assert_eq!(
            textile("\"Hello,\" she said.", 0, "xhtml"),
            "\t<p>&#8220;Hello,&#8221; she said.</p>"
        );
    }

    #[test]
    fn horizontal_rule() {
        assert_eq!(textile("----", 0, "xhtml"), "<hr />");
    }

    #[test]
    fn notextile_block_is_untouched_but_still_smartypants_escaped() {
        // Content inside <notextile> skips Textile markup processing
        // (the `*won't*` isn't turned into `<strong>`) but still goes
        // through `smartyPants` at the very end of `textile()`, since
        // that runs on the whole document after `retrieve()` -- so the
        // apostrophe is still curled. Verified against live Python.
        assert_eq!(
            textile(
                "<notextile>This *won't* be processed</notextile>",
                0,
                "xhtml"
            ),
            "\t<p>This *won&#8217;t* be processed</p>"
        );
    }

    #[test]
    fn restricted_basic_formatting() {
        assert_eq!(
            textile_restricted("Hello *world*", true, true, "xhtml"),
            "\t<p>Hello <strong>world</strong></p>"
        );
    }

    #[test]
    fn restricted_escapes_raw_html() {
        assert_eq!(
            textile_restricted("Hello <script>alert(1)</script>", true, true, "xhtml"),
            "\t<p>Hello &#60;script&#62;alert(1)&#60;/script&#62;</p>"
        );
    }

    #[test]
    fn restricted_links_get_nofollow() {
        assert_eq!(
            textile_restricted(
                "See \"Google\":http://google.com for more",
                true,
                true,
                "xhtml"
            ),
            "\t<p>See <a href=\"http://google.com\" rel=\"nofollow\">Google</a> for more</p>"
        );
    }

    #[test]
    fn multiline_paragraph_gets_br_tags() {
        assert_eq!(
            textile("Line one\nLine two\nLine three", 0, "xhtml"),
            "\t<p>Line one<br />Line two<br />Line three</p>"
        );
    }

    #[test]
    fn html_type_html_uses_bare_br() {
        let out = Textile::default().textile("Line one\nLine two", None, 0, "html");
        assert_eq!(out, "\t<p>Line one<br>Line two</p>");
    }

    #[test]
    fn extended_blockquote_spans_multiple_paragraphs() {
        assert_eq!(
            textile(
                "bq.. This is paragraph one of an extended quote.\n\nAnd this is paragraph two.\n\np. Back to normal.",
                0,
                "xhtml"
            ),
            "\t<blockquote>\n\t\t<p>This is paragraph one of an extended quote.</p>\n\t\t<p>And this is paragraph two.</p>\n\t</blockquote>\n\n\t<p>Back to normal.</p>"
        );
    }

    #[test]
    fn macro_default_copyright_glyph() {
        assert_eq!(
            textile("Copyright {(c)} 2024", 0, "xhtml"),
            "\t<p>Copyright &#169; 2024</p>"
        );
    }

    #[test]
    fn dimension_sign_glyph() {
        assert_eq!(
            textile("The room is 3 x 4 meters.", 0, "xhtml"),
            "\t<p>The room is 3 &#215; 4 meters.</p>"
        );
    }

    #[test]
    fn deleted_span() {
        assert_eq!(
            textile("This is -deleted- text.", 0, "xhtml"),
            "\t<p>This is <del>deleted</del> text.</p>"
        );
    }

    #[test]
    fn getimagesize_returns_none_rather_than_panicking_on_bad_url() {
        // No live-network test here, matching this crate's existing
        // precedent for network-touching code (see
        // `oeb::polish::download`'s test module, which likewise only
        // tests the pure/local helper functions and never performs a
        // real fetch). This exercises `getimagesize`'s error paths
        // (malformed URL, unreachable host) without needing network
        // access in CI/sandboxed environments.
        assert_eq!(getimagesize("not a url"), None);
        assert_eq!(
            getimagesize("http://localhost.invalid.example/nope.png"),
            None
        );
    }

    #[test]
    fn f_image_skips_getimagesize_when_get_sizes_is_false() {
        // `get_sizes` defaults to `false` (matching Python: nothing in
        // `__init__`/the free functions ever sets it), so `fImage`
        // never calls `getimagesize` in the default configuration --
        // this is really a test that the image path doesn't attempt
        // network I/O at all unless explicitly opted into, without
        // needing to mock a network call.
        let mut t = Textile::default();
        assert!(!t.get_sizes);
        let out = t.image("!http://example.com/a.jpg!");
        assert_eq!(out, "<img src=\"http://example.com/a.jpg\" alt=\"\" />");
    }

    #[test]
    fn pba_restricted_lang_bug_is_preserved() {
        let t = Textile::new(true, false, false);
        assert_eq!(t.pba("[fr]", None), " lang=\"%s\"");
    }

    #[test]
    fn pba_basic_attrs() {
        let t = Textile::default();
        assert_eq!(t.pba("(foo-bar)", None), " class=\"foo-bar\"");
        assert_eq!(t.pba("(#myid)", None), " id=\"myid\"");
        assert_eq!(
            t.pba("(foo-bar#myid)", None),
            " class=\"foo-bar\" id=\"myid\""
        );
        assert_eq!(t.pba("[fr]", None), " lang=\"fr\"");
    }
}
