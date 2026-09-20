//! The real `OutputFormatPlugin` trait and its registration (issue
//! #797, sibling of #796's input side).
//!
//! # Why this is not the mechanical mirror of the input side
//!
//! #796's 20 input plugins all already shared one signature, so the
//! trait wrote itself. The output plugins do **not**. Before this
//! issue they diverged on four separate axes:
//!
//! 1. `&mut OEBBook` (epub, html, htmlz, lit, oeb, txt) vs `&OEBBook`
//!    (docx, fb2, lrf, mobi, odt, pdb, pdf, pml, rb, rtf, snb, tcr).
//! 2. With a trailing `&ConversionOptions` (most) vs without it
//!    (html, htmlz, pml).
//! 3. `Result<()>` (all but one) vs `Result<Vec<String>>` (lit).
//! 4. Writing a file vs writing a directory (html writes a directory).
//!
//! # The chosen signature, and why
//!
//! `convert(&mut OEBBook, &Path, &ConversionOptions) -> Result<Vec<String>>`
//! -- the superset, so no existing plugin has to lose anything.
//!
//! - **`&mut` uniformly.** A `&mut` reborrows as `&` for free, so the
//!   twelve read-only plugins adapt with no cost, while the six that
//!   genuinely mutate keep working. The reverse is impossible. The
//!   real trade-off, stated plainly: the trait no longer tells a reader
//!   *which* outputs mutate the book, so a caller must assume any
//!   output may. That information was already absent from the dispatch
//!   site (which held a `mut book` and passed `&mut` or `&` per branch),
//!   so nothing is lost relative to what existed -- but it is a real
//!   weakening versus a hypothetical split trait, and a split trait was
//!   rejected only because it would double every call site for one bit
//!   of information no current caller uses.
//! - **`Vec<String>` is warnings.** Not cosmetic: `LitOutput::convert`
//!   returns real warnings (its own plus `litize_oeb`'s), and the old
//!   dispatch printed each to stderr. Returning `Result<()>` would have
//!   silently dropped them. Every other plugin returns an empty vec.
//!
//! # Not a second conversion engine
//!
//! Same constraint as #796: issue #476 deleted a parallel engine from
//! `calibre_conversion`. This does not reintroduce one. The existing
//! `Plumber::write_output` still does the work; only its handler lookup
//! changed.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use calibre_customize::registry::{PluginKind, PluginRegistry, RegistryError};
use calibre_customize::{Plugin, PluginInstallationType};

use crate::conversion::options::ConversionOptions;
use crate::oeb::book::OEBBook;

/// A real output-format plugin: writes an [`OEBBook`] out in one
/// concrete format.
pub trait OutputFormatPlugin: Plugin {
    /// Lowercase extensions this plugin produces, without the dot.
    fn file_types(&self) -> Vec<String>;

    /// Writes `book` to `output_path`, returning any human-readable
    /// warnings produced along the way (see the module doc -- only LIT
    /// currently produces any; everything else returns an empty vec).
    fn convert(&self, book: &mut OEBBook, output_path: &Path, opts: &ConversionOptions) -> Result<Vec<String>>;
}

impl PluginKind for dyn OutputFormatPlugin {
    const KIND: &'static str = "OutputFormat";
    fn upcast(arc: Arc<Self>) -> Arc<dyn Plugin> {
        arc
    }
}

/// Generates a builtin output plugin wrapping one of this crate's real
/// `*_output` modules.
///
/// The `mode` token selects how to adapt that module's own signature to
/// the unified trait (see the module doc for the four axes of
/// divergence):
///
/// - `mut_book` -- takes `&mut OEBBook`, returns `Result<()>`
/// - `ref_book` -- takes `&OEBBook`, returns `Result<()>`
/// - `mut_book_warnings` -- takes `&mut OEBBook`, returns `Result<Vec<String>>`
macro_rules! builtin_output_plugin {
    ($vis_name:ident, $name:literal, $inner:path, [$($ext:literal),+ $(,)?], $mode:ident) => {
        pub struct $vis_name;

        impl Plugin for $vis_name {
            fn name(&self) -> &str {
                $name
            }
            fn description(&self) -> &str {
                concat!("Write an OEB book out as ", $name)
            }
            fn installation_type(&self) -> Option<PluginInstallationType> {
                Some(PluginInstallationType::Builtin)
            }
            fn type_name(&self) -> &str {
                "Output"
            }
        }

        impl OutputFormatPlugin for $vis_name {
            fn file_types(&self) -> Vec<String> {
                vec![$($ext.to_string()),+]
            }
            fn convert(&self, book: &mut OEBBook, output_path: &Path, opts: &ConversionOptions) -> Result<Vec<String>> {
                builtin_output_plugin!(@call $mode, $inner, book, output_path, opts)
            }
        }
    };

    (@call mut_book, $inner:path, $book:ident, $path:ident, $opts:ident) => {{
        <$inner>::new().convert($book, $path, $opts)?;
        Ok(Vec::new())
    }};
    (@call ref_book, $inner:path, $book:ident, $path:ident, $opts:ident) => {{
        // A `&mut` reborrows as `&` -- this is where the twelve
        // read-only plugins adapt to the unified `&mut` signature.
        <$inner>::new().convert(&*$book, $path, $opts)?;
        Ok(Vec::new())
    }};
    (@call mut_book_warnings, $inner:path, $book:ident, $path:ident, $opts:ident) => {{
        <$inner>::new().convert($book, $path, $opts)
    }};
}

// Extension sets transcribed verbatim from the `if/else` chain in
// `Plumber::write_output` that this replaces, so the set of formats
// produced is identical to what shipped previously.
builtin_output_plugin!(EpubOutputPlugin, "EPUB Output", crate::output::epub_output::EPUBOutput, ["epub"], mut_book);
builtin_output_plugin!(DocxOutputPlugin, "DOCX Output", crate::output::docx_output::DOCXOutput, ["docx"], ref_book);
builtin_output_plugin!(MobiOutputPlugin, "MOBI Output", crate::output::mobi_output::MOBIOutput, ["mobi", "azw", "prc"], ref_book);
builtin_output_plugin!(RbOutputPlugin, "RB Output", crate::output::rb_output::RBOutput, ["rb"], ref_book);
builtin_output_plugin!(LitOutputPlugin, "LIT Output", crate::output::lit_output::LitOutput, ["lit"], mut_book_warnings);
builtin_output_plugin!(TxtOutputPlugin, "TXT Output", crate::output::txt_output::TXTOutput, ["txt", "md", "markdown", "text"], mut_book);
builtin_output_plugin!(SnbOutputPlugin, "SNB Output", crate::output::snb_output::SnbOutput, ["snb"], ref_book);
builtin_output_plugin!(RtfOutputPlugin, "RTF Output", crate::output::rtf_output::RTFOutput, ["rtf"], ref_book);
builtin_output_plugin!(Fb2OutputPlugin, "FB2 Output", crate::output::fb2_output::FB2Output, ["fb2"], ref_book);
builtin_output_plugin!(PdfOutputPlugin, "PDF Output", crate::output::pdf_output::PDFOutput, ["pdf"], ref_book);
builtin_output_plugin!(LrfOutputPlugin, "LRF Output", crate::output::lrf_output::LRFOutput, ["lrf"], ref_book);
builtin_output_plugin!(OebOutputPlugin, "OEB Output", crate::output::oeb_output::OEBOutput, ["oeb"], mut_book);
builtin_output_plugin!(PdbOutputPlugin, "PDB Output", crate::output::pdb_output::PDBOutput, ["pdb"], ref_book);
builtin_output_plugin!(OdtOutputPlugin, "ODT Output", crate::output::odt_output::ODTOutput, ["odt"], ref_book);
builtin_output_plugin!(TcrOutputPlugin, "TCR Output", crate::output::tcr_output::TCROutput, ["tcr"], ref_book);

/// Registers every builtin output-format plugin.
///
/// **Three real output modules are deliberately absent**, matching the
/// dispatch this replaces: `html_output`, `htmlz_output` and
/// `pml_output` were never reachable from `Plumber::write_output`
/// either. `html_output` in particular writes a *directory* rather than
/// a file, which the unified single-`output_path` signature does not
/// distinguish. Wiring them up is real, separable work -- listing them
/// here without the dispatch supporting them would be worse than
/// leaving the existing gap visible.
pub fn register_builtin_output_plugins(registry: &mut PluginRegistry) -> Result<(), RegistryError> {
    macro_rules! reg {
        ($($p:expr),+ $(,)?) => {
            $( registry.register::<dyn OutputFormatPlugin>(Arc::new($p))?; )+
        };
    }
    reg!(
        EpubOutputPlugin,
        DocxOutputPlugin,
        MobiOutputPlugin,
        RbOutputPlugin,
        LitOutputPlugin,
        TxtOutputPlugin,
        SnbOutputPlugin,
        RtfOutputPlugin,
        Fb2OutputPlugin,
        PdfOutputPlugin,
        LrfOutputPlugin,
        OebOutputPlugin,
        PdbOutputPlugin,
        OdtOutputPlugin,
        TcrOutputPlugin,
    );
    Ok(())
}

/// Process-wide registry of builtin output plugins. See
/// [`crate::conversion::input_plugin::builtin_input_registry`] for why
/// this is a lazily-built immutable global rather than threaded through
/// every caller.
pub fn builtin_output_registry() -> &'static PluginRegistry {
    static REGISTRY: std::sync::OnceLock<PluginRegistry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = PluginRegistry::new();
        register_builtin_output_plugins(&mut registry).expect("builtin output plugin names are unique");
        registry
    })
}

/// Finds the highest-priority enabled output plugin producing `ext`.
pub fn resolve_output_plugin(registry: &PluginRegistry, ext: &str) -> Option<Arc<dyn OutputFormatPlugin>> {
    let ext = ext.to_lowercase();
    registry.plugins_of::<dyn OutputFormatPlugin>().into_iter().find(|p| p.file_types().iter().any(|t| t == &ext))
}

/// Every extension the registry's enabled output plugins can produce,
/// uppercased and sorted. Counterpart of
/// [`crate::conversion::input_plugin::supported_input_extensions_uppercase`].
pub fn supported_output_extensions_uppercase(registry: &PluginRegistry) -> Vec<String> {
    let mut exts: Vec<String> = registry.plugins_of::<dyn OutputFormatPlugin>().iter().flat_map(|p| p.file_types()).map(|e| e.to_uppercase()).collect();
    exts.sort();
    exts.dedup();
    exts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every extension the pre-#797 `if/else` chain in
    /// `Plumber::write_output` dispatched on.
    const EVERY_PREVIOUSLY_DISPATCHED_EXT: &[&str] =
        &["epub", "docx", "mobi", "azw", "prc", "rb", "lit", "txt", "md", "markdown", "text", "snb", "rtf", "fb2", "pdf", "lrf", "oeb", "pdb", "odt", "tcr"];

    #[test]
    fn every_format_the_old_hardcoded_chain_dispatched_still_resolves() {
        let registry = builtin_output_registry();
        for ext in EVERY_PREVIOUSLY_DISPATCHED_EXT {
            assert!(resolve_output_plugin(registry, ext).is_some(), "output format {ext:?} no longer resolves to a plugin");
        }
    }

    #[test]
    fn all_fifteen_builtin_output_plugins_register_without_a_name_collision() {
        let mut registry = PluginRegistry::new();
        register_builtin_output_plugins(&mut registry).unwrap();
        assert_eq!(registry.len(), 15);
    }

    #[test]
    fn an_unknown_extension_resolves_to_nothing_so_the_caller_can_fall_back() {
        // `write_output`'s own `else` branch is NOT an error -- it falls
        // back to OEB directory output. Returning `None` here is what
        // lets the caller keep doing that.
        assert!(resolve_output_plugin(builtin_output_registry(), "xyz").is_none());
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert!(resolve_output_plugin(builtin_output_registry(), "EPUB").is_some());
    }

    #[test]
    fn a_disabled_output_plugin_stops_resolving() {
        let mut registry = PluginRegistry::new();
        register_builtin_output_plugins(&mut registry).unwrap();
        assert!(resolve_output_plugin(&registry, "epub").is_some());

        registry.set_enabled("EPUB Output", false).unwrap();
        assert!(resolve_output_plugin(&registry, "epub").is_none());
        assert!(resolve_output_plugin(&registry, "mobi").is_some(), "disabling one output must not affect the others");
    }

    #[test]
    fn the_three_unreachable_output_modules_are_still_unreachable() {
        // Documents the pre-existing gap this issue deliberately did not
        // change: html/htmlz/pml outputs were never dispatched before.
        let registry = builtin_output_registry();
        for ext in ["html", "htmlz", "pml"] {
            assert!(resolve_output_plugin(registry, ext).is_none(), "{ext} output was not dispatched before #797 and should not have been silently added");
        }
    }

    #[test]
    fn the_derived_output_extension_list_covers_the_old_srv_const_and_the_three_it_was_missing() {
        // `calibre_srv::convert::WRITABLE_FORMATS` was hand-maintained
        // and had drifted: it omitted MD/MARKDOWN/TEXT even though
        // `write_output` really did dispatch them. Deriving from the
        // registry fixes that.
        let derived = supported_output_extensions_uppercase(builtin_output_registry());
        let old_hardcoded = ["EPUB", "DOCX", "MOBI", "AZW", "PRC", "RB", "LIT", "TXT", "SNB", "RTF", "PDF", "LRF", "OEB", "PDB", "ODT", "TCR"];
        for f in old_hardcoded {
            assert!(derived.contains(&f.to_string()), "{f} dropped out of the derived output list");
        }
        for missing in ["MD", "MARKDOWN", "TEXT"] {
            assert!(derived.contains(&missing.to_string()), "{missing} really is dispatched by write_output and should now be offered");
        }
    }
}
