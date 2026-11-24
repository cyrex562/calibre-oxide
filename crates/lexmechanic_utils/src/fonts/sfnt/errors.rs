//! Port of `calibre.utils.fonts.sfnt.errors`.

use std::fmt;

/// A font is structurally malformed or uses a feature this port
/// doesn't support (e.g. an unrecognized sfnt version signature).
#[derive(Debug, Clone)]
pub struct UnsupportedFont(pub String);

impl fmt::Display for UnsupportedFont {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for UnsupportedFont {}

/// A font has no glyph data to work with.
#[derive(Debug, Clone)]
pub struct NoGlyphs(pub String);

impl fmt::Display for NoGlyphs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for NoGlyphs {}
