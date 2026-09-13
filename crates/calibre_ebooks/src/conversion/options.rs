//! Real default values for the conversion-pipeline options every
//! `oeb::transforms::*` stage [`crate::conversion::plumber::Plumber::run_transforms`]
//! wires in reads.
//!
//! Not a generic `OptionRecommendation` registry (issue #126's own
//! scope -- the dynamic plugin-driven CLI assembly layer) -- a
//! concrete, statically-typed struct carrying the SAME real default
//! values `plumber.py`'s own `pipeline_options` declares
//! (`old_src/src/calibre/ebooks/conversion/plumber.py`), so every
//! transform this crate wires in runs with faithful upstream default
//! behavior even before any CLI-driven customization exists. A future
//! #126 CLI would set fields on this same struct instead of
//! introducing a second, parallel options type.
//!
//! # A few individual transform modules' own `Default` impls are NOT
//! upstream's real CLI defaults
//!
//! Found while wiring this in: [`crate::oeb::transforms::flatcss::FlattenContext::default`]
//! (`dpi=96`, `disable_font_rescaling=true`, margins=`-1.0`) and
//! [`crate::oeb::transforms::jacket::JacketOptions::default`]
//! (`insert_metadata=true`) are each that *module's own* convenience
//! default for standalone unit testing, not upstream's real
//! `OptionRecommendation.recommended_value` (`dpi=100`,
//! `disable_font_rescaling=false`, margins=`5.0`,
//! `insert_metadata=false`). [`ConversionOptions`]'s own conversion
//! methods build each transform's options type explicitly with the
//! real upstream values rather than delegating to that type's own
//! `Default`.
//!
//! # Real, disclosed narrowing: no named output/input profile catalog
//!
//! Real upstream ships ~20 named device profiles (kindle, ipad, nook,
//! generic eink, ...), each overriding `fbase`/`fkey`/`screen_size`/
//! `dpi`/etc. Only the **default** profile's own values (`fbase=12.0`,
//! `fkey=[5,7,9,12,13.5,17,20,22,24]`, `dpi=100.0`) are hardcoded here
//! -- matching what a real `ebook-convert` invocation with no
//! `--output-profile`/`--input-profile` flag actually uses. Named
//! profile selection is a real, separate gap (needs porting the whole
//! `customize/profiles.py` catalog), not attempted here.

use crate::oeb::transforms::flatcss::{FlattenContext, FlattenerOptions};
use crate::oeb::transforms::jacket::JacketOptions;
use crate::oeb::transforms::structure::StructureOptions;

/// The default output profile's own real constants
/// (`customize/profiles.py`'s base `Plugin`/`OutputProfile` classes).
const DEFAULT_FBASE: f64 = 12.0;
const DEFAULT_FKEY: [f64; 9] = [5.0, 7.0, 9.0, 12.0, 13.5, 17.0, 20.0, 22.0, 24.0];
const DEFAULT_DPI: f64 = 100.0;

#[derive(Debug, Clone)]
pub struct ConversionOptions {
    // Font / CSS (flatcss.py's CSSFlattener inputs)
    /// `0.0` means "auto from the output profile" (real default).
    pub base_font_size: f64,
    pub font_size_mapping: Option<Vec<f64>>,
    pub disable_font_rescaling: bool,
    pub minimum_line_height: f64,
    /// `0.0` means "no line-height override" (real default).
    pub line_height: f64,
    pub embed_all_fonts: bool,
    pub subset_embedded_fonts: bool,
    /// Real upstream also accepts a *path* to a CSS file here (read and
    /// substituted for its own value if it exists on disk) -- not
    /// ported, since the default (`None`) never exercises that branch.
    pub extra_css: Option<String>,

    // Margins (page_margin.py)
    pub margin_top: f64,
    pub margin_bottom: f64,
    pub margin_left: f64,
    pub margin_right: f64,
    pub remove_fake_margins: bool,
    pub change_justification: Option<String>,

    // Paragraph spacing
    pub remove_paragraph_spacing: bool,
    pub remove_paragraph_spacing_indent_size: f64,
    pub insert_blank_line: bool,
    pub insert_blank_line_size: f64,

    // Structure / metadata / misc pipeline stages
    pub linearize_tables: bool,
    pub add_alt_text_to_img: bool,
    pub unsmarten_punctuation: bool,
    pub remove_first_image: bool,
    pub insert_metadata: bool,
    pub prefer_metadata_cover: bool,
    pub verbose: u8,
    pub pretty_print: bool,

    pub structure: StructureOptions,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        ConversionOptions {
            base_font_size: 0.0,
            font_size_mapping: None,
            disable_font_rescaling: false,
            minimum_line_height: 120.0,
            line_height: 0.0,
            embed_all_fonts: false,
            subset_embedded_fonts: false,
            extra_css: None,

            margin_top: 5.0,
            margin_bottom: 5.0,
            margin_left: 5.0,
            margin_right: 5.0,
            remove_fake_margins: true,
            change_justification: None,

            remove_paragraph_spacing: false,
            remove_paragraph_spacing_indent_size: 1.5,
            insert_blank_line: false,
            insert_blank_line_size: 0.5,

            linearize_tables: false,
            add_alt_text_to_img: false,
            unsmarten_punctuation: false,
            remove_first_image: false,
            insert_metadata: false,
            prefer_metadata_cover: false,
            verbose: 0,
            pretty_print: false,

            structure: StructureOptions::default(),
        }
    }
}

impl ConversionOptions {
    /// Port of the fbase/fkey/lineh/`needs_old_markup` resolution
    /// inlined in `Plumber.run` just before constructing
    /// `CSSFlattener`. `needs_old_markup` faithfully covers the `lit`
    /// case (`self.output_plugin.file_type == 'lit'`); the MOBI-format
    /// `mobi_file_type == 'old'` half of the real condition depends on
    /// an output-plugin-specific option this port doesn't have yet, so
    /// `output_ext == "mobi"` never triggers it here (a real, narrow
    /// gap -- MOBI output already defaults to the newer KF8 markup in
    /// this port regardless).
    pub fn flattener_options(&self, output_ext: &str) -> FlattenerOptions {
        let needs_old_markup = output_ext == "lit";
        let fbase = if self.base_font_size > 1e-4 { self.base_font_size } else { DEFAULT_FBASE };
        let fkey = self.font_size_mapping.clone().unwrap_or_else(|| DEFAULT_FKEY.to_vec());
        let lineh = if self.line_height > 1e-4 { Some(self.line_height) } else { None };
        FlattenerOptions { fbase: Some(fbase), fkey: Some(fkey), lineh, unfloat: needs_old_markup, untable: needs_old_markup }
    }

    /// Port of the rest of `CSSFlattener`'s construction context.
    pub fn flatten_context(&self, output_ext: &str) -> FlattenContext {
        let page_break_on_body = matches!(output_ext, "mobi" | "azw" | "azw3" | "prc" | "lit");
        FlattenContext {
            base_font_size: DEFAULT_FBASE,
            dest_base_font_size: if self.base_font_size > 1e-4 { self.base_font_size } else { DEFAULT_FBASE },
            dpi: DEFAULT_DPI,
            margin_left: self.margin_left,
            margin_right: self.margin_right,
            margin_top: self.margin_top,
            margin_bottom: self.margin_bottom,
            change_justification: self.change_justification.clone(),
            disable_font_rescaling: self.disable_font_rescaling,
            minimum_line_height: self.minimum_line_height,
            remove_paragraph_spacing: self.remove_paragraph_spacing,
            remove_paragraph_spacing_indent_size: self.remove_paragraph_spacing_indent_size,
            insert_blank_line: self.insert_blank_line,
            insert_blank_line_size: self.insert_blank_line_size,
            page_break_on_body,
            user_css: self.extra_css.clone().unwrap_or_default(),
            output_profile_is_kindle: false,
        }
    }

    /// Port of `Jacket`/`RemoveFirstImage`'s shared options, using the
    /// default output profile's own real `ratings_char`/
    /// `empty_ratings_char`/`short_name` values.
    pub fn jacket_options(&self) -> JacketOptions {
        JacketOptions {
            remove_first_image: self.remove_first_image,
            insert_metadata: self.insert_metadata,
            smarten_punctuation: false,
            ratings_char: "*".to_string(),
            empty_ratings_char: " ".to_string(),
            output_profile_short_name: "default".to_string(),
            pubdate_format: "MMM yyyy".to_string(),
            timestamp_format: "dd MMM yyyy".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_real_upstream_recommended_values() {
        let o = ConversionOptions::default();
        assert_eq!(o.base_font_size, 0.0);
        assert!(!o.embed_all_fonts);
        assert!(!o.subset_embedded_fonts);
        assert!(o.remove_fake_margins, "real upstream's own default is True, unlike FlattenContext's own unrelated test-convenience Default");
        assert_eq!(o.margin_top, 5.0);
        assert!(!o.insert_metadata, "real upstream's own default is False, unlike JacketOptions's own unrelated test-convenience Default");
    }

    #[test]
    fn flattener_options_falls_back_to_the_default_profiles_fbase_and_fkey() {
        let o = ConversionOptions::default();
        let f = o.flattener_options("epub");
        assert_eq!(f.fbase, Some(12.0));
        assert_eq!(f.fkey, Some(vec![5.0, 7.0, 9.0, 12.0, 13.5, 17.0, 20.0, 22.0, 24.0]));
        assert_eq!(f.lineh, None);
        assert!(!f.unfloat);
    }

    #[test]
    fn flattener_options_honors_an_explicit_base_font_size() {
        let mut o = ConversionOptions::default();
        o.base_font_size = 16.0;
        let f = o.flattener_options("epub");
        assert_eq!(f.fbase, Some(16.0));
    }

    #[test]
    fn flatten_context_sets_page_break_on_body_for_mobi_like_formats() {
        let o = ConversionOptions::default();
        assert!(o.flatten_context("mobi").page_break_on_body);
        assert!(o.flatten_context("lit").page_break_on_body);
        assert!(!o.flatten_context("epub").page_break_on_body);
    }
}
