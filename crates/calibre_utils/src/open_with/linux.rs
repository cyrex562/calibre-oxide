//! Port of `old_src/src/calibre/utils/open_with/linux.py`: finding
//! installed applications that can open a given file type (via
//! freedesktop.org `.desktop` files and XDG icon themes), and
//! building a launchable command line for one.
//!
//! # Scope
//!
//! Real: [`parse_desktop_file`] (the `.desktop` INI-format parser,
//! including localized-key handling and the `Hidden`/`Type`/`Exec`
//! validity checks), [`find_programs`] (XDG `applications/` directory
//! scanning + MIME-type filtering), [`find_icons`] (XDG icon-theme
//! directory scanning), and [`entry_to_cmdline`] (`%f`/`%F`/`%u`/`%U`/
//! `%i`/`%c`/`%k`/`%%` field-code substitution).
//!
//! Narrowed:
//! - [`find_icons`] recomputes the icon-theme scan on every call
//!   rather than upstream's `msgpack`-serialized, mtime-keyed
//!   persistent cache (`icon-theme-cache.calibre_msgpack`) -- a real
//!   performance optimization this port doesn't need to be correct,
//!   so it's dropped rather than pulled in as a dependency.
//! - Sorting uses a plain case-insensitive string compare instead of
//!   `calibre.utils.icu.numeric_sort_key` (no numeric/locale-aware
//!   collation exists in `calibre_utils::icu` yet) -- affects only
//!   the *order* results are returned in, not which programs are
//!   found.
//! - [`localize_string`]'s "current UI language" comes from the
//!   `LC_ALL`/`LC_MESSAGES`/`LANG`/`LANGUAGE` environment variables
//!   (falling back to `"en"`), not calibre's own preference-backed
//!   `get_lang()` (translation-catalog machinery, explicitly out of
//!   scope for `calibre_utils::localization`, see issue #140).
//! - Custom, non-standard localized `.desktop` keys (anything besides
//!   `Name`/`GenericName`/`Comment`/`Icon` with a `[lang]` suffix) are
//!   parsed but dropped rather than preserved -- nothing in this
//!   port's own call graph (or upstream's `entry_to_cmdline`/
//!   `process_desktop_file`) ever reads them.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use regex::Regex;
use walkdir::WalkDir;

use crate::localization::canonicalize_lang;

/// A `.desktop` entry as freshly parsed: `Name`/`GenericName`/
/// `Comment`/`Icon` are still per-locale (`None` is the unlocalized
/// default), matching the on-disk `Key[lang]=value` shape.
#[derive(Debug, Clone, Default)]
pub struct RawDesktopEntry {
    pub desktop_file_path: PathBuf,
    pub exec: Vec<String>,
    pub mime_type: HashSet<String>,
    pub name: IndexMap<Option<String>, String>,
    pub generic_name: IndexMap<Option<String>, String>,
    pub comment: IndexMap<Option<String>, String>,
    pub icon: IndexMap<Option<String>, String>,
}

/// A `.desktop` entry after locale resolution and icon-path lookup
/// ([`process_desktop_file`]) -- what [`find_programs`] returns and
/// [`entry_to_cmdline`] consumes.
#[derive(Debug, Clone, Default)]
pub struct DesktopEntry {
    pub desktop_file_path: PathBuf,
    pub exec: Vec<String>,
    pub mime_type: HashSet<String>,
    pub name: String,
    pub generic_name: String,
    pub comment: String,
    /// An absolute path to the icon file, if one was found.
    pub icon: Option<String>,
}

/// Port of `parse_localized_key`: splits `"Name[en_US]"` into
/// `("Name", Some("en_US"))`, or `("Name", None)` for an unsuffixed key.
fn parse_localized_key(key: &str) -> (&str, Option<String>) {
    match key.find('[') {
        Some(i) if key.ends_with(']') => (&key[..i], Some(key[i + 1..key.len() - 1].to_string())),
        _ => (key, None),
    }
}

/// Port of `unquote_exec`. Returns `None` on malformed shell quoting
/// (upstream's uncaught `shlex.split` `ValueError`, which propagates
/// up to `find_programs`'s own `except Exception: continue` -- here,
/// the caller does the equivalent by treating a `None` return from
/// [`parse_desktop_file`] as "skip this file").
fn unquote_exec(val: &str) -> Option<Vec<String>> {
    let val = val.replace("\\\\", "\\");
    shlex::split(&val)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    false
}

/// Port of `parse_desktop_file`. Returns `None` for a file that's
/// unreadable, not valid UTF-8, `Hidden`, not `Type=Application`, or
/// missing any of `Exec`/`MimeType`/`Name`.
pub fn parse_desktop_file(path: &Path) -> Option<RawDesktopEntry> {
    let raw = std::fs::read(path).ok()?;
    let raw = String::from_utf8(raw).ok()?;

    let gpat = Regex::new(r"^\[(.+?)\]\s*$").unwrap();
    let kpat = Regex::new(r"^([-a-zA-Z0-9\[\]@_.]+)\s*=\s*(.+)$").unwrap();

    let mut group: Option<String> = None;
    let mut exec: Option<Vec<String>> = None;
    let mut mime_type: Option<HashSet<String>> = None;
    let mut name: IndexMap<Option<String>, String> = IndexMap::new();
    let mut generic_name: IndexMap<Option<String>, String> = IndexMap::new();
    let mut comment: IndexMap<Option<String>, String> = IndexMap::new();
    let mut icon: IndexMap<Option<String>, String> = IndexMap::new();

    for line in raw.split('\n') {
        if let Some(caps) = gpat.captures(line) {
            if group.as_deref() == Some("Desktop Entry") {
                break;
            }
            group = Some(caps[1].to_string());
            continue;
        }
        if group.as_deref() != Some("Desktop Entry") {
            continue;
        }
        let Some(caps) = kpat.captures(line) else { continue };
        let k = caps.get(1).unwrap().as_str();
        let v = caps.get(2).unwrap().as_str();

        if k == "Hidden" && v == "true" {
            return None;
        }
        if k == "Type" && v != "Application" {
            return None;
        }

        if k == "Exec" {
            let cmdline = unquote_exec(v)?;
            if let Some(first) = cmdline.first() {
                if !Path::new(first).is_absolute() || is_executable(Path::new(first)) {
                    exec = Some(cmdline);
                }
            }
        } else if k == "MimeType" {
            mime_type = Some(v.split(';').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect());
        } else if k.starts_with("Name") || k.starts_with("GenericName") || k.starts_with("Comment") || k.starts_with("Icon") {
            let (base, lang) = parse_localized_key(k);
            let map = match base {
                "Name" => Some(&mut name),
                "GenericName" => Some(&mut generic_name),
                "Comment" => Some(&mut comment),
                "Icon" => Some(&mut icon),
                _ => None,
            };
            if let Some(map) = map {
                map.insert(lang, v.to_string());
            }
        }
        // Every other key (custom localized or not) is dropped -- see
        // the module doc.
    }

    let exec = exec?;
    let mime_type = mime_type?;
    if name.is_empty() {
        return None;
    }

    Some(RawDesktopEntry { desktop_file_path: path.to_path_buf(), exec, mime_type, name, generic_name, comment, icon })
}

/// The current UI language, narrowed to environment-variable
/// inspection -- see the module doc.
fn system_lang() -> String {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
        if let Ok(val) = std::env::var(var) {
            let val = val.split(['.', '@']).next().unwrap_or("").trim();
            if !val.is_empty() && val != "C" && val != "POSIX" {
                return val.to_string();
            }
        }
    }
    "en".to_string()
}

/// Port of `localize_string`: picks the value whose key's base
/// language matches the current UI language, else the unlocalized
/// (`None`-keyed) default, else `""`.
pub fn localize_string(data: &IndexMap<Option<String>, String>) -> String {
    let lang = canonicalize_lang(&system_lang());
    let key_matches = |key: &Option<String>| -> bool {
        match key {
            None => false,
            Some(k) => {
                let base = k.split(['_', '.', '@']).next().unwrap_or("");
                canonicalize_lang(base) == lang
            }
        }
    };
    if let Some((_, v)) = data.iter().find(|(k, _)| key_matches(k)) {
        return v.clone();
    }
    data.get(&None).cloned().unwrap_or_default()
}

/// Port of `process_desktop_file`: resolves `Icon` to an absolute
/// path via `icons` (upstream's own `find_icons()`, passed in here
/// instead of read from a lazily-populated module global) and
/// localizes `Name`/`GenericName`/`Comment`.
pub fn process_desktop_file(entry: RawDesktopEntry, icons: &IndexMap<String, PathBuf>) -> DesktopEntry {
    let icon = entry.icon.get(&None).cloned().filter(|s| !s.is_empty()).and_then(|icon_val| {
        if Path::new(&icon_val).is_absolute() {
            Some(icon_val)
        } else {
            icons.get(&icon_val).map(|p| p.to_string_lossy().into_owned())
        }
    });

    DesktopEntry {
        desktop_file_path: entry.desktop_file_path,
        exec: entry.exec,
        mime_type: entry.mime_type,
        name: localize_string(&entry.name),
        generic_name: localize_string(&entry.generic_name),
        comment: localize_string(&entry.comment),
        icon,
    }
}

fn xdg_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var("XDG_DATA_HOME").ok().filter(|s| !s.is_empty()).map(PathBuf::from).or_else(|| {
        std::env::var("HOME").ok().filter(|s| !s.is_empty()).map(|h| PathBuf::from(h).join(".local/share"))
    });
    if let Some(home) = home {
        dirs.push(home);
    }
    let extra = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share/:/usr/share/".to_string());
    for d in extra.split(':') {
        if !d.is_empty() {
            dirs.push(PathBuf::from(d));
        }
    }
    dirs
}

/// Port of `find_icons`: scans XDG icon-theme directories for the
/// largest available `.svg`/`.png`/`.xpm` icon per name. See the
/// module doc for why this recomputes rather than caches.
pub fn find_icons() -> IndexMap<String, PathBuf> {
    let mut base_dirs = Vec::new();
    let home = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOME").ok().filter(|s| !s.is_empty()).map(|h| format!("{h}/.local/share")));
    if let Some(home) = home {
        base_dirs.push(PathBuf::from(home).join("icons"));
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            base_dirs.push(PathBuf::from(home).join(".icons"));
        }
    }
    let xdg_data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for b in xdg_data_dirs.split(':') {
        if !b.is_empty() {
            base_dirs.push(PathBuf::from(b).join("icons"));
        }
    }
    base_dirs.push(PathBuf::from("/usr/share/pixmaps"));

    let sz_pat = Regex::new(r"/((?:\d+x\d+)|scalable)/").unwrap();
    let exts: HashSet<&str> = ["svg", "png", "xpm"].into_iter().collect();

    // name -> (neg_size, insertion_index, size, path); sorted so the
    // largest (most negative neg_size) sorts first, matching upstream's
    // `(-sz, idx, sz, path)` sort tuple.
    let mut candidates: IndexMap<String, Vec<(i64, usize, i64, PathBuf)>> = IndexMap::new();

    for loc in &base_dirs {
        let Ok(subdirs) = std::fs::read_dir(loc) else { continue };
        for entry in subdirs.flatten() {
            let d = entry.path();
            if !d.is_dir() {
                continue;
            }
            for f in WalkDir::new(&d).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()) {
                let path = f.path();
                let Some(bn) = path.file_name().and_then(|s| s.to_str()) else { continue };
                let Some((_, ext)) = bn.rsplit_once('.') else { continue };
                if !exts.contains(ext) {
                    continue;
                }
                let name = bn[..bn.len() - ext.len() - 1].to_string();
                let path_str = path.to_string_lossy();
                if let Some(caps) = sz_pat.captures_iter(&path_str).last() {
                    let sz_str = caps.get(1).unwrap().as_str();
                    let sz: i64 = if sz_str == "scalable" {
                        100000
                    } else {
                        sz_str.split('x').next().and_then(|s| s.parse().ok()).unwrap_or(0)
                    };
                    let list = candidates.entry(name).or_default();
                    let idx = list.len();
                    list.push((-sz, idx, sz, path.to_path_buf()));
                }
            }
        }
    }

    let mut ans = IndexMap::new();
    for (name, mut list) in candidates {
        list.sort();
        ans.insert(name, list[0].3.clone());
    }
    ans
}

/// Port of `find_programs`: every installed application whose
/// `MimeType` list intersects the MIME types guessed for `extensions`.
pub fn find_programs(extensions: &[&str]) -> Vec<DesktopEntry> {
    let extensions: HashSet<String> = extensions.iter().map(|e| e.to_lowercase()).collect();
    let mime_types: HashSet<String> = extensions
        .iter()
        .filter_map(|ext| mime_guess::from_path(format!("file.{ext}")).first_raw().map(str::to_string))
        .collect();

    let mut desktop_files: IndexMap<String, PathBuf> = IndexMap::new();
    for base in xdg_data_dirs() {
        let apps_dir = base.join("applications");
        if !apps_dir.is_dir() {
            continue;
        }
        for entry in WalkDir::new(&apps_dir).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("desktop") {
                if let Some(bn) = path.file_name().and_then(|s| s.to_str()) {
                    desktop_files.entry(bn.to_string()).or_insert_with(|| path.to_path_buf());
                }
            }
        }
    }

    let icons = find_icons();
    let mut ans: Vec<DesktopEntry> = Vec::new();
    for path in desktop_files.values() {
        let Some(raw) = parse_desktop_file(path) else { continue };
        if raw.mime_type.iter().any(|m| mime_types.contains(m)) {
            ans.push(process_desktop_file(raw, &icons));
        }
    }
    ans.sort_by_key(|a| a.name.to_lowercase());
    ans
}

/// Port of `os.path.abspath`: lexical join with the current directory
/// and `.`/`..` normalization, without requiring the path to exist
/// (unlike [`std::fs::canonicalize`]).
fn abspath(path: &Path) -> PathBuf {
    let joined =
        if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) };
    let mut out = PathBuf::new();
    for comp in joined.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Port of `entry_to_cmdline`: substitutes the [Desktop Entry
/// Specification field
/// codes](https://specifications.freedesktop.org/desktop-entry-spec/latest/exec-variables.html)
/// `entry.exec` uses to reference the file being opened.
pub fn entry_to_cmdline(entry: &DesktopEntry, path: &Path) -> Vec<String> {
    let abs = abspath(path);
    let path_str = abs.to_string_lossy().into_owned();
    let file_uri = format!("file://{path_str}");

    let mut cmd = entry.exec.clone();
    if let Some(idx) = cmd.iter().position(|s| s == "%i") {
        if let Some(icon) = &entry.icon {
            cmd.splice(idx..idx + 1, ["--icon".to_string(), icon.clone()]);
        } else {
            cmd.remove(idx);
        }
    }

    let sub_pat = Regex::new(r"%[fFuUdDnNickvm%]").unwrap();
    let mut result = Vec::with_capacity(cmd.len());
    result.push(cmd[0].clone());
    for token in &cmd[1..] {
        let replaced = sub_pat.replace_all(token, |caps: &regex::Captures| {
            match caps[0].chars().last().unwrap() {
                'f' | 'F' => path_str.clone(),
                'u' | 'U' => file_uri.clone(),
                '%' => "%".to_string(),
                'c' => entry.name.clone(),
                'k' => entry.desktop_file_path.to_string_lossy().into_owned(),
                _ => caps[0].to_string(),
            }
        });
        result.push(replaced.into_owned());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate process-wide environment
    /// variables (`XDG_DATA_HOME` etc.) -- `cargo test` runs tests in
    /// parallel by default, and env vars are process-global state.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn write_desktop_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn parses_a_well_formed_desktop_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_desktop_file(
            dir.path(),
            "test.desktop",
            "[Desktop Entry]\nType=Application\nName=Test App\nName[fr]=Application de test\nExec=/bin/cat %f\nMimeType=text/plain;text/x-log;\n",
        );
        let entry = parse_desktop_file(&path).unwrap();
        assert_eq!(entry.name.get(&None), Some(&"Test App".to_string()));
        assert_eq!(entry.name.get(&Some("fr".to_string())), Some(&"Application de test".to_string()));
        assert_eq!(entry.exec, vec!["/bin/cat", "%f"]);
        assert!(entry.mime_type.contains("text/plain"));
        assert!(entry.mime_type.contains("text/x-log"));
    }

    #[test]
    fn hidden_entries_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_desktop_file(
            dir.path(),
            "hidden.desktop",
            "[Desktop Entry]\nType=Application\nHidden=true\nName=X\nExec=/bin/true\nMimeType=text/plain;\n",
        );
        assert!(parse_desktop_file(&path).is_none());
    }

    #[test]
    fn non_application_entries_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_desktop_file(
            dir.path(),
            "link.desktop",
            "[Desktop Entry]\nType=Link\nName=X\nExec=/bin/true\nMimeType=text/plain;\n",
        );
        assert!(parse_desktop_file(&path).is_none());
    }

    #[test]
    fn missing_required_fields_return_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_desktop_file(dir.path(), "incomplete.desktop", "[Desktop Entry]\nType=Application\nName=X\n");
        assert!(parse_desktop_file(&path).is_none(), "missing Exec/MimeType");
    }

    #[test]
    fn parsing_stops_at_the_end_of_the_desktop_entry_group() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_desktop_file(
            dir.path(),
            "grouped.desktop",
            "[Desktop Entry]\nType=Application\nName=X\nExec=/bin/true\nMimeType=text/plain;\n[Desktop Action foo]\nName=Should not be seen\n",
        );
        let entry = parse_desktop_file(&path).unwrap();
        assert_eq!(entry.name.get(&None), Some(&"X".to_string()));
    }

    #[test]
    fn entry_to_cmdline_substitutes_field_codes() {
        let entry = DesktopEntry {
            desktop_file_path: PathBuf::from("/usr/share/applications/test.desktop"),
            exec: vec!["/bin/app".to_string(), "--file".to_string(), "%f".to_string(), "%i".to_string()],
            name: "Test App".to_string(),
            icon: Some("test-icon".to_string()),
            ..Default::default()
        };
        let cmd = entry_to_cmdline(&entry, Path::new("book.epub"));
        assert_eq!(cmd[0], "/bin/app");
        assert_eq!(cmd[1], "--file");
        assert!(cmd[2].ends_with("book.epub"), "{:?}", cmd);
        assert_eq!(&cmd[3..], &["--icon", "test-icon"]);
    }

    #[test]
    fn entry_to_cmdline_drops_percent_i_when_there_is_no_icon() {
        let entry = DesktopEntry {
            exec: vec!["/bin/app".to_string(), "%f".to_string(), "%i".to_string()],
            icon: None,
            ..Default::default()
        };
        let cmd = entry_to_cmdline(&entry, Path::new("book.epub"));
        assert_eq!(cmd.len(), 2);
    }

    #[test]
    fn entry_to_cmdline_substitutes_u_as_a_file_uri() {
        let entry = DesktopEntry { exec: vec!["/bin/app".to_string(), "%u".to_string()], ..Default::default() };
        let cmd = entry_to_cmdline(&entry, Path::new("/tmp/book.epub"));
        assert_eq!(cmd[1], "file:///tmp/book.epub");
    }

    #[test]
    fn localize_string_prefers_the_matching_language() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("LC_ALL", "fr_FR.UTF-8");
        let mut data = IndexMap::new();
        data.insert(None, "Default".to_string());
        data.insert(Some("fr".to_string()), "Francais".to_string());
        assert_eq!(localize_string(&data), "Francais");
        std::env::remove_var("LC_ALL");
    }

    #[test]
    fn localize_string_falls_back_to_the_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("LC_ALL", "de_DE.UTF-8");
        let mut data = IndexMap::new();
        data.insert(None, "Default".to_string());
        data.insert(Some("fr".to_string()), "Francais".to_string());
        assert_eq!(localize_string(&data), "Default");
        std::env::remove_var("LC_ALL");
    }

    #[test]
    fn find_programs_locates_a_program_by_mime_type() {
        let _guard = ENV_LOCK.lock().unwrap();
        let data_home = tempfile::tempdir().unwrap();
        let apps_dir = data_home.path().join("applications");
        std::fs::create_dir_all(&apps_dir).unwrap();
        write_desktop_file(
            &apps_dir,
            "editor.desktop",
            "[Desktop Entry]\nType=Application\nName=My Editor\nExec=/bin/cat %f\nMimeType=text/plain;\n",
        );

        let prev_home = std::env::var("XDG_DATA_HOME").ok();
        let prev_dirs = std::env::var("XDG_DATA_DIRS").ok();
        std::env::set_var("XDG_DATA_HOME", data_home.path());
        std::env::set_var("XDG_DATA_DIRS", "/nonexistent-xdg-dir");

        let programs = find_programs(&["txt"]);

        match prev_home {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        match prev_dirs {
            Some(v) => std::env::set_var("XDG_DATA_DIRS", v),
            None => std::env::remove_var("XDG_DATA_DIRS"),
        }

        assert_eq!(programs.len(), 1, "{:?}", programs);
        assert_eq!(programs[0].name, "My Editor");
    }

    #[test]
    fn find_icons_picks_the_largest_size() {
        let _guard = ENV_LOCK.lock().unwrap();
        let data_home = tempfile::tempdir().unwrap();
        let theme_16 = data_home.path().join("icons/hicolor/16x16/apps");
        let theme_48 = data_home.path().join("icons/hicolor/48x48/apps");
        std::fs::create_dir_all(&theme_16).unwrap();
        std::fs::create_dir_all(&theme_48).unwrap();
        std::fs::write(theme_16.join("my-app.png"), b"small").unwrap();
        std::fs::write(theme_48.join("my-app.png"), b"large").unwrap();

        let prev = std::env::var("XDG_DATA_HOME").ok();
        std::env::set_var("XDG_DATA_HOME", data_home.path());
        let icons = find_icons();
        match prev {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }

        let found = icons.get("my-app").unwrap();
        assert!(found.starts_with(&theme_48), "expected the 48x48 icon, got {found:?}");
    }
}
