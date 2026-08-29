//! Port of `old_src/src/calibre/ebooks/rtf2xml/configure_txt.py`
//! (`Configure`).
//!
//! Locates and parses the user's `.rtf2xml` configuration file (a
//! simple `key = value` text format, `#`-comments and blank lines
//! ignored), returning a validated, defaulted [`ConfigOptions`].
//! Genuinely about environment/filesystem interaction (like
//! `output.rs`), not a content transform: checks `$HOME/.rtf2xml`,
//! then `$USERPROFILE/.rtf2xml` (Windows), then a caller-supplied
//! script directory (`script_dir`, standing in for Python's
//! `sys.path[0]` -- there's no direct Rust equivalent of "the running
//! script's own directory", so the caller supplies it), falling back
//! to whatever explicit path the caller passed in.
//!
//! [`resolve_config_path`] takes the `HOME`/`USERPROFILE` values as
//! explicit parameters rather than reading `std::env::var` itself, so
//! it's a pure, easily-testable function; [`get_configuration`] reads
//! the real environment and calls it.
//!
//! `get_configuration(self, type)`'s own `type` parameter is entirely
//! unused in the Python body -- dropped here.
//!
//! # Two representational simplifications
//!
//! Several config values (`smart-output`, the four `convert-*` font
//! flags, `no-pyxml`) are semantically booleans, but Python's own
//! handling leaves them inconsistently typed: the `'true'` case is
//! never explicitly converted (no `else` branch does it), so the
//! dict ends up holding either the int `0` or the literal string
//! `'true'` depending on path -- distinct representations a
//! downstream truthiness check (`if value:`) treats identically
//! anyway. This port normalizes all of these to a plain `bool` up
//! front; the *observable* boolean meaning is unchanged, only the
//! internal representation is cleaner.
//!
//! Sixteen keys total are allowed in the config file
//! (see [`ALLOWABLE`]); only nine of them (`configuration-directory`,
//! `smart-output`, `level`, `indent`, and the four `convert-*` flags)
//! are actually validated, typed, and defaulted by `__parse_dict` --
//! the rest (`group-styles`, `group-borders`, `headings-to-sections`,
//! `lists`, `raw-dtd-path`, `write-empty-paragraphs`, `script-name`)
//! pass straight through as raw, optional strings with no processing
//! at all. [`ConfigOptions`] keeps that same split.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use thiserror::Error;

const ALLOWABLE: [&str; 16] = [
    "configuration-directory",
    "smart-output",
    "level",
    "convert-symbol",
    "convert-wingdings",
    "convert-zapf-dingbats",
    "convert-caps",
    "indent",
    "group-styles",
    "group-borders",
    "headings-to-sections",
    "lists",
    "raw-dtd-path",
    "write-empty-paragraphs",
    "config-location",
    "script-name",
];

/// Port of the two `raise self.__bug_handler(msg)` sites in
/// `get_configuration`/`__parse_dict`. Python's own more specific
/// diagnostics (unknown option, bad directory, bad boolean/int value)
/// are printed to stderr at the point of failure (matched here via
/// `eprintln!`) but never reach the exception message itself -- only
/// the generic wrapper text does, which is why there are just two
/// variants here too.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigureError {
    #[error(
        "{line}Error in configuration.txt, line {line_num}\nOptions take the form of option = value.\nPlease correct the configuration file \"{config_file}\" before continuing\n"
    )]
    MalformedLine { line: String, line_num: usize, config_file: String },
    #[error("Please correct the configuration file \"{0}\" before continuing\n")]
    InvalidConfiguration(String),
}

pub type Result<T> = std::result::Result<T, ConfigureError>;

/// Port of `get_configuration`'s returned `return_dict`. See this
/// module's own doc for why the "validated/typed/defaulted" and
/// "raw pass-through" fields are split into two groups below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigOptions {
    pub config_location: Option<String>,
    pub configure_directory: Option<String>,
    pub smart_output: bool,
    pub level: i64,
    pub indent: i64,
    pub convert_symbol: bool,
    pub convert_wingdings: bool,
    pub convert_zapf_dingbats: bool,
    pub convert_caps: bool,
    /// Port of the four hardcoded tail assignments -- always these
    /// exact values, regardless of what's in the config file.
    pub xslt_processor: Option<String>,
    pub no_namespace: Option<String>,
    pub format: String,
    pub no_pyxml: bool,

    // Raw pass-through -- never validated, typed, or defaulted.
    pub group_styles: Option<String>,
    pub group_borders: Option<String>,
    pub headings_to_sections: Option<String>,
    pub lists: Option<String>,
    pub raw_dtd_path: Option<String>,
    pub write_empty_paragraphs: Option<String>,
    pub script_name: Option<String>,
}

/// Port of `__get_file_name`.
pub fn resolve_config_path(
    configuration_file: Option<&str>,
    home: Option<&str>,
    userprofile: Option<&str>,
    script_dir: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(home) = home.filter(|h| !h.is_empty()) {
        let candidate = Path::new(home).join(".rtf2xml");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if let Some(profile) = userprofile.filter(|p| !p.is_empty()) {
        let candidate = Path::new(profile).join(".rtf2xml");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if let Some(dir) = script_dir {
        let candidate = dir.join(".rtf2xml");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    configuration_file.map(PathBuf::from)
}

fn parse_int_option(raw: &IndexMap<String, String>, key: &str, default: i64, config_location: &str) -> Result<i64> {
    match raw.get(key) {
        None => Ok(default),
        Some(v) if v.is_empty() => Ok(default),
        Some(v) => v.trim().parse::<i64>().map_err(|_| {
            eprintln!("\"{key}\" must be a number");
            eprintln!("You choose \"{v}\" ");
            ConfigureError::InvalidConfiguration(config_location.to_string())
        }),
    }
}

fn parse_bool_flag(raw: &IndexMap<String, String>, key: &str) -> bool {
    match raw.get(key).map(String::as_str) {
        None | Some("") | Some("false") => false,
        Some("true") => true,
        Some(_) => {
            eprintln!("\"{key}\" must be true or false.");
            // Matches Python: doesn't abort here (unlike the
            // structurally similar `smart-output` check) -- and per
            // Python's own truthiness of the still-unconverted,
            // non-empty malformed string, the value is treated as
            // truthy rather than defaulted to false.
            true
        }
    }
}

/// Port of `__parse_dict`.
fn parse_dict(mut raw: IndexMap<String, String>, config_location: Option<String>) -> Result<ConfigOptions> {
    if let Some(loc) = &config_location {
        raw.insert("config-location".to_string(), loc.clone());
    }
    let location_for_errors = config_location.clone().unwrap_or_default();

    for key in raw.keys() {
        if !ALLOWABLE.contains(&key.as_str()) {
            eprintln!("options \"{key}\" not a legal option.");
            return Err(ConfigureError::InvalidConfiguration(location_for_errors));
        }
    }

    let configure_directory = match raw.get("configuration-directory") {
        None => None,
        Some(dir) => {
            if !Path::new(dir).is_dir() {
                eprintln!("The directory \"{dir}\" does not appear to be a directory.");
                return Err(ConfigureError::InvalidConfiguration(location_for_errors));
            }
            Some(dir.clone())
        }
    };

    let smart_output = match raw.get("smart-output").map(String::as_str) {
        None | Some("") | Some("false") => false,
        Some("true") => true,
        Some(_) => {
            eprintln!("\"smart-output\" must be true or false.");
            return Err(ConfigureError::InvalidConfiguration(location_for_errors));
        }
    };

    let level = parse_int_option(&raw, "level", 1, &location_for_errors)?;
    let indent = parse_int_option(&raw, "indent", 0, &location_for_errors)?;

    Ok(ConfigOptions {
        config_location,
        configure_directory,
        smart_output,
        level,
        indent,
        convert_symbol: parse_bool_flag(&raw, "convert-symbol"),
        convert_wingdings: parse_bool_flag(&raw, "convert-wingdings"),
        convert_zapf_dingbats: parse_bool_flag(&raw, "convert-zapf-dingbats"),
        convert_caps: parse_bool_flag(&raw, "convert-caps"),
        xslt_processor: None,
        no_namespace: None,
        format: "raw".to_string(),
        no_pyxml: true,
        group_styles: raw.get("group-styles").cloned(),
        group_borders: raw.get("group-borders").cloned(),
        headings_to_sections: raw.get("headings-to-sections").cloned(),
        lists: raw.get("lists").cloned(),
        raw_dtd_path: raw.get("raw-dtd-path").cloned(),
        write_empty_paragraphs: raw.get("write-empty-paragraphs").cloned(),
        script_name: raw.get("script-name").cloned(),
    })
}

/// Port of `get_configuration`, reading the real environment and
/// filesystem. `show_config_file` gates the same diagnostic Python's
/// own `self.__show_config_file` does.
pub fn get_configuration(
    configuration_file: Option<&str>,
    script_dir: Option<&Path>,
    show_config_file: bool,
) -> Result<ConfigOptions> {
    let home = std::env::var("HOME").ok();
    let userprofile = std::env::var("USERPROFILE").ok();
    let resolved = resolve_config_path(configuration_file, home.as_deref(), userprofile.as_deref(), script_dir);
    get_configuration_from(resolved.as_deref(), show_config_file)
}

/// Port of `get_configuration`'s body once the config file path is
/// already resolved -- split out from [`get_configuration`] so tests
/// can exercise the file-parsing logic without touching real
/// environment variables.
pub fn get_configuration_from(resolved_path: Option<&Path>, show_config_file: bool) -> Result<ConfigOptions> {
    let config_location = resolved_path.map(|p| p.to_string_lossy().to_string());

    if show_config_file {
        match &config_location {
            Some(loc) => eprintln!("configuration file is \"{loc}\""),
            None => eprintln!("No configuration file found; using default values"),
        }
    }

    let mut raw: IndexMap<String, String> = IndexMap::new();
    if let Some(path) = resolved_path {
        let content = std::fs::read_to_string(path).map_err(|_| {
            ConfigureError::InvalidConfiguration(config_location.clone().unwrap_or_default())
        })?;
        for (idx, raw_line) in content.lines().enumerate() {
            let line_num = idx + 1;
            let line = raw_line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split('=').collect();
            if fields.len() != 2 {
                return Err(ConfigureError::MalformedLine {
                    line: line.to_string(),
                    line_num,
                    config_file: config_location.unwrap_or_default(),
                });
            }
            raw.insert(fields[0].trim().to_string(), fields[1].trim().to_string());
        }
    }

    parse_dict(raw, config_location)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(dir: &tempfile::TempDir, contents: &str) -> PathBuf {
        let path = dir.path().join(".rtf2xml");
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn no_config_file_produces_all_defaults() {
        let opts = get_configuration_from(None, false).unwrap();
        assert_eq!(opts.config_location, None);
        assert_eq!(opts.level, 1);
        assert_eq!(opts.indent, 0);
        assert!(!opts.smart_output);
        assert!(!opts.convert_symbol);
        assert_eq!(opts.group_styles, None);
        assert_eq!(opts.format, "raw");
        assert!(opts.no_pyxml);
    }

    #[test]
    fn a_well_formed_config_file_is_parsed_and_typed() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            &dir,
            "# a comment\n\nlevel = 3\nindent = 1\nsmart-output = true\nconvert-symbol = true\nconvert-caps = false\ngroup-styles = yes\n",
        );
        let opts = get_configuration_from(Some(&path), false).unwrap();
        assert_eq!(opts.level, 3);
        assert_eq!(opts.indent, 1);
        assert!(opts.smart_output);
        assert!(opts.convert_symbol);
        assert!(!opts.convert_caps);
        assert_eq!(opts.group_styles, Some("yes".to_string()));
        assert_eq!(opts.config_location, Some(path.to_string_lossy().to_string()));
    }

    #[test]
    fn an_unknown_option_key_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(&dir, "not-a-real-option = true\n");
        assert!(matches!(
            get_configuration_from(Some(&path), false),
            Err(ConfigureError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn a_nonexistent_configuration_directory_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(&dir, "configuration-directory = /no/such/dir\n");
        assert!(matches!(
            get_configuration_from(Some(&path), false),
            Err(ConfigureError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn an_existing_configuration_directory_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        std::fs::create_dir(&sub).unwrap();
        let path = write_config(&dir, &format!("configuration-directory = {}\n", sub.display()));
        let opts = get_configuration_from(Some(&path), false).unwrap();
        assert_eq!(opts.configure_directory, Some(sub.to_string_lossy().to_string()));
    }

    #[test]
    fn an_invalid_smart_output_value_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(&dir, "smart-output = maybe\n");
        assert!(matches!(
            get_configuration_from(Some(&path), false),
            Err(ConfigureError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn an_invalid_font_flag_value_is_treated_as_truthy_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(&dir, "convert-symbol = maybe\n");
        let opts = get_configuration_from(Some(&path), false).unwrap();
        assert!(opts.convert_symbol);
    }

    #[test]
    fn a_non_numeric_level_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(&dir, "level = abc\n");
        assert!(matches!(
            get_configuration_from(Some(&path), false),
            Err(ConfigureError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn a_malformed_line_errors_with_line_number() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(&dir, "level = 1\nthis line has no equals sign\n");
        let err = get_configuration_from(Some(&path), false).unwrap_err();
        match err {
            ConfigureError::MalformedLine { line_num, .. } => assert_eq!(line_num, 2),
            other => panic!("expected MalformedLine, got {other:?}"),
        }
    }

    #[test]
    fn resolve_config_path_prefers_home_over_userprofile_over_script_dir() {
        let home_dir = tempfile::tempdir().unwrap();
        std::fs::write(home_dir.path().join(".rtf2xml"), "").unwrap();
        let profile_dir = tempfile::tempdir().unwrap();
        std::fs::write(profile_dir.path().join(".rtf2xml"), "").unwrap();

        let resolved = resolve_config_path(
            None,
            Some(home_dir.path().to_str().unwrap()),
            Some(profile_dir.path().to_str().unwrap()),
            None,
        );
        assert_eq!(resolved, Some(home_dir.path().join(".rtf2xml")));
    }

    #[test]
    fn resolve_config_path_falls_back_to_the_explicit_configuration_file() {
        let resolved = resolve_config_path(Some("/explicit/path"), None, None, None);
        assert_eq!(resolved, Some(PathBuf::from("/explicit/path")));
    }
}
