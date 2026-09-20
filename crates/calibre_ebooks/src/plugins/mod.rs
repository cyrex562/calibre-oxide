//! This crate's own builtin plugins, and their registration into a
//! [`calibre_customize::registry::PluginRegistry`] (issue #795, part of
//! the #754 epic).
//!
//! # Why builtins live here rather than in `calibre_customize`
//!
//! Real upstream registers all ~374 of its builtin plugins from one
//! module (`calibre/customize/builtins.py`), because Python lets it
//! import every subsystem from there freely.
//!
//! This port can't: `calibre_ebooks` depends on `calibre_customize`, so
//! `calibre_customize` cannot depend back on this crate to construct
//! format-specific plugins. Builtins therefore live in the crate that
//! owns the behavior they wrap, and each such crate exposes its own
//! `register_builtins`. `calibre_customize::builtins` documents the
//! same split from the other side.

pub mod pml2pmlz;

use calibre_customize::registry::{PluginRegistry, RegistryError};
use std::sync::Arc;

/// Registers every builtin plugin this crate provides.
///
/// Upstream's builtin `FileTypePlugin` set is exactly two plugins
/// (`PML2PMLZ` and `TXT2TXTZ` in `customize/builtins.py`); this port
/// currently implements the first. `TXT2TXTZ` is a real, separable
/// follow-up -- it needs `get_images_from_polyglot_text`, which parses
/// Markdown/Textile image references out of the text, and has no port
/// yet.
pub fn register_builtins(registry: &mut PluginRegistry) -> Result<(), RegistryError> {
    registry.register::<dyn calibre_customize::FileTypePlugin>(Arc::new(pml2pmlz::Pml2Pmlz))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_builtins_registers_a_real_discoverable_plugin() {
        let mut registry = PluginRegistry::new();
        register_builtins(&mut registry).unwrap();

        let plugins = registry.file_type_plugins();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name(), "PML to PMLZ");
        assert!(plugins[0].file_types().contains(&"pml".to_string()));
        assert!(plugins[0].on_import());
    }

    #[test]
    fn registering_builtins_twice_is_refused_rather_than_silently_duplicating() {
        let mut registry = PluginRegistry::new();
        register_builtins(&mut registry).unwrap();
        assert!(register_builtins(&mut registry).is_err());
        assert_eq!(registry.len(), 1);
    }
}
