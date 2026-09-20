//! First-party plugins registered into a [`PluginRegistry`] by default
//! (issue #795).
//!
//! # What this used to be
//!
//! This module used to be an 18-line stub whose `list_plugins()`
//! returned three hardcoded `String`s -- `"MOBI Output"`, `"EPUB
//! Output"`, `"PDF Output"` -- naming plugins that did not exist, were
//! never registered, and could never be run. That was actively
//! misleading: a caller asking "what plugins are available?" got three
//! names back and no way to reach any of them.
//!
//! # Why this crate registers nothing itself
//!
//! Real upstream's `calibre/customize/builtins.py` registers ~374
//! builtin plugins, because in Python it can freely import every
//! subsystem it needs (conversion, devices, metadata, the GUI) from one
//! module.
//!
//! This port cannot do the same thing from here: `calibre_ebooks`
//! depends on `calibre_customize`, so `calibre_customize` cannot depend
//! back on `calibre_ebooks` to build format-specific plugins without a
//! dependency cycle. Builtin plugins therefore live in, and are
//! registered from, whichever crate actually owns the behavior --
//! `calibre_ebooks::plugins::register_builtins` is the first of those.
//!
//! So this module deliberately registers an *empty* set. That is a true
//! statement about what `calibre_customize` alone provides, and it is
//! the honest replacement for three invented names. See
//! [`register_builtins`].

use crate::registry::PluginRegistry;

/// Registers the builtin plugins owned by this crate -- currently none,
/// by design (see the module doc).
///
/// Kept as a real function rather than deleted so the composition point
/// exists and is discoverable: a caller assembling a registry calls
/// this plus each owning crate's own `register_builtins`, e.g.
///
/// ```ignore
/// let mut registry = PluginRegistry::new();
/// calibre_customize::builtins::register_builtins(&mut registry);
/// calibre_ebooks::plugins::register_builtins(&mut registry)?;
/// ```
pub fn register_builtins(_registry: &mut PluginRegistry) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registering_this_crates_builtins_adds_nothing_and_says_so_honestly() {
        // The regression this guards: the old stub reported three
        // plugin names that did not exist. An empty registry is the
        // truthful answer for this crate on its own.
        let mut registry = PluginRegistry::new();
        register_builtins(&mut registry);
        assert!(registry.is_empty());
    }
}
