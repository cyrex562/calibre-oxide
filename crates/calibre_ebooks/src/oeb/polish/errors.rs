//! Port of `old_src/src/calibre/ebooks/oeb/polish/errors.py`.
//!
//! Python defines four distinct exception classes so callers can
//! `except` by type. Rust idiom for that shape is a single `thiserror`
//! enum with one variant per class (matching the precedent in
//! `crate::mobi::MobiError`) rather than four separate structs -- callers
//! match on the variant instead of the type.

use std::cell::RefCell;

thread_local! {
    /// Port of the module-global `_drm_message` in `errors.py`, set via
    /// the `drm_message` context manager and read by `DRMError`'s
    /// constructor. A Python `contextmanager` that mutates module state
    /// for the duration of a `with` block maps to Rust as a thread-local
    /// plus an RAII guard that restores the previous value on `Drop`
    /// (`docs/AGENT_PORTING_GUIDE.md` #2: `contextlib` (with) -> RAII).
    static DRM_MESSAGE: RefCell<String> = const { RefCell::new(String::new()) };
}

/// RAII guard for [`drm_message_scope`]. Restores the previous DRM
/// message when dropped, mirroring `errors.py`'s `drm_message`
/// `@contextmanager`'s `finally` block.
pub struct DrmMessageGuard {
    previous: String,
}

impl Drop for DrmMessageGuard {
    fn drop(&mut self) {
        DRM_MESSAGE.with(|m| *m.borrow_mut() = std::mem::take(&mut self.previous));
    }
}

/// Sets the message [`PolishError::Drm`] will carry for as long as the
/// returned guard is alive. Port of `errors.py`'s `drm_message`
/// context manager:
///
/// ```python
/// with drm_message('This file uses obfuscated fonts'):
///     ...
///     raise DRMError()
/// ```
///
/// becomes
///
/// ```ignore
/// let _guard = drm_message_scope("This file uses obfuscated fonts");
/// // ...
/// return Err(PolishError::drm());
/// ```
#[must_use = "the DRM message reverts as soon as the guard is dropped"]
pub fn drm_message_scope(msg: impl Into<String>) -> DrmMessageGuard {
    let previous = DRM_MESSAGE.with(|m| std::mem::replace(&mut *m.borrow_mut(), msg.into()));
    DrmMessageGuard { previous }
}

fn current_drm_message() -> String {
    DRM_MESSAGE.with(|m| m.borrow().clone())
}

/// Port of `errors.py`'s four exception classes.
#[derive(Debug, thiserror::Error)]
pub enum PolishError {
    /// Port of `InvalidBook(ValueError)`: the container/book structure
    /// itself is broken (e.g. no OPF file found).
    #[error("{0}")]
    InvalidBook(String),

    /// Port of `DRMError(calibre.ebooks.DRMError)`. Its message defaults
    /// to a generic "locked with DRM" string, or whatever was set via
    /// [`drm_message_scope`] at the point of construction -- call
    /// [`PolishError::drm`] rather than building this variant directly
    /// so that lookup happens.
    #[error("{0}")]
    Drm(String),

    /// Port of `MalformedMarkup(ValueError)`.
    #[error("{0}")]
    MalformedMarkup(String),

    /// Port of `UnsupportedContainerType(Exception)`.
    #[error("{0}")]
    UnsupportedContainerType(String),
}

impl PolishError {
    /// Builds the `Drm` variant, reading whatever message is currently
    /// active via [`drm_message_scope`] (or a generic default), exactly
    /// like Python's `DRMError()` reading the `_drm_message` global at
    /// construction time.
    pub fn drm() -> PolishError {
        let msg = current_drm_message();
        let msg = if msg.is_empty() {
            "This file is locked with DRM. It cannot be edited.".to_string()
        } else {
            msg
        };
        PolishError::Drm(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drm_error_uses_generic_message_by_default() {
        let err = PolishError::drm();
        assert!(matches!(err, PolishError::Drm(ref m) if m.contains("locked with DRM")));
    }

    #[test]
    fn drm_message_scope_overrides_and_restores() {
        {
            let _guard = drm_message_scope("custom obfuscation message");
            let err = PolishError::drm();
            assert!(matches!(err, PolishError::Drm(ref m) if m == "custom obfuscation message"));
        }
        // Guard dropped: reverts to the generic default.
        let err = PolishError::drm();
        assert!(matches!(err, PolishError::Drm(ref m) if m.contains("locked with DRM")));
    }

    #[test]
    fn invalid_book_carries_message() {
        let err = PolishError::InvalidBook("Could not locate opf file".to_string());
        assert_eq!(err.to_string(), "Could not locate opf file");
    }
}
