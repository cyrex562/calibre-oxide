//! Conversion configuration.
//!
//! Port of `old_src/src/calibre/ebooks/conversion/config.py`.
//!
//! Coverage:
//! - `GuiRecommendations`: the dict-with-disabled-set the GUI uses to
//!   remember per-conversion-profile prefs (fully ported, JSON
//!   serialize with a `json:` prefix for on-disk compatibility with
//!   existing Calibre installs).
//! - `OPTIONS`: the input/pipe/output-format → option-name registry.
//! - `save_defaults` / `load_defaults`: atomic per-profile persistence
//!   via [`crate::config::atomic_write`] — matches the write
//!   discipline in `docs/FAULT_TOLERANCE.md` (temp + rename + fsync
//!   parent).
//! - Format-preference helpers: `sort_formats_by_preference`,
//!   `get_output_formats`, `get_sorted_output_formats`.
//! - `options_for_input_fmt` / `options_for_output_fmt`: take a
//!   plugin-name resolver callback so tests don't need the live
//!   plugin registry (deferred coupling to `calibre_customize::ui`).
//!
//! Deferred (needs LibraryHandle from #93):
//! - `save_specifics` / `load_specifics` / `delete_specifics` —
//!   per-book conversion options stored in `metadata.db`.
//! - `get_input_format_for_book`, `get_available_formats_for_book`,
//!   `get_supported_input_formats_for_book`,
//!   `get_preferred_input_format_for_book`.
//! - `NoSupportedInputFormats` error (exposed here as a placeholder
//!   type so callers can construct it).

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Serializable-friendly form of the Python `GuiRecommendations` dict.
/// Uses a `BTreeMap` so serialization is deterministic (JSON key
/// order matters when diffing config files).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GuiRecommendations {
    options: BTreeMap<String, Value>,
    disabled_options: HashSet<String>,
}

/// Prefix the Python `serialize()` wrote to distinguish JSON payloads
/// from the older `ast.literal_eval`-based pickle-ish format.
const JSON_PREFIX: &[u8] = b"json:";

impl GuiRecommendations {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.options.get(key)
    }

    pub fn set(&mut self, key: impl Into<String>, val: Value) {
        self.options.insert(key.into(), val);
    }

    pub fn contains(&self, key: &str) -> bool {
        self.options.contains_key(key)
    }

    pub fn is_empty(&self) -> bool {
        self.options.is_empty()
    }

    pub fn len(&self) -> usize {
        self.options.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.options.iter()
    }

    pub fn disabled(&self) -> &HashSet<String> {
        &self.disabled_options
    }

    pub fn mark_disabled(&mut self, key: impl Into<String>) {
        self.disabled_options.insert(key.into());
    }

    /// Python `to_recommendations(level)`. Returns `(name, value,
    /// level)` triples for every current option.
    pub fn to_recommendations(&self, level: OptionRecommendationLevel) -> Vec<(String, Value, OptionRecommendationLevel)> {
        self.options
            .iter()
            .map(|(k, v)| (k.clone(), v.clone(), level))
            .collect()
    }

    /// Python `serialize()`. Emits `json:` prefix + JSON payload so
    /// existing Calibre installs can still read the file after a
    /// roundtrip.
    pub fn serialize(&self) -> Vec<u8> {
        let json = serde_json::to_string_pretty(&self.options)
            .expect("BTreeMap<String, Value> always serializes");
        let mut out = Vec::with_capacity(JSON_PREFIX.len() + json.len());
        out.extend_from_slice(JSON_PREFIX);
        out.extend_from_slice(json.as_bytes());
        out
    }

    /// Python `deserialize(raw)`. Accepts both:
    /// - `json:` prefixed JSON (the modern format).
    /// - Raw JSON without prefix (defensive — older Python versions
    ///   sometimes wrote this).
    ///
    /// Python's `ast.literal_eval` fallback for the very old
    /// pickle-ish format is NOT supported — no known Calibre install
    /// still writes that format, and porting it requires implementing
    /// a Python literal parser.
    pub fn deserialize(&mut self, raw: &[u8]) {
        let payload = if raw.starts_with(JSON_PREFIX) {
            &raw[JSON_PREFIX.len()..]
        } else {
            raw
        };
        if payload.is_empty() {
            return;
        }
        if let Ok(v) = serde_json::from_slice::<BTreeMap<String, Value>>(payload) {
            self.options.extend(v);
        }
    }
}

impl Serialize for GuiRecommendations {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.options.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for GuiRecommendations {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let options = BTreeMap::<String, Value>::deserialize(deserializer)?;
        Ok(GuiRecommendations {
            options,
            disabled_options: HashSet::new(),
        })
    }
}

/// Python's `OptionRecommendation.LOW / MEDIUM / HIGH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OptionRecommendationLevel {
    Low,
    Medium,
    High,
}

/// Where per-conversion-profile defaults live. Path is
/// `<config_dir>/conversion/<sanitized_name>.json`. Python used `.py`
/// suffix (it wrote raw JSON with a `json:` prefix into a file with
/// a misleading extension); we keep the same directory but use
/// `.json` — no code parses this by extension and the name is more
/// honest.
pub fn conversion_config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CALIBRE_CONFIG_DIRECTORY") {
        return PathBuf::from(dir).join("conversion");
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("calibre")
        .join("conversion")
}

pub fn name_to_path(name: &str) -> PathBuf {
    conversion_config_dir().join(format!("{}.json", sanitize_config_name(name)))
}

/// Sanitize a profile name for use as a filename. Matches the intent
/// of Python's `sanitize_file_name` — strip filesystem-hostile
/// characters. Empty result → single underscore so we never emit a
/// zero-length filename.
fn sanitize_config_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| if "\\/:*?\"<>|\0".contains(c) { '_' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    let trimmed = out.trim();
    if trimmed.is_empty() {
        "_".to_string()
    } else {
        out = trimmed.to_string();
        out
    }
}

/// Atomic per-profile save. Writes to a tempfile in the same
/// directory, fsyncs, then renames — matches
/// `docs/FAULT_TOLERANCE.md` §2 (write discipline). No journaling
/// here because a lost conversion-profile-default is not a
/// library-integrity concern.
pub fn save_defaults(name: &str, recs: &GuiRecommendations) -> Result<()> {
    let path = name_to_path(name);
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("mkdir {:?}", dir))?;
    atomic_write(&path, &recs.serialize())
}

pub fn load_defaults(name: &str) -> Result<GuiRecommendations> {
    let path = name_to_path(name);
    if !path.exists() {
        return Ok(GuiRecommendations::new());
    }
    let raw = std::fs::read(&path).with_context(|| format!("read {:?}", path))?;
    let mut r = GuiRecommendations::new();
    r.deserialize(&raw);
    Ok(r)
}

fn atomic_write(target: &Path, bytes: &[u8]) -> Result<()> {
    let dir = target.parent().unwrap_or(Path::new("."));
    let tmp = target.with_extension("json.tmp");
    {
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("create {:?}", tmp))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, target)
        .with_context(|| format!("rename {:?} -> {:?}", tmp, target))?;
    // fsync the directory so the rename is durable.
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Placeholder for the DB-integrated per-book conversion options.
/// Real implementation blocked on issue #93 (LibraryHandle).
#[derive(Debug, Error)]
pub enum NoSupportedInputFormats {
    #[error("no supported input formats; available: {available:?}")]
    Empty { available: Vec<String> },
}

pub fn sort_formats_by_preference(formats: &[String], prefs: &[String]) -> Vec<String> {
    // Build an uppercase→rank map for O(1) key lookup.
    let ranks: std::collections::HashMap<String, usize> = prefs
        .iter()
        .enumerate()
        .map(|(i, s)| (s.to_uppercase(), i))
        .collect();
    let mut with_rank: Vec<(&String, usize)> = formats
        .iter()
        .map(|f| {
            let r = ranks
                .get(&f.to_uppercase())
                .copied()
                .unwrap_or(prefs.len());
            (f, r)
        })
        .collect();
    // Stable sort by rank — formats not in `prefs` keep their input
    // order (matches Python's `sorted(..., key=...)`).
    with_rank.sort_by_key(|(_, r)| *r);
    with_rank.into_iter().map(|(f, _)| f.clone()).collect()
}

/// Preferred-format-first output-format list. Matches Python's
/// `get_output_formats`: filter to a `restrict` list if provided,
/// otherwise sort with EPUB/AZW3/MOBI up front.
pub fn get_output_formats(
    preferred: Option<&str>,
    all_available: &[String],
    restrict: Option<&[String]>,
) -> Vec<String> {
    let all_upper: HashSet<String> = all_available
        .iter()
        .map(|s| s.to_uppercase())
        .filter(|s| s != "OEB") // Python explicitly discards OEB.
        .collect();
    let pfo = preferred.map(|s| s.to_uppercase()).unwrap_or_default();

    if let Some(restrict) = restrict {
        let mut fmts: Vec<String> = restrict.iter().map(|s| s.to_uppercase()).collect();
        if !pfo.is_empty() && !fmts.contains(&pfo) && all_upper.contains(&pfo) {
            fmts.push(pfo);
        }
        return fmts;
    }
    let mut fmts: Vec<String> = all_upper.into_iter().collect();
    fmts.sort_by_key(|f| match f.as_str() {
        "EPUB" => "!A".to_string(),
        "AZW3" => "!B".to_string(),
        "MOBI" => "!C".to_string(),
        _ => f.clone(),
    });
    fmts
}

pub fn get_sorted_output_formats(
    preferred: Option<&str>,
    all_available: &[String],
    restrict: Option<&[String]>,
) -> Vec<String> {
    let pfo = preferred.map(|s| s.to_uppercase()).unwrap_or_default();
    let mut fmts = get_output_formats(preferred, all_available, restrict);
    if !pfo.is_empty() {
        fmts.retain(|f| f != &pfo);
        fmts.insert(0, pfo);
    }
    fmts
}

/// The Python `OPTIONS` dict: input/pipe/output group → format
/// → list of option names. Wrapped in `OnceLock` so it's built
/// once per process and shared read-only.
pub fn options_registry() -> &'static OptionsRegistry {
    static REG: OnceLock<OptionsRegistry> = OnceLock::new();
    REG.get_or_init(build_options_registry)
}

pub struct OptionsRegistry {
    pub input: BTreeMap<&'static str, &'static [&'static str]>,
    pub pipe: BTreeMap<&'static str, &'static [&'static str]>,
    pub output: BTreeMap<&'static str, &'static [&'static str]>,
}

fn build_options_registry() -> OptionsRegistry {
    let input: &[(&str, &[&str])] = &[
        ("comic", &[
            "colors", "dont_normalize", "keep_aspect_ratio", "right2left",
            "despeckle", "no_sort", "no_process", "landscape",
            "dont_sharpen", "disable_trim", "wide", "output_format",
            "dont_grayscale", "comic_image_size", "dont_add_comic_pages_to_toc",
        ]),
        ("docx", &["docx_no_cover", "docx_no_pagebreaks_between_notes", "docx_inline_subsup"]),
        ("fb2", &["no_inline_fb2_toc"]),
        ("pdf", &[
            "no_images", "unwrap_factor", "pdf_engine",
            "pdf_header_skip", "pdf_footer_skip",
            "pdf_header_regex", "pdf_footer_regex",
        ]),
        ("rtf", &["ignore_wmf"]),
        ("txt", &[
            "paragraph_type", "formatting_type", "markdown_extensions",
            "preserve_spaces", "txt_in_remove_indents",
        ]),
    ];
    let pipe: &[(&str, &[&str])] = &[
        ("debug", &["debug_pipeline"]),
        ("heuristics", &[
            "enable_heuristics", "markup_chapter_headings",
            "italicize_common_cases", "fix_indents", "html_unwrap_factor",
            "unwrap_lines", "delete_blank_paragraphs", "format_scene_breaks",
            "replace_scene_breaks", "dehyphenate", "renumber_headings",
        ]),
        ("look_and_feel", &[
            "change_justification", "extra_css", "base_font_size",
            "font_size_mapping", "line_height", "minimum_line_height",
            "embed_font_family", "embed_all_fonts", "subset_embedded_fonts",
            "smarten_punctuation", "unsmarten_punctuation",
            "disable_font_rescaling", "insert_blank_line",
            "remove_paragraph_spacing", "remove_paragraph_spacing_indent_size",
            "insert_blank_line_size", "input_encoding", "filter_css",
            "expand_css", "asciiize", "keep_ligatures", "linearize_tables",
            "transform_css_rules", "transform_html_rules",
        ]),
        ("metadata", &["prefer_metadata_cover"]),
        ("page_setup", &[
            "margin_top", "margin_left", "margin_right", "margin_bottom",
            "input_profile", "output_profile",
        ]),
        ("search_and_replace", &[
            "search_replace", "sr1_search", "sr1_replace",
            "sr2_search", "sr2_replace", "sr3_search", "sr3_replace",
        ]),
        ("structure_detection", &[
            "chapter", "chapter_mark", "start_reading_at",
            "remove_first_image", "remove_fake_margins", "insert_metadata",
            "page_breaks_before", "add_alt_text_to_img",
        ]),
        ("toc", &[
            "level1_toc", "level2_toc", "level3_toc",
            "toc_threshold", "max_toc_links", "no_chapters_in_toc",
            "use_auto_toc", "toc_filter", "duplicate_links_in_toc",
        ]),
    ];
    let output: &[(&str, &[&str])] = &[
        ("azw3", &[
            "prefer_author_sort", "toc_title", "mobi_toc_at_start",
            "dont_compress", "no_inline_toc", "share_not_sync",
        ]),
        ("docx", &[
            "docx_page_size", "docx_custom_page_size",
            "docx_no_cover", "docx_no_toc",
            "docx_page_margin_left", "docx_page_margin_top",
            "docx_page_margin_right", "docx_page_margin_bottom",
            "preserve_cover_aspect_ratio",
        ]),
        ("epub", &[
            "dont_split_on_page_breaks", "flow_size",
            "no_default_epub_cover", "no_svg_cover",
            "epub_inline_toc", "epub_toc_at_end", "toc_title",
            "preserve_cover_aspect_ratio", "epub_flatten",
            "epub_version", "epub_max_image_size",
        ]),
        ("kepub", &[
            "dont_split_on_page_breaks", "flow_size",
            "kepub_max_image_size", "kepub_prefer_justification",
            "kepub_affect_hyphenation", "kepub_disable_hyphenation",
            "kepub_hyphenation_min_chars",
            "kepub_hyphenation_min_chars_before",
            "kepub_hyphenation_min_chars_after",
            "kepub_hyphenation_limit_lines",
        ]),
        ("fb2", &["sectionize", "fb2_genre"]),
        ("htmlz", &["htmlz_css_type", "htmlz_class_style", "htmlz_title_filename"]),
        ("lrf", &[
            "wordspace", "header", "header_format", "minimum_indent",
            "serif_family", "render_tables_as_images", "sans_family",
            "mono_family", "text_size_multiplier_for_rendered_tables",
            "autorotation", "header_separation", "minimum_indent",
        ]),
        ("mobi", &[
            "prefer_author_sort", "toc_title", "mobi_keep_original_images",
            "mobi_ignore_margins", "mobi_toc_at_start", "dont_compress",
            "no_inline_toc", "share_not_sync", "personal_doc",
            "mobi_file_type",
        ]),
        ("pdb", &["format", "inline_toc", "pdb_output_encoding"]),
        ("pdf", &[
            "use_profile_size", "paper_size", "custom_size", "pdf_hyphenate",
            "preserve_cover_aspect_ratio", "pdf_serif_family", "unit",
            "pdf_sans_family", "pdf_mono_family", "pdf_standard_font",
            "pdf_default_font_size", "pdf_mono_font_size", "pdf_page_numbers",
            "pdf_footer_template", "pdf_header_template", "pdf_add_toc",
            "toc_title", "pdf_page_margin_left", "pdf_page_margin_top",
            "pdf_page_margin_right", "pdf_page_margin_bottom", "pdf_no_cover",
            "pdf_use_document_margins", "pdf_page_number_map", "pdf_odd_even_offset",
        ]),
        ("pml", &["inline_toc", "full_image_depth", "pml_output_encoding"]),
        ("rb", &["inline_toc"]),
        ("snb", &[
            "snb_insert_empty_line", "snb_dont_indent_first_line",
            "snb_hide_chapter_name", "snb_full_screen",
        ]),
        ("txt", &[
            "newline", "max_line_length", "force_max_line_length",
            "inline_toc", "txt_output_formatting", "keep_links",
            "keep_image_references", "keep_color", "txt_output_encoding",
        ]),
    ];
    let mut reg = OptionsRegistry {
        input: input.iter().copied().collect(),
        pipe: pipe.iter().copied().collect(),
        output: output.iter().copied().collect(),
    };
    // Python: `OPTIONS['output']['txtz'] = OPTIONS['output']['txt']`.
    if let Some(txt) = reg.output.get("txt").copied() {
        reg.output.insert("txtz", txt);
    }
    reg
}

/// Resolver callback: given a format string, return the plugin's
/// full canonical name (e.g. `"epub"` → `"epub_input"`) or `None`
/// if unknown. Passing the callback avoids a hard dependency on the
/// live plugin registry (`calibre_customize::ui`).
pub type PluginNameResolver<'a> = dyn Fn(&str) -> Option<String> + 'a;

pub fn options_for_input_fmt<'a>(
    fmt: &str,
    resolve: &'a PluginNameResolver<'a>,
) -> (Option<String>, Vec<&'static str>) {
    options_for_fmt(fmt, resolve, &options_registry().input)
}

pub fn options_for_output_fmt<'a>(
    fmt: &str,
    resolve: &'a PluginNameResolver<'a>,
) -> (Option<String>, Vec<&'static str>) {
    options_for_fmt(fmt, resolve, &options_registry().output)
}

fn options_for_fmt<'a>(
    fmt: &str,
    resolve: &'a PluginNameResolver<'a>,
    map: &BTreeMap<&'static str, &'static [&'static str]>,
) -> (Option<String>, Vec<&'static str>) {
    let fmt = fmt.to_ascii_lowercase();
    let Some(full_name) = resolve(&fmt) else {
        return (None, Vec::new());
    };
    // Python: `name = full_name.rpartition('_')[0]` — everything
    // before the last `_`, or the empty string if no `_`.
    let name = match full_name.rsplit_once('_') {
        Some((prefix, _)) => prefix.to_string(),
        None => String::new(),
    };
    let opts = map.get(name.as_str()).map(|s| s.to_vec()).unwrap_or_default();
    (Some(full_name), opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serialize_prefixes_with_json_marker() {
        let mut r = GuiRecommendations::new();
        r.set("a", json!(1));
        let bytes = r.serialize();
        assert!(bytes.starts_with(JSON_PREFIX));
    }

    #[test]
    fn serialize_deserialize_roundtrip() {
        let mut r = GuiRecommendations::new();
        r.set("keep_ligatures", json!(true));
        r.set("base_font_size", json!(12.5));
        r.set("toc_title", json!("Contents"));
        let bytes = r.serialize();

        let mut r2 = GuiRecommendations::new();
        r2.deserialize(&bytes);
        assert_eq!(r2.get("keep_ligatures"), Some(&json!(true)));
        assert_eq!(r2.get("base_font_size"), Some(&json!(12.5)));
        assert_eq!(r2.get("toc_title"), Some(&json!("Contents")));
        assert_eq!(r2.len(), 3);
    }

    #[test]
    fn deserialize_accepts_bare_json_without_prefix() {
        // Defensive: some older Python code paths wrote bare JSON.
        let mut r = GuiRecommendations::new();
        r.deserialize(br#"{"foo": 42}"#);
        assert_eq!(r.get("foo"), Some(&json!(42)));
    }

    #[test]
    fn deserialize_empty_is_noop() {
        let mut r = GuiRecommendations::new();
        r.deserialize(b"");
        assert!(r.is_empty());
        r.deserialize(b"json:");
        assert!(r.is_empty());
    }

    #[test]
    fn deserialize_malformed_is_silently_ignored() {
        // Python's `try: ... except: pass`. Preserve the fail-safe
        // behavior.
        let mut r = GuiRecommendations::new();
        r.deserialize(b"json:not valid json");
        assert!(r.is_empty());
    }

    #[test]
    fn options_registry_has_expected_input_format_options() {
        let reg = options_registry();
        assert!(reg.input.contains_key("comic"));
        assert!(reg.input.contains_key("docx"));
        assert!(reg.input.contains_key("pdf"));
    }

    #[test]
    fn options_registry_has_txtz_aliased_to_txt() {
        // Python: OPTIONS['output']['txtz'] = OPTIONS['output']['txt']
        let reg = options_registry();
        let txt = reg.output.get("txt").copied().unwrap();
        let txtz = reg.output.get("txtz").copied().unwrap();
        assert_eq!(txt, txtz);
    }

    #[test]
    fn options_for_input_fmt_strips_plugin_suffix() {
        // Plugin name is e.g. "comic_input"; the OPTIONS key is "comic".
        let resolver = |fmt: &str| match fmt {
            "cbz" | "cbr" => Some("comic_input".to_string()),
            _ => None,
        };
        let (name, opts) = options_for_input_fmt("CBZ", &resolver);
        assert_eq!(name.as_deref(), Some("comic_input"));
        assert!(opts.contains(&"landscape"));
    }

    #[test]
    fn options_for_output_fmt_unknown_format_returns_none() {
        let resolver = |_: &str| None;
        let (name, opts) = options_for_output_fmt("xyz", &resolver);
        assert!(name.is_none());
        assert!(opts.is_empty());
    }

    #[test]
    fn sort_formats_by_preference_ranks_known_first() {
        let prefs = vec!["EPUB".to_string(), "MOBI".to_string()];
        let formats = vec!["PDF".to_string(), "MOBI".to_string(), "EPUB".to_string()];
        let sorted = sort_formats_by_preference(&formats, &prefs);
        assert_eq!(sorted, vec!["EPUB", "MOBI", "PDF"]);
    }

    #[test]
    fn sort_formats_by_preference_preserves_order_of_unranked() {
        let prefs: Vec<String> = Vec::new();
        let formats = vec!["PDF".to_string(), "MOBI".to_string(), "EPUB".to_string()];
        let sorted = sort_formats_by_preference(&formats, &prefs);
        assert_eq!(sorted, vec!["PDF", "MOBI", "EPUB"]);
    }

    #[test]
    fn get_output_formats_puts_epub_azw3_mobi_first_by_default() {
        let all = vec!["PDF".to_string(), "MOBI".to_string(), "EPUB".to_string(), "AZW3".to_string()];
        let fmts = get_output_formats(None, &all, None);
        assert_eq!(&fmts[0..3], &["EPUB", "AZW3", "MOBI"]);
    }

    #[test]
    fn get_output_formats_drops_oeb() {
        let all = vec!["EPUB".to_string(), "OEB".to_string()];
        let fmts = get_output_formats(None, &all, None);
        assert!(!fmts.iter().any(|f| f == "OEB"));
    }

    #[test]
    fn get_output_formats_respects_restrict() {
        let all = vec!["PDF".to_string(), "MOBI".to_string(), "EPUB".to_string()];
        let restrict = vec!["EPUB".to_string()];
        let fmts = get_output_formats(Some("MOBI"), &all, Some(&restrict));
        // Restricted set + preferred (if available) appended.
        assert_eq!(fmts, vec!["EPUB", "MOBI"]);
    }

    #[test]
    fn get_sorted_output_formats_puts_preferred_first() {
        let all = vec!["PDF".to_string(), "EPUB".to_string(), "AZW3".to_string()];
        let fmts = get_sorted_output_formats(Some("PDF"), &all, None);
        assert_eq!(fmts[0], "PDF");
    }

    #[test]
    fn save_load_defaults_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("CALIBRE_CONFIG_DIRECTORY", tmp.path());
        let mut r = GuiRecommendations::new();
        r.set("toc_title", json!("Contents"));
        save_defaults("epub_output", &r).unwrap();
        let loaded = load_defaults("epub_output").unwrap();
        assert_eq!(loaded.get("toc_title"), Some(&json!("Contents")));
        std::env::remove_var("CALIBRE_CONFIG_DIRECTORY");
    }

    #[test]
    fn sanitize_config_name_strips_hostile_chars() {
        assert_eq!(sanitize_config_name("epub/../evil"), "epub_.._evil");
        assert_eq!(sanitize_config_name("with:colon"), "with_colon");
        assert_eq!(sanitize_config_name(""), "_");
        assert_eq!(sanitize_config_name("   "), "_");
    }
}
