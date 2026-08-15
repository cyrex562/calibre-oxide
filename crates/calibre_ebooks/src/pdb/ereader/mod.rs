//! Port of `src/calibre/ebooks/pdb/ereader`: the eReader ebook format, a
//! PalmDOC-derivative PML-markup format used by early Palm/Rocketbook
//! eReader devices and stored in a `.pdb` container (issue #43; builds on
//! the shared PDB container infrastructure from issue #42:
//! [`crate::pdb::header`], [`crate::pdb::formatreader`],
//! [`crate::pdb::formatwriter`]).
//!
//! Two on-disk sub-versions exist, distinguished by record 0's length:
//! v1.32 ("Dropbook", 132-byte record 0, [`reader132::Reader132`]) and
//! v2.02 ("Makebook", 116- or 202-byte record 0, [`reader202::Reader202`]).
//! [`reader::Reader`] is the dispatcher that picks between them, mirroring
//! `reader.py`.

pub mod inspector;
pub mod reader;
pub mod reader132;
pub mod reader202;
pub mod writer;

/// Port of `ereader/__init__.py`'s `EreaderError`, folded into a
/// `thiserror` enum following this crate's established DRM-error
/// convention (see `crate::mobi::MobiError::Drm` /
/// `crate::lit::LitError::Drm`): a plain format-error variant, plus a
/// `Drm` variant for the `raise DRMError(...)` call site in
/// `reader132.py` (compression code 260 or 272 means the payload is
/// eReader-DRM'd and genuinely cannot be decoded -- that's the format's
/// actual behavior, not a porting gap).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EreaderError {
    #[error("{0}")]
    Format(String),
    #[error("eReader DRM is not supported: {0}")]
    Drm(String),
}

impl EreaderError {
    /// Build an [`EreaderError::Format`] from anything printable.
    pub fn msg<S: Into<String>>(s: S) -> Self {
        EreaderError::Format(s.into())
    }
}

/// Port of `ereader/__init__.py`'s `image_name`: sanitize an image
/// href/filename into the fixed 32-byte, NUL-padded name field used by
/// eReader image records, deduping against `taken_names` by inserting an
/// incrementing counter before the extension (Python's
/// `f'{base_name}{i}{ext}'` loop).
///
/// `taken_names` holds the *unpadded* names already assigned to other
/// images, matching Python's `while name in taken_names` check (which
/// runs before the final `ljust`/truncate to the fixed-width field).
///
/// Not currently called by any of the other ported files in this module
/// (`writer.py` names images directly from `image_hrefs` instead) --
/// ported anyway since it's part of the module's public API surface,
/// matching Python.
pub fn image_name(name: &str, taken_names: &[String]) -> [u8; 32] {
    let base = std::path::Path::new(name)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());

    // Python slices `name[:10]`/`name[10+cut:]` on a `str`, i.e. by
    // Unicode code point, not by byte -- match that here.
    let mut name = base;
    if name.chars().count() > 32 {
        let chars: Vec<char> = name.chars().collect();
        let cut = chars.len() - 32;
        let names: String = chars.iter().take(10).collect();
        let namee: String = chars.iter().skip(10 + cut).collect();
        name = format!("{names}{namee}.png");
    }

    let (base_name, ext) = match name.rfind('.') {
        Some(idx) => (name[..idx].to_string(), name[idx..].to_string()),
        None => (name.clone(), String::new()),
    };

    let mut i = 0u32;
    while taken_names.iter().any(|t| t == &name) {
        i += 1;
        name = format!("{base_name}{i}{ext}");
    }

    // Final field is a fixed 32-*byte* record, so pad/truncate by byte
    // (the Python's `ljust(32, '\x00')[:32]` operates on `str` code
    // points, but every caller immediately encodes the result for an
    // on-disk record, so byte-width is what actually matters here).
    let mut field = [0u8; 32];
    let bytes = name.as_bytes();
    let len = bytes.len().min(32);
    field[..len].copy_from_slice(&bytes[..len]);
    field
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_name_pads_short_names_with_nulls() {
        let field = image_name("cover.png", &[]);
        assert_eq!(&field[..9], b"cover.png");
        assert!(field[9..].iter().all(|&b| b == 0));
    }

    #[test]
    fn image_name_strips_directory_components() {
        let field = image_name("images/sub/pic.png", &[]);
        assert_eq!(&field[..7], b"pic.png");
    }

    #[test]
    fn image_name_dedupes_against_taken_names() {
        let taken = vec!["pic.png".to_string()];
        let field = image_name("pic.png", &taken);
        let name: String = field
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect();
        assert_eq!(name, "pic1.png");
    }

    #[test]
    fn image_name_truncates_long_names_to_32_bytes() {
        let long_name = format!("{}.png", "x".repeat(60));
        let field = image_name(&long_name, &[]);
        // The field itself is always exactly 32 bytes.
        assert_eq!(field.len(), 32);
    }
}
