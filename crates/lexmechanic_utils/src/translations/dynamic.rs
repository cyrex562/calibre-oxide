//! Port of `old_src/src/calibre/translations/dynamic.py`: dynamic,
//! per-language lookup of translations for user-visible strings,
//! caching the loaded catalog per language so repeated calls for the
//! same language don't re-parse a `.mo` file.
//!
//! # Scope
//!
//! Real: the caching behavior (`_CACHE`) and the fallback-to-original-
//! text behavior when no translation is available for a language or a
//! given string.
//!
//! Narrowed: upstream loads `messages.mo` for a language out of a
//! bundled `localization/locales.zip` resource (via
//! `calibre.utils.resources.get_path` + `calibre.utils.localization.
//! get_lc_messages_path`) -- this project doesn't vendor that archive
//! (same class of gap as issue #432's static-asset blocker). Rather
//! than hard-coding a zip path that doesn't exist here, [`Translator`]
//! takes its catalog-loading strategy as an injected closure, so
//! callers that DO have real `.mo` data (e.g. loaded from disk, or
//! embedded via `include_bytes!`) get real, working dynamic
//! translation, while this module itself stays honest about not
//! bundling any locale data of its own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::mo_reader::Catalog;

/// Port of `translate`'s `_CACHE`-backed lookup, parameterized over
/// how a language's catalog is loaded (see this module's own doc for
/// why that's not hard-coded to a bundled `locales.zip`).
pub struct Translator<F: Fn(&str) -> Option<Catalog>> {
    loader: F,
    cache: Mutex<HashMap<String, Option<Arc<Catalog>>>>,
}

impl<F: Fn(&str) -> Option<Catalog>> Translator<F> {
    pub fn new(loader: F) -> Self {
        Translator { loader, cache: Mutex::new(HashMap::new()) }
    }

    /// Port of `translate(lang, text)`. Falls back to `text` itself
    /// (matching upstream's `getattr(__builtins__, '_', lambda x: x)`
    /// fallback) when `lang` has no catalog, or the catalog has no
    /// entry for `text`.
    pub fn translate(&self, lang: &str, text: &str) -> String {
        let mut cache = self.cache.lock().unwrap();
        let catalog = cache.entry(lang.to_string()).or_insert_with(|| (self.loader)(lang).map(Arc::new)).clone();
        match catalog {
            Some(catalog) => catalog.gettext(text).unwrap_or_else(|| text.to_string()),
            None => text.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translations::msgfmt::compile_po;

    #[test]
    fn translates_via_an_injected_catalog_loader() {
        let translator = Translator::new(|lang: &str| {
            if lang == "fr" {
                let po = "msgid \"Hello\"\nmsgstr \"Bonjour\"\n";
                let (mo, _stats) = compile_po(po, false).unwrap();
                Catalog::parse(&mo).ok()
            } else {
                None
            }
        });
        assert_eq!(translator.translate("fr", "Hello"), "Bonjour");
        assert_eq!(translator.translate("fr", "Untranslated"), "Untranslated");
        assert_eq!(translator.translate("de", "Hello"), "Hello");
    }

    #[test]
    fn caches_the_loaded_catalog_per_language() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let load_count = AtomicU32::new(0);
        let translator = Translator::new(|_lang: &str| {
            load_count.fetch_add(1, Ordering::SeqCst);
            let (mo, _stats) = compile_po("msgid \"Hi\"\nmsgstr \"Salut\"\n", false).unwrap();
            Catalog::parse(&mo).ok()
        });
        translator.translate("fr", "Hi");
        translator.translate("fr", "Hi");
        translator.translate("fr", "Hi");
        assert_eq!(load_count.load(Ordering::SeqCst), 1);
    }
}
