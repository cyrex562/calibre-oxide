//! The file name a rename template gives a book (shared by `POST /rename-files`
//! and the "Fix titles" run, [`crate::retitle`]).
//!
//! One function, so a preview and the rename it previews cannot disagree:
//! template evaluation, taking the last `/` component, sanitizing, avoiding the
//! files already in the book's folder, and avoiding the names this batch has
//! already promised to other books in the same folder.

use std::collections::HashSet;

use lexmechanic_utils::filenames::sanitize_file_name;
use lexmechanic_utils::formatter::string_format;

use crate::cache::Cache;
use crate::formatter_functions::{CacheCatalog, CacheFunctions, CacheValueSource};

/// Evaluates `template` for one book and resolves it to a usable, unclaimed
/// file name stem.
///
/// `title_override` evaluates the template as if the book's title were that
/// value (the DB is not touched): the name a suggested title *would* give.
///
/// `claimed` carries the `(folder, stem)` pairs already handed out in this
/// batch. Two books can share a folder, and a template keyed on something they
/// share gives both the same name, so the batch has to remember what it has
/// promised, not just what is on disk.
pub fn propose_stem(cache: &Cache, book_id: i32, template: &str, title_override: Option<&str>, claimed: &mut HashSet<(String, String)>) -> Result<String, String> {
    let mut value_source = CacheValueSource::new(cache, book_id).map_err(|e| e.to_string())?;
    if let Some(title) = title_override {
        value_source = value_source.with_override("title", title);
    }
    let functions = CacheFunctions::new(cache, book_id);
    let rendered = string_format::evaluate_template(template, &value_source, &CacheCatalog, &functions)?;

    // A filename, not a path: a template written for save-to-disk has `/` in
    // it, and the last component is the part that names the file.
    let last = rendered.rsplit('/').next().unwrap_or(&rendered).trim();
    let stem = sanitize_file_name(last);
    if stem.trim().is_empty() {
        return Err("the template produced an empty filename".to_string());
    }

    let folder = cache.field_for(book_id, "path").ok().flatten().unwrap_or_default();
    let mut stem = cache.available_format_stem(book_id, &stem).map_err(|e| e.to_string())?;
    let mut suffix = 1;
    while !claimed.insert((folder.clone(), stem.clone())) {
        stem = format!("{} ({suffix})", sanitize_file_name(last));
        suffix += 1;
        if suffix > 1000 {
            return Err("too many books in one folder want this name".to_string());
        }
    }
    Ok(stem)
}
