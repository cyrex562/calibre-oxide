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
//! done), #597 (`tiny-skia` canvas + `Half`/`Blocks`/`Cross`), #598
//! (text layout + rendering bridge), #599 (`Banner`, real curve
//! math), #600 (`Ornamental`, transform stamping), #601 (entry
//! points/wiring).
//!
//! # This file's own scope
//!
//! [`ColorTheme`]/[`load_color_themes`]/[`theme_to_colors`]/[`color`]
//! (port of the `Colors {{{` section) and [`sanitize`]/
//! [`escape_formatting`]/[`unescape_formatting`]/
//! [`parse_text_formatting`] (port of the non-Qt-dependent half of the
//! `Draw text {{{` section). `parse_text_formatting` deliberately
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
}
