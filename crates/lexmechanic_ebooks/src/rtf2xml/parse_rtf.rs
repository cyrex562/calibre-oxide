//! Port of `old_src/src/calibre/ebooks/rtf2xml/ParseRtf.py`
//! (`ParseRtf`), the pipeline orchestrator tying every other pass in
//! this module together into one sequential run, in exactly Python's
//! own call order.
//!
//! [`parse_rtf`] is the pure core: raw RTF *bytes* in (this is the one
//! place in the whole `rtf2xml` port that accepts bytes rather than an
//! already-decoded `&str`, since the input hasn't been through
//! [`super::line_endings::fix_line_endings`]'s illegal-byte cleanup
//! yet), a fully-converted XML `String` out, with no real file I/O.
//! [`parse_rtf_file`] is the thin real-I/O wrapper: reads the input
//! file, calls [`parse_rtf`], and writes the result (plus any
//! extracted `\pict` sidecar) via [`super::output`].
//!
//! # What's *not* here
//!
//! - The temp-file / [`super::copy`] / rename dance Python's own
//!   `ParseRtf` performs around *every single* intervening pass (each
//!   one reopens the same temp file) is exactly the "pipeline
//!   plumbing" every other module in this port already documents as
//!   out of scope -- this port threads a `String` through instead.
//! - `__bracket_match`'s `run_level > 2`-gated
//!   [`super::check_brackets`] sanity check after nearly every pass is
//!   a pure debugging aid (stderr-only, never changes control flow or
//!   the transformed content) -- omitted for the same reason. Callers
//!   who want it can call [`super::check_brackets::check_brackets`]
//!   themselves on any intermediate content.
//! - `char_data` (a constructor parameter naming an external
//!   character-map file) has no counterpart: [`super::hex_2_utf8`]'s
//!   own port has no `char_file` parameter at all -- its character
//!   data is the ported [`super::char_set`] table, not an external
//!   file.
//!
//! # The encoding-diagnosis fallback
//!
//! Python's `process_tokens_obj.process_tokens()` call is wrapped in a
//! `try`/`except InvalidRtfException`: on failure, it constructs a
//! *second* `DefaultEncoding` (this time with `check_raw=True`,
//! scanning the *raw* RTF bytes rather than tokenized content) purely
//! to build a richer diagnostic -- checking whether the raw bytes
//! decode cleanly under the guessed codepage, and appending a
//! "does not appear to be correctly encoded" hint if not -- before
//! re-raising. [`parse_rtf`] reproduces this: on
//! [`super::process_tokens::process_tokens`] failure, it runs the same
//! two-step diagnosis and returns [`ParseRtfError::InvalidRtf`] with
//! the enriched message. If the diagnosis machinery itself fails (an
//! unrecognized encoding label), this port falls back to the
//! *original* process-tokens error rather than propagating a second,
//! more confusing failure -- a defensive choice with no Python
//! equivalent to preserve, since Python's own `LookupError` there
//! would just propagate a completely different, undocumented
//! exception type.

use std::fs;
use std::path::Path;

use thiserror::Error;

use super::convert_to_tags::ConvertToTagsOptions;
use super::default_encoding::DefaultEncoding;
use super::hex_2_utf8::{AreaToConvert, Hex2Utf8};
use super::make_lists::MakeListsOptions;
use super::old_rtf;
use super::output::{self, OutputOptions};
use super::{
    add_brackets, body_styles, colors, combine_borders, convert_to_tags, delete_info, field_strings, fields_large,
    fields_small, fonts, footnote, group_borders, group_styles, header, headings_to_sections, hex_2_utf8, info,
    inline, line_endings, list_numbers, make_lists, paragraph_def, paragraphs, pict, preamble_div, preamble_rest,
    process_tokens, sections, styles, table, table_info, tokenize,
};

/// Every failure [`parse_rtf`] can return. Most variants wrap a
/// sub-pass's own error type verbatim (`#[from]`); see each sub-pass's
/// own module for what triggers it.
#[derive(Debug, Error)]
pub enum ParseRtfError {
    /// Port of `InvalidRtfException`, raised (with an enriched
    /// message) when [`super::process_tokens::process_tokens`] fails
    /// -- see this module's own doc for the encoding-diagnosis
    /// fallback that builds the message.
    #[error("{0}")]
    InvalidRtf(String),
    /// Port of the `run_level > 5`-gated raise when old-style RTF is
    /// detected.
    #[error("Older RTF\nrun_level is \"{0}\"\n")]
    OlderRtfTooStrict(u32),
    #[error(transparent)]
    Tokenize(#[from] tokenize::InvalidUnicodeCodePoint),
    #[error(transparent)]
    DeleteInfo(#[from] delete_info::DeleteInfoError),
    #[error(transparent)]
    Fonts(#[from] fonts::FontsError),
    #[error(transparent)]
    Colors(#[from] colors::ColorsError),
    #[error(transparent)]
    Styles(#[from] styles::StylesError),
    #[error(transparent)]
    Info(#[from] info::InfoError),
    #[error(transparent)]
    PreambleDiv(#[from] preamble_div::PreambleDivError),
    #[error(transparent)]
    FieldStrings(#[from] field_strings::FieldStringsError),
    #[error(transparent)]
    Sections(#[from] sections::SectionsError),
    #[error(transparent)]
    ParagraphDef(#[from] paragraph_def::ParagraphDefError),
    #[error(transparent)]
    BodyStyles(#[from] body_styles::BodyStylesError),
    #[error(transparent)]
    TableInfo(#[from] table_info::TableInfoError),
    #[error(transparent)]
    GroupStyles(#[from] group_styles::GroupStylesError),
    #[error(transparent)]
    GroupBorders(#[from] group_borders::GroupBordersError),
    #[error(transparent)]
    Inline(#[from] inline::InlineError),
    #[error(transparent)]
    Hex2Utf8(#[from] hex_2_utf8::Hex2Utf8Error),
    #[error(transparent)]
    ConvertToTags(#[from] convert_to_tags::ConvertToTagsError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Output(#[from] output::OutputError),
}

pub type Result<T> = std::result::Result<T, ParseRtfError>;

/// Port of `ParseRtf.__init__`'s parameters (minus `in_file`/`deb_dir`/
/// `char_data`/`out_file`/`out_dir`, which [`parse_rtf`]/
/// [`parse_rtf_file`] take directly rather than folding into this
/// struct -- see this module's own doc for `char_data`, and `deb_dir`
/// for why: it only ever gates the omitted debug-copy plumbing).
#[derive(Debug, Clone)]
pub struct ParseRtfOptions {
    pub dtd_path: Option<String>,
    pub convert_symbol: bool,
    pub convert_wingdings: bool,
    pub convert_zapf: bool,
    pub convert_caps: bool,
    pub run_level: u32,
    pub indent: bool,
    pub replace_illegals: bool,
    pub form_lists: bool,
    pub headings_to_sections: bool,
    pub group_styles: bool,
    pub group_borders: bool,
    pub empty_paragraphs: bool,
    pub no_dtd: bool,
    pub default_encoding: String,
}

impl Default for ParseRtfOptions {
    fn default() -> Self {
        Self {
            dtd_path: Some(String::new()),
            convert_symbol: false,
            convert_wingdings: false,
            convert_zapf: false,
            convert_caps: false,
            run_level: 1,
            indent: false,
            replace_illegals: true,
            form_lists: true,
            headings_to_sections: true,
            group_styles: true,
            group_borders: true,
            empty_paragraphs: true,
            no_dtd: false,
            default_encoding: "cp1252".to_string(),
        }
    }
}

/// Result of [`parse_rtf`]: the final XML, any `\pict` sidecar content
/// extracted along the way, and Python's own `self.__exit_level`
/// return value.
#[derive(Debug, Clone)]
pub struct ParseRtfOutput {
    pub xml: String,
    pub pict_document: Option<String>,
    pub exit_level: i64,
}

/// Port of the `except InvalidRtfException` block. `tokenized_content`
/// is what `self.__temp_file` holds at that point in Python (already
/// through `tokenize.py`, but never reaching `process_tokens.py`
/// successfully) -- used only to build a `DefaultEncoding` guess.
/// `original_raw_rtf` is Python's *separate* `self.__file` -- the
/// untouched original input -- which is what the final
/// `check_encoding` probe actually scans.
fn diagnose_invalid_rtf(
    tokenized_content: &str,
    original_raw_rtf: &[u8],
    default_encoding_label: &str,
    base_msg: &str,
) -> String {
    let mut encode_obj = DefaultEncoding::new(tokenized_content, default_encoding_label, true);
    let enc = format!("cp{}", encode_obj.get_codepage());
    let mut msg = format!("{base_msg}\nException in token processing");
    if let Ok(true) = super::check_encoding::check_encoding_bytes(original_raw_rtf, &enc, false) {
        msg.push_str("\nFile does not appear to be correctly encoded.\n");
    }
    msg
}

/// Port of `ParseRtf.parse_rtf`, operating on already-read content
/// (see this module's own doc; [`parse_rtf_file`] is the file-reading
/// wrapper). `raw_rtf` is the *original* RTF source bytes, not yet
/// cleaned by [`super::line_endings`].
pub fn parse_rtf(raw_rtf: &[u8], opts: &ParseRtfOptions) -> Result<ParseRtfOutput> {
    let run_level = opts.run_level;

    let cleaned = line_endings::fix_line_endings(raw_rtf, opts.replace_illegals);
    let mut content = String::from_utf8_lossy(&cleaned).into_owned();

    content = tokenize::tokenize(&content)?;

    content = match process_tokens::process_tokens(&content, run_level) {
        Ok(out) => out.content,
        Err(e) => {
            let msg = diagnose_invalid_rtf(&content, raw_rtf, &opts.default_encoding, &e.to_string());
            return Err(ParseRtfError::InvalidRtf(msg));
        }
    };

    let delete_info_out = delete_info::delete_info(&content, run_level)?;
    content = delete_info_out.content;
    let found_destination = delete_info_out.found_delete;

    let pict_out = pict::process_pict(&content);
    content = pict_out.content;

    content = combine_borders::combine_borders(&content);

    let footnote_sep = footnote::separate_footnotes(&content);
    content = footnote_sep.content;

    let header_sep = header::separate_headers(&content);
    content = header_sep.content;

    content = list_numbers::fix_list_numbers(&content);

    // Port of `PreambleDiv(no_namespace=None, ...)`: `ParseRtf.py`
    // never overrides this constructor default, so it's always false
    // in this pipeline.
    let preamble_out = preamble_div::make_preamble_divisions(&content, false, run_level)?;
    content = preamble_out.content;
    let list_of_lists = preamble_out.list_of_lists;

    let mut encode_obj = DefaultEncoding::new(content.clone(), &opts.default_encoding, false);
    let (platform, code_page, default_font_num) = encode_obj.find_default_encoding();

    let mut hex2utf_obj = Hex2Utf8::new(AreaToConvert::Preamble, code_page.clone(), run_level);
    content = hex2utf_obj.convert_hex_2_utf8(&content)?;

    let fonts_out = fonts::convert_fonts(&content, &default_font_num, run_level)?;
    content = fonts_out.content;
    let special_font_dict = fonts_out.special_font_dict;

    content = colors::convert_colors(&content, run_level)?;

    content = styles::convert_styles(&content, run_level)?;

    content = info::fix_info_with_run_level(&content, run_level)?;

    content = preamble_rest::fix_preamble(&content, &platform.to_string(), &special_font_dict.default_font, &code_page);

    let is_old_rtf = old_rtf::check_if_old_rtf(&content, run_level);
    if is_old_rtf {
        if run_level > 5 {
            return Err(ParseRtfError::OlderRtfTooStrict(run_level));
        }
        if run_level > 1 {
            eprintln!("File could be older RTF...");
            if found_destination {
                eprintln!("File also has newer RTF.\nWill do the best to convert...");
            }
        }
        content = add_brackets::add_brackets(&content, run_level);
    }

    content = fields_small::fix_fields(&content);
    content = fields_large::fix_fields(&content, run_level)?;

    content = sections::make_sections(&content, run_level)?;
    content = paragraphs::make_paragraphs(&content, opts.empty_paragraphs);

    let paragraph_def_out = paragraph_def::make_paragraph_def(&content, &special_font_dict.default_font, run_level)?;
    content = paragraph_def_out.content;
    content = body_styles::insert_info(&content, &paragraph_def_out.body_style_strings, run_level)?;

    let (table_content, table_data) = table::make_table(&content);
    content = table_content;
    content = table_info::insert_info(&content, &table_data, run_level)?;

    if opts.form_lists {
        let make_lists_opts = MakeListsOptions {
            headings_to_sections: opts.headings_to_sections,
            no_headings_as_list: true,
            write_list_info: false,
            run_level,
        };
        content = make_lists::make_lists(&content, &list_of_lists, make_lists_opts);
    }

    if opts.headings_to_sections {
        content = headings_to_sections::make_sections(&content);
    }

    if opts.group_styles {
        content = group_styles::group_styles(&content, true, run_level)?;
    }

    if opts.group_borders {
        content = group_borders::group_borders(&content, run_level)?;
    }

    content = inline::form_tags(&content, run_level)?;

    hex2utf_obj.update_values(
        AreaToConvert::Body,
        opts.convert_caps,
        opts.convert_symbol,
        opts.convert_wingdings,
        opts.convert_zapf,
        true,
        true,
        true,
    );
    content = hex2utf_obj.convert_hex_2_utf8(&content)?;

    content = header::join_headers(&content, header_sep.found_a_header);
    content = footnote::join_footnotes(&content, footnote_sep.found_a_footnote);

    let convert_opts = ConvertToTagsOptions {
        no_dtd: opts.no_dtd,
        dtd_path: opts.dtd_path.clone(),
        indent: opts.indent,
        run_level,
    };
    let xml = convert_to_tags::convert_to_tags(&content, &convert_opts)?;

    Ok(ParseRtfOutput { xml, pict_document: pict_out.pict_document, exit_level: 0 })
}

/// Port of `ParseRtf.parse_rtf`'s full, real-I/O behavior: reads
/// `in_file`, calls [`parse_rtf`], and writes the result via
/// [`super::output`] -- to `out_dir`/`out_file`/`out_file`-in-`out_dir`
/// per [`OutputOptions`], or stdout if neither is given. Any extracted
/// `\pict` sidecar content is the caller's own responsibility (Python
/// writes it next to `out_file`/`orig_file`; this port just returns it
/// in [`ParseRtfOutput::pict_document`] rather than guessing a path).
pub fn parse_rtf_file(
    in_file: &Path,
    opts: &ParseRtfOptions,
    output_opts: &OutputOptions,
) -> Result<ParseRtfOutput> {
    let raw = fs::read(in_file)?;
    let result = parse_rtf(&raw, opts)?;
    output::output(&result.xml, in_file, output_opts)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_rtf_document_converts_to_xml() {
        let rtf = br"{\rtf1\ansi\ansicpg1252 Hello, world!\par}";
        let opts = ParseRtfOptions::default();
        let out = parse_rtf(rtf, &opts).unwrap();
        assert!(out.xml.starts_with("<?xml"), "{}", out.xml);
        assert!(out.xml.contains("Hello, world!"), "{}", out.xml);
        assert_eq!(out.exit_level, 0);
        assert!(out.pict_document.is_none());
    }

    #[test]
    fn bold_text_produces_an_inline_tag() {
        let rtf = br"{\rtf1\ansi\ansicpg1252 plain {\b bold text} more\par}";
        let out = parse_rtf(rtf, &ParseRtfOptions::default()).unwrap();
        assert!(out.xml.contains("<inline"), "{}", out.xml);
        assert!(out.xml.contains("bold text"), "{}", out.xml);
    }

    #[test]
    fn a_hex_escaped_character_is_converted_to_utf8() {
        // \'e9 is 'é' under cp1252 -- rendered as an XML numeric
        // character reference, matching tokenize.rs's own convention
        // for non-ASCII code points.
        let rtf = br"{\rtf1\ansi\ansicpg1252 caf\'e9\par}";
        let out = parse_rtf(rtf, &ParseRtfOptions::default()).unwrap();
        assert!(out.xml.to_lowercase().contains("&#x00e9;"), "{}", out.xml);
    }

    #[test]
    fn a_document_with_a_table_control_word_produces_a_table_tag() {
        let rtf = br"{\rtf1\ansi\ansicpg1252{\trowd\cellx1000\cellx2000 a\cell b\cell\row}\par}";
        let out = parse_rtf(rtf, &ParseRtfOptions::default()).unwrap();
        assert!(out.xml.contains("<table"), "{}", out.xml);
    }

    #[test]
    fn a_paragraph_style_named_heading_1_becomes_a_section_when_enabled() {
        let rtf = br"{\rtf1\ansi\ansicpg1252{\stylesheet{\s1 heading 1;}}\pard\s1 Chapter One\par}";
        let mut opts = ParseRtfOptions::default();
        opts.headings_to_sections = true;
        let out = parse_rtf(rtf, &opts).unwrap();
        assert!(out.xml.contains("<section"), "{}", out.xml);
    }

    #[test]
    fn garbage_that_process_tokens_rejects_returns_an_enriched_invalid_rtf_error() {
        let rtf = br"{\rtf1 \unbalanced-and-not-real-rtf-tokens-\'zz";
        let err = parse_rtf(rtf, &ParseRtfOptions::default()).unwrap_err();
        match err {
            ParseRtfError::InvalidRtf(msg) => {
                assert!(msg.contains("Exception in token processing"), "{msg}");
            }
            other => panic!("expected InvalidRtf, got {other:?}"),
        }
    }

    #[test]
    fn indent_option_adds_newlines_between_block_tags() {
        let rtf = br"{\rtf1\ansi\ansicpg1252 Hello\par}";
        let mut compact = ParseRtfOptions::default();
        compact.indent = false;
        let mut indented = ParseRtfOptions::default();
        indented.indent = true;
        let compact_out = parse_rtf(rtf, &compact).unwrap();
        let indented_out = parse_rtf(rtf, &indented).unwrap();
        assert!(indented_out.xml.matches('\n').count() > compact_out.xml.matches('\n').count());
    }

    #[test]
    fn no_dtd_suppresses_the_doctype_declaration() {
        let rtf = br"{\rtf1\ansi\ansicpg1252 Hello\par}";
        let mut opts = ParseRtfOptions::default();
        opts.no_dtd = true;
        let out = parse_rtf(rtf, &opts).unwrap();
        assert!(!out.xml.contains("DOCTYPE"), "{}", out.xml);
    }

    #[test]
    fn parse_rtf_file_reads_and_writes_through_the_real_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let in_path = dir.path().join("input.rtf");
        std::fs::write(&in_path, br"{\rtf1\ansi\ansicpg1252 Hello, file!\par}").unwrap();

        let opts = ParseRtfOptions::default();
        let out_path = dir.path().join("output.xml");
        let output_opts = OutputOptions { output_dir: None, out_file: Some(out_path.to_str().unwrap()), no_ask: true };

        let result = parse_rtf_file(&in_path, &opts, &output_opts).unwrap();
        assert!(result.xml.contains("Hello, file!"));

        let written = std::fs::read_to_string(&out_path).unwrap();
        assert_eq!(written, result.xml);
    }
}
