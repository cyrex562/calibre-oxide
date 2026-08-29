//! Port of `old_src/src/calibre/ebooks/rtf2xml/output.py` (`Output`).
//!
//! Decides where the finished XML output goes: a directory (deriving
//! the file name from the original RTF's basename, or an explicit
//! `out_file` name within that directory), an explicit file path, or
//! stdout. Unlike every other file in this crate's rtf2xml port, this
//! one is genuinely about file I/O rather than a transform between
//! intermediate-format pipeline stages -- there's no "reopen a temp
//! file" plumbing to skip here, this *is* the pipeline's actual final
//! output step, so it's ported as real file/stdout writes taking the
//! already-converted XML content directly (rather than reopening
//! `self.__file` as Python does, matching the read-side convention
//! every other module in this port already uses).
//!
//! # A dropped interactive-overwrite prompt
//!
//! `__output_to_dir_func`'s `not self.__no_ask` branch calls Python's
//! builtin `input()` to interactively ask the user whether to
//! overwrite an existing output file (after printing the prompt via
//! `sys.stderr.write`). There's no terminal-interaction equivalent
//! available to a pure library function, so [`output_to_dir`] instead
//! returns [`OutputError::NeedsConfirmation`] when the target already
//! exists and `no_ask` is `false` -- preserving the *intent* (never
//! silently overwrite without permission) without literally blocking
//! on stdin. `no_ask: true` (the default in both Python and here)
//! always overwrites directly, matching Python's own default.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum OutputError {
    #[error(transparent)]
    Io(#[from] io::Error),
    /// See this module's own doc for why this replaces Python's
    /// interactive overwrite prompt.
    #[error("{0:?} already exists; confirm and pass no_ask: true to overwrite, or move it aside first")]
    NeedsConfirmation(PathBuf),
}

pub type Result<T> = std::result::Result<T, OutputError>;

/// Options `Output.__init__` takes beyond the content and original
/// file path. No `Default` impl: Python's own default for `no_ask` is
/// `True`, which a derived `Default` (`bool`'s own default is
/// `false`) would silently get backwards -- construct explicitly.
#[derive(Debug, Clone)]
pub struct OutputOptions<'a> {
    pub output_dir: Option<&'a Path>,
    pub out_file: Option<&'a str>,
    pub no_ask: bool,
}

/// Port of `Output.output`.
pub fn output(content: &str, orig_file: &Path, opts: &OutputOptions) -> Result<()> {
    if let Some(output_dir) = opts.output_dir {
        output_to_dir(content, orig_file, output_dir, opts.out_file, opts.no_ask)
    } else if let Some(out_file) = opts.out_file {
        output_to_file(content, Path::new(out_file))
    } else {
        output_to_standard(content)
    }
}

/// Port of `__output_to_dir_func`. `os.path.splitext`'s "everything
/// before the last dot" is the same rule as [`Path::file_stem`], so
/// this doesn't need to reimplement it.
pub fn output_to_dir(
    content: &str,
    orig_file: &Path,
    output_dir: &Path,
    out_file: Option<&str>,
    no_ask: bool,
) -> Result<()> {
    let output_path = match out_file {
        Some(name) => output_dir.join(name),
        None => {
            let base_name = orig_file.file_stem().unwrap_or_default();
            output_dir.join(format!("{}.xml", base_name.to_string_lossy()))
        }
    };
    if output_path.is_file() && !no_ask {
        return Err(OutputError::NeedsConfirmation(output_path));
    }
    fs::write(output_path, content)?;
    Ok(())
}

/// Port of `__output_to_file_func`.
pub fn output_to_file(content: &str, path: &Path) -> Result<()> {
    fs::write(path, content)?;
    Ok(())
}

/// Port of `__output_to_standard_func`.
pub fn output_to_standard(content: &str) -> Result<()> {
    io::stdout().write_all(content.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_to_dir_derives_the_file_name_from_the_original_rtf_basename() {
        let dir = tempfile::tempdir().unwrap();
        let orig = Path::new("/some/where/report.rtf");
        output_to_dir("<doc/>", orig, dir.path(), None, true).unwrap();
        let written = fs::read_to_string(dir.path().join("report.xml")).unwrap();
        assert_eq!(written, "<doc/>");
    }

    #[test]
    fn output_to_dir_uses_an_explicit_out_file_name_when_given() {
        let dir = tempfile::tempdir().unwrap();
        let orig = Path::new("report.rtf");
        output_to_dir("<doc/>", orig, dir.path(), Some("custom.xml"), true).unwrap();
        let written = fs::read_to_string(dir.path().join("custom.xml")).unwrap();
        assert_eq!(written, "<doc/>");
        assert!(!dir.path().join("report.xml").exists());
    }

    #[test]
    fn output_to_dir_overwrites_silently_when_no_ask_is_true() {
        let dir = tempfile::tempdir().unwrap();
        let orig = Path::new("report.rtf");
        fs::write(dir.path().join("report.xml"), "old").unwrap();
        output_to_dir("new", orig, dir.path(), None, true).unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("report.xml")).unwrap(), "new");
    }

    #[test]
    fn output_to_dir_asks_for_confirmation_when_the_target_exists_and_no_ask_is_false() {
        let dir = tempfile::tempdir().unwrap();
        let orig = Path::new("report.rtf");
        fs::write(dir.path().join("report.xml"), "old").unwrap();
        let err = output_to_dir("new", orig, dir.path(), None, false).unwrap_err();
        assert!(matches!(err, OutputError::NeedsConfirmation(_)));
        // The existing file is untouched.
        assert_eq!(fs::read_to_string(dir.path().join("report.xml")).unwrap(), "old");
    }

    #[test]
    fn output_to_dir_writes_directly_when_the_target_does_not_exist_even_with_no_ask_false() {
        let dir = tempfile::tempdir().unwrap();
        let orig = Path::new("report.rtf");
        output_to_dir("<doc/>", orig, dir.path(), None, false).unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("report.xml")).unwrap(), "<doc/>");
    }

    #[test]
    fn output_to_file_writes_to_the_exact_path_given() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wherever.xml");
        output_to_file("<doc/>", &path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "<doc/>");
    }

    #[test]
    fn output_dispatches_to_dir_when_output_dir_is_set_even_if_out_file_is_too() {
        let dir = tempfile::tempdir().unwrap();
        let orig = Path::new("report.rtf");
        let opts = OutputOptions { output_dir: Some(dir.path()), out_file: Some("named.xml"), no_ask: true };
        output("<doc/>", orig, &opts).unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("named.xml")).unwrap(), "<doc/>");
    }

    #[test]
    fn output_dispatches_to_file_when_only_out_file_is_set() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("named.xml");
        let opts = OutputOptions { output_dir: None, out_file: Some(path.to_str().unwrap()), no_ask: true };
        output("<doc/>", Path::new("report.rtf"), &opts).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "<doc/>");
    }
}
