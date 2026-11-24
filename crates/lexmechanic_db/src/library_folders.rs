//! The library's folder tree (tranche 19b): which folders hold books, how many,
//! and the empty folders the user made to fill later.
//!
//! # Where the tree comes from
//!
//! A book's `path` is its folder, relative to the library root. [`list`] reads
//! every `books.path` in **one query** and counts, for each folder and each of
//! its parents, the books at or under it (`books`) and directly in it
//! (`books_here`). Nothing walks the disk, so it is cheap on a library of tens
//! of thousands of books on a NAS.
//!
//! # Empty folders
//!
//! A folder with no books has no `books.path` to show it. Two ways to keep it:
//! walk the disk for empty directories (slow on a NAS, and it would also show
//! every stray empty directory), or **remember the ones the user made**. This
//! remembers them, in the library preference [`EMPTY_FOLDERS_PREF`] (a JSON
//! list of folders; it goes through the change log like any other preference).
//! "New subfolder" creates the directory on disk (so a file manager sees it too)
//! and adds it to the list. The tree is the union of the two, and a remembered
//! folder is dropped from the list when it is deleted from the tree. A
//! remembered folder whose directory vanished (the organize engine removes
//! directories as they empty) still shows, and moving books into it re-creates it.
//!
//! # Rename
//!
//! [`plan_rename`] checks the rename (not onto an existing folder, not into
//! itself) and plans it with [`Target::Rebase`]: every book moves with the
//! engine's own executor, so it is journaled, fingerprinted and undone by
//! `POST /unorganize-library`.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, bail, Result};
use serde::Serialize;

use lexmechanic_utils::filenames::{path_is_within, path_key};
use crate::cache::Cache;
use crate::organize::{self, OrganizePlan, Scope, Target};

/// The library preference holding the empty folders the user made.
pub const EMPTY_FOLDERS_PREF: &str = "lexmechanic.empty_folders";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FolderEntry {
    /// `/`-separated, relative to the library, as first seen (casing).
    pub path: String,
    /// Books in this folder and below it.
    pub books: usize,
    /// Books directly in this folder.
    pub books_here: usize,
    /// The user made it and it holds no books yet.
    pub empty: bool,
    /// An author folder (calibre's `Author/Title (id)` layout): see [`list_with`].
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub author: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderTree {
    /// Every book, whatever its folder (books at the library root included).
    pub total: usize,
    /// Sorted by path, case-insensitively; every parent precedes its children.
    pub folders: Vec<FolderEntry>,
    /// How many author folders the library has (listed or not).
    pub author_folders: usize,
    /// The author folders were left out of `folders`.
    pub author_folders_hidden: bool,
}

/// A folder as stored: `/`-separated, no leading or trailing separator.
fn norm(path: &str) -> String {
    path.split(['/', '\\']).map(str::trim).filter(|p| !p.is_empty() && *p != ".").collect::<Vec<_>>().join("/")
}

fn remembered(cache: &Cache) -> Vec<String> {
    cache.get_preference_compat(EMPTY_FOLDERS_PREF).ok().flatten().and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok()).unwrap_or_default().iter().map(|p| norm(p)).filter(|p| !p.is_empty()).collect()
}

fn save_remembered(cache: &Cache, mut folders: Vec<String>) -> Result<()> {
    folders.sort_by_key(|f| path_key(f));
    folders.dedup_by_key(|f| path_key(f));
    if folders.is_empty() {
        cache.clear_preference_compat(EMPTY_FOLDERS_PREF)
    } else {
        cache.set_preference_compat(EMPTY_FOLDERS_PREF, &serde_json::to_string(&folders)?)
    }
}

/// Whether `path` is `folder` or inside it (segment-wise, case-insensitive).
fn within(path: &str, folder: &str) -> bool {
    path_is_within(path, folder)
}

/// The folder a book sits in, as the tree shows it.
///
/// calibre keeps each book in its own directory named `Title (id)`
/// (`Author/Title (42)`), which is storage, not a folder anyone made: listing
/// one node per book turns a 50,000-book library into a 50,000-row tree. That
/// last segment is left out, so the book counts toward `Author`. A book that
/// was moved in flat (`Novels/Dune.pdf`, whose `path` is `Novels`) has no such
/// directory and keeps its folder as it is. The directory is recognised by its
/// name ending in the book's *own* id, so a folder someone called `Chapter (3)`
/// is not mistaken for one.
pub fn folder_of(book_id: i64, path: &str) -> String {
    let parts: Vec<&str> = path.split(['/', '\\']).map(str::trim).filter(|p| !p.is_empty() && *p != ".").collect();
    match parts.split_last() {
        Some((last, rest)) if last.ends_with(&format!(" ({book_id})")) => rest.join("/"),
        _ => parts.join("/"),
    }
}

/// `Title (n)` -> `Title`: the text of a calibre-style per-book directory
/// name, or `None` when the segment does not end in ` (digits)`.
fn per_book_dir_title(segment: &str) -> Option<&str> {
    let inner = segment.strip_suffix(')')?;
    let (title, digits) = inner.rsplit_once(" (")?;
    (!title.is_empty() && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(title)
}

/// The folder tree: two queries, no disk access.
///
/// A per-book directory is recognised by its name ending in the book's own id
/// ([`folder_of`]), or -- for books whose id changed since the directory was
/// named (an absorbed library keeps the source's ids, a restore may get a new
/// one) -- by its shape: `Title (n)` that no other book shares or sits under,
/// holding formats named `Title - Author`, as calibre names them. A folder
/// someone called `Dune (1965)` with a plain `Dune.pdf` in it is not one.
pub fn list(cache: &Cache) -> Result<FolderTree> {
    list_with(cache, true)
}

/// [`list`], optionally without the **author folders**: a folder whose books
/// all sit in per-book directories below it and that holds nothing else (calibre's
/// `Author/Title (id)`). They are storage, not folders anyone made, and in a library
/// with thousands of authors they were most of the tree (3,800 of 13,000 nodes at
/// 50,000 books). With `include_author_folders` false they are left out and counted
/// in [`FolderTree::hidden_author_folders`]; the books still count toward the total
/// and every parent folder.
pub fn list_with(cache: &Cache, include_author_folders: bool) -> Result<FolderTree> {
    // (the folder a book is shown in, whether that was reached by dropping a per-book directory)
    let paths: Vec<(String, bool)> = {
        let conn = cache.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, path FROM books")?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut names: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
        let mut stmt = conn.prepare("SELECT book, name FROM data")?;
        for row in stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))? {
            let (book, name) = row?;
            names.entry(book).or_default().push(name.to_lowercase());
        }
        let mut occupied: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut ancestors: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (_, p) in &rows {
            let parts: Vec<&str> = p.as_deref().unwrap_or("").split(['/', '\\']).map(str::trim).filter(|p| !p.is_empty() && *p != ".").collect();
            *occupied.entry(path_key(&parts.join("/"))).or_default() += 1;
            for depth in 1..parts.len() {
                ancestors.insert(path_key(&parts[..depth].join("/")));
            }
        }
        rows.into_iter()
            .map(|(id, p)| {
                let p = p.unwrap_or_default();
                let by_id = folder_of(id, &p);
                if by_id != norm(&p) {
                    return (by_id, true);
                }
                let parts: Vec<&str> = p.split(['/', '\\']).map(str::trim).filter(|p| !p.is_empty() && *p != ".").collect();
                if let Some((last, rest)) = parts.split_last() {
                    if let Some(title) = per_book_dir_title(last) {
                        let key = path_key(&parts.join("/"));
                        let alone = occupied.get(&key) == Some(&1) && !ancestors.contains(&key);
                        let prefix = format!("{} - ", title.to_lowercase());
                        if alone && names.get(&id).is_some_and(|n| n.iter().any(|n| n.starts_with(&prefix))) {
                            return (rest.join("/"), true);
                        }
                    }
                }
                (by_id, false)
            })
            .collect()
    };
    let total = paths.len();
    // lower-cased path -> entry
    let mut map: BTreeMap<String, FolderEntry> = BTreeMap::new();
    // lower-cased path -> books directly there that were in a per-book directory
    let mut in_book_dirs: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (path, from_book_dir) in &paths {
        let parts: Vec<&str> = path.split(['/', '\\']).map(str::trim).filter(|p| !p.is_empty() && *p != ".").collect();
        for depth in 1..=parts.len() {
            let here = parts[..depth].join("/");
            let key = path_key(&here);
            if depth == parts.len() && *from_book_dir {
                *in_book_dirs.entry(key.clone()).or_default() += 1;
            }
            let entry = map.entry(key).or_insert_with(|| FolderEntry { path: here, books: 0, books_here: 0, empty: false, author: false });
            entry.books += 1;
            if depth == parts.len() {
                entry.books_here += 1;
            }
        }
    }
    for folder in remembered(cache) {
        let parts: Vec<&str> = folder.split('/').collect();
        for depth in 1..=parts.len() {
            let here = parts[..depth].join("/");
            let leaf = depth == parts.len();
            let entry = map.entry(path_key(&here)).or_insert_with(|| FolderEntry { path: here, books: 0, books_here: 0, empty: false, author: false });
            entry.empty = entry.empty || (leaf && entry.books == 0);
        }
    }
    // An author folder: every book directly in it is in a per-book directory, and it
    // has no subfolders (a user-made folder inside it makes it a real folder).
    let parents: std::collections::HashSet<String> = map.keys().filter_map(|k| k.rsplit_once('/').map(|(parent, _)| parent.to_string())).collect();
    for (key, entry) in map.iter_mut() {
        entry.author = entry.books_here > 0 && in_book_dirs.get(key) == Some(&entry.books_here) && !parents.contains(key);
    }
    let mut folders: Vec<FolderEntry> = map.into_values().collect();
    let author_folders = folders.iter().filter(|f| f.author).count();
    if !include_author_folders {
        folders.retain(|f| !f.author);
    }
    // `a/b` must come right after `a`, before `a-x`: sort by the segments.
    folders.sort_by_key(|f| path_key(&f.path).split('/').map(String::from).collect::<Vec<_>>());
    Ok(FolderTree { total, folders, author_folders, author_folders_hidden: !include_author_folders })
}

fn escapes_library(library: &Path, folder: &str) -> bool {
    let mut at = library.to_path_buf();
    for part in folder.split('/') {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() => return true,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    false
}

/// Makes an empty folder: the directory on disk and an entry in the remembered
/// list. Returns the folder as stored. Existing folders are fine (idempotent).
pub fn create_folder(cache: &Cache, folder: &str) -> Result<String> {
    let folder = organize::validate_folder(folder).map_err(|e| anyhow!(e))?;
    let library = &cache.backend.library_path;
    if escapes_library(library, &folder) {
        bail!("the folder {folder} goes through a link out of the library");
    }
    std::fs::create_dir_all(library.join(&folder))?;
    let mut all = remembered(cache);
    if !all.iter().any(|f| path_key(f) == path_key(&folder)) {
        all.push(folder.clone());
        save_remembered(cache, all)?;
    }
    Ok(folder)
}

/// Removes an empty folder the user made: its directory (never recursive, so a
/// directory with any file in it stays and the call fails) and its entry.
/// Refused when books are in it or below it.
pub fn delete_empty_folder(cache: &Cache, folder: &str) -> Result<()> {
    let folder = organize::validate_folder(folder).map_err(|e| anyhow!(e))?;
    let tree = list(cache)?;
    if tree.folders.iter().any(|f| path_key(&f.path) == path_key(&folder) && f.books > 0) {
        bail!("the folder {folder} still has books in it");
    }
    let library = &cache.backend.library_path;
    if escapes_library(library, &folder) {
        bail!("the folder {folder} goes through a link out of the library");
    }
    let abs = library.join(&folder);
    if abs.is_dir() {
        std::fs::remove_dir(&abs).map_err(|e| anyhow!("could not remove the folder (is something other than books in it?): {e}"))?;
    }
    let keep: Vec<String> = remembered(cache).into_iter().filter(|f| !within(f, &folder)).collect();
    save_remembered(cache, keep)
}

/// The dry run of renaming `from` to `to` (both relative to the library): every
/// book at or under `from` moves, keeping its sub-folders and file names.
///
/// Refused, with a message: an invalid folder, the same name, `to` inside
/// `from`, and a `to` that already holds books or exists on disk (merging two
/// folders is a move of books, which names each collision; a rename never
/// does it silently). A change of case alone is a rename.
pub fn plan_rename(cache: &Cache, from: &str, to: &str) -> Result<OrganizePlan> {
    let from = organize::validate_folder(from).map_err(|e| anyhow!(e))?;
    let to = organize::validate_folder(to).map_err(|e| anyhow!(e))?;
    if from == to {
        bail!("that is already the folder's name");
    }
    if within(&to, &from) && path_key(&to) != path_key(&from) {
        bail!("a folder cannot be renamed to a place inside itself");
    }
    if path_key(&to) != path_key(&from) {
        let tree = list(cache)?;
        if tree.folders.iter().any(|f| path_key(&f.path) == path_key(&to)) || cache.backend.library_path.join(&to).exists() {
            bail!("a folder named {to} already exists; move the books into it instead");
        }
    }
    organize::plan_to(cache, Target::Rebase { from: &from, to: &to }, &Scope::PathPrefix(from.clone()))
}

/// Before a rename runs: the empty folders the user made under `from` go to the
/// same place under `to` (directory and list entry), so they travel with it.
pub fn move_remembered(cache: &Cache, from: &str, to: &str) -> Result<()> {
    let (from, to) = (norm(from), norm(to));
    let library = &cache.backend.library_path;
    let mut all = remembered(cache);
    let mut changed = false;
    for folder in all.iter_mut() {
        if !within(folder, &from) {
            continue;
        }
        // Folding can change a name's byte length, so cut by segments.
        let skip = from.split('/').filter(|s| !s.is_empty()).count();
        let rest = folder.split('/').skip(skip).collect::<Vec<_>>().join("/");
        let new = if rest.is_empty() { to.clone() } else { format!("{to}/{rest}") };
        if path_key(&new) == path_key(folder) {
            // Only the case changes: the directory is the same one, so it is
            // renamed (through a temporary name; a direct rename to a name that
            // differs only in case is refused or ignored on some volumes), not
            // created anew and removed.
            let (old_abs, new_abs) = (library.join(&*folder), library.join(&new));
            if old_abs.is_dir() && !escapes_library(library, &new) {
                let temp = old_abs.with_file_name(format!("{}.case-{}", old_abs.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), &uuid::Uuid::new_v4().simple().to_string()[..6]));
                if std::fs::rename(&old_abs, &temp).is_ok() && std::fs::rename(&temp, &new_abs).is_err() {
                    let _ = std::fs::rename(&temp, &old_abs);
                }
            }
        } else {
            if !escapes_library(library, &new) {
                let _ = std::fs::create_dir_all(library.join(&new));
            }
            let _ = std::fs::remove_dir(library.join(&*folder));
        }
        *folder = new;
        changed = true;
    }
    if changed {
        save_remembered(cache, all)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{walk, ScanOptions};
    use crate::scan_pipeline::JobControl;
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

    fn rows(tree: &FolderTree) -> Vec<(String, usize, usize)> {
        tree.folders.iter().map(|f| (f.path.clone(), f.books, f.books_here)).collect()
    }

    #[test]
    fn per_book_directories_are_not_folders_of_the_tree() {
        assert_eq!(folder_of(11, "Unknown/book_1 (11)"), "Unknown");
        assert_eq!(folder_of(4, "Author/Title (4)"), "Author");
        assert_eq!(folder_of(4, "Title (4)"), "", "a book at the root has no folder");
        // Another book's id, or a folder that merely looks like one, is a real folder.
        assert_eq!(folder_of(5, "Author/Title (4)"), "Author/Title (4)");
        assert_eq!(folder_of(3, "Novels/Chapter (3)/Extras"), "Novels/Chapter (3)/Extras");
        assert_eq!(folder_of(0, "Novels"), "Novels", "a book moved in flat keeps its folder");
        assert_eq!(folder_of(7, "a\\b\\T (7)"), "a/b");

        let (dir, cache) = library();
        for (i, name) in ["One", "Two", "Three"].iter().enumerate() {
            write(&dir.path().join(format!("inbox/{name}.pdf")), format!("%PDF {i}").as_bytes());
        }
        index(dir.path(), &cache);
        // The layout calibre gives books: Author/Title (id).
        let one = id(&cache, "One");
        let two = id(&cache, "Two");
        let three = id(&cache, "Three");
        cache.set_book_path(one, &format!("Unknown/One ({one})")).unwrap();
        cache.set_book_path(two, &format!("Unknown/Two ({two})")).unwrap();
        cache.set_book_path(three, "Novels").unwrap();

        let tree = list(&cache).unwrap();
        assert_eq!(tree.total, 3);
        assert_eq!(rows(&tree), vec![("Novels".to_string(), 1, 1), ("Unknown".to_string(), 2, 2)], "one node per real folder, none per book");
    }

    #[test]
    fn a_per_book_directory_is_recognised_by_its_shape_when_its_number_is_not_the_books_id() {
        let (dir, cache) = library();
        for name in ["Absorbed", "Dune", "Shared A", "Shared B"] {
            write(&dir.path().join(format!("inbox/{name}.pdf")), name.as_bytes());
        }
        index(dir.path(), &cache);
        let (absorbed, dune, a, b) = (id(&cache, "Absorbed"), id(&cache, "Dune"), id(&cache, "Shared A"), id(&cache, "Shared B"));
        // An absorbed book keeps the source library's id in its directory name.
        cache.set_book_path(absorbed, "Herbert, Frank/Absorbed (9001)").unwrap();
        cache.backend.conn.lock().unwrap().execute("UPDATE data SET name = 'Absorbed - Frank Herbert' WHERE book = ?1", [absorbed]).unwrap();
        // A folder a person made, holding a plainly named file, is a folder.
        cache.set_book_path(dune, "Fiction/Dune (1965)").unwrap();
        // Two books in one `Title (n)` directory: it is shared, so it is a folder.
        cache.set_book_path(a, "Shelf/Pair (7)").unwrap();
        cache.set_book_path(b, "Shelf/Pair (7)").unwrap();
        cache.backend.conn.lock().unwrap().execute("UPDATE data SET name = 'Pair - Someone' WHERE book IN (?1, ?2)", [a, b]).unwrap();

        let paths: Vec<String> = list(&cache).unwrap().folders.into_iter().map(|f| f.path).collect();
        assert_eq!(paths, vec!["Fiction", "Fiction/Dune (1965)", "Herbert, Frank", "Shelf", "Shelf/Pair (7)"]);
    }

    #[test]
    fn an_empty_library_has_no_folders() {
        let (_dir, cache) = library();
        let tree = list(&cache).unwrap();
        assert_eq!((tree.total, tree.folders.len()), (0, 0));
    }

    #[test]
    fn counts_include_subfolders_and_nest() {
        let (dir, cache) = library();
        write(&dir.path().join("Gaming/RPG/Rifts/A.pdf"), b"%PDF a");
        write(&dir.path().join("Gaming/RPG/Rifts/B.pdf"), b"%PDF b");
        write(&dir.path().join("Gaming/RPG/Rifts/Sub/C.pdf"), b"%PDF c");
        write(&dir.path().join("Gaming/RPG/Rift/D.pdf"), b"%PDF d");
        write(&dir.path().join("Gaming-x/E.pdf"), b"%PDF e");
        write(&dir.path().join("Loose.pdf"), b"%PDF loose");
        index(dir.path(), &cache);
        let tree = list(&cache).unwrap();
        assert_eq!(tree.total, 6, "the book at the library root counts in the total");
        assert_eq!(
            rows(&tree),
            [
                ("Gaming".to_string(), 4, 0),
                ("Gaming/RPG".to_string(), 4, 0),
                ("Gaming/RPG/Rift".to_string(), 1, 1),
                ("Gaming/RPG/Rifts".to_string(), 3, 2),
                ("Gaming/RPG/Rifts/Sub".to_string(), 1, 1),
                ("Gaming-x".to_string(), 1, 1),
            ]
        );
    }

    #[test]
    fn a_new_subfolder_is_made_on_disk_remembered_and_shown_empty() {
        let (dir, cache) = library();
        write(&dir.path().join("Ref/A.pdf"), b"%PDF a");
        index(dir.path(), &cache);
        assert_eq!(create_folder(&cache, r"Ref\New One").unwrap(), "Ref/New One");
        assert!(dir.path().join("Ref/New One").is_dir());
        let tree = list(&cache).unwrap();
        let new = tree.folders.iter().find(|f| f.path == "Ref/New One").unwrap();
        assert_eq!((new.books, new.empty), (0, true));
        assert_eq!(tree.folders.iter().find(|f| f.path == "Ref").unwrap().books, 1);
        // Idempotent, and bad folders are refused.
        create_folder(&cache, "Ref/New One").unwrap();
        assert_eq!(list(&cache).unwrap().folders.len(), 2);
        assert!(create_folder(&cache, "../x").is_err());
        assert!(create_folder(&cache, "C:/x").is_err());
        // The first book moved in makes it a real folder; delete refuses it then.
        let a = id(&cache, "A");
        let p = organize::plan_to(&cache, Target::Folder("Ref/New One"), &Scope::Ids(vec![a])).unwrap();
        organize::execute(&cache, &p, &JobControl::new()).unwrap();
        let tree = list(&cache).unwrap();
        assert_eq!(tree.folders.iter().find(|f| f.path == "Ref/New One").map(|f| (f.books, f.empty)), Some((1, false)));
        assert!(delete_empty_folder(&cache, "Ref/New One").is_err());
    }

    #[test]
    fn an_empty_folder_can_be_deleted_and_the_directory_goes() {
        let (dir, cache) = library();
        create_folder(&cache, "Plans/Later").unwrap();
        assert!(dir.path().join("Plans/Later").is_dir());
        delete_empty_folder(&cache, "Plans/Later").unwrap();
        assert!(!dir.path().join("Plans/Later").exists());
        assert!(list(&cache).unwrap().folders.is_empty());
    }

    #[test]
    fn the_remembered_list_survives_a_rebuild_from_the_change_log() {
        let (_dir, cache) = library();
        create_folder(&cache, "Later/Stuff").unwrap();
        cache.rebuild_from_change_log().unwrap();
        assert!(list(&cache).unwrap().folders.iter().any(|f| f.path == "Later/Stuff" && f.empty));
    }

    #[test]
    fn rename_folder_moves_every_book_keeps_subfolders_and_is_undone() {
        let (dir, cache) = library();
        write(&dir.path().join("Gaming/Rifts/A.pdf"), b"%PDF a");
        write(&dir.path().join("Gaming/Rifts/Sub/B.pdf"), b"%PDF b");
        write(&dir.path().join("Gaming/Other/C.pdf"), b"%PDF c");
        index(dir.path(), &cache);
        let plan = plan_rename(&cache, "Gaming/Rifts", "Gaming/Rifts RPG").unwrap();
        assert_eq!(plan.books.len(), 2);
        assert!(dir.path().join("Gaming/Rifts/A.pdf").exists(), "a dry run moves nothing");
        let report = organize::execute(&cache, &plan, &JobControl::new()).unwrap();
        assert_eq!((report.books_moved, report.failed.len()), (2, 0), "{report:?}");
        assert!(dir.path().join("Gaming/Rifts RPG/A.pdf").is_file());
        assert!(dir.path().join("Gaming/Rifts RPG/Sub/B.pdf").is_file());
        assert!(!dir.path().join("Gaming/Rifts").exists(), "the old folder is gone");
        assert!(dir.path().join("Gaming/Other/C.pdf").is_file(), "a sibling is untouched");
        assert_eq!(rows(&list(&cache).unwrap()).iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["Gaming", "Gaming/Other", "Gaming/Rifts RPG", "Gaming/Rifts RPG/Sub"]);

        let undone = organize::undo(&cache, &report.run_id, &JobControl::new()).unwrap();
        assert_eq!(undone.books_restored, 2, "{undone:?}");
        assert!(dir.path().join("Gaming/Rifts/Sub/B.pdf").is_file());
        assert!(!dir.path().join("Gaming/Rifts RPG").exists());
        assert_eq!(cache.field_for(id(&cache, "B"), "path").unwrap().unwrap(), "Gaming/Rifts/Sub");
    }

    #[test]
    fn rename_folder_refuses_collisions_and_nonsense() {
        let (dir, cache) = library();
        write(&dir.path().join("Gaming/Rifts/A.pdf"), b"%PDF a");
        write(&dir.path().join("Gaming/Palladium/B.pdf"), b"%PDF b");
        create_folder(&cache, "Gaming/Empty").unwrap();
        index(dir.path(), &cache);
        for (from, to) in [("Gaming/Rifts", "Gaming/Palladium"), ("Gaming/Rifts", "gaming/palladium"), ("Gaming/Rifts", "Gaming/Empty"), ("Gaming/Rifts", "Gaming/Rifts"), ("Gaming", "Gaming/Rifts/Inside"), ("Gaming/Rifts", "../x"), ("Gaming/Rifts", "C:/x")] {
            assert!(plan_rename(&cache, from, to).is_err(), "{from} -> {to} should be refused");
        }
        // A directory that is on disk but unknown to the library also blocks it.
        std::fs::create_dir_all(dir.path().join("Gaming/Stray")).unwrap();
        assert!(plan_rename(&cache, "Gaming/Rifts", "Gaming/Stray").is_err());
    }

    #[test]
    fn rename_folder_can_change_only_the_case() {
        let (dir, cache) = library();
        write(&dir.path().join("Gaming/rifts/A.pdf"), b"%PDF a");
        index(dir.path(), &cache);
        let plan = plan_rename(&cache, "Gaming/rifts", "Gaming/Rifts").unwrap();
        let report = organize::execute(&cache, &plan, &JobControl::new()).unwrap();
        assert_eq!((report.books_moved, report.failed.len()), (1, 0), "{report:?}");
        assert_eq!(cache.field_for(id(&cache, "A"), "path").unwrap().unwrap(), "Gaming/Rifts");
    }

    #[test]
    fn rename_carries_the_empty_folders_made_under_it() {
        let (dir, cache) = library();
        write(&dir.path().join("Gaming/A.pdf"), b"%PDF a");
        create_folder(&cache, "Gaming/Later").unwrap();
        index(dir.path(), &cache);
        let plan = plan_rename(&cache, "Gaming", "Games").unwrap();
        move_remembered(&cache, "Gaming", "Games").unwrap();
        organize::execute(&cache, &plan, &JobControl::new()).unwrap();
        assert!(dir.path().join("Games/Later").is_dir());
        assert!(!dir.path().join("Gaming").exists());
        let tree = list(&cache).unwrap();
        assert!(tree.folders.iter().any(|f| f.path == "Games/Later" && f.empty));
        assert!(!tree.folders.iter().any(|f| f.path.starts_with("Gaming")));
    }

    /// calibre's `Author/Title (id)` layout makes a folder per author that nobody made: left
    /// out on request (a third of the tree at 50,000 books), counted, and still there otherwise.
    #[test]
    fn author_folders_can_be_left_out_of_the_tree() {
        let (dir, cache) = library();
        for name in ["One", "Two", "Three", "Four"] {
            write(&dir.path().join(format!("inbox/{name}.pdf")), name.as_bytes());
        }
        index(dir.path(), &cache);
        let (one, two, three, four) = (id(&cache, "One"), id(&cache, "Two"), id(&cache, "Three"), id(&cache, "Four"));
        cache.set_book_path(one, &format!("Herbert, Frank/One ({one})")).unwrap();
        cache.set_book_path(two, &format!("Herbert, Frank/Two ({two})")).unwrap();
        cache.set_book_path(three, "Novels").unwrap();
        // An author folder that holds a folder somebody made is a real folder.
        cache.set_book_path(four, &format!("Asimov, Isaac/Four ({four})")).unwrap();
        create_folder(&cache, "Asimov, Isaac/Drafts").unwrap();

        let all = list_with(&cache, true).unwrap();
        assert_eq!(all.folders.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["Asimov, Isaac", "Asimov, Isaac/Drafts", "Herbert, Frank", "Novels"]);
        assert_eq!((all.author_folders, all.author_folders_hidden), (1, false));
        assert!(all.folders.iter().find(|f| f.path == "Herbert, Frank").unwrap().author);

        let tree = list_with(&cache, false).unwrap();
        assert_eq!(tree.total, 4, "hiding folders never hides books");
        assert_eq!(tree.folders.iter().map(|f| (f.path.as_str(), f.books)).collect::<Vec<_>>(), vec![("Asimov, Isaac", 1), ("Asimov, Isaac/Drafts", 0), ("Novels", 1)]);
        assert_eq!((tree.author_folders, tree.author_folders_hidden), (1, true));
    }
}
