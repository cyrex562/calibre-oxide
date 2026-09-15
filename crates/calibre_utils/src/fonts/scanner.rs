//! Port of `calibre.utils.fonts.scanner` (issue #556, split from #63):
//! system font directory scanning, family grouping, and the
//! font-for-text-coverage picker every one of this crate's real
//! callers actually needs -- confirmed by grepping every real
//! (non-`gui2/`) importer of `fonts.scanner` in upstream calibre
//! before picking this up: `ebooks/oeb/transforms/embed_fonts.py`
//! (`embed_all_fonts`, issue #169's own target), `flatcss.py`'s
//! `--embed-font-family`, `oeb/polish/embed.py`, `docx/fonts.py`, and
//! `fonts/utils.py`'s own `get_font_for_text`. The issue that split
//! this off called it "GUI-and-OS-integration-adjacent" with "no real
//! caller" -- that framing turned out to be stale; this crate already
//! has five real `todo!()`/placeholder call sites blocked on exactly
//! this module (see each site's own comment: `embed_fonts.rs`,
//! `flatcss.rs`, `oeb/polish/embed.rs`, `docx/fonts.rs`,
//! `calibre_utils::fonts::utils`).
//!
//! # Disclosed simplifications
//!
//! - **Synchronous, not a background `Thread`.** Upstream's
//!   `FontScanner` extends `Thread` and starts scanning at import
//!   time, so a desktop GUI's startup isn't blocked on a slow disk
//!   scan; every real caller calls `.join()` before actually reading
//!   any result, so the *visible* behavior is "scan, then use" either
//!   way. This crate has no GUI startup-responsiveness concern to
//!   protect, so [`FontScanner::scan`] just runs synchronously --
//!   same real algorithm, no threading indirection to model.
//! - **No persistent disk cache.** Upstream's `JSONConfig`-backed
//!   `scanner_cache` (keyed by file size+mtime) avoids re-parsing every
//!   font file on every scan. Not ported: a real, valuable
//!   optimization, but orthogonal to correctness, and every current
//!   caller only needs one scan per process lifetime regardless.
//! - **`fc_list`'s real fontconfig FFI query isn't ported.**
//!   [`font_dirs`] always uses [`default_font_dirs`]'s literal list on
//!   Linux, which already covers the standard paths a real fontconfig
//!   query would also report for a typical install; a narrower, not a
//!   silent, scope than upstream's dynamic query.
//! - **macOS's font directory list is included** (literal upstream
//!   data -- zero transcription risk) **but unverified** on real macOS
//!   hardware, since this project develops on Linux.
//! - **Windows' `font_dirs` needs `winutil`'s special-folder FFI** --
//!   not ported, matching this crate's established Windows-only
//!   narrowing (issues #78/#79/#258); falls back to
//!   [`default_font_dirs`].
//! - **`path_significance`'s prefix check is real `Path` component
//!   comparison**, not upstream's raw string `str.startswith` (which
//!   would treat `/usr/share/fontsX` as "inside" `/usr/share/fonts`) --
//!   a real, narrow correctness improvement, not a behavior change any
//!   real font layout would ever exercise differently.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::metadata::FontMetadata;
use super::utils::{get_printable_characters, panose_to_css_generic_family, supports_text};
use crate::icu::lower as icu_lower;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoFonts(pub String);

impl fmt::Display for NoFonts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for NoFonts {}

/// One font face's metadata plus the path it was read from -- the
/// per-entry shape `FontScanner`'s internal `cached_fonts`/
/// `font_family_map` dicts carry upstream.
#[derive(Debug, Clone)]
pub struct FontFace {
    pub path: PathBuf,
    pub is_otf: bool,
    pub font_family: String,
    pub font_weight: String,
    pub font_style: String,
    pub font_stretch: &'static str,
    pub full_name: String,
    pub subfamily_name: Option<String>,
    pub preferred_subfamily_name: Option<String>,
    pub wws_subfamily_name: Option<String>,
    pub panose: [u8; 10],
}

impl FontFace {
    fn from_metadata(path: PathBuf, fm: &FontMetadata) -> Option<Self> {
        Some(FontFace {
            path,
            is_otf: fm.is_otf,
            font_family: fm.font_family.clone()?,
            font_weight: fm.font_weight.clone(),
            font_style: fm.font_style.to_string(),
            font_stretch: fm.font_stretch,
            full_name: fm.names.full_name.clone().unwrap_or_default(),
            subfamily_name: fm.names.subfamily_name.clone(),
            preferred_subfamily_name: fm.names.preferred_subfamily_name.clone(),
            wws_subfamily_name: fm.names.wws_subfamily_name.clone(),
            panose: fm.characteristics.panose,
        })
    }
}

/// Port of `default_font_dirs`.
pub fn default_font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/opt/share/fonts"), PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts")];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join(".fonts"));
    }
    dirs
}

/// Port of `font_dirs`. See the module doc for what's real vs.
/// disclosed as narrower here (`fc_list`'s FFI query, Windows'
/// registry-based paths).
pub fn font_dirs() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        let mut dirs = vec![PathBuf::from("/Library/Fonts"), PathBuf::from("/System/Library/Fonts"), PathBuf::from("/usr/share/fonts"), PathBuf::from("/var/root/Library/Fonts")];
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(".fonts"));
            dirs.push(home.join("Library/Fonts"));
        }
        dirs
    } else {
        default_font_dirs()
    }
}

/// Port of `font_priority`: try to ensure the "Regular" face sorts
/// first for a family.
fn font_priority(face: &FontFace) -> u8 {
    let style_normal = face.font_style == "normal";
    let width_normal = face.font_stretch == "normal";
    let weight_normal = face.font_weight == "normal";
    let num_normal = [style_normal, width_normal, weight_normal].into_iter().filter(|&x| x).count() as u8;
    let subfamily = face.wws_subfamily_name.as_deref().or(face.preferred_subfamily_name.as_deref()).or(face.subfamily_name.as_deref()).unwrap_or("");
    if num_normal == 3 && subfamily == "Regular" {
        return 0;
    }
    if num_normal == 3 {
        return 1;
    }
    if subfamily == "Regular" {
        return 2;
    }
    3 + (3 - num_normal)
}

/// Port of `path_significance`. See the module doc for the real
/// (component-wise, not raw-string) prefix check.
fn path_significance(path: &Path, folders: &[PathBuf]) -> i64 {
    for (i, q) in folders.iter().enumerate() {
        if path.starts_with(q) {
            return i as i64;
        }
    }
    -1
}

/// Port of `build_families`.
fn build_families(faces: Vec<FontFace>, folders: &[PathBuf]) -> (HashMap<String, Vec<FontFace>>, Vec<String>) {
    let mut families: HashMap<String, Vec<FontFace>> = HashMap::new();
    for f in faces {
        let key = icu_lower(&f.font_family);
        if key.is_empty() {
            continue;
        }
        families.entry(key).or_default().push(f);
    }

    for faces in families.values_mut() {
        // Drop duplicate font files (same family/weight/stretch/style
        // fingerprint), preferring the one from a more significant
        // (e.g. user-level over system-level) font directory.
        let mut fmap: HashMap<(String, String, &'static str, String), usize> = HashMap::new();
        let mut keep = vec![true; faces.len()];
        for i in 0..faces.len() {
            let f = &faces[i];
            let fingerprint = (icu_lower(&f.font_family), f.font_weight.clone(), f.font_stretch, f.font_style.clone());
            match fmap.get(&fingerprint) {
                Some(&existing) => {
                    if path_significance(&f.path, folders) >= path_significance(&faces[existing].path, folders) {
                        keep[existing] = false;
                        fmap.insert(fingerprint, i);
                    } else {
                        keep[i] = false;
                    }
                }
                None => {
                    fmap.insert(fingerprint, i);
                }
            }
        }
        let mut i = 0;
        faces.retain(|_| {
            let k = keep[i];
            i += 1;
            k
        });
        faces.sort_by_key(font_priority);
    }

    let mut font_families: Vec<String> = families.values().filter_map(|f| f.first()).map(|f| f.font_family.clone()).collect();
    font_families.sort_by(|a, b| crate::icu::strcmp(a, b));
    (families, font_families)
}

/// Port of `FontScanner`. See the module doc for what's real vs.
/// disclosed as a narrower/deferred scope.
#[derive(Debug, Default)]
pub struct FontScanner {
    font_family_map: HashMap<String, Vec<FontFace>>,
    font_families: Vec<String>,
}

impl FontScanner {
    /// Port of `FontScanner.do_scan`/`build_families`, minus the
    /// persistent cache and background thread (see the module doc).
    /// `folders` are searched in order; `allowed_extensions` matches
    /// `{'ttf', 'otf'}` case-insensitively.
    pub fn scan(folders: &[PathBuf], allowed_extensions: &[&str]) -> Self {
        let exts: Vec<String> = allowed_extensions.iter().map(|e| e.to_lowercase()).collect();
        let mut faces = Vec::new();
        for folder in folders {
            if !folder.is_dir() {
                continue;
            }
            for entry in walkdir::WalkDir::new(folder).into_iter().filter_map(|e| e.ok()) {
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                let Some(ext) = path.extension().and_then(|e| e.to_str()) else { continue };
                if !exts.contains(&ext.to_lowercase()) {
                    continue;
                }
                let Ok(raw) = std::fs::read(path) else { continue };
                let Ok(fm) = FontMetadata::new(&raw) else { continue };
                if let Some(face) = FontFace::from_metadata(path.to_path_buf(), &fm) {
                    faces.push(face);
                }
            }
        }
        let (font_family_map, font_families) = build_families(faces, folders);
        FontScanner { font_family_map, font_families }
    }

    /// Port of `find_font_families`.
    pub fn find_font_families(&self) -> &[String] {
        &self.font_families
    }

    /// Port of `fonts_for_family`.
    pub fn fonts_for_family(&self, family: &str) -> Result<&[FontFace], NoFonts> {
        self.font_family_map.get(&icu_lower(family)).map(Vec::as_slice).ok_or_else(|| NoFonts(format!("No fonts found for the family: {family:?}")))
    }

    /// Port of `legacy_fonts_for_family`.
    pub fn legacy_fonts_for_family(&self, family: &str) -> HashMap<&'static str, (PathBuf, String)> {
        let mut ans = HashMap::new();
        let Ok(faces) = self.fonts_for_family(family) else { return ans };
        for (i, face) in faces.iter().enumerate() {
            let key = if i == 0 {
                "normal"
            } else if face.font_style == "italic" || face.font_style == "oblique" {
                if face.font_weight == "bold" {
                    "bi"
                } else {
                    "italic"
                }
            } else if face.font_weight == "bold" {
                "bold"
            } else {
                continue;
            };
            ans.insert(key, (face.path.clone(), face.full_name.clone()));
        }
        ans
    }

    /// Port of `get_font_data`.
    pub fn get_font_data(&self, face: &FontFace) -> std::io::Result<Vec<u8>> {
        std::fs::read(&face.path)
    }

    /// Port of `find_font_for_text`. Returns `(family, faces)` or
    /// `(None, None)`.
    pub fn find_font_for_text(&self, text: &str, allowed_families: &[&str], preferred_families: &[&str]) -> (Option<String>, Option<Vec<FontFace>>) {
        let text = get_printable_characters(text);
        let mut found: HashMap<String, (String, Vec<FontFace>)> = HashMap::new();

        for family in &self.font_families {
            let Ok(candidates) = self.fonts_for_family(family) else { continue };
            let faces: Vec<FontFace> = candidates
                .iter()
                .filter(|f| match self.get_font_data(f) {
                    Ok(raw) => supports_text(&raw, &text, true),
                    Err(_) => false,
                })
                .cloned()
                .collect();
            let Some(first) = faces.first() else { continue };
            let generic_family = panose_to_css_generic_family(&first.panose);
            if allowed_families.contains(&generic_family.as_str()) || preferred_families.first() == Some(&generic_family.as_str()) {
                return (Some(family.clone()), Some(faces));
            }
            found.entry(generic_family).or_insert_with(|| (family.clone(), faces));
        }

        for f in preferred_families {
            if let Some((family, faces)) = found.get(*f) {
                return (Some(family.clone()), Some(faces.clone()));
            }
        }
        (None, None)
    }
}

static FONT_SCANNER: OnceLock<FontScanner> = OnceLock::new();

/// Port of the module-level `font_scanner` singleton, scanned lazily
/// on first use instead of upstream's background `Thread` started at
/// import time (see the module doc for why that distinction is
/// invisible to every real caller, which always waits for the scan
/// before reading a result either way).
///
/// Folders searched: [`font_dirs`] plus `<config_dir>/fonts` (a real
/// user override location, matching upstream). **Not included**:
/// calibre's own bundled Liberation font family
/// (upstream's `P('fonts/liberation')`) -- a real third-party asset
/// this port hasn't vendored, the same open question issue #484 (the
/// vendored MathJax bundle) already settled for a different asset;
/// disclosed here as a real, separate gap rather than silently
/// assumed present.
pub fn font_scanner() -> &'static FontScanner {
    FONT_SCANNER.get_or_init(|| {
        let mut folders = font_dirs();
        folders.push(crate::constants::config_dir().join("fonts"));
        FontScanner::scan(&folders, &["ttf", "otf"])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_sfnt(tables: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]);
        out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let header_len = 12 + tables.len() * 16;
        let mut data_section = Vec::new();
        let mut records = Vec::new();
        let mut offset = header_len;
        for (tag, data) in tables {
            records.push((**tag, 0u32, offset, data.len()));
            data_section.extend_from_slice(data);
            while data_section.len() % 4 != 0 {
                data_section.push(0);
            }
            offset = header_len + data_section.len();
        }
        for (tag, checksum, table_offset, table_length) in records {
            out.extend_from_slice(&tag);
            out.extend_from_slice(&checksum.to_be_bytes());
            out.extend_from_slice(&(table_offset as u32).to_be_bytes());
            out.extend_from_slice(&(table_length as u32).to_be_bytes());
        }
        out.extend_from_slice(&data_section);
        out
    }

    fn utf16_be(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
    }

    fn build_name_table(records: &[(u16, u16, u16, u16, Vec<u8>)]) -> Vec<u8> {
        let mut header = Vec::new();
        header.extend_from_slice(&0u16.to_be_bytes());
        header.extend_from_slice(&(records.len() as u16).to_be_bytes());
        let string_storage_offset = 6 + records.len() * 12;
        header.extend_from_slice(&(string_storage_offset as u16).to_be_bytes());
        let mut string_storage = Vec::new();
        let mut record_entries = Vec::new();
        for (platform_id, encoding_id, language_id, name_id, text) in records {
            let str_offset = string_storage.len();
            string_storage.extend_from_slice(text);
            record_entries.extend_from_slice(&platform_id.to_be_bytes());
            record_entries.extend_from_slice(&encoding_id.to_be_bytes());
            record_entries.extend_from_slice(&language_id.to_be_bytes());
            record_entries.extend_from_slice(&name_id.to_be_bytes());
            record_entries.extend_from_slice(&(text.len() as u16).to_be_bytes());
            record_entries.extend_from_slice(&(str_offset as u16).to_be_bytes());
        }
        let mut out = header;
        out.extend_from_slice(&record_entries);
        out.extend_from_slice(&string_storage);
        out
    }

    fn build_os2_table(weight: u16, width: u16, selection: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&0i16.to_be_bytes());
        out.extend_from_slice(&weight.to_be_bytes());
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        for _ in 0..11 {
            out.extend_from_slice(&0i16.to_be_bytes());
        }
        out.extend_from_slice(&[0u8; 10]);
        out.extend_from_slice(&[0u8; 16]);
        out.extend_from_slice(&[0u8; 4]);
        out.extend_from_slice(&selection.to_be_bytes());
        out
    }

    fn write_font(dir: &Path, filename: &str, family: &str, subfamily: &str, weight: u16, selection: u16) -> PathBuf {
        let name = build_name_table(&[(3, 1, 1033, 1, utf16_be(family)), (3, 1, 1033, 2, utf16_be(subfamily)), (3, 1, 1033, 4, utf16_be(&format!("{family} {subfamily}")))]);
        let os2 = build_os2_table(weight, 5, selection);
        let raw = build_sfnt(&[(b"name", &name), (b"OS/2", &os2)]);
        let path = dir.join(filename);
        std::fs::write(&path, &raw).unwrap();
        path
    }

    /// A single-segment format-4 cmap mapping `start..=end` to glyph
    /// ids offset by 1, plus the required terminator segment -- same
    /// technique `fonts::utils`'s own `get_glyph_ids` tests use.
    fn build_cmap_table(start: u16, end: u16) -> Vec<u8> {
        let mut sub = Vec::new();
        sub.extend_from_slice(&4u16.to_be_bytes());
        let length_pos = sub.len();
        sub.extend_from_slice(&0u16.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes());
        sub.extend_from_slice(&4u16.to_be_bytes());
        sub.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        sub.extend_from_slice(&end.to_be_bytes());
        sub.extend_from_slice(&0xffffu16.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes());
        sub.extend_from_slice(&start.to_be_bytes());
        sub.extend_from_slice(&0xffffu16.to_be_bytes());
        sub.extend_from_slice(&1i16.to_be_bytes());
        sub.extend_from_slice(&1i16.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes());
        let len = sub.len() as u16;
        sub[length_pos..length_pos + 2].copy_from_slice(&len.to_be_bytes());

        let mut cmap = Vec::new();
        cmap.extend_from_slice(&0u16.to_be_bytes());
        cmap.extend_from_slice(&1u16.to_be_bytes());
        cmap.extend_from_slice(&3u16.to_be_bytes());
        cmap.extend_from_slice(&1u16.to_be_bytes());
        cmap.extend_from_slice(&12u32.to_be_bytes());
        cmap.extend_from_slice(&sub);
        cmap
    }

    fn write_font_with_cmap(dir: &Path, filename: &str, family: &str, start: u16, end: u16) -> PathBuf {
        let name = build_name_table(&[(3, 1, 1033, 1, utf16_be(family)), (3, 1, 1033, 2, utf16_be("Regular")), (3, 1, 1033, 4, utf16_be(&format!("{family} Regular")))]);
        let os2 = build_os2_table(400, 5, 1 << 6);
        let cmap = build_cmap_table(start, end);
        let raw = build_sfnt(&[(b"name", &name), (b"OS/2", &os2), (b"cmap", &cmap)]);
        let path = dir.join(filename);
        std::fs::write(&path, &raw).unwrap();
        path
    }

    #[test]
    fn scans_a_directory_and_groups_faces_by_family_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        write_font(dir.path(), "a.ttf", "My Family", "Regular", 400, 1 << 6);
        write_font(dir.path(), "b.ttf", "my family", "Bold", 700, 1 << 5);
        write_font(dir.path(), "c.txt", "Not A Font", "Regular", 400, 0); // wrong extension, skipped

        let scanner = FontScanner::scan(&[dir.path().to_path_buf()], &["ttf", "otf"]);
        assert_eq!(scanner.find_font_families(), &["My Family".to_string()]);
        let faces = scanner.fonts_for_family("MY FAMILY").unwrap();
        assert_eq!(faces.len(), 2);
        // Regular sorts first (font_priority).
        assert_eq!(faces[0].font_weight, "normal");
    }

    #[test]
    fn fonts_for_family_errors_for_an_unknown_family() {
        let scanner = FontScanner::default();
        assert!(scanner.fonts_for_family("Nope").is_err());
    }

    #[test]
    fn legacy_fonts_for_family_maps_normal_bold_italic_and_bi() {
        let dir = tempfile::tempdir().unwrap();
        write_font(dir.path(), "r.ttf", "Fam", "Regular", 400, 1 << 6);
        write_font(dir.path(), "b.ttf", "Fam", "Bold", 700, 1 << 5);
        write_font(dir.path(), "i.ttf", "Fam", "Italic", 400, 1 << 0);
        write_font(dir.path(), "bi.ttf", "Fam", "Bold Italic", 700, (1 << 0) | (1 << 5));

        let scanner = FontScanner::scan(&[dir.path().to_path_buf()], &["ttf"]);
        let legacy = scanner.legacy_fonts_for_family("Fam");
        assert!(legacy.contains_key("normal"));
        assert!(legacy.contains_key("bold"));
        assert!(legacy.contains_key("italic"));
        assert!(legacy.contains_key("bi"));
    }

    #[test]
    fn a_duplicate_font_in_a_more_significant_folder_wins() {
        let system_dir = tempfile::tempdir().unwrap();
        let user_dir = tempfile::tempdir().unwrap();
        write_font(system_dir.path(), "a.ttf", "Fam", "Regular", 400, 1 << 6);
        let user_path = write_font(user_dir.path(), "a.ttf", "Fam", "Regular", 400, 1 << 6);

        // `path_significance` treats a LATER position in `folders` as
        // MORE significant (`>=` favors the new match on a tie or
        // greater index) -- matching real upstream's own `font_dirs()`
        // list shape, which puts user directories (`~/.local/share/
        // fonts`, `~/.fonts`) after the system ones
        // (`/usr/share/fonts`, ...). System dir listed first here to
        // match that same real convention.
        let folders = vec![system_dir.path().to_path_buf(), user_dir.path().to_path_buf()];
        let scanner = FontScanner::scan(&folders, &["ttf"]);
        let faces = scanner.fonts_for_family("Fam").unwrap();
        assert_eq!(faces.len(), 1, "the duplicate should be deduped, not both kept");
        assert_eq!(faces[0].path, user_path);
    }

    #[test]
    fn get_font_data_reads_the_real_file_back() {
        let dir = tempfile::tempdir().unwrap();
        write_font(dir.path(), "a.ttf", "Fam", "Regular", 400, 1 << 6);
        let scanner = FontScanner::scan(&[dir.path().to_path_buf()], &["ttf"]);
        let face = &scanner.fonts_for_family("Fam").unwrap()[0];
        let data = scanner.get_font_data(face).unwrap();
        assert!(!data.is_empty());
    }

    #[test]
    fn find_font_for_text_picks_a_family_that_actually_covers_the_text() {
        let dir = tempfile::tempdir().unwrap();
        // Covers only 0x30-0x39 (digits) -- can't render "AB".
        write_font_with_cmap(dir.path(), "digits.ttf", "Digits Only", 0x30, 0x39);
        // Covers 0x41-0x5a (A-Z) -- can render "AB".
        write_font_with_cmap(dir.path(), "letters.ttf", "Letters", 0x41, 0x5a);

        let scanner = FontScanner::scan(&[dir.path().to_path_buf()], &["ttf"]);
        let (family, faces) = scanner.find_font_for_text("AB", &["serif", "sans-serif"], &["serif", "sans-serif", "monospace", "cursive", "fantasy"]);
        assert_eq!(family.as_deref(), Some("Letters"));
        assert_eq!(faces.unwrap().len(), 1);
    }

    #[test]
    fn find_font_for_text_returns_none_when_nothing_covers_it() {
        let dir = tempfile::tempdir().unwrap();
        write_font_with_cmap(dir.path(), "digits.ttf", "Digits Only", 0x30, 0x39);

        let scanner = FontScanner::scan(&[dir.path().to_path_buf()], &["ttf"]);
        let (family, faces) = scanner.find_font_for_text("AB", &["serif", "sans-serif"], &["serif", "sans-serif", "monospace", "cursive", "fantasy"]);
        assert_eq!(family, None);
        assert!(faces.is_none());
    }

    #[test]
    fn default_font_dirs_includes_the_standard_linux_paths() {
        let dirs = default_font_dirs();
        assert!(dirs.contains(&PathBuf::from("/usr/share/fonts")));
    }
}
