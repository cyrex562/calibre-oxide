//! Port of `old_src/src/calibre/ebooks/covers.py` (issue #116):
//! calibre's programmatic/generative cover-art renderer.
//!
//! # Scope of this file
//!
//! `covers.py` is 778 lines drawing full cover images with Qt's
//! `QPainter` (5 "Style" classes -- `Cross`/`Half`/`Banner`/
//! `Ornamental`/`Blocks` -- plus shared text-layout/color-theme/
//! template-formatting infrastructure). Research (an Explore-agent
//! pass reading the whole file plus this crate's existing
//! `tiny-skia`/`resvg`/`usvg` dependencies) found the "needs Tauri
//! image-generation wiring" framing this issue was filed under is
//! stale: cover generation is a pure `Metadata + theme + style -> PNG
//! bytes` function needing zero GUI/webview plumbing, and this crate
//! already has a real, working headless 2D-rendering pipeline
//! (`oeb::transforms::rasterize`, built on the same `tiny-skia`/
//! `resvg`/`usvg` stack) that's a genuine `QPainter` substitute for
//! everything except text shaping -- and even that has a viable path
//! via `usvg`'s own internal `harfrust`/`skrifa`-based text engine
//! (already a transitive dependency), not a new crate.
//!
//! Split into real sub-issues by tractability (least to most graphics
//! dependency): #595 (color themes + text-formatting tokenizer,
//! done), #596 (this file's other half -- field-template formatting,
//! done), #597 (`tiny-skia` canvas + `Half`/`Blocks`/`Cross`, done),
//! #598 (text layout + rendering bridge), #599 (`Banner`, real curve
//! math), #600 (`Ornamental`, transform stamping), #601 (entry
//! points/wiring).
//!
//! # This file's own scope
//!
//! [`ColorTheme`]/[`load_color_themes`]/[`theme_to_colors`]/[`color`]
//! (port of the `Colors {{{` section), [`sanitize`]/
//! [`escape_formatting`]/[`unescape_formatting`]/
//! [`parse_text_formatting`] (port of the non-Qt-dependent half of the
//! `Draw text {{{` section), the `program:`/default-dialect field-
//! template formatting (`vformat`/[`format_text`]), and (#597) the
//! `tiny-skia`-backed [`render_cross`]/[`render_half`]/[`render_blocks`]
//! `Style` implementations. `parse_text_formatting` deliberately
//! stops at producing [`FormatRange`] data (tag/start/length) rather
//! than wrapping it in a `QTextCharFormat`/`QTextLayout::FormatRange`
//! equivalent -- that belongs to #598's real text-rendering bridge,
//! which needs to decide what text-shaping API actually consumes this
//! data.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use calibre_utils::cleantext::{clean_ascii_chars, clean_xml_chars};
use calibre_utils::formatter::interp::{evaluate as gpm_evaluate, RawValue, ValueSource};
use calibre_utils::formatter::parser::parse as gpm_parse;
use calibre_utils::formatter::{lexer, PureCatalog, PureFunctions};
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Mask, Paint, PathBuilder, Pixmap, Point, Rect,
    SpreadMode, Transform,
};

use crate::metadata::authors::authors_to_string;
use crate::metadata::meta::MetaInformation;
use crate::oeb::transforms::jacket::fmt_sidx;

// ===================================================================
// Colors
// ===================================================================

/// Port of `ColorTheme`. Each field is `#rrggbb` (no leading `#`
/// stored, matching how `default_color_themes`'s literal strings are
/// written; [`color`] adds it back only when needed -- nothing in
/// this file's own scope needs a parsed RGB struct yet, that's for
/// whichever `Style` port actually paints with it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorTheme {
    pub color1: String,
    pub color2: String,
    pub contrast_color1: String,
    pub contrast_color2: String,
}

/// Port of `to_theme`: splits a `"c1 c2 cc1 cc2"` hex-quad string into
/// a [`ColorTheme`].
pub fn to_theme(x: &str) -> ColorTheme {
    let mut parts = x.split_whitespace();
    ColorTheme {
        color1: parts.next().unwrap_or_default().to_string(),
        color2: parts.next().unwrap_or_default().to_string(),
        contrast_color1: parts.next().unwrap_or_default().to_string(),
        contrast_color2: parts.next().unwrap_or_default().to_string(),
    }
}

/// Port of `fallback_colors`.
pub fn fallback_colors() -> ColorTheme {
    to_theme("ffffff 000000 000000 ffffff")
}

/// Port of `default_color_themes`. Returns `(name, theme)` pairs in
/// upstream's own literal order (Python's dict preserves insertion
/// order; this crate has no need for name-keyed lookup elsewhere, so
/// a `Vec` avoids an unnecessary `HashMap` for 4 fixed entries).
pub fn default_color_themes() -> Vec<(&'static str, ColorTheme)> {
    vec![
        ("Earth", to_theme("e8d9ac c7b07b 564628 382d1a")),
        ("Grass", to_theme("d8edb5 abc8a4 375d3b 183128")),
        ("Water", to_theme("d3dcf2 829fe4 00448d 00305a")),
        ("Silver", to_theme("e6f1f5 aab3b6 6e7476 3b3e40")),
    ]
}

/// Port of `theme_to_colors` -- a no-op passthrough in this port (see
/// [`ColorTheme`]'s own doc: values are already the parsed-shape hex
/// strings `to_theme` produces, there's no separate raw-dict
/// intermediate form the way Python's `QColor`-keyed dict is).
pub fn theme_to_colors(theme: ColorTheme) -> ColorTheme {
    theme
}

/// Port of `load_color_themes`: every enabled color theme (user
/// overrides in `prefs_color_themes` merged over the 4 built-ins,
/// `prefs_disabled_color_themes` names removed) -- falling back to
/// all 4 built-ins if every one of them got disabled (matching
/// upstream's own "ignore disabled" fallback rather than returning an
/// empty list a caller would have nothing to pick a random theme
/// from). `prefs_color_themes`' values are already-parsed
/// [`ColorTheme`]s, matching upstream's own `prefs.color_themes`
/// shape (a dict-of-dicts, the same shape `default_color_themes`'
/// own values already have -- not a raw "c1 c2 cc1 cc2" string, which
/// only `to_theme` itself consumes).
pub fn load_color_themes(
    prefs_color_themes: &HashMap<String, ColorTheme>,
    prefs_disabled_color_themes: &[String],
) -> Vec<ColorTheme> {
    let mut themes: Vec<(String, ColorTheme)> = default_color_themes().into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    for (k, v) in prefs_color_themes {
        if let Some(existing) = themes.iter_mut().find(|(name, _)| name == k) {
            existing.1 = v.clone();
        } else {
            themes.push((k.clone(), v.clone()));
        }
    }
    let disabled: std::collections::HashSet<&str> = prefs_disabled_color_themes.iter().map(String::as_str).collect();
    let ans: Vec<ColorTheme> = themes
        .into_iter()
        .filter(|(k, _)| !disabled.contains(k.as_str()))
        .map(|(_, v)| theme_to_colors(v))
        .collect();
    if ans.is_empty() {
        default_color_themes().into_iter().map(|(_, v)| v).collect()
    } else {
        ans
    }
}

/// Port of `color(color_theme, name)`: the named color, or the
/// fallback if it's missing/empty (upstream's `QColor.isValid()`
/// check -- this port's `ColorTheme` fields are plain possibly-empty
/// strings rather than a parsed color type that can itself be
/// invalid, so "empty" is the equivalent signal).
pub fn color(theme: &ColorTheme, name: &str) -> String {
    let val = match name {
        "color1" => &theme.color1,
        "color2" => &theme.color2,
        "contrast_color1" => &theme.contrast_color1,
        "contrast_color2" => &theme.contrast_color2,
        _ => "",
    };
    if val.is_empty() {
        let fb = fallback_colors();
        match name {
            "color1" => fb.color1,
            "color2" => fb.color2,
            "contrast_color1" => fb.contrast_color1,
            "contrast_color2" => fb.contrast_color2,
            _ => String::new(),
        }
    } else {
        val.to_string()
    }
}

// ===================================================================
// Text sanitization/escaping
// ===================================================================

/// Port of `sanitize`. `force_unicode` is a no-op here -- a Rust
/// `&str` is already guaranteed valid Unicode, unlike Python 2-era
/// `bytes`-or-`str` ambiguity that function existed to resolve.
pub fn sanitize(s: &str) -> String {
    clean_xml_chars(&clean_ascii_chars(s)).nfc().collect()
}

/// Port of `escape_formatting`.
pub fn escape_formatting(val: &str) -> String {
    val.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Port of `unescape_formatting`.
pub fn unescape_formatting(val: &str) -> String {
    val.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

// ===================================================================
// parse_text_formatting
// ===================================================================

/// One resolved `<b>`/`<strong>`/`<i>`/`<em>` span, in character (not
/// byte) offsets into the text [`parse_text_formatting`] returns
/// alongside it. Deliberately not a `QTextCharFormat` equivalent --
/// see this module's own doc for why that's out of scope here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatRange {
    pub bold: bool,
    pub italic: bool,
    pub start: usize,
    pub length: usize,
}

enum PendingRange {
    Closed { tag: String, start: usize, length: usize },
    Open { tag: String, start: usize },
}

enum Tok {
    Text(String),
    Tag(String, bool),
}

/// Port of `parse_text_formatting`: strips `<b>`/`<strong>`/`<i>`/
/// `<em>` (and any other `<tag>`/`</tag>` markup, which is dropped
/// without producing a format range) out of `text`, returning the
/// markup-free text plus the resolved bold/italic spans. Faithfully
/// replicates a real upstream quirk: offsets are computed as if
/// `&amp;` were already collapsed to `&` (matching what the caller's
/// later `unescape_formatting` call will do), but the *returned* text
/// itself is NOT unescaped here -- `&amp;`/`&lt;`/`&gt;` stay literal
/// in the output string, only the range math anticipates the later
/// unescape.
pub fn parse_text_formatting(text: &str) -> (String, Vec<FormatRange>) {
    static TAG_RE: OnceLock<Regex> = OnceLock::new();
    let tag_re = TAG_RE.get_or_init(|| Regex::new(r"</?([a-zA-Z1-6]+)/?>").unwrap());

    let mut tokens: Vec<Tok> = Vec::new();
    let mut pos = 0usize;
    for caps in tag_re.captures_iter(text) {
        let whole = caps.get(0).unwrap();
        let q = &text[pos..whole.start()];
        if !q.is_empty() {
            tokens.push(Tok::Text(q.to_string()));
        }
        let tag_name = caps.get(1).unwrap().as_str().to_lowercase();
        let closing = whole.as_str().get(0..2).map(|s| s.contains('/')).unwrap_or(false);
        tokens.push(Tok::Tag(tag_name, closing));
        pos = whole.end();
    }
    if !tokens.is_empty() {
        if pos < text.len() {
            tokens.push(Tok::Text(text[pos..].to_string()));
        }
    } else {
        tokens.push(Tok::Text(text.to_string()));
    }

    let mut ranges: Vec<PendingRange> = Vec::new();
    let mut open_stack: Vec<(String, usize)> = Vec::new();
    let mut offset = 0usize;
    let mut out_text = String::new();
    for tok in tokens {
        match tok {
            Tok::Tag(tag, closing) => {
                if closing {
                    if let Some((open_tag, start)) = open_stack.pop() {
                        let length = offset.saturating_sub(start);
                        if length > 0 {
                            ranges.push(PendingRange::Closed { tag: open_tag, start, length });
                        }
                    }
                } else if matches!(tag.as_str(), "b" | "strong" | "i" | "em") {
                    open_stack.push((tag, offset));
                }
            }
            Tok::Text(t) => {
                offset += t.replace("&amp;", "&").chars().count();
                out_text.push_str(&t);
            }
        }
    }
    for (tag, start) in open_stack {
        ranges.push(PendingRange::Open { tag, start });
    }

    let text_char_len = out_text.chars().count();
    let mut formats = Vec::new();
    for pr in ranges {
        let (tag, start, length) = match pr {
            PendingRange::Closed { tag, start, length } => (tag, start, length),
            PendingRange::Open { tag, start } => (tag, start, text_char_len.saturating_sub(start)),
        };
        let bold = matches!(tag.as_str(), "b" | "strong");
        let italic = matches!(tag.as_str(), "i" | "em");
        if !bold && !italic {
            continue;
        }
        if length > 0 {
            formats.push(FormatRange { bold, italic, start, length });
        }
    }
    (out_text, formats)
}

// ===================================================================
// Field-template formatting (issue #596)
// ===================================================================
//
// Port of the `Format text using templates {{{` section's
// `Formatter`/`formatter`/`format_fields`/`format_text` plus the real
// `TemplateFormatter.evaluate`/`format_field` dispatch they call into
// (`old_src/src/calibre/utils/formatter.py`).
//
// Real upstream `TemplateFormatter.evaluate` dispatches a template on
// its own prefix: `program:` (full GPM, `calibre_utils::formatter`'s
// existing real parser/interpreter), `python:` (a full Python `exec`
// sandbox -- N/A, no Python interpreter in this port, matching this
// whole project's established GUI/Python-dependent narrowing
// pattern), or -- the default, and what `covers.py`'s own
// `title_template`/`subtitle_template` actually use -- a
// `string.Formatter`-style `{field}`/`{field:format_spec}`
// substitution (`vformat`), where the format-spec itself can hold a
// single bare-quoted GPM expression (`{field:'expr'}` or a bare
// `{field:'...'}`), evaluated via the exact same GPM machinery with
// the field's own value bound to `$`.
//
// **Real, disclosed narrowing**: real `format_field` also supports an
// "old-style" `{field:funcname(args)}` call syntax (a legacy
// alternate spelling of the same thing the bare-quoted-expression
// style already covers, needing its own comma-aware argument-scanner
// with backslash-escaping rules). Not ported -- a template using it
// degrades to having that text treated as a literal `_do_format` type
// spec (harmless, not a crash) rather than dispatching a function
// call. `covers.py`'s own real default templates never use this
// style. `_do_format` (a Python `str.format()` numeric/width/
// precision mini-language applied to the final value) is implemented
// only for the empty-format-spec case real templates actually
// exercise -- see [`apply_display_format`]'s own doc.

/// Field access for [`evaluate_template`]/[`format_field`], backed by
/// a [`MetaInformation`] plus the two derived values real
/// `format_text` computes before formatting (`Unknown`-filtered
/// authors, `formatted_series_index`) -- port of `format_text`'s own
/// `preserve_fields`-scoped mutation of `mi.authors`/
/// `mi.formatted_series_index`, done here as local derived values
/// instead of temporarily mutating a caller's struct (Rust has no
/// need for Python's save/restore-on-exit dance to get the same net
/// effect).
///
/// Deliberately exposes only the fields `covers.py`'s own real
/// templates reference (`title`/`authors`/`series`/
/// `formatted_series_index`) -- a real, disclosed narrower field set
/// than upstream's full `Metadata` object, which can answer `field()`
/// for any of dozens of book attributes. A future caller with a
/// custom template needing more fields would extend this.
#[derive(Clone)]
struct CoverValueSource {
    title: String,
    authors: String,
    series: String,
    formatted_series_index: String,
}

impl CoverValueSource {
    fn raw(&self, name: &str) -> Option<&str> {
        match name {
            "title" => Some(&self.title),
            "authors" => Some(&self.authors),
            "series" => Some(&self.series),
            "formatted_series_index" => Some(&self.formatted_series_index),
            _ => None,
        }
    }
}

impl ValueSource for CoverValueSource {
    /// Port of `field(name)`'s real `formatter.get_value(name, [],
    /// kwargs)` delegation to `Formatter.get_value` -- i.e. `field()`
    /// itself is escaped ([`escape_formatting`]), confirmed by reading
    /// `formatter_functions.py`'s real `BuiltinField.evaluate` (not
    /// assumed): `covers.py`'s own default `footer_template` splits/
    /// joins on the literal `&amp;` precisely because `field('authors')`
    /// returns the escaped join, not a plain `&`.
    fn get_value(&self, name: &str) -> Option<String> {
        self.raw(name).map(escape_formatting)
    }
    /// Port of `raw_field`'s real `getattr(self.parent_book, name,
    /// default)` path -- genuinely unescaped, a different real code
    /// path from `get_value`/`field()` above, not merely this port's
    /// own convention.
    fn get_raw_value(&self, name: &str) -> Option<RawValue> {
        self.raw(name).map(|s| RawValue::Scalar(s.to_string()))
    }
}

/// Port of `_eval_program`/`Formatter.evaluate`'s `program:` branch:
/// runs `program_text` through the real GPM lexer/parser/interpreter,
/// with `dollar_val` bound to the special `$` local (matching
/// `format_field`'s own `_eval_program(val, expr, ...)` call, where
/// `val` is the field's current value).
fn run_gpm(program_text: &str, dollar_val: &str, values: &CoverValueSource) -> Result<String, String> {
    let tokens = lexer::scan(program_text).map_err(|p| format!("lex error at byte {p}"))?;
    let expr = gpm_parse(&tokens, &PureCatalog, Default::default()).map_err(|e| e.to_string())?;
    let mut globals = HashMap::new();
    gpm_evaluate(&expr, dollar_val, Box::new(values.clone()), &PureFunctions, &mut globals).map_err(|e| e.to_string())
}

/// Port of `_explode_format_string`: unwraps a `prefix|fmt|suffix`
/// format spec into its 3 parts (matching upstream's own regex
/// `^(.*)\|([^\|]*)\|(.*)$`); a spec with no `|`-delimited middle
/// section returns unchanged with empty prefix/suffix.
fn explode_format_string(fmt: &str) -> (&str, &str, &str) {
    if let Some(first) = fmt.find('|') {
        if let Some(rel_last) = fmt[first + 1..].rfind('|') {
            let last = first + 1 + rel_last;
            if last > first {
                return (&fmt[first + 1..last], &fmt[..first], &fmt[last + 1..]);
            }
        }
    }
    (fmt, "", "")
}

/// Port of `_do_format`: applies a Python `str.format()`-style
/// single-char type spec to `val`. Real upstream supports the full
/// numeric/width/precision/alignment grammar via `('{0:'+fmt+'}').format(val)`;
/// this only implements the empty-spec passthrough (`if not fmt or
/// not val: return val`) that `covers.py`'s own real templates
/// exercise -- their only non-empty format specs are the bare-quoted
/// GPM-expression kind [`format_field`] intercepts before this is
/// ever reached, so `dispfmt` is always empty in practice for this
/// consumer. A non-empty spec is returned unchanged rather than
/// attempting a real Python format-mini-language reimplementation,
/// disclosed here rather than silently pretending full fidelity.
fn apply_display_format(val: &str, fmt: &str) -> String {
    if fmt.is_empty() || val.is_empty() {
        return val.to_string();
    }
    val.to_string()
}

/// Port of `TemplateFormatter.format_field`.
fn format_field(val: &str, fmt: &str, values: &CoverValueSource) -> Result<String, String> {
    let (fmt, prefix, suffix) = explode_format_string(fmt);

    let mut val = val.to_string();
    let mut dispfmt = fmt.to_string();

    let p = if fmt.starts_with('\'') {
        Some(0usize)
    } else {
        fmt.find(":'").map(|i| i + 1)
    };
    if let Some(p) = p {
        if fmt.ends_with('\'') && fmt.len() > p + 1 {
            let inner = &fmt[p + 1..fmt.len() - 1];
            val = run_gpm(inner, &val, values)?;
            dispfmt = match fmt[..p].find(':') {
                None => String::new(),
                Some(colon) => fmt[..colon].to_string(),
            };
        }
        // else: malformed (starts with a quote-triggering pattern but
        // doesn't end in a quote) -- falls through with dispfmt
        // unchanged, matching upstream's own fallthrough to the
        // old-style-call check (which this port doesn't implement
        // either, see this section's own doc).
    }
    if !val.is_empty() {
        val = apply_display_format(&val, &dispfmt);
    }
    if val.is_empty() {
        return Ok(String::new());
    }
    Ok(format!("{prefix}{val}{suffix}"))
}

/// Port of `TemplateFormatter.evaluate`'s default (`vformat`) branch:
/// `string.Formatter`-style `{field}`/`{field:format_spec}`
/// substitution. `{{`/`}}` escape to literal braces. Every substituted
/// field value comes back already escaped from [`CoverValueSource::get_value`]
/// (matching `Formatter(SafeFormat).get_value`'s real override, and
/// GPM's own `field()` builtin, which delegates to the identical
/// method -- confirmed by reading `formatter_functions.py`'s real
/// `BuiltinField.evaluate`, not assumed).
///
/// **Real, disclosed narrowing**: finds the format spec via the first
/// top-level `}` after the field name rather than reproducing Python's
/// full brace-nesting-aware `string.Formatter.parse` grammar (which
/// also allows a nested replacement field *inside* a format spec, e.g.
/// `{val:{width}}`). `covers.py`'s own real templates never nest
/// braces this way.
fn vformat(fmt: &str, values: &CoverValueSource) -> Result<String, String> {
    let mut out = String::new();
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        match chars[i] {
            '{' if chars.get(i + 1) == Some(&'{') => {
                out.push('{');
                i += 2;
            }
            '}' if chars.get(i + 1) == Some(&'}') => {
                out.push('}');
                i += 2;
            }
            '{' => {
                let start = i + 1;
                let end = chars[start..].iter().position(|&c| c == '}').map(|p| start + p).unwrap_or(chars.len());
                let field_spec: String = chars[start..end].iter().collect();
                let (field_name, format_spec) = match field_spec.find(':') {
                    Some(p) => (&field_spec[..p], &field_spec[p + 1..]),
                    None => (field_spec.as_str(), ""),
                };
                // `get_value` already escapes (matching real
                // `Formatter(SafeFormat).get_value`'s override, which
                // both `vformat`'s own field substitution AND GPM's
                // `field()` builtin delegate to identically) -- no
                // separate escape step here.
                let val = values.get_value(field_name).unwrap_or_default();
                out.push_str(&format_field(&val, format_spec, values)?);
                i = end + 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(out)
}

/// Port of `TemplateFormatter.evaluate`'s real dispatch (`program:` /
/// default). `python:` templates are N/A -- no Python interpreter in
/// this port.
fn evaluate_template(fmt: &str, values: &CoverValueSource) -> Result<String, String> {
    match fmt.strip_prefix("program:") {
        Some(rest) => run_gpm(rest, "", values),
        None => vformat(fmt, values),
    }
}

/// Port of `Formatter.safe_format`: never fails, substituting
/// `"Template error <message>"` on any real evaluation error (matching
/// upstream's `error_value + ' ' + error_message(e)`, with
/// `error_value` being the untranslated literal `"Template error"`).
fn safe_format(fmt: &str, values: &CoverValueSource) -> String {
    match evaluate_template(fmt, values) {
        Ok(s) => s,
        Err(e) => format!("Template error {e}"),
    }
}

/// The 3 templates [`format_text`] evaluates, port of the relevant
/// slice of `cprefs`' defaults (`title_template`/`subtitle_template`/
/// `footer_template`) -- the rest of `cprefs` (font sizes/families,
/// cover dimensions, theme/style enable-lists) belongs to whichever
/// later sub-issue actually consumes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverTemplates {
    pub title_template: String,
    pub subtitle_template: String,
    pub footer_template: String,
}

impl Default for CoverTemplates {
    fn default() -> Self {
        CoverTemplates {
            title_template: "<b>{title}".to_string(),
            subtitle_template: "{series:'test($, strcat(\"<i>\", $, \"</i> - \", raw_field(\"formatted_series_index\")), \"\")'}".to_string(),
            // Matches the real Python literal exactly, including its
            // `&amp;` separator (not a plain `&`): `field('authors')`
            // returns an escaped value (see `CoverValueSource::get_value`'s
            // own doc), so the real author-list join separator this
            // template splits/joins on really is `' &amp; '`, not
            // `' & '`.
            footer_template: "program:\n\
                # Show at most two authors, on separate lines.\n\
                authors = field('authors');\n\
                num = count(authors, ' &amp; ');\n\
                authors = sublist(authors, 0, 2, ' &amp; ');\n\
                authors = list_re(authors, ' &amp; ', '(.+)', '<b>\\1');\n\
                authors = re(authors, ' &amp; ', '<br>');\n\
                re(authors, '&amp;&amp;', '&amp;')"
                .to_string(),
        }
    }
}

/// Port of `format_text(mi, prefs)`: `(title, subtitle, footer)`.
/// `use_roman` replaces `get_use_roman()`'s real GUI-config lookup
/// (`config['use_roman_numerals_for_series_number']`) with an explicit
/// parameter -- no GUI config store in this port, matching the
/// project's established pattern for GUI-preference-backed upstream
/// globals (e.g. issue #168 jacket's `page_setup` narrowing).
pub fn format_text(mi: &MetaInformation, templates: &CoverTemplates, use_roman: bool) -> (String, String, String) {
    let authors: Vec<String> = mi.authors.iter().filter(|a| a.as_str() != "Unknown").cloned().collect();
    let formatted_series_index = fmt_sidx(Some(mi.series_index), use_roman);
    let source = CoverValueSource {
        title: mi.title.clone(),
        authors: authors_to_string(&authors),
        series: mi.series.clone().unwrap_or_default(),
        formatted_series_index,
    };
    (
        safe_format(&templates.title_template, &source),
        safe_format(&templates.subtitle_template, &source),
        safe_format(&templates.footer_template, &source),
    )
}

// ===================================================================
// Styles (issue #597): `tiny-skia` canvas + the `Half`/`Blocks`/
// `Cross` `Style` classes.
// ===================================================================
//
// Establishes the real `QImage`->`tiny_skia::Pixmap` canvas/PNG-encode
// pattern for `covers.py`'s 5 `Style` classes, using the 3 cheapest
// (no curve math, no transform-stamping, no text) as the proving
// ground. `Banner` (#599) and `Ornamental` (#600) are separate issues.
//
// Every real `Style.__call__(painter, rect, color_theme, title_block,
// subtitle_block, footer_block)` reads only a handful of scalar fields
// off `title_block`/`subtitle_block` (`Block` objects `layout_text`,
// #598, produces) -- never the `Block`s themselves. [`TextBlockGeometry`]
// carries exactly those fields (only [`render_cross`] needs it; `Half`/
// `Blocks` ignore their `title_block`/`subtitle_block` params in the
// real Python too), so this port's function signatures don't need to
// change once #598 lands -- #598 just computes and passes real values
// into the same fields.

/// Port of the base `Style.load_colors`: the 4 theme colors, parsed
/// from [`color`]'s hex strings into real paintable colors.
#[derive(Debug, Clone, Copy)]
pub struct StyleColors {
    pub color1: Color,
    pub color2: Color,
    pub ccolor1: Color,
    pub ccolor2: Color,
}

impl StyleColors {
    pub fn load(theme: &ColorTheme) -> Self {
        StyleColors {
            color1: hex_to_color(&color(theme, "color1")),
            color2: hex_to_color(&color(theme, "color2")),
            ccolor1: hex_to_color(&color(theme, "contrast_color1")),
            ccolor2: hex_to_color(&color(theme, "contrast_color2")),
        }
    }
}

/// Parses a bare `"rrggbb"` hex string (as produced by [`to_theme`]/
/// [`color`]) into a `tiny_skia::Color`. Falls back to opaque black on
/// a malformed string -- every real caller's input ultimately comes
/// from [`default_color_themes`]/user-supplied hex-quad strings the
/// same way upstream's own `QColor('#' + val)` would, so a malformed
/// value here is already a real, pre-existing data problem, not
/// something this port's color math should mask with `Result`
/// plumbing no upstream call site has either.
fn hex_to_color(hex: &str) -> Color {
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    let (Some(r), Some(g), Some(b)) = (byte(0), byte(2), byte(4)) else {
        return Color::BLACK;
    };
    Color::from_rgba8(r, g, b, 255)
}

/// Port of the base `Style.calculate_margins`. `Banner` overrides this
/// (#599); `Cross`/`Half`/`Blocks` all use the shared base behavior.
#[derive(Debug, Clone, Copy)]
pub struct Margins {
    pub hmargin: i32,
    pub vmargin: i32,
}

pub fn calculate_margins(cover_width: u32, cover_height: u32) -> Margins {
    Margins {
        hmargin: ((50.0 / 600.0) * cover_width as f64) as i32,
        vmargin: ((50.0 / 800.0) * cover_height as f64) as i32,
    }
}

/// The scalar fields real `Cross.__call__` reads off `title_block`/
/// `subtitle_block` (see this section's module doc).
#[derive(Debug, Clone, Copy, Default)]
pub struct TextBlockGeometry {
    pub title_x: f32,
    pub title_y: f32,
    pub title_height: f32,
    pub title_leading: f32,
    pub subtitle_height: f32,
    pub subtitle_line_spacing: f32,
}

/// Port of a real `Style.__call__`'s return tuple: `(title_color,
/// subtitle_color, footer_color)`.
#[derive(Debug, Clone, Copy)]
pub struct StyleResultColors {
    pub title: Color,
    pub subtitle: Color,
    pub footer: Color,
}

/// Builds a rounded-rectangle path with equal x/y corner radius,
/// clamped to at most half the smaller side (matching Qt's own
/// `addRoundedRect` clamping). `tiny-skia-path` has no built-in
/// rounded-rect helper, so this hand-builds the standard 4-corner
/// cubic-Bezier circular-arc approximation (the `k = 0.5522847498 * r`
/// constant used by every vector-graphics rounded-rect implementation,
/// including Qt's own).
fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<tiny_skia::Path> {
    let r = radius.max(0.0).min(w / 2.0).min(h / 2.0);
    let k = 0.5522847498 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Port of `Half.__call__`: a 3-stop vertical `QLinearGradient` fill.
pub fn render_half(pixmap: &mut Pixmap, width: f32, height: f32, colors: &StyleColors) -> StyleResultColors {
    let rect = Rect::from_xywh(0.0, 0.0, width, height).expect("non-degenerate cover rect");
    let shader = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(0.0, height),
        vec![
            GradientStop::new(0.0, colors.color1),
            GradientStop::new(0.7, colors.color2),
            GradientStop::new(1.0, colors.color1),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .expect("linear gradient with 3 distinct, non-degenerate stops");
    let paint = Paint { shader, ..Default::default() };
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    StyleResultColors { title: colors.ccolor1, subtitle: colors.ccolor1, footer: colors.ccolor1 }
}

/// Port of `Blocks.__call__`: the whole cover in `color1`, with the
/// bottom third overpainted in `color2`. (Upstream's own body computes
/// an unused intermediate `QRect` before the first `fillRect` -- dead
/// code with no observable effect, not reproduced here.)
pub fn render_blocks(pixmap: &mut Pixmap, width: u32, height: u32, colors: &StyleColors) -> StyleResultColors {
    let full = Rect::from_xywh(0.0, 0.0, width as f32, height as f32).expect("non-degenerate cover rect");
    let paint1 = Paint { shader: tiny_skia::Shader::SolidColor(colors.color1), ..Default::default() };
    pixmap.fill_rect(full, &paint1, Transform::identity(), None);

    let y = height - height / 3;
    let band = Rect::from_xywh(0.0, y as f32, width as f32, (height - y) as f32).expect("non-degenerate band rect");
    let paint2 = Paint { shader: tiny_skia::Shader::SolidColor(colors.color2), ..Default::default() };
    pixmap.fill_rect(band, &paint2, Transform::identity(), None);

    StyleResultColors { title: colors.ccolor1, subtitle: colors.ccolor1, footer: colors.ccolor2 }
}

/// Port of `Cross.__call__`: the whole cover in `color1`, a rounded-
/// corner band behind the title/subtitle text in `color2` (`setClipPath`
/// -> a [`Mask`] filled with the rounded-rect path, matching Qt's own
/// general clip-to-arbitrary-path mechanism rather than special-casing
/// "the fill happens to match the clip shape exactly"), and a solid
/// left-margin bar in `color2`.
///
/// Real `QPainterPath.addRoundedRect(rect, 10, 10 * rect.width() /
/// rect.height(), Qt.SizeMode.RelativeSize)` resolves (worked out from
/// Qt's own `RelativeSize` semantics: `rx = xRadius/100 * width/2`,
/// `ry = yRadius/100 * height/2`) to an absolute circular corner radius
/// of exactly `0.05 * rect.width()` regardless of the band's aspect
/// ratio -- substituted directly rather than reproducing the
/// percentage/`RelativeSize` indirection, which has no other real
/// caller in this port.
pub fn render_cross(
    pixmap: &mut Pixmap,
    width: u32,
    height: u32,
    colors: &StyleColors,
    blocks: &TextBlockGeometry,
) -> StyleResultColors {
    let (width_f, height_f) = (width as f32, height as f32);
    let full = Rect::from_xywh(0.0, 0.0, width_f, height_f).expect("non-degenerate cover rect");
    let paint1 = Paint { shader: tiny_skia::Shader::SolidColor(colors.color1), ..Default::default() };
    pixmap.fill_rect(full, &paint1, Transform::identity(), None);

    let band_y = blocks.title_y as i32;
    let band_h = blocks.title_height + blocks.subtitle_height + blocks.subtitle_line_spacing / 2.0 + blocks.title_leading;
    let band = Rect::from_xywh(0.0, band_y as f32, width_f, band_h).expect("non-degenerate title band rect");

    let radius = 0.05 * width_f;
    if let Some(path) = rounded_rect_path(0.0, band_y as f32, width_f, band_h, radius) {
        if let Some(mut mask) = Mask::new(width, height) {
            mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
            let paint2 = Paint { shader: tiny_skia::Shader::SolidColor(colors.color2), ..Default::default() };
            pixmap.fill_rect(band, &paint2, Transform::identity(), Some(&mask));
        }
    }

    let left_w = blocks.title_x as i32;
    if left_w > 0 {
        if let Some(left) = Rect::from_xywh(0.0, 0.0, left_w as f32, height_f) {
            let paint2 = Paint { shader: tiny_skia::Shader::SolidColor(colors.color2), ..Default::default() };
            pixmap.fill_rect(left, &paint2, Transform::identity(), None);
        }
    }

    StyleResultColors { title: colors.ccolor2, subtitle: colors.ccolor2, footer: colors.ccolor1 }
}

// ===================================================================
// Banner style (issue #599): real curve math (`rotate_vector`/
// `draw_curved_line`) + fold-rectangle geometry.
// ===================================================================
//
// Unlike `Cross` (#597, ported before #598's real `Block` existed and
// so takes a placeholder [`TextBlockGeometry`]), `Banner` is ported
// after #598 landed and reads real title/subtitle geometry directly
// off [`crate::covers_text::Block`] -- `position()`/`height()`/
// `leading`/`line_spacing` all already exist for exactly this.

/// Port of `Banner.calculate_margins`: overrides the base `Style`
/// margins entirely (does not call `Style.calculate_margins` for
/// `hmargin`, only inherits `vmargin`), plus a `fold_width` the base
/// `Style` has no equivalent of.
#[derive(Debug, Clone, Copy)]
pub struct BannerMargins {
    pub hmargin: i32,
    pub vmargin: i32,
    pub fold_width: i32,
}

pub fn banner_margins(cover_width: u32, cover_height: u32) -> BannerMargins {
    BannerMargins {
        hmargin: (0.15 * cover_width as f64) as i32,
        vmargin: calculate_margins(cover_width, cover_height).vmargin,
        fold_width: (0.1 * cover_width as f64) as i32,
    }
}

/// Port of `rotate_vector`.
fn rotate_vector(angle: f32, x: f32, y: f32) -> (f32, f32) {
    (x * angle.cos() - y * angle.sin(), x * angle.sin() + y * angle.cos())
}

/// Port of `draw_curved_line`: appends a cubic Bezier segment from the
/// path's current point to `(dx, dy)` further along, whose two control
/// points are `(c1_frac, c1_amp)`/`(c2_frac, c2_amp)` -- expressed as
/// fractions of the segment's own length/amplitude in a frame rotated
/// to the segment's own direction (`atan2(dy, dx)`) -- rotated back
/// into absolute coordinates via [`rotate_vector`]. This is the exact
/// real curve-construction primitive every `Banner` ribbon/fold edge
/// uses; `tiny-skia-path`'s `cubic_to` is a direct match for Qt's
/// `QPainterPath.cubicTo`, so only the control-point math is new
/// porting work.
fn draw_curved_line(pb: &mut PathBuilder, dx: f32, dy: f32, c1_frac: f32, c1_amp: f32, c2_frac: f32, c2_amp: f32) {
    let length = (dx * dx + dy * dy).sqrt();
    let angle = dy.atan2(dx);
    let (c1x, c1y) = rotate_vector(angle, c1_frac * length, c1_amp * length);
    let (c2x, c2y) = rotate_vector(angle, c2_frac * length, c2_amp * length);
    let pos = pb.last_point().unwrap_or(Point::from_xy(0.0, 0.0));
    pb.cubic_to(pos.x + c1x, pos.y + c1y, pos.x + c2x, pos.y + c2y, pos.x + dx, pos.y + dy);
}

/// Port of `QColor::darker()`'s real default (`factor=200`): scales
/// the HSV "value" channel by `100/factor` (i.e. halves it), keeping
/// hue/saturation unchanged -- real HSV-space darkening, not a flat
/// RGB scale (which would visibly differ for saturated colors).
fn darker(color: Color) -> Color {
    let (r, g, b) = (color.red(), color.green(), color.blue());
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let v = max;
    let s = if max > 0.0 { delta / max } else { 0.0 };
    let h = if delta <= 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let new_v = (v * 0.5).clamp(0.0, 1.0);
    let c = new_v * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = new_v - c;
    Color::from_rgba((r1 + m).clamp(0.0, 1.0), (g1 + m).clamp(0.0, 1.0), (b1 + m).clamp(0.0, 1.0), color.alpha()).unwrap_or(Color::BLACK)
}

/// The 5 closed paths a real `Banner.__call__` builds: the main
/// ribbon plus a fold and inner-fold-shading path on each side.
#[derive(Debug, Clone)]
pub struct BannerPaths {
    pub main: tiny_skia::Path,
    pub left_fold: tiny_skia::Path,
    pub left_inner: tiny_skia::Path,
    pub right_fold: tiny_skia::Path,
    pub right_inner: tiny_skia::Path,
}

/// Port of `draw_fold` (a real closure in the Python, capturing
/// `deltax`/`height`/`rwidth` from `Banner.__call__`'s own scope).
#[allow(clippy::too_many_arguments)]
fn draw_fold(x: f32, m: f32, corner: Point, rtop: f32, rwidth: f32, deltax: f32, height: f32) -> Option<(tiny_skia::Path, tiny_skia::Path)> {
    let mut pb = PathBuilder::new();
    pb.move_to(x, rtop);
    draw_curved_line(&mut pb, rwidth * m, 0.0, 0.1, 0.1 * m, 0.5, -0.2 * m);
    let fold_upper = pb.last_point()?;
    let cur = pb.last_point()?;
    pb.line_to(cur.x - deltax * m, cur.y + height);
    let fold_corner = pb.last_point()?;
    draw_curved_line(&mut pb, -rwidth * m, 0.0, 0.2, -0.1 * m, 0.8, -0.1 * m);
    draw_curved_line(&mut pb, deltax * m, -height, 0.2, 0.1 * m, 0.8, 0.1 * m);
    let fold = pb.finish()?;

    let mut inner_pb = PathBuilder::new();
    inner_pb.move_to(corner.x, corner.y);
    let dp = (fold_corner.x - corner.x, fold_corner.y - corner.y);
    draw_curved_line(&mut inner_pb, dp.0, dp.1, 0.5, 0.3 * m, 1.0, 0.0 * m);
    inner_pb.line_to(fold_upper.x, fold_upper.y);
    inner_pb.close();
    let inner_fold = inner_pb.finish()?;

    Some((fold, inner_fold))
}

/// Port of `Banner.__call__`'s path-construction half (the
/// `title_block`/`subtitle_block`-derived geometry through to the 5
/// closed paths, before any painting).
pub fn build_banner_paths(width: u32, height: u32, margins: &BannerMargins, title_block: &crate::covers_text::Block, subtitle_block: &crate::covers_text::Block) -> Option<BannerPaths> {
    const GRADE: f32 = 0.07;
    let (width_f, height_f) = (width as f32, height as f32);
    let hmargin = margins.hmargin as f32;

    let top = title_block.position().1 + 2.0;
    // Real Python floor-divides (`//`) an already-integer `line_spacing`
    // (`QFontMetrics::lineSpacing()` returns `int`); this port's own
    // `line_spacing` is a real `f32` (see `covers_text`'s own disclosed
    // no-Qt-"+1"-rounding narrowing), so only the final `//`'s flooring
    // is replicated here, not a pre-rounded integer input.
    let extra_spacing = if subtitle_block.line_spacing != 0.0 { (subtitle_block.line_spacing / 2.0).floor() } else { (title_block.line_spacing / 3.0).floor() };
    let band_height = title_block.height() + subtitle_block.height() + extra_spacing + title_block.leading;
    let right = (width_f - 1.0) - hmargin; // QRect::right() == x + width - 1
    let band_width = right - hmargin;

    let mut main_pb = PathBuilder::new();
    main_pb.move_to(hmargin, top);
    draw_curved_line(&mut main_pb, width_f - 2.0 * hmargin, 0.0, 0.1, -0.1, 0.9, -0.1);
    let deltax = GRADE * band_height;
    main_pb.line_to(right + deltax, top + band_height);
    let right_corner = main_pb.last_point()?;
    draw_curved_line(&mut main_pb, -band_width - 2.0 * deltax, 0.0, 0.1, 0.05, 0.9, 0.05);
    let left_corner = main_pb.last_point()?;
    main_pb.close();
    let main = main_pb.finish()?;

    let rwidth = margins.fold_width as f32;
    let yfrac = 0.1;
    let width23 = (0.67 * rwidth) as i32 as f32;
    let rtop = top + band_height * yfrac;

    let (left_fold, left_inner) = draw_fold(hmargin - width23, 1.0, left_corner, rtop, rwidth, deltax, band_height)?;
    let (right_fold, right_inner) = draw_fold(right + width23, -1.0, right_corner, rtop, rwidth, deltax, band_height)?;

    Some(BannerPaths { main, left_fold, left_inner, right_fold, right_inner })
}

/// Port of `Banner.__call__`'s painting half: fills + 3px round-join
/// strokes for the fold/inner-fold/main paths, in upstream's own real
/// draw order (folds, then inner-fold shading, then the main ribbon on
/// top).
pub fn render_banner(pixmap: &mut Pixmap, width: u32, height: u32, colors: &StyleColors, margins: &BannerMargins, title_block: &crate::covers_text::Block, subtitle_block: &crate::covers_text::Block) -> Option<StyleResultColors> {
    let full = Rect::from_xywh(0.0, 0.0, width as f32, height as f32)?;
    let paint1 = Paint { shader: tiny_skia::Shader::SolidColor(colors.color1), ..Default::default() };
    pixmap.fill_rect(full, &paint1, Transform::identity(), None);

    let paths = build_banner_paths(width, height, margins, title_block, subtitle_block)?;
    let stroke = tiny_skia::Stroke { width: 3.0, line_join: tiny_skia::LineJoin::Round, ..Default::default() };
    let stroke_paint = Paint { shader: tiny_skia::Shader::SolidColor(colors.ccolor2), ..Default::default() };
    let fill_paint = Paint { shader: tiny_skia::Shader::SolidColor(colors.color2), ..Default::default() };
    let inner_fill_paint = Paint { shader: tiny_skia::Shader::SolidColor(darker(colors.color2)), ..Default::default() };

    for fold in [&paths.left_fold, &paths.right_fold] {
        pixmap.fill_path(fold, &fill_paint, FillRule::Winding, Transform::identity(), None);
        pixmap.stroke_path(fold, &stroke_paint, &stroke, Transform::identity(), None);
    }
    for inner in [&paths.left_inner, &paths.right_inner] {
        pixmap.fill_path(inner, &inner_fill_paint, FillRule::Winding, Transform::identity(), None);
        pixmap.stroke_path(inner, &stroke_paint, &stroke, Transform::identity(), None);
    }
    pixmap.fill_path(&paths.main, &fill_paint, FillRule::Winding, Transform::identity(), None);
    pixmap.stroke_path(&paths.main, &stroke_paint, &stroke, Transform::identity(), None);

    Some(StyleResultColors { title: colors.ccolor2, subtitle: colors.ccolor2, footer: colors.ccolor1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_theme_splits_a_hex_quad_string() {
        let t = to_theme("e8d9ac c7b07b 564628 382d1a");
        assert_eq!(t.color1, "e8d9ac");
        assert_eq!(t.color2, "c7b07b");
        assert_eq!(t.contrast_color1, "564628");
        assert_eq!(t.contrast_color2, "382d1a");
    }

    #[test]
    fn load_color_themes_returns_the_four_builtins_by_default() {
        let themes = load_color_themes(&HashMap::new(), &[]);
        assert_eq!(themes.len(), 4);
        assert!(themes.iter().any(|t| t.color1 == "e8d9ac"));
    }

    #[test]
    fn load_color_themes_respects_disabled_and_falls_back_when_all_disabled() {
        let disabled: Vec<String> = vec!["Earth".to_string(), "Grass".to_string(), "Water".to_string(), "Silver".to_string()];
        let themes = load_color_themes(&HashMap::new(), &disabled);
        // All 4 builtins disabled -> fall back to all 4 builtins anyway.
        assert_eq!(themes.len(), 4);
    }

    #[test]
    fn load_color_themes_merges_user_overrides() {
        let mut overrides = HashMap::new();
        overrides.insert("Custom".to_string(), to_theme("111111 222222 333333 444444"));
        let themes = load_color_themes(&overrides, &[]);
        assert_eq!(themes.len(), 5);
        assert!(themes.iter().any(|t| t.color1 == "111111"));
    }

    #[test]
    fn color_falls_back_when_a_theme_field_is_empty() {
        let theme = ColorTheme {
            color1: String::new(),
            color2: "abcdef".to_string(),
            contrast_color1: String::new(),
            contrast_color2: String::new(),
        };
        assert_eq!(color(&theme, "color1"), "ffffff");
        assert_eq!(color(&theme, "color2"), "abcdef");
    }

    #[test]
    fn sanitize_normalizes_and_cleans() {
        assert_eq!(sanitize("hello"), "hello");
    }

    #[test]
    fn escape_and_unescape_formatting_round_trip() {
        let raw = "Tom & Jerry <3>";
        let escaped = escape_formatting(raw);
        assert_eq!(escaped, "Tom &amp; Jerry &lt;3&gt;");
        assert_eq!(unescape_formatting(&escaped), raw);
    }

    #[test]
    fn parse_text_formatting_extracts_a_bold_span() {
        let (text, formats) = parse_text_formatting("Hello <b>World</b>!");
        assert_eq!(text, "Hello World!");
        assert_eq!(formats.len(), 1);
        assert_eq!(formats[0], FormatRange { bold: true, italic: false, start: 6, length: 5 });
    }

    #[test]
    fn parse_text_formatting_extracts_nested_bold_and_italic() {
        let (text, formats) = parse_text_formatting("A <b>bold <i>and italic</i> text</b> B");
        assert_eq!(text, "A bold and italic text B");
        // Inner italic span closes first ("A bold " is 7 chars, "and italic" is 10 chars).
        let italic = formats.iter().find(|f| f.italic).unwrap();
        assert_eq!((italic.start, italic.length), (7, 10));
        let bold = formats.iter().find(|f| f.bold).unwrap();
        assert_eq!((bold.start, bold.length), (2, 20));
    }

    #[test]
    fn parse_text_formatting_ignores_unrecognized_tags() {
        let (text, formats) = parse_text_formatting("A <span>plain</span> B");
        assert_eq!(text, "A plain B");
        assert!(formats.is_empty());
    }

    #[test]
    fn parse_text_formatting_handles_an_unclosed_tag_by_extending_to_the_end() {
        let (text, formats) = parse_text_formatting("Start <b>rest of the text");
        assert_eq!(text, "Start rest of the text");
        assert_eq!(formats.len(), 1);
        assert_eq!(formats[0].start, 6);
        assert_eq!(formats[0].length, text.chars().count() - 6);
    }

    #[test]
    fn parse_text_formatting_with_no_tags_returns_the_text_unchanged() {
        let (text, formats) = parse_text_formatting("plain text");
        assert_eq!(text, "plain text");
        assert!(formats.is_empty());
    }

    #[test]
    fn parse_text_formatting_counts_amp_entities_as_one_character_for_offsets() {
        // "&amp;" (5 raw chars) counts as ONE character for offset
        // purposes (anticipating the caller's later unescape), but the
        // returned text keeps the literal "&amp;".
        let (text, formats) = parse_text_formatting("A &amp; <b>B</b>");
        assert_eq!(text, "A &amp; B");
        assert_eq!(formats[0].start, 4);
    }

    fn mi(title: &str, authors: &[&str], series: Option<&str>, series_index: f64) -> MetaInformation {
        MetaInformation {
            title: title.to_string(),
            authors: authors.iter().map(|a| a.to_string()).collect(),
            series: series.map(str::to_string),
            series_index,
            ..Default::default()
        }
    }

    #[test]
    fn format_text_title_template_substitutes_the_title() {
        let m = mi("My Book", &["Jane Doe"], None, 1.0);
        let (title, _, _) = format_text(&m, &CoverTemplates::default(), false);
        assert_eq!(title, "<b>My Book");
    }

    #[test]
    fn format_text_subtitle_template_evaluates_the_embedded_gpm_expression() {
        let m = mi("My Book", &["Jane Doe"], Some("The Series"), 2.0);
        let (_, subtitle, _) = format_text(&m, &CoverTemplates::default(), false);
        assert_eq!(subtitle, "<i>The Series</i> - 2");
    }

    #[test]
    fn format_text_subtitle_template_is_empty_when_there_is_no_series() {
        let m = mi("My Book", &["Jane Doe"], None, 1.0);
        let (_, subtitle, _) = format_text(&m, &CoverTemplates::default(), false);
        assert_eq!(subtitle, "");
    }

    #[test]
    fn format_text_footer_template_runs_the_real_program_mode_default() {
        let m = mi("My Book", &["Jane Doe", "John Smith"], None, 1.0);
        let (_, _, footer) = format_text(&m, &CoverTemplates::default(), false);
        assert!(!footer.starts_with("Template error"), "footer: {footer}");
        assert!(footer.contains("Jane Doe"), "footer: {footer}");
        assert!(footer.contains("John Smith"), "footer: {footer}");
        assert!(footer.contains("<b>"), "footer: {footer}");
    }

    #[test]
    fn format_text_footer_template_filters_out_the_literal_unknown_author() {
        let m = mi("My Book", &["Unknown"], None, 1.0);
        let (_, _, footer) = format_text(&m, &CoverTemplates::default(), false);
        assert!(!footer.contains("Unknown"), "footer: {footer}");
    }

    #[test]
    fn safe_format_falls_back_to_a_template_error_message_on_a_real_failure() {
        let source = CoverValueSource {
            title: "T".to_string(),
            authors: "A".to_string(),
            series: String::new(),
            formatted_series_index: String::new(),
        };
        let out = safe_format("program:\nthis_is_not_a_real_function()", &source);
        assert!(out.starts_with("Template error"), "{out}");
    }

    #[test]
    fn vformat_handles_escaped_braces() {
        let source = CoverValueSource {
            title: "T".to_string(),
            authors: String::new(),
            series: String::new(),
            formatted_series_index: String::new(),
        };
        assert_eq!(vformat("{{{title}}}", &source).unwrap(), "{T}");
    }

    #[test]
    fn get_value_escapes_but_get_raw_value_does_not() {
        let source = CoverValueSource {
            title: "Tom & Jerry".to_string(),
            authors: String::new(),
            series: String::new(),
            formatted_series_index: String::new(),
        };
        assert_eq!(source.get_value("title").unwrap(), "Tom &amp; Jerry");
        match source.get_raw_value("title").unwrap() {
            RawValue::Scalar(s) => assert_eq!(s, "Tom & Jerry"),
            _ => panic!("expected a scalar"),
        }
    }

    fn px(pixmap: &Pixmap, x: u32, y: u32) -> (u8, u8, u8) {
        let c = pixmap.pixel(x, y).unwrap().demultiply();
        (c.red(), c.green(), c.blue())
    }

    fn theme() -> ColorTheme {
        // color1=red, color2=green, ccolor1=blue, ccolor2=white.
        ColorTheme {
            color1: "ff0000".to_string(),
            color2: "00ff00".to_string(),
            contrast_color1: "0000ff".to_string(),
            contrast_color2: "ffffff".to_string(),
        }
    }

    #[test]
    fn hex_to_color_parses_a_theme_hex_string() {
        let c = hex_to_color("e8d9ac");
        assert_eq!((c.to_color_u8().red(), c.to_color_u8().green(), c.to_color_u8().blue()), (0xe8, 0xd9, 0xac));
    }

    #[test]
    fn calculate_margins_matches_the_real_ratio() {
        let m = calculate_margins(600, 800);
        assert_eq!((m.hmargin, m.vmargin), (50, 50));
        let m2 = calculate_margins(1200, 1600);
        assert_eq!((m2.hmargin, m2.vmargin), (100, 100));
    }

    #[test]
    fn render_half_produces_a_top_to_bottom_to_top_gradient() {
        let colors = StyleColors::load(&theme());
        let mut pixmap = Pixmap::new(100, 100).unwrap();
        let result = render_half(&mut pixmap, 100.0, 100.0, &colors);
        // Real returned colors are all ccolor1 (blue).
        assert_eq!(result.title.to_color_u8().blue(), 0xff);
        // Gradient endpoints (0.0 and 1.0 stops) are both color1 (red);
        // the 0.7 stop is color2 (green). Pixel centers never land
        // exactly on a stop's fractional position, so these check
        // "clearly dominated by" rather than an exact color.
        let (r, g, _) = px(&pixmap, 50, 0);
        assert!(r > 240 && g < 20, "near top should be mostly color1 (red): got ({r},{g})");
        let (r, g, _) = px(&pixmap, 50, 99);
        assert!(r > 240 && g < 20, "near bottom should be mostly color1 (red): got ({r},{g})");
        let (r, g, _) = px(&pixmap, 50, 70);
        assert!(g > 240 && r < 20, "near the 0.7 stop should be mostly color2 (green): got ({r},{g})");
    }

    #[test]
    fn render_blocks_splits_top_two_thirds_from_bottom_third() {
        let colors = StyleColors::load(&theme());
        let mut pixmap = Pixmap::new(90, 90).unwrap();
        let result = render_blocks(&mut pixmap, 90, 90, &colors);
        assert_eq!(result.footer.to_color_u8().green(), 0xff); // ccolor2 = white -> green channel opaque too, checked below
        // y = 90 - 90/3 = 60: rows above are color1 (red), at/after are color2 (green).
        assert_eq!(px(&pixmap, 45, 59), (0xff, 0, 0));
        assert_eq!(px(&pixmap, 45, 60), (0, 0xff, 0));
        assert_eq!(px(&pixmap, 45, 89), (0, 0xff, 0));
    }

    #[test]
    fn render_cross_fills_background_left_bar_and_clipped_title_band() {
        let colors = StyleColors::load(&theme());
        let mut pixmap = Pixmap::new(200, 200).unwrap();
        let blocks = TextBlockGeometry {
            title_x: 40.0,
            title_y: 50.0,
            title_height: 60.0,
            title_leading: 0.0,
            subtitle_height: 0.0,
            subtitle_line_spacing: 0.0,
        };
        render_cross(&mut pixmap, 200, 200, &colors, &blocks);

        // Outside the title band and left bar: pure background (color1 = red).
        assert_eq!(px(&pixmap, 150, 10), (0xff, 0, 0));
        // Left margin bar (x < title_x): color2 (green), full height --
        // including rows that fall inside the title band's y-range,
        // since the real Python draws this bar *after* the clipped
        // band, unconditionally overwriting it there too.
        assert_eq!(px(&pixmap, 10, 190), (0, 0xff, 0));
        assert_eq!(px(&pixmap, 10, 55), (0, 0xff, 0));
        // Center of the title band (right of the left bar), well inside
        // the rounded rect: color2 (green).
        assert_eq!(px(&pixmap, 100, 80), (0, 0xff, 0));
        // The rounded corners of the title band on its *right* side
        // (unobscured by the left bar) are clipped away by the mask,
        // leaving background color1 (red) showing through at the
        // extreme corner pixels -- proving the clip mask actually
        // clips rather than filling a plain rect.
        assert_eq!(px(&pixmap, 199, 50), (0xff, 0, 0));
        assert_eq!(px(&pixmap, 199, 109), (0xff, 0, 0));
        // A pixel just inside that same right edge, away from the
        // rounded corner, is still color2 -- the clip only removes the
        // corners, not the whole right edge.
        assert_eq!(px(&pixmap, 199, 80), (0, 0xff, 0));
    }

    fn real_block(db: &fontdb::Database, text: &str, width: f32, pixel_size: f32) -> crate::covers_text::Block {
        let font = crate::covers_text::FontFace::query(db, "Liberation Serif", fontdb::Family::Serif).expect("a font must be installed on the test host");
        let face = font.face();
        let metrics = crate::covers_text::FontMetrics::new(&face, pixel_size);
        crate::covers_text::Block::new(text, width, &face, &metrics, 1000.0, crate::covers_text::HAlign::Center)
    }

    #[test]
    fn rotate_vector_matches_hand_computed_values() {
        // 0 radians: identity.
        let (x, y) = rotate_vector(0.0, 3.0, -5.0);
        assert!((x - 3.0).abs() < 1e-5 && (y - (-5.0)).abs() < 1e-5);
        // PI/2: (x,y) -> (-y, x).
        let (x, y) = rotate_vector(std::f32::consts::FRAC_PI_2, 3.0, -5.0);
        assert!((x - 5.0).abs() < 1e-4, "x={x}");
        assert!((y - 3.0).abs() < 1e-4, "y={y}");
    }

    #[test]
    fn draw_curved_line_produces_the_exact_hand_computed_control_points() {
        let mut pb = PathBuilder::new();
        pb.move_to(0.0, 0.0);
        // A horizontal (angle=0) segment -- rotate_vector is the
        // identity at angle 0, so control points are exactly
        // (c1_frac*length, c1_amp*length) and (c2_frac*length, c2_amp*length).
        draw_curved_line(&mut pb, 100.0, 0.0, 0.1, -0.1, 0.9, -0.1);
        let path = pb.finish().unwrap();
        let segments: Vec<_> = path.segments().collect();
        assert_eq!(segments.len(), 2); // MoveTo + CubicTo
        match segments[1] {
            tiny_skia::PathSegment::CubicTo(c1, c2, end) => {
                assert!((c1.x - 10.0).abs() < 1e-3 && (c1.y - (-10.0)).abs() < 1e-3, "{c1:?}");
                assert!((c2.x - 90.0).abs() < 1e-3 && (c2.y - (-10.0)).abs() < 1e-3, "{c2:?}");
                assert!((end.x - 100.0).abs() < 1e-3 && end.y.abs() < 1e-3, "{end:?}");
            }
            other => panic!("expected a CubicTo segment, got {other:?}"),
        }
    }

    #[test]
    fn darker_halves_the_hsv_value_of_a_pure_color() {
        let red = Color::from_rgba8(200, 0, 0, 255);
        let d = darker(red).to_color_u8();
        // Pure red's HSV value == its red channel; darker(200) halves it.
        assert_eq!(d.red(), 100);
        assert_eq!(d.green(), 0);
        assert_eq!(d.blue(), 0);
    }

    #[test]
    fn banner_margins_matches_the_real_ratios() {
        let m = banner_margins(1200, 1600);
        assert_eq!(m.hmargin, 180); // 0.15 * 1200
        assert_eq!(m.fold_width, 120); // 0.1 * 1200
        assert_eq!(m.vmargin, 100); // base Style ratio: (50/800)*1600
    }

    #[test]
    fn build_banner_paths_produces_a_ribbon_spanning_the_full_width() {
        let db = crate::covers_text::load_system_fonts();
        let title = real_block(&db, "Title", 800.0, 60.0);
        let subtitle = crate::covers_text::Block::empty();
        let margins = banner_margins(1200, 1600);
        let paths = build_banner_paths(1200, 1600, &margins, &title, &subtitle).expect("real banner geometry");

        // The ribbon's top edge runs straight across at `hmargin..width-hmargin`,
        // but its bottom edge is horizontally slanted by `deltax = GRADE *
        // band_height` (the real "folded banner" look: `right_corner` is
        // shifted right by `deltax`, `left_corner` left by `deltax`), so
        // the real bounding box is wider than the plain margins by
        // exactly that amount -- not a fixed small tolerance.
        let band_height = title.height() + subtitle.height() + (title.line_spacing / 3.0).floor() + title.leading;
        let deltax = 0.07 * band_height;
        let bounds = paths.main.bounds();
        assert!((bounds.left() - (margins.hmargin as f32 - deltax)).abs() < 2.0, "{bounds:?} deltax={deltax}");
        assert!((bounds.right() - ((1200.0 - margins.hmargin as f32) + deltax)).abs() < 2.0, "{bounds:?} deltax={deltax}");

        // The fold paths sit near the left/right margins, straddling them.
        let left_bounds = paths.left_fold.bounds();
        assert!(left_bounds.left() < margins.hmargin as f32);
        assert!(left_bounds.right() > 0.0);
        let right_bounds = paths.right_fold.bounds();
        assert!(right_bounds.right() > (1200.0 - margins.hmargin as f32));
    }

    #[test]
    fn render_banner_paints_the_ribbon_over_the_background() {
        let db = crate::covers_text::load_system_fonts();
        let title = real_block(&db, "A Banner Title", 800.0, 60.0);
        let subtitle = crate::covers_text::Block::empty();
        let margins = banner_margins(1200, 1600);
        let colors = StyleColors::load(&theme()); // color1=red, color2=green
        let mut pixmap = Pixmap::new(1200, 1600).unwrap();

        let result = render_banner(&mut pixmap, 1200, 1600, &colors, &margins, &title, &subtitle).expect("real banner render");
        assert_eq!(result.title.to_color_u8().blue(), 0xff); // ccolor2 = blue

        // Far outside the ribbon (top-left corner): still pure background.
        assert_eq!(px(&pixmap, 2, 2), (0xff, 0, 0));
        // Somewhere well inside the ribbon's vertical band and clear of
        // the fold overlays: real green ribbon fill was painted, not
        // just the red background.
        let ribbon_y = title.position().1 as u32 + 5;
        let (r, g, _) = px(&pixmap, 600, ribbon_y);
        assert!(g > r, "expected ribbon-green to dominate at (600,{ribbon_y}), got r={r} g={g}");
    }
}

