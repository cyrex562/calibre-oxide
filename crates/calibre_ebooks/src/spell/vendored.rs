//! The dictionaries shipped with this binary (issue #865).
//!
//! # Why they are baked in rather than configured
//!
//! `builtin_dictionaries` reads `.aff`/`.dic` files from a directory,
//! so spell check needs real files on disk. Requiring the user to
//! supply them -- a `--dictionaries-dir` flag, or a system hunspell
//! install -- means spell check silently does nothing on a machine
//! that has none, which is most Windows machines and plenty of Linux
//! ones. Baking them in follows the same convention as the vendored
//! MathJax bundle (`calibre_srv::mathjax`) and makes the feature work
//! from a single binary with no configuration and no network.
//!
//! They are extracted once, on first use, to a cache directory. That
//! is the smallest bridge between "compiled into the binary" and an
//! API that takes a path.
//!
//! # Licensing
//!
//! en-US is SCOWL (permissive, Kevin Atkinson); en-GB is LGPL
//! (Bartlett/Brown/Pinto) and carries its licence in its own `.aff`
//! header. Full attribution, and why calibre's own unattributed en-US
//! copy is deliberately not used, is in `resources/dictionaries/NOTICE`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use include_dir::{include_dir, Dir};

static DICTIONARIES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/resources/dictionaries");

/// Extracts the vendored dictionaries once and returns the directory.
///
/// Returns `None` if extraction fails -- spell check is then simply
/// unavailable, which is a far better outcome than refusing to start
/// a server over a cache directory.
pub fn dictionaries_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let base = std::env::temp_dir().join(format!("calibre-oxide-dictionaries-{}", env!("CARGO_PKG_VERSION")));
        // A marker written last, so a run interrupted mid-extraction
        // does not leave a half-populated directory looking complete.
        let marker = base.join(".complete");
        if marker.exists() {
            return Some(base);
        }
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).ok()?;
        DICTIONARIES.extract(&base).ok()?;
        std::fs::write(&marker, b"1").ok()?;
        Some(base)
    })
    .as_deref()
}

/// Every dictionary shipped with this binary.
pub fn builtin() -> Vec<super::dictionary::DictionaryMeta> {
    match dictionaries_dir() {
        Some(dir) => super::dictionary::builtin_dictionaries(dir),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point: a user gets working spell check without
    /// configuring anything or having hunspell installed.
    #[test]
    fn the_shipped_dictionaries_load_without_any_configuration() {
        let dicts = builtin();
        let locales: Vec<String> = dicts.iter().map(|d| format!("{}-{}", d.primary_locale.langcode, d.primary_locale.countrycode.as_deref().unwrap_or(""))).collect();

        // `langcode` is ISO 639-3, so English is "eng" rather than
        // "en" -- worth pinning, since the directory names and the
        // `locales` files both use the two-letter form and the
        // mismatch is easy to carry into a lookup by mistake.
        assert!(locales.iter().any(|l| l == "eng-US"), "en-US should ship: {locales:?}");
        assert!(locales.iter().any(|l| l == "eng-GB"), "en-GB should ship: {locales:?}");
    }

    #[test]
    fn a_shipped_dictionary_really_parses_and_recognises_words() {
        let dicts = builtin();
        let meta = dicts.iter().find(|d| d.primary_locale.countrycode.as_deref() == Some("US")).expect("en-US should ship");
        let mut loaded = super::super::dictionary::load_dictionary(meta).expect("the vendored dictionary should parse");

        assert!(loaded.dict.check("library"), "a common word should be recognised");
        assert!(!loaded.dict.check("libary"), "a misspelling should not be");
        let _ = &mut loaded;
    }

    /// Extraction is memoized and marker-guarded, so a second call
    /// must not re-extract or return a different directory.
    #[test]
    fn extraction_is_stable_across_calls() {
        assert_eq!(dictionaries_dir(), dictionaries_dir());
        assert!(dictionaries_dir().is_some_and(|d| d.join(".complete").exists()));
    }

    /// The attribution has to ship with the files, not just live in a
    /// commit message.
    #[test]
    fn the_licence_notice_ships_alongside_them() {
        let notice = DICTIONARIES.get_file("NOTICE").expect("NOTICE must be vendored with the dictionaries");
        let text = notice.contents_utf8().unwrap();
        assert!(text.contains("SCOWL"), "en-US provenance must be recorded");
        assert!(text.contains("LGPL"), "en-GB licence must be recorded");
    }
}
