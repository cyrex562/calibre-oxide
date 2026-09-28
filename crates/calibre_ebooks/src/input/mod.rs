use std::path::Path;

pub mod azw4_input;
pub mod chm_input;
pub mod comic_input;
pub mod djvu_input;
pub mod docx_input;
pub mod epub_input;
pub mod fb2_input;
pub mod html_input;
pub mod htmlz_input;
pub mod lit_input;
pub mod lrf_input;
pub mod mobi_input;
pub mod odt_input;
pub mod pdb_input;
pub mod pdf_input;
pub mod pml_input;
pub mod rar_input;
pub mod rb_input;
pub mod recipe_input;
pub mod rtf_input;
pub mod snb_input;
pub mod tcr_input;
pub mod txt_input;
pub mod zip_input;

/// Joins an archive-supplied relative path onto `base`, or `None` if it
/// would escape.
///
/// Only `Normal` components are kept. `ParentDir` (`..`), `RootDir` and
/// `Prefix` (a Windows drive or UNC root) are all rejected outright rather
/// than normalised away, because a path that contains them is not a path
/// this archive should be asking to write.
///
/// Component-based rather than string-based on purpose: it is platform
/// correct without special cases. On Windows `..\\..\\x` parses as two
/// `ParentDir`s and is rejected; on Unix the same text is one ordinary
/// filename and stays contained.
///
/// Deliberately does **not** canonicalize. The destination does not exist
/// yet, so canonicalizing it would fail -- and canonicalizing the parent
/// instead resolves symlinks, which turns a check into a TOCTOU race.
/// Rejecting the components outright needs no filesystem access at all.
pub(crate) fn safe_join(base: &Path, relative: &str) -> Option<std::path::PathBuf> {
    use std::path::Component;

    let candidate = Path::new(relative);
    if candidate.is_absolute() {
        return None;
    }

    let mut out = base.to_path_buf();
    let mut pushed = false;
    for component in candidate.components() {
        match component {
            Component::Normal(part) => {
                out.push(part);
                pushed = true;
            }
            // `./` carries no meaning here and is simply dropped.
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    // A path that contributed nothing (`.` alone) is not a file.
    if pushed {
        Some(out)
    } else {
        None
    }
}
