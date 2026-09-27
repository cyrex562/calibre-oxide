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
    // The `_no_opts` pair is for engines written before
    // `ConversionOptions` existed, whose `convert` takes only the book
    // and the path. **They therefore ignore every conversion option** --
    // an output profile or a margin set for one of these formats has no
    // effect. Threading options through them is real work on each engine
    // rather than registry plumbing, so it is not done here; the
    // alternative was leaving the formats unreachable altogether (#812).
    (@call mut_book_no_opts, $inner:path, $book:ident, $path:ident, $opts:ident) => {{
        let _ = $opts;
        <$inner>::new().convert($book, $path)?;
        Ok(Vec::new())
    }};
    (@call ref_book_no_opts, $inner:path, $book:ident, $path:ident, $opts:ident) => {{
        let _ = $opts;
        <$inner>::new().convert(&*$book, $path)?;
        Ok(Vec::new())
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
// Registered as part of #812. All three engines were written and tested
// but deliberately left unreachable when the registry was introduced
// (#797), which scoped itself to preserving the old `if/else` dispatch
// exactly. Wiring them is the whole of that deferral.
builtin_output_plugin!(HtmlzOutputPlugin, "HTMLZ Output", crate::output::htmlz_output::HTMLZOutput, ["htmlz"], mut_book_no_opts);
builtin_output_plugin!(PmlOutputPlugin, "PML Output", crate::output::pml_output::PMLOutput, ["pml"], ref_book_no_opts);
// Note: `HTMLOutput` treats its path as a *directory* and writes a tree
// of files into it, unlike every other output plugin, which writes one
// file. That matches upstream's own HTML output, which also produces a
// directory -- but a caller that assumes `output_path` names a file will
// be surprised.
builtin_output_plugin!(HtmlOutputPlugin, "HTML Output", crate::output::html_output::HTMLOutput, ["html"], mut_book_no_opts);

/// KEPUB output (#812) -- Kobo's EPUB dialect.
///
/// Hand-written rather than generated by [`builtin_output_plugin`]
/// because KEPUB is not a serialisation of an `OEBBook` at all: it is an
/// EPUB with Kobo's span markup and scripts layered on. So this writes a
/// real EPUB first and then kepubifies it, which is exactly what
/// upstream's own `KEPUBOutput` does by subclassing `EPUBOutput` and
/// post-processing its result.
///
/// Unlike the three `_no_opts` plugins above, this one does thread
/// `ConversionOptions` through: `extra_css` reaches kepubify's option
/// builder, which is what decides whether widow/orphan and `@page` rules
/// get stripped.
pub struct KepubOutputPlugin;

impl Plugin for KepubOutputPlugin {
    fn name(&self) -> &str {
        "KEPUB Output"
    }
    fn description(&self) -> &str {
        "Write an OEB book out as a Kobo KEPUB"
    }
    fn installation_type(&self) -> Option<PluginInstallationType> {
        Some(PluginInstallationType::Builtin)
    }
    fn type_name(&self) -> &str {
        "Output"
    }
}

/// AZW3 output (#812) -- Kindle's KF8 format, written on its own rather
/// than as the KF8 half of a joint MOBI6+KF8 file.
///
/// Hand-written because `KF8Book` is not one of the `*_output` engines
/// the macro delegates to: the book is built by `create_kf8_book` and
/// then serialised by `KF8Book::to_bytes`, which assembles the PalmDB
/// header and records itself. That mirrors upstream's `AZW3Output`,
/// which calls `create_kf8_book(..., for_joint=False)` and then
/// `kf8.write(output_path)`.
pub struct Azw3OutputPlugin;

impl Plugin for Azw3OutputPlugin {
    fn name(&self) -> &str {
        "AZW3 Output"
    }
    fn description(&self) -> &str {
        "Write an OEB book out as a Kindle KF8 (AZW3) file"
    }
    fn installation_type(&self) -> Option<PluginInstallationType> {
        Some(PluginInstallationType::Builtin)
    }
    fn type_name(&self) -> &str {
        "Output"
    }
}

impl OutputFormatPlugin for Azw3OutputPlugin {
    fn file_types(&self) -> Vec<String> {
        vec!["azw3".to_string()]
    }

    fn convert(&self, book: &mut OEBBook, output_path: &Path, opts: &ConversionOptions) -> Result<Vec<String>> {
        // Mapped from the real `ConversionOptions` rather than taking
        // `Kf8WriterOpts::default()`: the four MOBI writer options mean
        // the same thing for KF8, and silently ignoring a `dont_compress`
        // the user asked for is the kind of thing #690 had to go back and
        // fix for the MOBI path.
        let kf8_opts = crate::mobi::writer8::main::Kf8WriterOpts {
            dont_compress: opts.mobi.dont_compress,
            prefer_author_sort: opts.mobi.prefer_author_sort,
            share_not_sync: opts.mobi.share_not_sync,
            mobi_keep_original_images: opts.mobi.mobi_keep_original_images,
            extra_css: opts.extra_css.clone(),
            ..Default::default()
        };

        let kf8 = crate::mobi::writer8::main::create_kf8_book(book, kf8_opts)?;
        // `to_bytes` needs the metadata for record0's EXTH block, and the
        // book is borrowed mutably above, so the serialise step reads it
        // back afterwards rather than holding both borrows at once.
        let bytes = kf8.to_bytes(&book.metadata)?;
        std::fs::write(output_path, &bytes)?;
        Ok(Vec::new())
    }
}

impl OutputFormatPlugin for KepubOutputPlugin {
    fn file_types(&self) -> Vec<String> {
        vec!["kepub".to_string()]
    }

    fn convert(&self, book: &mut OEBBook, output_path: &Path, opts: &ConversionOptions) -> Result<Vec<String>> {
        // The intermediate EPUB is named after the destination rather
        // than something generic: `kepubify_path` derives its own output
        // name from the input's stem when not given one, so a temp-file
        // stem could leak into the produced book.
        let staging = tempfile::tempdir()?;
        let stem = output_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "book".to_string());
        let epub_path = staging.path().join(format!("{stem}.epub"));

        crate::output::epub_output::EPUBOutput::new().convert(book, &epub_path, opts)?;

        let kepub_opts = crate::oeb::polish::kepubify::make_options(crate::oeb::polish::kepubify::MakeOptionsArgs {
            extra_css: opts.extra_css.clone().unwrap_or_default(),
            ..Default::default()
        });
        let fontdb = std::sync::Arc::new(crate::covers_text::load_system_fonts());

        // `allow_overwrite` is true because `output_path` is where the
        // caller asked for the book to go. The uniquifying behaviour
        // exists for the command-line case, where not clobbering the
        // input is the point; here the destination is explicit.
        let produced = crate::oeb::polish::kepubify::kepubify_path(&epub_path, Some(output_path), 1, true, &kepub_opts, &fontdb)?;
        if produced != output_path {
            std::fs::rename(&produced, output_path)?;
        }
        Ok(Vec::new())
    }
}

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
        HtmlzOutputPlugin,
        PmlOutputPlugin,
        HtmlOutputPlugin,
        KepubOutputPlugin,
        Azw3OutputPlugin,
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
    fn every_builtin_output_plugin_registers_without_a_name_collision() {
        // 15 transcribed from the old dispatch chain, plus HTMLZ, PML and
        // HTML, which #812 wired after #797 deliberately left them out,
        // plus KEPUB and AZW3, which #812 added as real new plugins.
        const EXPECTED: usize = 20;
        let mut registry = PluginRegistry::new();
        register_builtin_output_plugins(&mut registry).unwrap();
        assert_eq!(registry.len(), EXPECTED);
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
    fn the_three_formerly_unreachable_output_modules_are_now_reachable() {
        // Was `the_three_unreachable_output_modules_are_still_unreachable`,
        // which pinned the gap #797 deliberately did not close: those
        // engines existed and were tested, but the registry was scoped to
        // reproducing the old `if/else` dispatch exactly, so wiring them
        // would have been a silent scope increase.
        //
        // #812 is that wiring, done deliberately -- so the guard flips
        // rather than being deleted. It still earns its place: it is what
        // fails if one of the three is dropped from the registry again.
        let registry = builtin_output_registry();
        for ext in ["html", "htmlz", "pml"] {
            assert!(resolve_output_plugin(registry, ext).is_some(), "{ext} output was wired by #812 and should stay reachable");
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
