pub mod containers;
pub mod debug;
pub mod headers;
pub mod huffcdic;
pub mod index;
pub mod langcodes;
pub mod markup;
pub mod mobi6;
pub mod mobi8;
pub mod mobiml;
pub mod ncx;
pub mod opf_writer;
pub mod reader;
pub mod tweak;
pub mod utils;
pub mod writer2;
pub mod writer8;

/// Errors specific to the MOBI reader pipeline, mirroring the exception
/// classes `mobi6.py`/`mobi8.py`/`calibre.ebooks.mobi` raise: `MobiError`
/// (generic format error), `TopazError`/`KFXError` (unsupported sibling
/// formats detected by magic bytes), and `DRMError` (the book is
/// protected). Reader code otherwise returns `anyhow::Result` -- this enum
/// exists so callers who want to distinguish "this isn't a format we can
/// read at all" from an I/O/parse error can match on it.
#[derive(Debug, thiserror::Error)]
pub enum MobiError {
    #[error("{0}")]
    Format(String),
    #[error("This is an Amazon Topaz book. It cannot be processed.")]
    Topaz,
    #[error(
        "This is an Amazon KFX book. It cannot be processed. See \
         https://www.mobileread.com/forums/showthread.php?t=283371 for \
         information on how to handle KFX books."
    )]
    Kfx,
    #[error("this book is DRM protected: {0}")]
    Drm(String),
}

/// A tiny message sink used in place of Python's `calibre.utils.logging`
/// `Log` object. `debug`/`warn` mirror `log.debug`/`log.warn` call sites in
/// the ported modules; messages are just accumulated so tests (and, later,
/// a real logging backend) can inspect them.
#[derive(Debug, Default, Clone)]
pub struct MobiLog {
    pub messages: Vec<String>,
}

impl MobiLog {
    pub fn debug(&mut self, msg: impl Into<String>) {
        self.messages.push(format!("DEBUG: {}", msg.into()));
    }

    pub fn warn(&mut self, msg: impl Into<String>) {
        self.messages.push(format!("WARNING: {}", msg.into()));
    }

    pub fn warnings(&self) -> impl Iterator<Item = &str> {
        self.messages
            .iter()
            .filter(|m| m.starts_with("WARNING"))
            .map(|s| s.as_str())
    }
}
