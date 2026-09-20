//! The real `InputFormatPlugin` trait and its registration (issue
//! #796, part of the #754 plugin epic).
//!
//! # Why this trait lives here and not in `calibre_customize`
//!
//! `calibre_customize::conversion` already declared an
//! `InputFormatPlugin`, but it had **zero implementors** and its
//! `convert` returned a `PathBuf` to an OPF while taking a
//! `HashMap<String, String>` of options -- a placeholder whose own
//! in-file comments admitted as much ("In Rust we'd likely pass a
//! context struct", "Dealing with `OEBBook` type which is complex").
//!
//! The real input plugins in this crate all share a different, already
//! uniform signature -- `convert(&self, input_path: &Path, output_dir:
//! &Path) -> Result<OEBBook>` -- which `plumber.rs`'s own doc comment
//! had already noticed and relied on. That signature cannot be
//! expressed in `calibre_customize`, because `OEBBook` is defined here
//! and `calibre_ebooks` depends on `calibre_customize`, not the other
//! way round.
//!
//! So the trait lives here, and is registered into the shared registry
//! through [`calibre_customize::registry::PluginKind`] -- which exists
//! precisely so a crate can own a plugin trait the registry cannot
//! name. The placeholder in `calibre_customize::conversion` is left
//! alone rather than deleted: it is still the shape the *WASM* input
//! ABI will likely want (paths and stringly options cross a sandbox
//! boundary; an `OEBBook` does not), and #798 will revisit it.
//!
//! # Not a second conversion engine
//!
//! Issue #476 deleted a parallel `InputPlugin`/`OutputPlugin`/
//! `ConversionPipeline` stack from `calibre_conversion` as "strictly
//! worse than what already existed one crate over". This is explicitly
//! *not* that: no new engine, no new pipeline. The existing, real
//! [`crate::conversion::plumber::convert_to_oebbook`] keeps doing the
//! work; it just resolves its handler from the registry instead of from
//! a hardcoded 20-branch `if/else` chain.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use calibre_customize::registry::{PluginKind, PluginRegistry, RegistryError};
use calibre_customize::{Plugin, PluginInstallationType};

use crate::oeb::book::OEBBook;

/// A real input-format plugin: turns a file of one or more extensions
/// into an in-memory [`OEBBook`].
pub trait InputFormatPlugin: Plugin {
    /// Lowercase extensions this plugin handles, without the dot.
    fn file_types(&self) -> Vec<String>;

    /// Port of the input-plugin `convert` step. `output_dir` is where
    /// the plugin may extract archive/binary content it needs.
    fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook>;
}

impl PluginKind for dyn InputFormatPlugin {
    const KIND: &'static str = "InputFormat";
    fn upcast(arc: Arc<Self>) -> Arc<dyn Plugin> {
        arc
    }
}

/// Generates a builtin input plugin wrapping one of this crate's real
/// `*_input` modules.
///
/// Each of those modules is a plain struct with an inherent
/// `convert(&self, &Path, &Path) -> Result<OEBBook>` of exactly the
/// shape [`InputFormatPlugin::convert`] needs, so every wrapper is the
/// same three lines. Written as a macro rather than 20 copies so the
/// delegation cannot drift between formats.
macro_rules! builtin_input_plugin {
    ($vis_name:ident, $name:literal, $inner:path, [$($ext:literal),+ $(,)?]) => {
        pub struct $vis_name;

        impl Plugin for $vis_name {
            fn name(&self) -> &str {
                $name
            }
            fn description(&self) -> &str {
                concat!("Convert ", $name, " files to an OEB book")
            }
            fn installation_type(&self) -> Option<PluginInstallationType> {
                Some(PluginInstallationType::Builtin)
            }
            fn type_name(&self) -> &str {
                "Input"
            }
        }

        impl InputFormatPlugin for $vis_name {
            fn file_types(&self) -> Vec<String> {
                vec![$($ext.to_string()),+]
            }
            fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
                <$inner>::new().convert(input_path, output_dir)
            }
        }
    };
}

// The extension sets below are transcribed from the `if/else` chain
// `convert_to_oebbook` used before this issue, unchanged -- so the set
// of formats accepted is identical to what shipped previously.
builtin_input_plugin!(EpubInputPlugin, "EPUB Input", crate::input::epub_input::EPUBInput, ["epub"]);
builtin_input_plugin!(MobiInputPlugin, "MOBI Input", crate::input::mobi_input::MOBIInput, ["mobi", "azw", "azw3", "prc"]);
builtin_input_plugin!(HtmlInputPlugin, "HTML Input", crate::input::html_input::HTMLInput, ["html", "htm", "xhtml"]);
builtin_input_plugin!(TxtInputPlugin, "TXT Input", crate::input::txt_input::TXTInput, ["txt", "md", "markdown", "text", "textile"]);
builtin_input_plugin!(DocxInputPlugin, "DOCX Input", crate::input::docx_input::DOCXInput, ["docx"]);
builtin_input_plugin!(ComicInputPlugin, "Comic Input", crate::input::comic_input::ComicInput, ["cbz", "zip"]);
builtin_input_plugin!(Fb2InputPlugin, "FB2 Input", crate::input::fb2_input::FB2Input, ["fb2"]);
builtin_input_plugin!(RbInputPlugin, "RB Input", crate::input::rb_input::RBInput, ["rb"]);
builtin_input_plugin!(LitInputPlugin, "LIT Input", crate::input::lit_input::LitInput, ["lit"]);
builtin_input_plugin!(SnbInputPlugin, "SNB Input", crate::input::snb_input::SnbInput, ["snb"]);
builtin_input_plugin!(RtfInputPlugin, "RTF Input", crate::input::rtf_input::RTFInput, ["rtf"]);
builtin_input_plugin!(PdfInputPlugin, "PDF Input", crate::input::pdf_input::PDFInput, ["pdf"]);
builtin_input_plugin!(LrfInputPlugin, "LRF Input", crate::input::lrf_input::LRFInput, ["lrf"]);
builtin_input_plugin!(TcrInputPlugin, "TCR Input", crate::input::tcr_input::TCRInput, ["tcr"]);
builtin_input_plugin!(PdbInputPlugin, "PDB Input", crate::input::pdb_input::PDBInput, ["pdb"]);
builtin_input_plugin!(OdtInputPlugin, "ODT Input", crate::input::odt_input::ODTInput, ["odt"]);
builtin_input_plugin!(DjvuInputPlugin, "DJVU Input", crate::input::djvu_input::DJVUInput, ["djvu"]);
builtin_input_plugin!(RecipeInputPlugin, "Recipe Input", crate::input::recipe_input::RecipeInput, ["recipe"]);
builtin_input_plugin!(ChmInputPlugin, "CHM Input", crate::input::chm_input::CHMInput, ["chm"]);
builtin_input_plugin!(Azw4InputPlugin, "AZW4 Input", crate::input::azw4_input::AZW4Input, ["azw4"]);

/// Registers every builtin input-format plugin.
pub fn register_builtin_input_plugins(registry: &mut PluginRegistry) -> Result<(), RegistryError> {
    macro_rules! reg {
        ($($p:expr),+ $(,)?) => {
            $( registry.register::<dyn InputFormatPlugin>(Arc::new($p))?; )+
        };
    }
    reg!(
        EpubInputPlugin,
        MobiInputPlugin,
        HtmlInputPlugin,
        TxtInputPlugin,
        DocxInputPlugin,
        ComicInputPlugin,
        Fb2InputPlugin,
        RbInputPlugin,
        LitInputPlugin,
        SnbInputPlugin,
        RtfInputPlugin,
        PdfInputPlugin,
        LrfInputPlugin,
        TcrInputPlugin,
        PdbInputPlugin,
        OdtInputPlugin,
        DjvuInputPlugin,
        RecipeInputPlugin,
        ChmInputPlugin,
        Azw4InputPlugin,
    );
    Ok(())
}

/// The process-wide registry of builtin input plugins, used by
/// [`crate::conversion::plumber::convert_to_oebbook`].
///
/// `convert_to_oebbook` is a free function called from several places
/// (including `oeb::iterator::book::extract_book`) and has never taken
/// a registry parameter, so threading one through every caller would be
/// a wide, unrelated API change. This lazily-built registry keeps that
/// signature intact while still making the dispatch registry-driven.
///
/// Deliberately separate from any registry a *server* owns: this one
/// contains builtins only and is never mutated after construction, so
/// it has none of the shared-mutable-state problems that made
/// [`PluginRegistry`] itself an owned struct rather than a global.
/// Runtime-loaded plugins (#798) will need the caller-supplied form,
/// which is why [`resolve_input_plugin`] takes a registry explicitly.
pub fn builtin_input_registry() -> &'static PluginRegistry {
    static REGISTRY: std::sync::OnceLock<PluginRegistry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = PluginRegistry::new();
        register_builtin_input_plugins(&mut registry).expect("builtin input plugin names are unique");
        registry
    })
}

/// Finds the highest-priority enabled input plugin handling `ext`.
///
/// `ext` is matched case-insensitively against each plugin's
/// [`InputFormatPlugin::file_types`].
pub fn resolve_input_plugin(registry: &PluginRegistry, ext: &str) -> Option<Arc<dyn InputFormatPlugin>> {
    let ext = ext.to_lowercase();
    registry.plugins_of::<dyn InputFormatPlugin>().into_iter().find(|p| p.file_types().iter().any(|t| t == &ext))
}

/// Every extension the registry's enabled input plugins can read,
/// uppercased and sorted.
///
/// Exists so callers don't hand-maintain their own copy of the
/// dispatch table. `calibre_srv::convert` did exactly that (a
/// `READABLE_FORMATS` const "matching its own real dispatch table
/// exactly"), which was correct only for as long as someone kept the
/// two in sync by hand -- now it is derived.
pub fn supported_input_extensions_uppercase(registry: &PluginRegistry) -> Vec<String> {
    let mut exts: Vec<String> = registry.plugins_of::<dyn InputFormatPlugin>().iter().flat_map(|p| p.file_types()).map(|e| e.to_uppercase()).collect();
    exts.sort();
    exts.dedup();
    exts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every extension the pre-#796 `if/else` chain in
    /// `convert_to_oebbook` accepted. Guards against a format silently
    /// dropping out of the dispatch during the move to the registry.
    const EVERY_PREVIOUSLY_SUPPORTED_EXT: &[&str] = &[
        "epub", "mobi", "azw", "azw3", "prc", "html", "htm", "xhtml", "txt", "md", "markdown", "text", "textile", "docx", "cbz", "zip", "fb2", "rb",
        "lit", "snb", "rtf", "pdf", "lrf", "tcr", "pdb", "odt", "djvu", "recipe", "chm", "azw4",
    ];

    #[test]
    fn every_format_the_old_hardcoded_chain_handled_still_resolves() {
        let registry = builtin_input_registry();
        for ext in EVERY_PREVIOUSLY_SUPPORTED_EXT {
            assert!(resolve_input_plugin(registry, ext).is_some(), "input format {ext:?} no longer resolves to a plugin");
        }
    }

    #[test]
    fn the_derived_uppercase_extension_list_matches_the_old_hardcoded_table_exactly() {
        // `calibre_srv::convert::READABLE_FORMATS` used to be this exact
        // hand-maintained list, described in its own doc as "matching
        // its own real dispatch table exactly". It is now derived from
        // the registry; this pins the derivation against what it
        // replaced so the change is provably behavior-preserving.
        let old_hardcoded_readable_formats = [
            "EPUB", "MOBI", "AZW", "AZW3", "PRC", "HTML", "HTM", "XHTML", "TXT", "MD", "MARKDOWN", "TEXT", "TEXTILE", "DOCX", "CBZ", "ZIP", "FB2",
            "RB", "LIT", "SNB", "RTF", "PDF", "LRF", "TCR", "PDB", "ODT", "DJVU", "RECIPE", "CHM", "AZW4",
        ];
        let mut expected: Vec<String> = old_hardcoded_readable_formats.iter().map(|s| s.to_string()).collect();
        expected.sort();

        assert_eq!(supported_input_extensions_uppercase(builtin_input_registry()), expected);
    }

    #[test]
    fn an_unsupported_extension_resolves_to_nothing() {
        assert!(resolve_input_plugin(builtin_input_registry(), "xyz").is_none());
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert!(resolve_input_plugin(builtin_input_registry(), "EPUB").is_some());
        assert!(resolve_input_plugin(builtin_input_registry(), "Epub").is_some());
    }

    #[test]
    fn all_twenty_builtin_input_plugins_register_without_a_name_collision() {
        let mut registry = PluginRegistry::new();
        register_builtin_input_plugins(&mut registry).unwrap();
        assert_eq!(registry.len(), 20);
        assert_eq!(registry.plugins_of::<dyn InputFormatPlugin>().len(), 20);
    }

    #[test]
    fn a_disabled_input_plugin_stops_resolving() {
        let mut registry = PluginRegistry::new();
        register_builtin_input_plugins(&mut registry).unwrap();
        assert!(resolve_input_plugin(&registry, "epub").is_some());

        registry.set_enabled("EPUB Input", false).unwrap();
        assert!(resolve_input_plugin(&registry, "epub").is_none(), "a disabled input plugin must not be selected for conversion");
        // ...and the rest are unaffected.
        assert!(resolve_input_plugin(&registry, "mobi").is_some());
    }

    #[test]
    fn input_plugins_are_listed_with_their_real_metadata() {
        let registry = builtin_input_registry();
        let listed = registry.list();
        let epub = listed.iter().find(|p| p.name == "EPUB Input").expect("EPUB Input should be listed");
        assert_eq!(epub.kind, "InputFormat");
        assert_eq!(epub.installation_type, Some(PluginInstallationType::Builtin));
        assert!(epub.enabled);
    }
}
