//! Port of `old_src/src/calibre/utils/file_type_icons.py`: which
//! icon name a given file-type/extension maps to in calibre's GUI
//! file browser.

/// Port of `EXT_MAP`: extension (lowercase, no leading dot) -> icon
/// name. Returns `None` for an extension with no specific icon
/// (upstream's own callers fall back to `EXT_MAP['default']`
/// themselves rather than baking that in here).
pub fn icon_for_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "dir" => "dir",
        "zero" => "zero",
        "jpeg" | "jpg" => "jpeg",
        "gif" => "gif",
        "png" => "png",
        "bmp" => "bmp",
        "cbz" => "cbz",
        "cbr" => "cbr",
        "svg" => "svg",
        "html" | "htmlz" | "htm" | "xhtml" | "xhtm" => "html",
        "lit" => "lit",
        "lrf" => "lrf",
        "lrx" => "lrx",
        "pdf" => "pdf",
        "pdr" => "zero",
        "rar" => "rar",
        "zip" => "zip",
        "txt" | "text" => "txt",
        "prc" | "azw" | "mobi" | "pobi" => "mobi",
        "mbp" => "zero",
        "azw1" => "tpz",
        "azw2" => "azw2",
        "azw3" => "azw3",
        "kfx" | "kfx-zip" => "kfx",
        "azw4" => "pdf",
        "tpz" => "tpz",
        "tan" => "zero",
        "epub" => "epub",
        "fb2" => "fb2",
        "rtf" => "rtf",
        "odt" => "odt",
        "snb" => "snb",
        "djv" | "djvu" => "djvu",
        "xps" | "oxps" => "xps",
        "docx" => "docx",
        "opml" => "opml",
        _ => return None,
    })
}

/// Port of `EXT_MAP['default']`: the icon used when [`icon_for_ext`]
/// has no specific mapping.
pub const DEFAULT_ICON: &str = "unknown";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_extensions_to_their_icon() {
        assert_eq!(icon_for_ext("epub"), Some("epub"));
        assert_eq!(icon_for_ext("mobi"), Some("mobi"));
        assert_eq!(icon_for_ext("azw3"), Some("azw3"));
    }

    #[test]
    fn aliases_share_the_same_icon() {
        assert_eq!(icon_for_ext("jpg"), icon_for_ext("jpeg"));
        assert_eq!(icon_for_ext("htm"), icon_for_ext("html"));
        assert_eq!(icon_for_ext("prc"), icon_for_ext("mobi"));
    }

    #[test]
    fn an_unknown_extension_falls_back_to_the_default() {
        assert_eq!(icon_for_ext("not-a-real-ext"), None);
    }
}
