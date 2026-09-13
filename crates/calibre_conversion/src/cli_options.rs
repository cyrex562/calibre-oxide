//! Real CLI option parsing for `ebook-convert`, populating
//! [`calibre_ebooks::conversion::options::ConversionOptions`] -- the
//! remaining real half of issue #126, once
//! [`crate::cli_helpers::check_command_line_options`] (#20) handles the
//! positional input/output arguments and issue #686 gave
//! `Plumber::run()` a real pipeline for these options to actually
//! control.
//!
//! # A hand-written `clap::Parser` struct, not a dynamic
//! `OptionRecommendation` registry
//!
//! Real upstream's `option_recommendation_to_cli_option`
//! (`cli.py:85-129`) dynamically builds an `optparse` parser at runtime
//! from every input/output plugin's own declared
//! `OptionRecommendation` list -- a real design need in Python, where
//! plugins are dynamically discovered and each one contributes its own
//! options. In this port every input/output plugin is a fixed,
//! compile-time-known set (no dynamic plugin loading exists or is
//! planned), so a statically-typed `clap::Parser` struct with the same
//! real option names, defaults, and CLI-flag semantics is the natural
//! Rust equivalent -- not a narrowing, just the idiomatic shape for a
//! statically-typed language. This mirrors the same kind of adaptation
//! made throughout this port (e.g. enums in place of Python class
//! hierarchies).
//!
//! # Real flag-naming quirks preserved
//!
//! - `remove_fake_margins`'s real recommended value is `True` (see
//!   `DEFAULT_TRUE_OPTIONS`, `cli.py:126`), so its real CLI switch is
//!   the *negating* `--disable-remove-fake-margins`, not a plain
//!   `--remove-fake-margins` (which would be a no-op, since it's
//!   already on). Ported as `disable_remove_fake_margins: bool`.
//! - `verbose` uses `action='count'` (`cli.py:99`), i.e. repeatable
//!   `-v` flags -- ported via `clap::ArgAction::Count`.
//!
//! # Real, disclosed narrowing: only options with a real destination
//!
//! Real `HEURISTIC_OPTIONS` (`markup_chapter_headings`,
//! `italicize_common_cases`, `fix_indents`, `html_unwrap_factor`,
//! `unwrap_lines`, `delete_blank_paragraphs`, `format_scene_breaks`,
//! `dehyphenate`, `renumber_headings`, `replace_scene_breaks` --
//! `cli_helpers::HEURISTIC_OPTIONS`, from issue #20) control
//! `HTMLPreProcessor`, which isn't wired into `Plumber::run()` at all
//! yet -- exposing CLI flags for them would be pure theater (parsed
//! but silently ignored). Not included here; add them once
//! `HTMLPreProcessor` itself is real and wired in.

use calibre_ebooks::conversion::options::ConversionOptions;
use clap::Parser;

/// Real conversion-pipeline options, matching
/// [`ConversionOptions`]/`StructureOptions`'s own real upstream
/// default values (unset flags fall back to
/// [`ConversionOptions::default`], not to a second, possibly-drifting
/// set of defaults declared here).
#[derive(Parser, Debug, Default)]
#[command(name = "ebook-convert", disable_help_flag = false)]
pub struct ConvertArgs {
    // --- Font / CSS ---
    #[arg(long)]
    pub base_font_size: Option<f64>,
    #[arg(long, value_delimiter = ',')]
    pub font_size_mapping: Option<Vec<f64>>,
    #[arg(long)]
    pub disable_font_rescaling: bool,
    #[arg(long)]
    pub minimum_line_height: Option<f64>,
    #[arg(long)]
    pub line_height: Option<f64>,
    #[arg(long)]
    pub embed_all_fonts: bool,
    #[arg(long)]
    pub subset_embedded_fonts: bool,
    #[arg(long)]
    pub extra_css: Option<String>,

    // --- Margins ---
    #[arg(long)]
    pub margin_top: Option<f64>,
    #[arg(long)]
    pub margin_bottom: Option<f64>,
    #[arg(long)]
    pub margin_left: Option<f64>,
    #[arg(long)]
    pub margin_right: Option<f64>,
    /// Real recommended value is `True`; the real CLI switch negates
    /// it -- see this module's own doc.
    #[arg(long)]
    pub disable_remove_fake_margins: bool,
    #[arg(long, value_parser = ["left", "justify", "original"])]
    pub change_justification: Option<String>,

    // --- Paragraphs ---
    #[arg(long)]
    pub remove_paragraph_spacing: bool,
    #[arg(long)]
    pub remove_paragraph_spacing_indent_size: Option<f64>,
    #[arg(long)]
    pub insert_blank_line: bool,
    #[arg(long)]
    pub insert_blank_line_size: Option<f64>,

    // --- Structure / metadata / misc ---
    #[arg(long)]
    pub linearize_tables: bool,
    #[arg(long)]
    pub add_alt_text_to_img: bool,
    #[arg(long)]
    pub unsmarten_punctuation: bool,
    #[arg(long)]
    pub remove_first_image: bool,
    #[arg(long)]
    pub insert_metadata: bool,
    #[arg(long)]
    pub prefer_metadata_cover: bool,
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,
    #[arg(long)]
    pub pretty_print: bool,

    // --- Table of contents / structure detection ---
    #[arg(long)]
    pub use_auto_toc: bool,
    #[arg(long)]
    pub no_chapters_in_toc: bool,
    #[arg(long)]
    pub toc_threshold: Option<usize>,
    #[arg(long)]
    pub max_toc_links: Option<usize>,
    #[arg(long)]
    pub duplicate_links_in_toc: bool,
    #[arg(long)]
    pub toc_filter: Option<String>,
    #[arg(long)]
    pub page_breaks_before: Option<String>,
    #[arg(long)]
    pub chapter: Option<String>,
    #[arg(long, value_parser = ["pagebreak", "rule", "both", "none"])]
    pub chapter_mark: Option<String>,
    #[arg(long)]
    pub level1_toc: Option<String>,
    #[arg(long)]
    pub level2_toc: Option<String>,
    #[arg(long)]
    pub level3_toc: Option<String>,
    #[arg(long)]
    pub start_reading_at: Option<String>,
}

impl ConvertArgs {
    /// Applies every explicitly-set flag onto a fresh
    /// [`ConversionOptions::default`] (the real upstream default
    /// values), leaving every unset field at its real default.
    pub fn into_conversion_options(self) -> ConversionOptions {
        let mut o = ConversionOptions::default();

        if let Some(v) = self.base_font_size {
            o.base_font_size = v;
        }
        if let Some(v) = self.font_size_mapping {
            o.font_size_mapping = Some(v);
        }
        o.disable_font_rescaling = self.disable_font_rescaling;
        if let Some(v) = self.minimum_line_height {
            o.minimum_line_height = v;
        }
        if let Some(v) = self.line_height {
            o.line_height = v;
        }
        o.embed_all_fonts = self.embed_all_fonts;
        o.subset_embedded_fonts = self.subset_embedded_fonts;
        if self.extra_css.is_some() {
            o.extra_css = self.extra_css;
        }

        if let Some(v) = self.margin_top {
            o.margin_top = v;
        }
        if let Some(v) = self.margin_bottom {
            o.margin_bottom = v;
        }
        if let Some(v) = self.margin_left {
            o.margin_left = v;
        }
        if let Some(v) = self.margin_right {
            o.margin_right = v;
        }
        if self.disable_remove_fake_margins {
            o.remove_fake_margins = false;
        }
        if self.change_justification.is_some() {
            o.change_justification = self.change_justification;
        }

        o.remove_paragraph_spacing = self.remove_paragraph_spacing;
        if let Some(v) = self.remove_paragraph_spacing_indent_size {
            o.remove_paragraph_spacing_indent_size = v;
        }
        o.insert_blank_line = self.insert_blank_line;
        if let Some(v) = self.insert_blank_line_size {
            o.insert_blank_line_size = v;
        }

        o.linearize_tables = self.linearize_tables;
        o.add_alt_text_to_img = self.add_alt_text_to_img;
        o.unsmarten_punctuation = self.unsmarten_punctuation;
        o.remove_first_image = self.remove_first_image;
        o.insert_metadata = self.insert_metadata;
        o.prefer_metadata_cover = self.prefer_metadata_cover;
        o.verbose = self.verbose;
        o.pretty_print = self.pretty_print;

        o.structure.use_auto_toc = self.use_auto_toc;
        o.structure.no_chapters_in_toc = self.no_chapters_in_toc;
        if let Some(v) = self.toc_threshold {
            o.structure.toc_threshold = v;
        }
        if let Some(v) = self.max_toc_links {
            o.structure.max_toc_links = v;
        }
        o.structure.duplicate_links_in_toc = self.duplicate_links_in_toc;
        if self.toc_filter.is_some() {
            o.structure.toc_filter = self.toc_filter;
        }
        if self.page_breaks_before.is_some() {
            o.structure.page_breaks_before = self.page_breaks_before;
        }
        if self.chapter.is_some() {
            o.structure.chapter = self.chapter;
        }
        if let Some(v) = self.chapter_mark {
            o.structure.chapter_mark = v;
        }
        if self.level1_toc.is_some() {
            o.structure.level1_toc = self.level1_toc;
        }
        if self.level2_toc.is_some() {
            o.structure.level2_toc = self.level2_toc;
        }
        if self.level3_toc.is_some() {
            o.structure.level3_toc = self.level3_toc;
        }
        if self.start_reading_at.is_some() {
            o.structure.start_reading_at = self.start_reading_at;
        }

        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> ConvertArgs {
        let mut v = vec!["ebook-convert".to_string()];
        v.extend(args.iter().map(|s| s.to_string()));
        ConvertArgs::parse_from(v)
    }

    #[test]
    fn no_flags_produces_real_upstream_defaults() {
        let opts = parse(&[]).into_conversion_options();
        let defaults = ConversionOptions::default();
        assert_eq!(opts.base_font_size, defaults.base_font_size);
        assert_eq!(opts.remove_fake_margins, defaults.remove_fake_margins);
        assert!(!opts.embed_all_fonts);
    }

    #[test]
    fn boolean_flags_turn_on_options_that_default_off() {
        let opts = parse(&["--embed-all-fonts", "--subset-embedded-fonts", "--add-alt-text-to-img"]).into_conversion_options();
        assert!(opts.embed_all_fonts);
        assert!(opts.subset_embedded_fonts);
        assert!(opts.add_alt_text_to_img);
    }

    #[test]
    fn disable_remove_fake_margins_negates_the_default_true_option() {
        let opts = parse(&[]).into_conversion_options();
        assert!(opts.remove_fake_margins);
        let opts = parse(&["--disable-remove-fake-margins"]).into_conversion_options();
        assert!(!opts.remove_fake_margins);
    }

    #[test]
    fn numeric_and_list_options_parse_correctly() {
        let opts = parse(&["--base-font-size", "16.5", "--font-size-mapping", "5,7,9,12,14,18,22,26,30"]).into_conversion_options();
        assert_eq!(opts.base_font_size, 16.5);
        assert_eq!(opts.font_size_mapping, Some(vec![5.0, 7.0, 9.0, 12.0, 14.0, 18.0, 22.0, 26.0, 30.0]));
    }

    #[test]
    fn verbose_counts_repeated_short_flags() {
        let opts = parse(&["-vvv"]).into_conversion_options();
        assert_eq!(opts.verbose, 3);
    }

    #[test]
    fn structure_options_thread_through() {
        let opts = parse(&["--use-auto-toc", "--toc-threshold", "3", "--chapter-mark", "rule"]).into_conversion_options();
        assert!(opts.structure.use_auto_toc);
        assert_eq!(opts.structure.toc_threshold, 3);
        assert_eq!(opts.structure.chapter_mark, "rule");
    }

    #[test]
    fn invalid_chapter_mark_choice_is_rejected() {
        let mut v = vec!["ebook-convert".to_string(), "--chapter-mark".to_string(), "bogus".to_string()];
        let result = ConvertArgs::try_parse_from(std::mem::take(&mut v));
        assert!(result.is_err());
    }
}
