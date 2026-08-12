//! Conversion CLI helpers.
//!
//! Port of `old_src/src/calibre/ebooks/conversion/cli.py`. The
//! plugin-driven `option_recommendation_to_cli_option` half is
//! deferred: it dynamically assembles an `optparse` OptionParser
//! from per-plugin `OptionRecommendation` records, which requires
//! porting the full `customize/conversion::OptionRecommendation`
//! infrastructure — a bigger surface than issue #20 warrants.
//!
//! This module ports the pieces that are pure input handling: the
//! usage banner, the two well-known option lists, and the input/
//! output-path validation used before dispatching to the plumber.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Options for which the Python default is `True` in `OptionParser`
/// but for which the CLI switch flips them to `False`. Ported as-is
/// so plugin option builders can key on this list.
pub const HEURISTIC_OPTIONS: &[&str] = &[
    "markup_chapter_headings",
    "italicize_common_cases",
    "fix_indents",
    "html_unwrap_factor",
    "unwrap_lines",
    "delete_blank_paragraphs",
    "format_scene_breaks",
    "dehyphenate",
    "renumber_headings",
    "replace_scene_breaks",
];

/// Superset of HEURISTIC_OPTIONS + "remove_fake_margins" — Python's
/// DEFAULT_TRUE_OPTIONS constant.
pub fn default_true_options() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = HEURISTIC_OPTIONS.to_vec();
    v.push("remove_fake_margins");
    v
}

#[derive(Debug, Error)]
pub enum CliArgError {
    #[error("You must specify the input AND output files")]
    MissingIoArgs,
    #[error("Cannot read from {path}")]
    InputUnreadable { path: PathBuf },
}

/// Port of Python `check_command_line_options`. Given the raw
/// `argv`-style list (with argv[0] being the program name), returns
/// `(input_abspath, output_abspath)` or an error.
///
/// The special `.EXT` shorthand for output (write to the input's
/// basename with the given extension) is preserved.
///
/// `is_readable` is dependency-injected so tests can drive the
/// function without touching the real filesystem.
pub fn check_command_line_options<F>(
    args: &[String],
    is_readable: F,
) -> Result<(PathBuf, PathBuf), CliArgError>
where
    F: Fn(&Path) -> bool,
{
    if args.len() < 3 || args[1].starts_with('-') || args[2].starts_with('-') {
        return Err(CliArgError::MissingIoArgs);
    }
    let input_raw = &args[1];
    let input_abs = std::path::absolute(input_raw)
        .unwrap_or_else(|_| PathBuf::from(input_raw));

    // Python skips the readability check when help is requested; we
    // treat that as the caller's responsibility (they'll call print
    // help elsewhere).
    let is_recipe = input_raw.ends_with(".recipe");
    let effective_input = if is_recipe && !is_readable(&input_abs) {
        PathBuf::from(input_raw)
    } else {
        input_abs.clone()
    };

    if !is_recipe && !is_readable(&input_abs) {
        return Err(CliArgError::InputUnreadable { path: input_abs });
    }

    let raw_output = &args[2];
    let output = expand_output_shorthand(raw_output, &effective_input);
    let output_abs =
        std::path::absolute(&output).unwrap_or(output);

    Ok((effective_input, output_abs))
}

/// Python:
/// ```text
/// if (output.startswith('.') and output[:2] not in {'..', '.'} and '/' not in
///         output and '\\' not in output):
///     output = os.path.splitext(os.path.basename(input))[0] + output
/// ```
///
/// A raw output of `.epub` means "write beside the input with the
/// same base name but the `.epub` extension." Preserving this
/// exactly, including the guards against `.` / `..` / paths.
fn expand_output_shorthand(raw_output: &str, input: &Path) -> PathBuf {
    if raw_output.starts_with('.')
        && raw_output != "."
        && raw_output != ".."
        && !raw_output.contains('/')
        && !raw_output.contains('\\')
    {
        let base = input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        return PathBuf::from(format!("{base}{raw_output}"));
    }
    PathBuf::from(raw_output)
}

/// The Python USAGE banner. Kept as a runtime constant so callers can
/// prepend it to their own clap-generated help text.
pub const USAGE_BANNER: &str = "\
input_file output_file [options]

Convert an e-book from one format to another.

input_file is the input and output_file is the output. Both must be \
specified as the first two arguments to the command.

The output e-book format is guessed from the file extension of \
output_file. output_file can also be of the special format .EXT where \
EXT is the output file extension. In this case, the name of the output \
file is derived from the name of the input file. Note that the filenames \
must not start with a hyphen. Finally, if output_file has no extension, \
then it is treated as a folder and an \"open e-book\" (OEB) consisting \
of HTML files is written to that folder. These files are the files that \
would normally have been passed to the output plugin.

After specifying the input and output file you can customize the \
conversion by specifying various options. The available options depend \
on the input and output file types. To get help on them specify the \
input and output file and then use the -h option.

For full documentation of the conversion system see \
https://manual.calibre-ebook.com/conversion.html";

#[cfg(test)]
mod tests {
    use super::*;

    fn args(io: &[&str]) -> Vec<String> {
        let mut v = vec!["ebook-convert".to_string()];
        v.extend(io.iter().map(|s| s.to_string()));
        v
    }

    #[test]
    fn heuristic_options_and_default_true_options_have_expected_shape() {
        // HEURISTIC_OPTIONS should be nonzero and DEFAULT_TRUE_OPTIONS
        // strictly larger (adds "remove_fake_margins").
        assert!(!HEURISTIC_OPTIONS.is_empty());
        let dto = default_true_options();
        assert!(dto.len() == HEURISTIC_OPTIONS.len() + 1);
        assert!(dto.contains(&"remove_fake_margins"));
    }

    #[test]
    fn missing_io_args_errors() {
        let err = check_command_line_options(&args(&[]), |_| true).unwrap_err();
        assert!(matches!(err, CliArgError::MissingIoArgs));

        let err = check_command_line_options(&args(&["only-one"]), |_| true).unwrap_err();
        assert!(matches!(err, CliArgError::MissingIoArgs));
    }

    #[test]
    fn args_starting_with_dash_error() {
        let err = check_command_line_options(&args(&["-h", "out.epub"]), |_| true).unwrap_err();
        assert!(matches!(err, CliArgError::MissingIoArgs));

        let err = check_command_line_options(&args(&["book.epub", "-h"]), |_| true).unwrap_err();
        assert!(matches!(err, CliArgError::MissingIoArgs));
    }

    #[test]
    fn unreadable_input_errors_unless_recipe() {
        // A non-recipe input that isn't readable errors.
        let err = check_command_line_options(&args(&["book.epub", "out.pdf"]), |_| false)
            .unwrap_err();
        assert!(matches!(err, CliArgError::InputUnreadable { .. }));

        // A .recipe input that isn't readable is allowed through — the
        // Python original treats recipes specially since they're
        // often built-in identifiers, not on-disk files.
        let (input, _out) =
            check_command_line_options(&args(&["news.recipe", "out.epub"]), |_| false).unwrap();
        assert!(input.to_string_lossy().ends_with("news.recipe"));
    }

    #[test]
    fn output_shorthand_ext_uses_input_basename() {
        let (input, output) =
            check_command_line_options(&args(&["book.pdf", ".epub"]), |_| true).unwrap();
        // Output should be the input's base name + `.epub` extension.
        assert!(input.to_string_lossy().ends_with("book.pdf"));
        assert!(output.to_string_lossy().ends_with("book.epub"));
    }

    #[test]
    fn output_shorthand_does_not_activate_for_dot_or_dotdot() {
        // "." and ".." must NOT be treated as shorthand. Absolute-
        // path resolution will resolve `..` to the parent of CWD;
        // the important guarantee is that the shorthand expansion
        // (input basename + ".." suffix) did NOT run — the result
        // must NOT contain `book..`.
        let (_i, out) =
            check_command_line_options(&args(&["book.pdf", ".."]), |_| true).unwrap();
        assert!(
            !out.to_string_lossy().contains("book.."),
            "shorthand should not have activated; got {:?}",
            out
        );
    }

    #[test]
    fn output_shorthand_does_not_activate_when_output_contains_slash() {
        let (_i, out) =
            check_command_line_options(&args(&["book.pdf", "./sub/dir"]), |_| true).unwrap();
        // Absolute path derived from `./sub/dir` — must contain `sub`
        // as a component (not the shorthand behavior).
        assert!(out.to_string_lossy().contains("sub"));
    }

    #[test]
    fn usage_banner_mentions_input_and_output() {
        // Regression guard: the banner must actually mention what
        // the CLI does. If someone deletes the banner or truncates
        // it, this catches it.
        assert!(USAGE_BANNER.contains("input_file"));
        assert!(USAGE_BANNER.contains("output_file"));
        assert!(USAGE_BANNER.contains("Convert"));
    }
}
