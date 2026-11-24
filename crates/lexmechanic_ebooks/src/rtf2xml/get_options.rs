//! Port of `old_src/src/calibre/ebooks/rtf2xml/get_options.py`
//! (`GetOptions`).
//!
//! Merges the resolved `.rtf2xml` config file ([`super::configure_txt::ConfigOptions`])
//! with the parsed command-line arguments ([`super::options_trem::OptionParser`])
//! into one final [`RunOptions`], applying CLI overrides on top of
//! config-file defaults, validating along the way (existing output
//! directory, numeric `--level`/`--indent`, acceptable `--format`,
//! exactly one input file, `.rtf`-extension-derived smart output
//! filename).
//!
//! [`get_options`] is the pure core (takes an already-resolved
//! `ConfigOptions`, easily testable); [`get_options_from_env`] is the
//! real-I/O convenience wrapper that also resolves the config file.
//!
//! `GetOptions.__init__`'s `rtf_dir` parameter is stored but never
//! read anywhere else in the class -- confirmed by grep, dropped here.
//!
//! # Two confirmed, preserved upstream bugs
//!
//! - The CLI option registered for "convert Wingdings" is spelled
//!   `'windings'` (missing a `g`) in `options_dict`, but the code that
//!   *acts* on a parsed option checks for `'wingdings'` (correctly
//!   spelled) instead. The two spellings never match, so `--windings`
//!   parses successfully as a recognized option but silently does
//!   nothing -- there is no way to turn on Wingdings conversion from
//!   the CLI at all (only `--no-wingdings`, spelled consistently both
//!   places, actually works). Preserved exactly: [`option_spec`]
//!   registers `"windings"`; the override logic checks `"wingdings"`.
//! - `'no-ask'` is checked (`if 'no-ask' in the_keys: ...`) but never
//!   registered in `options_dict` at all -- a user passing `--no-ask`
//!   would fail CLI validation before this check is ever reached, so
//!   it can never actually fire. Preserved as dead-but-harmless code
//!   (the check itself is safe, just always false in practice).
//! - `if options_dict == 1: sys.exit(1)` in `__get_config_options`
//!   checks for a sentinel `configure_txt.Configure.get_configuration`
//!   can never actually return to its caller (that function always
//!   either raises or returns a real dict -- see `configure_txt.rs`'s
//!   own doc). Not ported: [`get_options_from_env`] simply propagates
//!   [`super::configure_txt::ConfigureError`] via `?`.
//!
//! # A crash averted
//!
//! If more than one positional argument is given, Python sets
//! `valid = 0` but does *not* return early, and later code
//! unconditionally reads `return_options['in-file']` -- which was
//! never set in this branch, so re-deriving a smart-output filename
//! (only reachable when `smart-output` is configured true and no
//! `--output` was given) would `KeyError`. This port just skips that
//! derivation when `in_file` is `None` instead of panicking, per this
//! crate's standing convention.

use std::path::Path;

use indexmap::IndexMap;

use super::configure_txt::{ConfigOptions, ConfigureError};
use super::options_trem::{OptionParser, OptionSpec};

const ACCEPTABLE_FORMATS: [&str; 3] = ["sdoc", "raw", "tei"];

/// Port of `GetOptions.get_options`'s returned `return_options`.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub valid: bool,
    pub help: bool,
    pub config_flag: bool,
    pub version: bool,
    pub dir: Option<String>,
    pub out_file: Option<String>,
    pub level: i64,
    pub raw_dtd_path: Option<String>,
    pub format: String,
    pub show_warnings: bool,
    pub convert_symbol: bool,
    pub convert_wingdings: bool,
    pub convert_zapf: bool,
    pub convert_caps: bool,
    pub no_dtd: bool,
    pub no_ask: bool,
    pub no_namespace: Option<bool>,
    pub headings_to_sections: bool,
    pub form_lists: bool,
    pub group_styles: bool,
    pub group_borders: bool,
    pub empty_paragraphs: bool,
    pub in_file: Option<String>,
    pub indent: i64,
    pub smart_output: bool,
}

/// Port of `options_dict` inside `GetOptions.get_options` -- see this
/// module's own doc for the `'windings'`/`'wingdings'` spelling bug,
/// preserved here (this is what registers `"windings"` as the legal
/// long option, not `"wingdings"`).
fn option_spec() -> IndexMap<String, OptionSpec> {
    let entries: [(&str, bool, Option<char>); 32] = [
        ("dir", true, None),
        ("help", false, Some('h')),
        ("show-warnings", false, None),
        ("caps", false, None),
        ("no-caps", false, None),
        ("symbol", false, None),
        ("no-symbol", false, None),
        ("windings", false, None),
        ("no-wingdings", false, None),
        ("zapf", false, None),
        ("no-zapf", false, None),
        ("font", false, None),
        ("no-font", false, None),
        ("dtd", true, None),
        ("no-dtd", false, None),
        ("version", false, None),
        ("output", true, Some('o')),
        ("no-namespace", false, None),
        ("level", true, None),
        ("indent", true, None),
        ("no-lists", false, None),
        ("lists", false, None),
        ("group-styles", false, None),
        ("no-group-styles", false, None),
        ("group-borders", false, None),
        ("no-group-borders", false, None),
        ("headings-to-sections", false, None),
        ("no-headings-to-sections", false, None),
        ("empty-para", false, None),
        ("no-empty-para", false, None),
        ("format", true, Some('f')),
        ("config", false, None),
    ];
    entries.into_iter().map(|(name, takes_arg, short)| (name.to_string(), OptionSpec { takes_arg, short })).collect()
}

/// Port of a `config_value in {'true', '1'}` / `in {'false', '0'}` /
/// else-default tri-state check, shared by `headings-to-sections`,
/// `write-empty-paragraphs`, `lists`, `group-styles`, and
/// `group-borders` in `__get_config_options` (each with its own
/// `default_if_unset_or_unrecognized`).
fn normalize_tri_state(raw: Option<&str>, default_if_unset_or_unrecognized: bool) -> bool {
    match raw {
        Some("true") | Some("1") => true,
        Some("false") | Some("0") => false,
        _ => default_if_unset_or_unrecognized,
    }
}

/// Port of `__get_config_options`.
fn get_config_options(config: &ConfigOptions) -> RunOptions {
    RunOptions {
        valid: true,
        help: false,
        config_flag: false,
        version: false,
        dir: None,
        out_file: None,
        level: config.level,
        raw_dtd_path: config.raw_dtd_path.clone(),
        format: config.format.clone(),
        show_warnings: false,
        convert_symbol: config.convert_symbol,
        convert_wingdings: config.convert_wingdings,
        convert_zapf: config.convert_zapf_dingbats,
        convert_caps: config.convert_caps,
        no_dtd: false,
        no_ask: false,
        no_namespace: None,
        headings_to_sections: normalize_tri_state(config.headings_to_sections.as_deref(), false),
        form_lists: normalize_tri_state(config.lists.as_deref(), false),
        group_styles: normalize_tri_state(config.group_styles.as_deref(), false),
        group_borders: normalize_tri_state(config.group_borders.as_deref(), false),
        empty_paragraphs: normalize_tri_state(config.write_empty_paragraphs.as_deref(), true),
        in_file: None,
        indent: config.indent,
        smart_output: config.smart_output,
    }
}

/// Port of `GetOptions.get_options`, taking an already-resolved
/// [`ConfigOptions`] rather than reading `.rtf2xml` itself -- see
/// [`get_options_from_env`] for the real-I/O wrapper.
/// `system_arguments` should be the full `argv` including the program
/// name at index 0 (matching Python's own parameter -- the leading
/// element is dropped internally, same as
/// [`super::options_trem::OptionParser`]'s constructor does in
/// Python, though this crate's port pushes that slicing to the
/// caller).
pub fn get_options(system_arguments: &[String], config: &ConfigOptions) -> RunOptions {
    let mut r = get_config_options(config);

    let parser = OptionParser::new(&option_spec());
    let cli_args = if system_arguments.is_empty() { &[][..] } else { &system_arguments[1..] };
    let (options, arguments) = match parser.parse(cli_args) {
        None => {
            r.valid = false;
            return r;
        }
        Some(pair) => pair,
    };

    if options.contains_key("help") {
        r.help = true;
        return r;
    }
    if options.contains_key("config") {
        r.config_flag = true;
        return r;
    }
    if options.contains_key("version") {
        r.version = true;
        return r;
    }

    // `out-dir` is always 0/unused in Python (its own "# unused"
    // comment) -- no field here; only `dir` (below) is meaningful.
    if let Some(Some(dir_val)) = options.get("dir") {
        if !Path::new(dir_val).is_dir() {
            eprintln!("Your output must be an existing directory.");
            r.valid = false;
        } else {
            r.dir = Some(dir_val.clone());
        }
    }

    if let Some(Some(out_val)) = options.get("output") {
        r.out_file = Some(out_val.clone());
    }

    if let Some(Some(level_str)) = options.get("level") {
        match level_str.trim().parse::<i64>() {
            Ok(n) => r.level = n,
            Err(_) => {
                eprintln!("The options \"--level\" must be a number.");
                r.valid = false;
                return r;
            }
        }
    }

    if let Some(Some(dtd_val)) = options.get("dtd") {
        r.raw_dtd_path = Some(dtd_val.clone());
    }

    if let Some(Some(format_val)) = options.get("format") {
        if !ACCEPTABLE_FORMATS.contains(&format_val.as_str()) {
            // Verbatim Python message -- it only mentions two of the
            // three acceptable values ('raw' is accepted but omitted
            // from the text); preserved as-is, not "fixed".
            eprintln!("--format must take either 'sdoc' or 'tei'");
            r.valid = false;
            return r;
        }
        r.format = format_val.clone();
    }

    r.show_warnings = options.contains_key("show-warnings");

    if options.contains_key("no-font") {
        r.convert_symbol = false;
        r.convert_zapf = false;
        r.convert_wingdings = false;
    }
    if options.contains_key("font") {
        r.convert_symbol = true;
        r.convert_zapf = true;
        r.convert_wingdings = true;
    }
    if options.contains_key("symbol") {
        r.convert_symbol = true;
    }
    if options.contains_key("no-symbol") {
        r.convert_symbol = false;
    }
    if options.contains_key("wingdings") {
        // Confirmed-dead in practice: see this module's own doc.
        r.convert_wingdings = true;
    }
    if options.contains_key("no-wingdings") {
        r.convert_wingdings = false;
    }
    if options.contains_key("zapf") {
        r.convert_zapf = true;
    }
    if options.contains_key("no-zapf") {
        r.convert_zapf = false;
    }
    if options.contains_key("caps") {
        r.convert_caps = true;
    }
    if options.contains_key("no-caps") {
        r.convert_caps = false;
    }

    r.no_dtd = options.contains_key("no-dtd");

    if options.contains_key("no-ask") {
        // Confirmed-dead in practice: see this module's own doc.
        r.no_ask = true;
        eprintln!("You can also permanetly set the no-ask option in the rtf2xml file.");
    }

    if options.contains_key("no-namespace") {
        r.no_namespace = Some(true);
    }

    if options.contains_key("headings-to-sections") {
        r.headings_to_sections = true;
    } else if options.contains_key("no-headings-to-sections") {
        r.headings_to_sections = false;
    }

    if options.contains_key("no-lists") {
        r.form_lists = false;
    } else if options.contains_key("lists") {
        r.form_lists = true;
    }

    if options.contains_key("group-styles") {
        r.group_styles = true;
    } else if options.contains_key("no-group-styles") {
        r.group_styles = false;
    }

    if options.contains_key("group-borders") {
        r.group_borders = true;
    } else if options.contains_key("no-group-borders") {
        r.group_borders = false;
    }

    if options.contains_key("empty-para") {
        r.empty_paragraphs = true;
    } else if options.contains_key("no-empty-para") {
        r.empty_paragraphs = false;
    }

    if arguments.is_empty() {
        eprintln!("You must provide a file to convert.");
        r.valid = false;
        return r;
    } else if arguments.len() > 1 {
        eprintln!("You can only convert one file at a time.");
        r.valid = false;
    } else {
        r.in_file = Some(arguments[0].clone());
    }

    if r.smart_output && r.out_file.is_none() {
        if let Some(in_file) = r.in_file.clone() {
            let path = Path::new(&in_file);
            let has_rtf_ext = path.extension().map(|e| e == "rtf").unwrap_or(false);
            if !has_rtf_ext {
                eprintln!(
                    "Sorry, but this file does not have an \"rtf\" extension, so \nthe script will not attempt to convert it.\nIf it is in fact an rtf file, use the \"-o\" option."
                );
                r.valid = false;
            } else {
                r.out_file = Some(path.with_extension("xml").to_string_lossy().to_string());
            }
        }
        // else: Python would KeyError here -- see this module's own doc.
    }

    if let Some(Some(indent_str)) = options.get("indent") {
        match indent_str.trim().parse::<i64>() {
            Ok(n) => r.indent = n,
            Err(_) => {
                eprintln!("--indent must take an integer");
                r.valid = false;
            }
        }
    }

    r
}

/// Port of `GetOptions.get_options`'s real behavior end to end:
/// resolves `.rtf2xml` via [`super::configure_txt::get_configuration`]
/// and delegates to [`get_options`].
pub fn get_options_from_env(
    system_arguments: &[String],
    configuration_file: Option<&str>,
    script_dir: Option<&Path>,
) -> Result<RunOptions, ConfigureError> {
    let config = super::configure_txt::get_configuration(configuration_file, script_dir, false)?;
    Ok(get_options(system_arguments, &config))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> ConfigOptions {
        ConfigOptions {
            config_location: None,
            configure_directory: None,
            smart_output: false,
            level: 1,
            indent: 0,
            convert_symbol: false,
            convert_wingdings: false,
            convert_zapf_dingbats: false,
            convert_caps: false,
            xslt_processor: None,
            no_namespace: None,
            format: "raw".to_string(),
            no_pyxml: true,
            group_styles: None,
            group_borders: None,
            headings_to_sections: None,
            lists: None,
            raw_dtd_path: None,
            write_empty_paragraphs: None,
            script_name: None,
        }
    }

    fn args(items: &[&str]) -> Vec<String> {
        std::iter::once("prog".to_string()).chain(items.iter().map(|s| s.to_string())).collect()
    }

    #[test]
    fn a_simple_input_file_is_valid_with_config_derived_defaults() {
        let r = get_options(&args(&["input.rtf"]), &default_config());
        assert!(r.valid);
        assert_eq!(r.in_file, Some("input.rtf".to_string()));
        assert_eq!(r.format, "raw");
        assert_eq!(r.level, 1);
        assert!(r.empty_paragraphs);
    }

    #[test]
    fn help_short_circuits_before_argument_validation() {
        let r = get_options(&args(&["-h"]), &default_config());
        assert!(r.help);
        assert!(r.valid);
    }

    #[test]
    fn no_input_file_is_invalid() {
        let r = get_options(&args(&[]), &default_config());
        assert!(!r.valid);
    }

    #[test]
    fn an_illegal_option_is_invalid() {
        let r = get_options(&args(&["--not-a-real-option", "input.rtf"]), &default_config());
        assert!(!r.valid);
    }

    #[test]
    fn output_option_sets_out_file_and_short_form_matches() {
        let r1 = get_options(&args(&["--output=out.xml", "input.rtf"]), &default_config());
        assert_eq!(r1.out_file, Some("out.xml".to_string()));
        let r2 = get_options(&args(&["-o", "out.xml", "input.rtf"]), &default_config());
        assert_eq!(r2.out_file, Some("out.xml".to_string()));
    }

    #[test]
    fn bad_format_value_is_invalid_with_the_verbatim_python_message() {
        let r = get_options(&args(&["--format=docx", "input.rtf"]), &default_config());
        assert!(!r.valid);
    }

    #[test]
    fn raw_format_is_accepted_even_though_the_error_message_omits_it() {
        let r = get_options(&args(&["--format=raw", "input.rtf"]), &default_config());
        assert!(r.valid);
        assert_eq!(r.format, "raw");
    }

    #[test]
    fn windings_flag_parses_but_never_actually_enables_wingdings_conversion() {
        let r = get_options(&args(&["--windings", "input.rtf"]), &default_config());
        assert!(r.valid, "the option should still parse as legal");
        assert!(!r.convert_wingdings, "but it should never take effect -- see this module's doc");
    }

    #[test]
    fn no_wingdings_flag_works_correctly() {
        let mut config = default_config();
        config.convert_wingdings = true;
        let r = get_options(&args(&["--no-wingdings", "input.rtf"]), &config);
        assert!(!r.convert_wingdings);
    }

    #[test]
    fn later_font_flags_override_earlier_ones_in_sequence() {
        let r = get_options(&args(&["--no-font", "--symbol", "input.rtf"]), &default_config());
        assert!(r.convert_symbol);
        assert!(!r.convert_zapf);
        assert!(!r.convert_wingdings);
    }

    #[test]
    fn smart_output_derives_an_xml_filename_from_an_rtf_input() {
        let mut config = default_config();
        config.smart_output = true;
        let r = get_options(&args(&["report.rtf"]), &config);
        assert!(r.valid);
        assert_eq!(r.out_file, Some("report.xml".to_string()));
    }

    #[test]
    fn smart_output_rejects_a_non_rtf_extension() {
        let mut config = default_config();
        config.smart_output = true;
        let r = get_options(&args(&["report.txt"]), &config);
        assert!(!r.valid);
    }

    #[test]
    fn invalid_level_and_indent_values_are_rejected() {
        let r1 = get_options(&args(&["--level=abc", "input.rtf"]), &default_config());
        assert!(!r1.valid);
        let r2 = get_options(&args(&["--indent=abc", "input.rtf"]), &default_config());
        assert!(!r2.valid);
    }

    #[test]
    fn config_derived_tri_state_flags_flow_through_when_not_overridden() {
        let mut config = default_config();
        config.group_styles = Some("true".to_string());
        config.group_borders = Some("0".to_string());
        config.headings_to_sections = Some("garbage".to_string());
        let r = get_options(&args(&["input.rtf"]), &config);
        assert!(r.group_styles);
        assert!(!r.group_borders);
        assert!(!r.headings_to_sections);
    }
}
