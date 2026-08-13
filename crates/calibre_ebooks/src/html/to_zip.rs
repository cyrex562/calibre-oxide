//! The HTML-to-ZIP import plugin.
//!
//! Port of `old_src/src/calibre/ebooks/html/to_zip.py`.
//!
//! Adding a loose HTML file to a calibre library runs this: it follows
//! the file's local links (see [`super::input`]), converts the result,
//! and packages everything into a zip so the library holds one file
//! rather than a directory of scattered pieces.
//!
//! # What is here and what is not
//!
//! The plugin's `run()` drives the whole conversion pipeline through
//! `gui_convert(..., abort_after_input_dump=True)` and then packages
//! what the pipeline dumped. The pipeline half is not portable yet —
//! `calibre_conversion`'s plumber does not have the input-dump entry
//! point — so this module ports the two halves that are: the settings
//! parsing, and [`package_dump`], which turns a dumped input directory
//! into the zip. Wiring the middle together is #147.
//!
//! `do_user_config` is a Qt dialog and is out of scope, like the other
//! Qt surfaces in this repo.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::epub::{initialize_container, RootFile};

/// The file types the plugin claims on import.
pub const FILE_TYPES: [&str; 6] = ["html", "htm", "xhtml", "xhtm", "shtm", "shtml"];

/// The plugin's name, as calibre registers it.
pub const PLUGIN_NAME: &str = "HTML to ZIP";

/// Help text for the encoding setting.
///
/// Port of the Python `customization_help`.
pub const CUSTOMIZATION_HELP: &str = "Character encoding for the input HTML files. Common choices \
include: utf-8, cp1252, cp1251 and latin1.";

/// The plugin's per-user settings.
///
/// Port of what the Python `parse_my_settings` returns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// Input encoding to force, if any.
    pub encoding: Option<String>,
    /// Add linked files breadth-first rather than depth-first.
    pub breadth_first: bool,
    /// Allow resources outside the HTML file's own folder.
    pub allow_local_files_outside_root: bool,
}

/// Parse the plugin's site customization string.
///
/// Port of the Python `parse_my_settings`, which accepts two formats:
/// JSON, written by newer versions, and the older `encoding|bf` pair
/// kept for settings saved before the change. Malformed JSON yields
/// the default settings rather than an error, as upstream.
pub fn parse_settings(raw: &str) -> Settings {
    if raw.starts_with('{') {
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw) else {
            return Settings::default();
        };
        return Settings {
            encoding: map
                .get("encoding")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            breadth_first: map.get("breadth_first").map(truthy).unwrap_or(false),
            allow_local_files_outside_root: map
                .get("allow_local_files_outside_root")
                .map(truthy)
                .unwrap_or(false),
        };
    }
    // The legacy form: everything before the first `|` is the encoding,
    // and the flag is the literal `bf`.
    let trimmed = raw.trim();
    let (encoding, rest) = match trimmed.split_once('|') {
        Some((e, r)) => (e, r),
        None => (trimmed, ""),
    };
    Settings {
        encoding: (!encoding.is_empty()).then(|| encoding.to_string()),
        breadth_first: rest == "bf",
        allow_local_files_outside_root: false,
    }
}

/// Python's notion of truthiness for the values that reach these
/// settings.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// What [`package_dump`] found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packaged {
    /// The OPF the container points at, relative to the dump.
    pub opf_name: String,
    /// Every part written into the zip.
    pub entries: Vec<String>,
    /// An NCX that was removed before packaging, if there was one.
    pub removed_ncx: Option<PathBuf>,
}

/// Package a dumped input directory into an EPUB-shaped zip.
///
/// Port of the tail of the Python `run()`: find the OPF, delete any
/// NCX beside it, and write the directory into a container whose
/// `META-INF/container.xml` points at that OPF.
///
/// The NCX is removed because the dump's is the input plugin's own
/// working copy, and leaving it would have the zip advertise a table of
/// contents that the conversion has not built yet.
pub fn package_dump(dump_dir: &Path, output: &Path) -> anyhow::Result<Packaged> {
    let mut opf: Option<PathBuf> = None;
    let mut ncx: Option<PathBuf> = None;
    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dump_dir)? {
        let path = entry?.path();
        match path.extension().and_then(|e| e.to_str()) {
            Some("opf") if opf.is_none() => opf = Some(path.clone()),
            Some("ncx") if ncx.is_none() => ncx = Some(path.clone()),
            _ => {}
        }
        entries.push(path);
    }
    let opf = opf.ok_or_else(|| {
        anyhow::anyhow!(
            "no OPF in the dumped input directory {}",
            dump_dir.display()
        )
    })?;
    let opf_name = opf
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    if let Some(path) = &ncx {
        std::fs::remove_file(path)?;
        entries.retain(|e| e != path);
    }

    let file = std::fs::File::create(output)?;
    let mut zip = initialize_container(file, &opf_name, &[] as &[RootFile])?;
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut written = Vec::new();
    let mut stack: Vec<PathBuf> = entries;
    stack.sort();
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            let mut children: Vec<PathBuf> = std::fs::read_dir(&path)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .collect();
            children.sort();
            stack.extend(children);
            continue;
        }
        let name = path
            .strip_prefix(dump_dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        zip.start_file(&name, options)?;
        std::io::Write::write_all(&mut zip, &std::fs::read(&path)?)?;
        written.push(name);
    }
    zip.finish()?;
    written.sort();

    Ok(Packaged {
        opf_name,
        entries: written,
        removed_ncx: ncx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tempfile::TempDir;

    #[test]
    fn json_settings_are_read() {
        let s = parse_settings(r#"{"encoding": "cp1251", "breadth_first": true}"#);
        assert_eq!(s.encoding.as_deref(), Some("cp1251"));
        assert!(s.breadth_first);
        assert!(!s.allow_local_files_outside_root);

        let s = parse_settings(r#"{"allow_local_files_outside_root": true}"#);
        assert!(s.allow_local_files_outside_root);
        assert_eq!(s.encoding, None);
    }

    #[test]
    fn the_legacy_pipe_form_is_still_understood() {
        // Settings saved before the switch to JSON.
        let s = parse_settings("cp1252|bf");
        assert_eq!(s.encoding.as_deref(), Some("cp1252"));
        assert!(s.breadth_first);

        let s = parse_settings("utf-8");
        assert_eq!(s.encoding.as_deref(), Some("utf-8"));
        assert!(!s.breadth_first);

        // Anything other than `bf` after the bar is not the flag.
        let s = parse_settings("utf-8|df");
        assert!(!s.breadth_first);
    }

    #[test]
    fn empty_and_malformed_settings_give_the_defaults() {
        for raw in ["", "   ", "{not json", "{}"] {
            assert_eq!(parse_settings(raw), Settings::default(), "parsing {raw:?}");
        }
    }

    #[test]
    fn a_json_encoding_of_the_empty_string_counts_as_unset() {
        assert_eq!(parse_settings(r#"{"encoding": ""}"#).encoding, None);
    }

    fn dump(dir: &TempDir, files: &[(&str, &str)]) {
        for (name, content) in files {
            let path = dir.path().join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
        }
    }

    #[test]
    fn packaging_produces_a_readable_container() {
        let src = TempDir::new().unwrap();
        dump(
            &src,
            &[
                ("book.opf", "<package/>"),
                ("index.html", "<html><body>hi</body></html>"),
                ("images/a.png", "notreallyapng"),
            ],
        );
        let out = TempDir::new().unwrap();
        let zip_path = out.path().join("out.zip");

        let packaged = package_dump(src.path(), &zip_path).unwrap();
        assert_eq!(packaged.opf_name, "book.opf");
        assert!(packaged.entries.contains(&"index.html".to_string()));
        assert!(packaged.entries.contains(&"images/a.png".to_string()));

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        // The mimetype entry is first and stored, per the EPUB spec.
        assert_eq!(archive.by_index(0).unwrap().name(), "mimetype");
        let mut container = String::new();
        archive
            .by_name("META-INF/container.xml")
            .unwrap()
            .read_to_string(&mut container)
            .unwrap();
        assert!(container.contains(r#"full-path="book.opf""#), "{container}");
        let mut html = String::new();
        archive
            .by_name("index.html")
            .unwrap()
            .read_to_string(&mut html)
            .unwrap();
        assert_eq!(html, "<html><body>hi</body></html>");
    }

    #[test]
    fn the_ncx_is_removed_before_packaging() {
        let src = TempDir::new().unwrap();
        dump(
            &src,
            &[
                ("book.opf", "<package/>"),
                ("toc.ncx", "<ncx/>"),
                ("a.html", "x"),
            ],
        );
        let out = TempDir::new().unwrap();
        let zip_path = out.path().join("out.zip");

        let packaged = package_dump(src.path(), &zip_path).unwrap();
        assert!(packaged.removed_ncx.is_some());
        assert!(!packaged.entries.iter().any(|e| e.ends_with(".ncx")));
        assert!(
            !src.path().join("toc.ncx").exists(),
            "removed from the dump too"
        );
    }

    #[test]
    fn a_dump_without_an_opf_is_an_error() {
        let src = TempDir::new().unwrap();
        dump(&src, &[("index.html", "x")]);
        let out = TempDir::new().unwrap();
        let err = package_dump(src.path(), &out.path().join("o.zip")).unwrap_err();
        assert!(err.to_string().contains("no OPF"), "{err}");
    }

    #[test]
    fn the_claimed_file_types_are_the_ones_calibre_claims() {
        assert!(FILE_TYPES.contains(&"html"));
        assert!(FILE_TYPES.contains(&"shtml"));
        assert_eq!(FILE_TYPES.len(), 6);
    }
}
