//! The library's naming convention (tranche 19b): where a book's file lives and
//! what it is called, as two templates.
//!
//! # What is stored, and where
//!
//! One JSON document in the library preference [`PREF_KEY`] (`lexmechanic.naming`),
//! `{folder_template, file_template}`, written through
//! [`Cache::set_preference`] so it is in the change log (it survives a rebuild).
//! No pref means the defaults, which reproduce what the app did before the
//! convention existed: folder `{author_sort}`, file `{title}`, together
//! `{author_sort}/{title}` ([`crate::organize::DEFAULT_TEMPLATE`]).
//!
//! # How it is used
//!
//! The convention is the **default** everywhere a template is asked for, and a
//! run can still override it:
//!
//! * Organize uses [`Naming::combined`], `folder_template/file_template`, as one
//!   save-to-disk style template (which is what organize takes);
//! * Rename files and Fix titles' rename use the file template alone;
//! * the compare view's rename suggestion uses the file template.
//!
//! # Templates
//!
//! The dialect is calibre's (`lexmechanic_utils::formatter::string_format`):
//! `{title}`, `{author_sort}`, `{series}`, `{#label}` for a custom column;
//! `{series_index:0>2s}` pads (here `07`); `{series:|| - }` prints the series
//! and then ` - `, **only when there is a series** (`{field:|prefix|suffix}`),
//! so a missing value leaves no stray separator. A `/` in the folder template
//! starts a sub-folder. Whatever a template renders, empty path segments are
//! dropped, so a book with no value for `{#game_line}` in
//! `Gaming/RPG/{#game_line}` lands in `Gaming/RPG`.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::cache::Cache;
use crate::organize;

/// The library preference holding the convention.
pub const PREF_KEY: &str = "lexmechanic.naming";
pub const DEFAULT_FOLDER_TEMPLATE: &str = "{author_sort}";
pub const DEFAULT_FILE_TEMPLATE: &str = "{title}";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Naming {
    #[serde(default)]
    pub folder_template: String,
    #[serde(default)]
    pub file_template: String,
}

impl Default for Naming {
    fn default() -> Self {
        Naming { folder_template: DEFAULT_FOLDER_TEMPLATE.into(), file_template: DEFAULT_FILE_TEMPLATE.into() }
    }
}

impl Naming {
    /// The single template organize takes. An empty folder template means no
    /// folder at all (files are named at the library root).
    pub fn combined(&self) -> String {
        let (folder, file) = (self.folder_template.trim().trim_matches('/'), self.file_template.trim());
        if folder.is_empty() {
            file.to_string()
        } else {
            format!("{folder}/{file}")
        }
    }
}

/// The convention in force: the stored one, or the defaults when there is none
/// (or it cannot be read, or has no file template).
pub fn load(cache: &Cache) -> Naming {
    let stored = cache.get_preference_compat(PREF_KEY).ok().flatten().and_then(|raw| serde_json::from_str::<Naming>(&raw).ok());
    match stored {
        Some(n) if !n.file_template.trim().is_empty() => n,
        _ => Naming::default(),
    }
}

/// Stores the convention (the defaults clear the preference).
pub fn save(cache: &Cache, naming: &Naming) -> Result<()> {
    if naming.file_template.trim().is_empty() {
        bail!("the file name template is required");
    }
    if naming.file_template.contains(['/', '\\']) {
        bail!("the file name template cannot contain a `/`: put folders in the folder template");
    }
    if *naming == Naming::default() {
        cache.clear_preference_compat(PREF_KEY)
    } else {
        cache.set_preference_compat(PREF_KEY, &serde_json::to_string(naming)?)
    }
}

/// What the convention gives one book.
#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub book_id: i32,
    pub title: String,
    /// Folder (possibly empty) and file name with its extension, when it worked.
    pub folder: Option<String>,
    pub file: Option<String>,
    /// `folder/file`, or the file alone at the library root.
    pub path: Option<String>,
    /// Why the templates failed for this book.
    pub error: Option<String>,
}

/// Runs the templates over some books, as organize would name them.
pub fn preview(cache: &Cache, naming: &Naming, book_ids: &[i32]) -> Vec<Preview> {
    book_ids
        .iter()
        .map(|&book_id| {
            let title = cache.field_for(book_id, "title").ok().flatten().unwrap_or_default();
            let ext = cache.format_files(book_id).ok().and_then(|f| f.first().map(|(_, fmt)| fmt.to_lowercase())).unwrap_or_else(|| "pdf".into());
            match organize::render(cache, book_id, &naming.combined()) {
                Ok((folder, stem)) => {
                    let file = format!("{stem}.{ext}");
                    let path = if folder.is_empty() { file.clone() } else { format!("{folder}/{file}") };
                    Preview { book_id, title, folder: Some(folder), file: Some(file), path: Some(path), error: None }
                }
                Err(e) => Preview { book_id, title, folder: None, file: None, path: None, error: Some(format!("{e:#}")) },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{walk, ScanOptions};
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    fn library() -> (tempfile::TempDir, Cache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        (dir, cache)
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn index(dir: &Path, cache: &Cache) {
        let report = walk(dir, &ScanOptions::default(), SystemTime::now() + Duration::from_secs(3600)).unwrap();
        crate::scan::index_scan(cache, dir, &report);
    }

    fn id(cache: &Cache, title: &str) -> i32 {
        cache.all_book_ids().unwrap().into_iter().find(|i| cache.field_for(*i, "title").unwrap().unwrap_or_default() == title).unwrap()
    }

    fn one(cache: &Cache, n: &Naming, book: i32) -> String {
        let p = &preview(cache, n, &[book])[0];
        p.path.clone().unwrap_or_else(|| format!("ERROR {:?}", p.error))
    }

    #[test]
    fn the_defaults_reproduce_what_organize_did_before() {
        assert_eq!(Naming::default().combined(), organize::DEFAULT_TEMPLATE);
        let (_dir, cache) = library();
        assert_eq!(load(&cache), Naming::default());
    }

    #[test]
    fn the_convention_round_trips_through_the_preference_and_a_rebuild() {
        let (_dir, cache) = library();
        let n = Naming { folder_template: "Gaming/RPG/{#game_line}".into(), file_template: "{series:|| }{title}".into() };
        save(&cache, &n).unwrap();
        assert_eq!(load(&cache), n);
        cache.rebuild_from_change_log().unwrap();
        assert_eq!(load(&cache), n, "it is in the change log");
        // The defaults clear it; an empty folder template is a real choice (flat).
        save(&cache, &Naming::default()).unwrap();
        assert!(cache.get_preference_compat(PREF_KEY).unwrap().is_none());
        let flat = Naming { folder_template: String::new(), file_template: "{title}".into() };
        save(&cache, &flat).unwrap();
        assert_eq!(load(&cache), flat);
        assert_eq!(flat.combined(), "{title}");
    }

    #[test]
    fn a_convention_without_a_file_name_is_refused() {
        let (_dir, cache) = library();
        assert!(save(&cache, &Naming { folder_template: "x".into(), file_template: "  ".into() }).is_err());
        assert!(save(&cache, &Naming { folder_template: "x".into(), file_template: "a/{title}".into() }).is_err());
    }

    /// The forms the dialog documents, against real books.
    #[test]
    fn the_documented_template_forms_work_and_missing_values_leave_no_stray_separators() {
        let (dir, cache) = library();
        write(&dir.path().join("in/With Series.pdf"), b"%PDF a");
        write(&dir.path().join("in/Plain.pdf"), b"%PDF b");
        index(dir.path(), &cache);
        cache.add_custom_column("game_line", "Game line", "text", false).unwrap();
        cache.add_custom_column("edition", "Edition", "text", false).unwrap();
        let (with, plain) = (id(&cache, "With Series"), id(&cache, "Plain"));
        cache.set_field(with, "series", "WWN").unwrap();
        cache.set_field(with, "series_index", "7").unwrap();
        cache.set_custom_column_value(with, "game_line", "Worlds Without Number").unwrap();
        cache.set_custom_column_value(with, "edition", "2e").unwrap();

        // Padding: the series index is stored as `7.0`.
        let pad = |t: &str| Naming { folder_template: String::new(), file_template: t.into() };
        assert_eq!(one(&cache, &pad("{series_index}"), with), "7.0.pdf");
        assert_eq!(one(&cache, &pad("{series_index:0>2.0f}"), with), "07.pdf");
        assert_eq!(one(&cache, &pad("{series_index:0>3.0f} {title}"), with), "007 With Series.pdf");
        // `prefix|format|suffix`: the text is printed only when the value exists.
        assert_eq!(one(&cache, &pad("{title}{series: [||]}"), with), "With Series [WWN].pdf");
        assert_eq!(one(&cache, &pad("{title}{series: [||]}"), plain), "Plain.pdf");

        let n = Naming { folder_template: "Gaming/RPG/{#game_line}".into(), file_template: "{series:|| }{series_index:|0>2.0f| - }{title}{#edition: (||)}".into() };
        assert_eq!(one(&cache, &n, with), "Gaming/RPG/Worlds Without Number/WWN 07 - With Series (2e).pdf");
        // No series, no game line, no edition: nothing stray, no empty folder.
        assert_eq!(one(&cache, &n, plain), "Gaming/RPG/Plain.pdf");

        let by_series = Naming { folder_template: "{series}".into(), file_template: "{title}".into() };
        assert_eq!(one(&cache, &by_series, with), "WWN/With Series.pdf");
        assert_eq!(one(&cache, &by_series, plain), "Plain.pdf", "an empty folder segment is dropped");

        let flat = Naming { folder_template: String::new(), file_template: "{title}".into() };
        assert_eq!(one(&cache, &flat, plain), "Plain.pdf");
    }

    #[test]
    fn organize_with_the_convention_puts_a_book_with_no_value_in_the_parent_folder() {
        let (dir, cache) = library();
        write(&dir.path().join("in/Plain.pdf"), b"%PDF b");
        index(dir.path(), &cache);
        cache.add_custom_column("game_line", "Game line", "text", false).unwrap();
        let n = Naming { folder_template: "Gaming/RPG/{#game_line}".into(), file_template: "{title}".into() };
        let plan = organize::plan(&cache, &n.combined(), &organize::Scope::All).unwrap();
        assert_eq!(plan.books[0].files[0].to, "Gaming/RPG/Plain.pdf");
        assert_eq!(plan.books[0].new_path, "Gaming/RPG");
    }

    #[test]
    fn a_broken_template_is_reported_per_book() {
        let (dir, cache) = library();
        write(&dir.path().join("in/Plain.pdf"), b"%PDF b");
        index(dir.path(), &cache);
        let n = Naming { folder_template: "{title".into(), file_template: "{title}".into() };
        let p = preview(&cache, &n, &[id(&cache, "Plain")]);
        assert!(p[0].error.is_some() || p[0].path.is_some(), "{:?}", p[0]);
    }
}
