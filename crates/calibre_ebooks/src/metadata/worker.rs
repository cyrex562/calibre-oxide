//! The metadata side of calibre's "add books" pipeline.
//!
//! Port of `src/calibre/ebooks/metadata/worker.py`. In the Python this
//! runs inside a job-system worker process; here it's a set of plain,
//! synchronous functions — calibre-oxide has no multiprocessing job
//! runner yet, and none of this module's own logic depends on being in
//! a subprocess, so the port drops the process boundary and keeps the
//! behavior.
//!
//! Two dependencies genuinely don't exist yet and are approximated
//! rather than faked with a `todo!`:
//!
//!  * `customize.ui.run_plugins_on_import` needs a plugin *registry*,
//!    which calibre-oxide doesn't have (see
//!    [`calibre_customize::builtins`]). [`run_import_plugins`] takes the
//!    plugin list as a parameter instead of reaching into a global one;
//!    an empty slice reproduces the Python's current real-world
//!    behavior exactly, since calibre-oxide has no built-in
//!    `FileTypePlugin`s registered either.
//!  * `metadata_from_formats`/`Metadata.smart_update` belong to
//!    `ebooks/metadata/meta.py` and `ebooks/metadata/__init__.py`,
//!    neither of which is tracked as ported yet. [`metadata_from_formats`]
//!    reimplements the part `worker.py` actually needs — OPF-sidecar
//!    priority, then first-usable-format-wins per field — documented
//!    at the function.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use calibre_customize::FileTypePlugin;
use calibre_utils::filenames::samefile;
use calibre_utils::icu;

use crate::metadata::MetaInformation;

/// `METADATA_PRIORITIES` in `meta.py` — formats not in this list get
/// priority 0, which is lower than everything listed (Python's
/// `collections.defaultdict(int)`).
const METADATA_PRIORITY_ORDER: [&str; 20] = [
    "html", "htm", "xhtml", "xhtm", "rtf", "fb2", "pdf", "prc", "odt", "epub", "lit", "lrx", "lrf",
    "mobi", "azw", "azw3", "azw1", "rb", "imp", "snb",
];

fn metadata_priority(ext: &str) -> usize {
    METADATA_PRIORITY_ORDER
        .iter()
        .position(|&e| e == ext)
        .map_or(0, |i| i + 1)
}

fn path_ext(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// Build an accumulator with every field genuinely empty, the way
/// Python's `MetaInformation(None, None)` starts — unlike
/// [`MetaInformation::default`], which fills in placeholders like
/// `"Unknown"` for display purposes.
fn empty_accumulator() -> MetaInformation {
    MetaInformation {
        title: String::new(),
        authors: Vec::new(),
        title_sort: None,
        author_sort: None,
        author_sort_map: HashMap::new(),
        publisher: None,
        tags: Vec::new(),
        series: None,
        series_index: 1.0,
        rating: None,
        pubdate: None,
        timestamp: None,
        comments: None,
        languages: Vec::new(),
        identifiers: HashMap::new(),
        user_metadata: HashMap::new(),
        cover_id: None,
        cover_data: (None, Vec::new()),
        uuid: None,
    }
}

/// Whether a title is unset, in the sense that matters for merging:
/// either genuinely empty, or the placeholder [`MetaInformation::default`]
/// fills in. Every per-format reader in this crate starts from
/// `MetaInformation::default()`, so a format it couldn't actually read
/// still comes back with `title == "Unknown"` — treating that as "no
/// title" is what lets a later, more informative format win.
fn is_placeholder_title(title: &str) -> bool {
    title.is_empty() || title == "Unknown"
}

fn is_placeholder_authors(authors: &[String]) -> bool {
    authors.is_empty() || authors == ["Unknown".to_string()]
}

fn is_placeholder_languages(languages: &[String]) -> bool {
    languages.is_empty() || languages == ["und".to_string()]
}

/// Fold `new` into `acc`, field by field, keeping whatever `acc`
/// already has and filling in the rest from `new`.
///
/// This is not `calibre.ebooks.metadata.Metadata.smart_update` — that
/// method isn't ported yet (see the module docs) — but it captures the
/// behavior `metadata_from_formats` actually relies on: the first
/// format tried that has real data for a field keeps it.
fn merge_into(acc: &mut MetaInformation, new: MetaInformation) {
    if is_placeholder_title(&acc.title) && !is_placeholder_title(&new.title) {
        acc.title = new.title;
    }
    if is_placeholder_authors(&acc.authors) && !is_placeholder_authors(&new.authors) {
        acc.authors = new.authors;
    }
    if acc.title_sort.is_none() {
        acc.title_sort = new.title_sort;
    }
    if acc.author_sort.is_none() {
        acc.author_sort = new.author_sort;
    }
    for (k, v) in new.author_sort_map {
        acc.author_sort_map.entry(k).or_insert(v);
    }
    if acc.publisher.is_none() {
        acc.publisher = new.publisher;
    }
    if acc.tags.is_empty() {
        acc.tags = new.tags;
    } else {
        for tag in new.tags {
            if !acc.tags.contains(&tag) {
                acc.tags.push(tag);
            }
        }
    }
    if acc.series.is_none() && new.series.is_some() {
        acc.series = new.series;
        acc.series_index = new.series_index;
    }
    if acc.rating.is_none() {
        acc.rating = new.rating;
    }
    if acc.pubdate.is_none() {
        acc.pubdate = new.pubdate;
    }
    if acc.timestamp.is_none() {
        acc.timestamp = new.timestamp;
    }
    if acc.comments.is_none() {
        acc.comments = new.comments;
    }
    if is_placeholder_languages(&acc.languages) && !is_placeholder_languages(&new.languages) {
        acc.languages = new.languages;
    }
    for (k, v) in new.identifiers {
        acc.identifiers.entry(k).or_insert(v);
    }
    for (k, v) in new.user_metadata {
        acc.user_metadata.entry(k).or_insert(v);
    }
    if acc.cover_id.is_none() {
        acc.cover_id = new.cover_id;
    }
    if acc.cover_data.1.is_empty() && !new.cover_data.1.is_empty() {
        acc.cover_data = new.cover_data;
    }
    if acc.uuid.is_none() {
        acc.uuid = new.uuid;
    }
}

/// `metadata_from_formats` in `meta.py`: read one [`MetaInformation`]
/// out of a book's set of format files.
///
/// An accompanying `.opf` wins outright if it parses and has a real
/// title — it's assumed to hold curated metadata. Otherwise every
/// format is tried in `METADATA_PRIORITIES` order (ascending, so
/// unlisted extensions — priority 0 — go first, exactly as the Python
/// sorts) and merged field by field via [`merge_into`]. If nothing
/// yields a title or authors, both fall back to `"Unknown"`, matching
/// the Python's final fallback.
///
/// The Python's version of the OPF check is narrower: `opf_metadata`
/// only returns a result at all when the OPF carries a
/// `dc:identifier` in calibre's own `application_id` scheme — i.e.
/// only for a sidecar calibre itself wrote, not an arbitrary foreign
/// OPF. [`MetaInformation`] has no `application_id` field to check, so
/// this accepts any OPF that parses and has a real title instead.
pub fn metadata_from_formats(paths: &[PathBuf]) -> MetaInformation {
    let mut sorted: Vec<PathBuf> = paths.to_vec();
    sorted.sort_by_key(|p| metadata_priority(&path_ext(p)));

    if let Some(opf_path) = sorted.iter().find(|p| path_ext(p) == "opf") {
        if let Ok(text) = fs::read_to_string(opf_path) {
            if let Ok(mi2) = crate::opf::parse_opf(&text) {
                // `parse_opf` starts from `MetaInformation::default()`
                // and only overwrites the title when it finds real
                // text, so a titleless OPF parses to "Unknown" here
                // too — the same placeholder-as-null convention the
                // rest of this module uses (and that calibre's own
                // `Metadata` class documents: `_('Unknown')` "is
                // null").
                if !is_placeholder_title(&mi2.title) {
                    return mi2;
                }
            }
        }
    }

    let mut mi = empty_accumulator();
    for path in &sorted {
        if let Ok(newmi) = crate::metadata::get_metadata(path) {
            merge_into(&mut mi, newmi);
        }
    }
    if is_placeholder_title(&mi.title) {
        mi.title = "Unknown".to_string();
    }
    if is_placeholder_authors(&mi.authors) {
        mi.authors = vec!["Unknown".to_string()];
    }
    mi
}

/// `run_plugins_on_import`, minus the global plugin registry — see the
/// module docs.
///
/// Preserves the original basename but adopts whatever extension the
/// last plugin produced, exactly as `run_import_plugins` in the
/// Python: `os.replace` if possible, falling back to a copy (the
/// Python falls back to `shutil.copyfile` on `OSError`, which on Unix
/// is what a cross-filesystem `rename` raises).
///
/// Files that aren't readable are dropped from the result entirely —
/// this is what the Python's `if not os.access(path, os.R_OK): continue`
/// does, and callers rely on it to filter unreadable inputs out of the
/// paths going on to the rest of the pipeline.
pub fn run_import_plugins(
    paths: &[PathBuf],
    group_id: &str,
    tdir: &Path,
    plugins: &[Box<dyn FileTypePlugin>],
) -> Result<Vec<PathBuf>> {
    let mut final_paths = Vec::with_capacity(paths.len());
    for path in paths {
        if fs::File::open(path).is_err() {
            continue;
        }

        let nfp = calibre_customize::ui::run_plugins_on_import(path, plugins);
        let mut path = path.clone();
        if fs::File::open(&nfp).is_ok() && !samefile(&nfp, &path) {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let ext = nfp
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            let dest = tdir.join(group_id).join(format!("{name}{ext}"));
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("worker: create {parent:?} for imported file"))?;
            }
            if fs::rename(&nfp, &dest).is_err() {
                fs::copy(&nfp, &dest)
                    .with_context(|| format!("worker: copy {nfp:?} to {dest:?}"))?;
            }
            path = dest;
        }
        final_paths.push(path);
    }
    Ok(final_paths)
}

/// `serialize_metadata_for` in `worker.py`: read metadata from `paths`,
/// pull the cover out to its own file, and serialize the rest to OPF.
///
/// The Python also stamps a dummy `application_id` when one is
/// missing; our [`MetaInformation`] has no such field to stamp — it's
/// a calibre-internal bookkeeping id our struct doesn't model — so
/// that step is dropped rather than faked. `uuid` genuinely is
/// modeled, and gets the same "generate one if missing" treatment
/// `metadata_to_opf(mi, default_lang=...)` gives it, since an OPF
/// without an identifier is not useful as a sidecar.
pub fn serialize_metadata_for(
    paths: &[PathBuf],
    tdir: &Path,
    group_id: &str,
) -> Result<(MetaInformation, String, bool)> {
    let mut mi = metadata_from_formats(paths);
    let cdata = std::mem::take(&mut mi.cover_data.1);
    mi.cover_data = (None, Vec::new());
    if mi.uuid.is_none() {
        mi.uuid = Some(uuid::Uuid::new_v4().to_string());
    }
    let opf = mi.to_xml();

    let mut has_cover = false;
    if !cdata.is_empty() {
        fs::write(tdir.join(format!("{group_id}.cdata")), &cdata)
            .context("worker: write cover data")?;
        has_cover = true;
    }
    Ok((mi, opf, has_cover))
}

/// The result of [`read_metadata_bulk`]: `read_metadata_bulk`'s
/// `{'opf': ..., 'cdata': ...}` dict, typed.
#[derive(Debug, Default, Clone)]
pub struct BulkMetadata {
    pub opf: Option<String>,
    pub cdata: Option<Vec<u8>>,
}

/// `read_metadata_bulk` in `worker.py`: like [`serialize_metadata_for`]
/// but without writing anything to disk, and with each half of the
/// result independently optional.
pub fn read_metadata_bulk(get_opf: bool, get_cover: bool, paths: &[PathBuf]) -> BulkMetadata {
    let mut mi = metadata_from_formats(paths);
    let cdata = std::mem::take(&mut mi.cover_data.1);
    mi.cover_data = (None, Vec::new());
    if mi.uuid.is_none() {
        mi.uuid = Some(uuid::Uuid::new_v4().to_string());
    }
    BulkMetadata {
        opf: get_opf.then(|| mi.to_xml()),
        cdata: get_cover.then_some(cdata).filter(|c| !c.is_empty()),
    }
}

/// `has_book` in `worker.py`: is this book's title already present in
/// a caller-supplied set of (already-lowercased) known titles?
pub fn has_book(mi: &MetaInformation, data_for_has_book: &HashSet<String>) -> bool {
    let title = mi.title.trim();
    !title.is_empty() && data_for_has_book.contains(&icu::lower(title))
}

/// `read_metadata` in `worker.py`: run import plugins, serialize
/// metadata, and optionally flag a likely duplicate against
/// `common_data`.
///
/// `common_data` stands in for the Python's `isinstance(common_data,
/// (set, frozenset))` check — `Option` already expresses "was a
/// dedup set supplied at all", so there's nothing further to test.
pub fn read_metadata(
    paths: &[PathBuf],
    group_id: &str,
    tdir: &Path,
    common_data: Option<&HashSet<String>>,
    plugins: &[Box<dyn FileTypePlugin>],
) -> Result<(Vec<PathBuf>, String, bool, Option<bool>)> {
    let paths = run_import_plugins(paths, group_id, tdir, plugins)?;
    let (mi, opf, has_cover) = serialize_metadata_for(&paths, tdir, group_id)?;
    let duplicate_info = common_data.map(|set| has_book(&mi, set));
    Ok((paths, opf, has_cover, duplicate_info))
}

#[cfg(test)]
mod tests {
    use super::*;
    use calibre_customize::Plugin;

    fn write_txt(dir: &Path, name: &str, title: &str, author: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("{title}\n\n\n{author}\n")).expect("write txt");
        path
    }

    /// `annotation` must be plain text with no child elements —
    /// `fb2.rs`'s annotation reader only takes the element's direct
    /// text node, not descendant text.
    fn write_fb2(dir: &Path, name: &str, title: &str, annotation: &str) -> PathBuf {
        let path = dir.join(name);
        let xml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0">
  <description>
    <title-info>
      <book-title>{title}</book-title>
      <annotation>{annotation}</annotation>
    </title-info>
  </description>
  <body><section><p>Body</p></section></body>
</FictionBook>"#
        );
        fs::write(&path, xml).expect("write fb2");
        path
    }

    fn write_opf(dir: &Path, name: &str, title: &str) -> PathBuf {
        let path = dir.join(name);
        let xml = format!(
            r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uuid_id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>{title}</dc:title>
  </metadata>
</package>"#
        );
        fs::write(&path, xml).expect("write opf");
        path
    }

    #[test]
    fn metadata_priority_matches_the_pythons_list_order() {
        assert_eq!(metadata_priority("html"), 1);
        assert_eq!(metadata_priority("snb"), 20);
        // Not in METADATA_PRIORITIES: defaultdict(int) => 0.
        assert_eq!(metadata_priority("opf"), 0);
        assert_eq!(metadata_priority("txt"), 0);
    }

    #[test]
    fn metadata_from_formats_prefers_an_opf_sidecar_with_a_title() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let opf = write_opf(tmp.path(), "book.opf", "OPF Title");
        let txt = write_txt(tmp.path(), "book.txt", "TXT Title", "TXT Author");
        let mi = metadata_from_formats(&[txt, opf]);
        assert_eq!(mi.title, "OPF Title");
        // The OPF short-circuits entirely; no author was in it.
        assert!(mi.authors.is_empty() || mi.authors == ["Unknown".to_string()]);
    }

    #[test]
    fn metadata_from_formats_ignores_a_titleless_opf() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let opf = write_opf(tmp.path(), "book.opf", "");
        let txt = write_txt(tmp.path(), "book.txt", "TXT Title", "TXT Author");
        let mi = metadata_from_formats(&[opf, txt]);
        assert_eq!(mi.title, "TXT Title");
        assert_eq!(mi.authors, vec!["TXT Author".to_string()]);
    }

    #[test]
    fn metadata_from_formats_merges_fields_across_formats() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // txt (priority 0) is tried before fb2 (priority 6): its title
        // wins, and fb2's title is discarded, but fb2's annotation
        // still fills the comments field txt never sets.
        let txt = write_txt(tmp.path(), "book.txt", "TXT Title", "TXT Author");
        let fb2 = write_fb2(tmp.path(), "book.fb2", "FB2 Title", "An annotation.");
        let mi = metadata_from_formats(&[fb2, txt]);
        assert_eq!(mi.title, "TXT Title");
        assert_eq!(mi.authors, vec!["TXT Author".to_string()]);
        assert_eq!(mi.comments.as_deref(), Some("An annotation."));
    }

    #[test]
    fn metadata_from_formats_falls_back_to_unknown() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let txt = write_txt(
            tmp.path(),
            "garbage.txt",
            "just one line, no pattern match",
            "",
        );
        // Overwrite with content that doesn't match the title/author
        // pattern at all.
        fs::write(&txt, b"no blank line pattern here").expect("write");
        let mi = metadata_from_formats(std::slice::from_ref(&txt));
        assert_eq!(mi.title, "Unknown");
        assert_eq!(mi.authors, vec!["Unknown".to_string()]);
    }

    #[test]
    fn metadata_from_formats_skips_unreadable_paths() {
        let mi = metadata_from_formats(&[PathBuf::from("/does/not/exist.txt")]);
        assert_eq!(mi.title, "Unknown");
    }

    struct RenameToOpf;
    impl Plugin for RenameToOpf {
        fn name(&self) -> &str {
            "rename to opf"
        }
    }
    impl FileTypePlugin for RenameToOpf {
        fn file_types(&self) -> Vec<String> {
            vec!["txt".to_string()]
        }
        fn on_import(&self) -> bool {
            true
        }
        fn run(&self, path_to_ebook: &Path) -> PathBuf {
            let renamed = path_to_ebook.with_extension("renamed");
            fs::copy(path_to_ebook, &renamed).expect("copy");
            renamed
        }
    }

    #[test]
    fn run_import_plugins_relocates_a_transformed_file_into_the_group_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = write_txt(tmp.path(), "book.txt", "T", "A");
        let plugins: Vec<Box<dyn FileTypePlugin>> = vec![Box::new(RenameToOpf)];

        let out = run_import_plugins(&[src.clone()], "42", tmp.path(), &plugins).expect("ok");
        assert_eq!(out.len(), 1);
        assert!(out[0].ends_with("book.renamed"));
        assert!(out[0].starts_with(tmp.path().join("42")));
        assert!(out[0].exists());
    }

    #[test]
    fn run_import_plugins_leaves_untouched_files_in_place() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = write_txt(tmp.path(), "book.txt", "T", "A");
        let out = run_import_plugins(&[src.clone()], "1", tmp.path(), &[]).expect("ok");
        assert_eq!(out, vec![src]);
    }

    #[test]
    fn run_import_plugins_drops_unreadable_paths() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("nope.txt");
        let out = run_import_plugins(&[missing], "1", tmp.path(), &[]).expect("ok");
        assert!(out.is_empty());
    }

    #[test]
    fn serialize_metadata_for_writes_cover_data_and_returns_opf() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let txt = write_txt(tmp.path(), "book.txt", "Cover Book", "An Author");
        let (mi, opf, has_cover) =
            serialize_metadata_for(&[txt], tmp.path(), "g1").expect("serializes");
        assert_eq!(mi.title, "Cover Book");
        assert!(opf.contains("<dc:title>Cover Book</dc:title>"));
        // No format here actually carries cover bytes.
        assert!(!has_cover);
        assert!(!tmp.path().join("g1.cdata").exists());
    }

    #[test]
    fn read_metadata_bulk_respects_the_get_flags() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let txt = write_txt(tmp.path(), "book.txt", "Bulk Book", "An Author");
        let ans = read_metadata_bulk(true, false, &[txt.clone()]);
        assert!(ans.opf.is_some());
        assert!(ans.cdata.is_none());

        let ans = read_metadata_bulk(false, true, &[txt]);
        assert!(ans.opf.is_none());
        // No cover in this fixture, so cdata is None even though it
        // was asked for.
        assert!(ans.cdata.is_none());
    }

    #[test]
    fn has_book_matches_case_insensitively_on_trimmed_title() {
        let mut mi = MetaInformation::default();
        mi.title = "  The Great Gatsby  ".to_string();
        let mut known = HashSet::new();
        known.insert("the great gatsby".to_string());
        assert!(has_book(&mi, &known));

        let mut other = HashSet::new();
        other.insert("moby dick".to_string());
        assert!(!has_book(&mi, &other));
    }

    #[test]
    fn has_book_is_false_for_an_empty_title() {
        let mut mi = MetaInformation::default();
        mi.title = "   ".to_string();
        let known: HashSet<String> = HashSet::new();
        assert!(!has_book(&mi, &known));
    }

    #[test]
    fn read_metadata_reports_duplicates_only_when_asked() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let txt = write_txt(tmp.path(), "book.txt", "Dup Title", "An Author");

        let (_, _, _, dup) =
            read_metadata(&[txt.clone()], "1", tmp.path(), None, &[]).expect("reads");
        assert_eq!(dup, None);

        let mut known = HashSet::new();
        known.insert("dup title".to_string());
        let (_, _, _, dup) =
            read_metadata(&[txt], "1", tmp.path(), Some(&known), &[]).expect("reads");
        assert_eq!(dup, Some(true));
    }
}
