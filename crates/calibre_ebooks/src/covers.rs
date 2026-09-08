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
//! dependency): #595 (this file -- color themes + text-formatting
//! tokenizer, done, zero graphics dependency), #596 (field-template
//! formatting -- needs more than `calibre_utils::formatter`'s already-
//! real GPM evaluator alone, see its own issue for why), #597
//! (`tiny-skia` canvas + `Half`/`Blocks`/`Cross`), #598 (text layout +
//! rendering bridge), #599 (`Banner`, real curve math), #600
//! (`Ornamental`, transform stamping), #601 (entry points/wiring).
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
}
