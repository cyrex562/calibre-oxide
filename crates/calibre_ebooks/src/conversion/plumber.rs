use crate::conversion::options::ConversionOptions;
use crate::oeb::book::OEBBook;
use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

/// Dispatch `input_path` to the input plugin matching its file extension
/// and return the resulting in-memory [`OEBBook`], extracting any
/// archive/binary content the plugin needs into `extract_dir`.
///
/// Port of the input-plugin-selection step inlined in Python's
/// `Plumber.run` (`calibre/ebooks/conversion/plumber.py`), pulled out
/// into its own function so callers other than [`Plumber::run`] --
/// notably `oeb::iterator::book::extract_book` (issue #38, the
/// `EbookIterator` port) -- can reuse the same format-dispatch table
/// instead of duplicating it. Every input plugin in this crate shares
/// the signature `convert(&self, input_path: &Path, output_dir: &Path)
/// -> Result<OEBBook>`, so this function is just that shared dispatch,
/// unchanged from what used to be the first half of `Plumber::run`.
pub fn convert_to_oebbook(input_path: &Path, extract_dir: &Path) -> Result<OEBBook> {
    let input_ext = input_path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    fs::create_dir_all(extract_dir)?;

    let book = if input_ext == "epub" {
        use crate::input::epub_input::EPUBInput;
        let input_plugin = EPUBInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if ["mobi", "azw", "azw3", "prc"].contains(&input_ext.as_str()) {
        use crate::input::mobi_input::MOBIInput;
        let input_plugin = MOBIInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if ["html", "htm", "xhtml"].contains(&input_ext.as_str()) {
        use crate::input::html_input::HTMLInput;
        let input_plugin = HTMLInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if ["txt", "md", "markdown", "text", "textile"].contains(&input_ext.as_str()) {
        use crate::input::txt_input::TXTInput;
        let input_plugin = TXTInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if input_ext == "docx" {
        use crate::input::docx_input::DOCXInput;
        let input_plugin = DOCXInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if ["cbz", "zip"].contains(&input_ext.as_str()) {
        use crate::input::comic_input::ComicInput;
        let input_plugin = ComicInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if input_ext == "fb2" {
        use crate::input::fb2_input::FB2Input;
        let input_plugin = FB2Input::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if input_ext == "rb" {
        use crate::input::rb_input::RBInput;
        let input_plugin = RBInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if input_ext == "lit" {
        use crate::input::lit_input::LitInput;
        let input_plugin = LitInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if input_ext == "snb" {
        use crate::input::snb_input::SnbInput;
        let input_plugin = SnbInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if input_ext == "rtf" {
        use crate::input::rtf_input::RTFInput;
        let input_plugin = RTFInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if input_ext == "pdf" {
        use crate::input::pdf_input::PDFInput;
        let input_plugin = PDFInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if input_ext == "lrf" {
        use crate::input::lrf_input::LRFInput;
        let input_plugin = LRFInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if input_ext == "tcr" {
        use crate::input::tcr_input::TCRInput;
        let input_plugin = TCRInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if input_ext == "pdb" {
        use crate::input::pdb_input::PDBInput;
        let input_plugin = PDBInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if input_ext == "odt" {
        use crate::input::odt_input::ODTInput;
        let input_plugin = ODTInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if input_ext == "djvu" {
        use crate::input::djvu_input::DJVUInput;
        let input_plugin = DJVUInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if input_ext == "recipe" {
        use crate::input::recipe_input::RecipeInput;
        let input_plugin = RecipeInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if input_ext == "chm" {
        use crate::input::chm_input::CHMInput;
        let input_plugin = CHMInput::new();
        input_plugin.convert(input_path, extract_dir)?
    } else if input_ext == "azw4" {
        use crate::input::azw4_input::AZW4Input;
        let input_plugin = AZW4Input::new();
        input_plugin.convert(input_path, extract_dir)?
    } else {
        bail!("Unsupported input format: {}", input_ext);
    };

    Ok(book)
}

/// Runs only the input stage of a conversion and writes the resulting
/// book out as a real OPF-plus-content-files directory at `dump_dir`.
///
/// Port of `Plumber.run(..., abort_after_input_dump=True)`: real
/// upstream runs the input plugin, then stops before any output stage,
/// leaving `tdir/input/` holding the parsed book as a plain directory
/// (an OPF, its manifest files, and -- when the input format produces
/// one -- an NCX). `ebooks/html/to_zip.py`'s own `run()` is this
/// function's real, single caller-of-note (issue #147): it re-packages
/// that directory into a zip. This port collapses upstream's two-level
/// `tdir` / `tdir/input` scratch layout into one caller-supplied
/// `dump_dir`, since the outer/inner split has no effect the caller can
/// observe -- only `dump_dir`'s own contents (what `package_dump`
/// reads) are.
///
/// **Not yet threaded through**: real upstream's own `run()` passes
/// `input_encoding`/`breadth_first`/`allow_local_files_outside_root`
/// `OptionRecommendation`s down into this stage; this port calls
/// [`convert_to_oebbook`] with each input plugin's own defaults, since
/// generic `OptionRecommendation` plumbing is issue #126's own separate
/// scope (this issue's own body draws the same line). A future #126
/// lands with a way to pass per-plugin options through this call.
pub fn dump_input(input_path: &Path, dump_dir: &Path) -> Result<()> {
    let extract_dir = tempdir()?;
    let mut book = convert_to_oebbook(input_path, extract_dir.path())?;
    fs::create_dir_all(dump_dir)?;
    crate::oeb::writer::OEBWriter::new().write_book(&mut book, dump_dir)?;
    Ok(())
}

pub struct Plumber {
    input_path: PathBuf,
    output_path: PathBuf,
    opts: ConversionOptions,
}

impl Plumber {
    pub fn new<P: AsRef<Path>>(input: P, output: P) -> Self {
        Self::with_options(input, output, ConversionOptions::default())
    }

    /// Real upstream's `Plumber` always carries a live `self.opts`,
    /// populated from every plugin's `OptionRecommendation` defaults
    /// (`setup_options`) and then overridden by whatever the CLI/GUI
    /// parsed. This port has no CLI option parser yet (issue #126's own
    /// scope) -- `Plumber::new` uses [`ConversionOptions::default`]
    /// (the real upstream default values), and this constructor is for
    /// callers that already have a customized [`ConversionOptions`].
    pub fn with_options<P: AsRef<Path>>(input: P, output: P, opts: ConversionOptions) -> Self {
        Self {
            input_path: input.as_ref().to_path_buf(),
            output_path: output.as_ref().to_path_buf(),
            opts,
        }
    }

    pub fn run(&self) -> Result<()> {
        // 1. Setup Request
        println!(
            "Conversion: {:?} -> {:?}",
            self.input_path, self.output_path
        );

        // 2. Input Plugin
        // We use a temp dir for intermediate extraction if needed,
        // but EPUBInput also takes a destination.
        // In the original python plumber, there is a complex temp dir management.
        // Here, EPUBInput needs a place to extract to.
        let temp_dir = tempdir()?;
        let extract_path = temp_dir.path().join("source");
        let mut book = convert_to_oebbook(&self.input_path, &extract_path)?;

        // 3. Transforms
        let output_ext = self.output_path.extension().and_then(|s| s.to_str()).map(|s| s.to_lowercase()).unwrap_or_default();
        self.run_transforms(&mut book, &output_ext)?;

        // 4. Output Plugin
        self.write_output(book)?;

        println!("Done.");
        Ok(())
    }

    /// Port of real `Plumber.run`'s transform stage (`plumber.py`'s own
    /// real sequence, `old_src/src/calibre/ebooks/conversion/plumber.py:1115-1247`):
    /// `DataURL` -> `Clean` (guide) -> `RemoveFirstImage`/`MergeMetadata`
    /// (jacket.py/metadata.py) -> `DetectStructure` -> `Jacket` ->
    /// `AddAltText` -> `LinearizeTables` -> `UnsmartenPunctuation` ->
    /// `CSSFlattener` -> `RemoveFakeMargins`/`RemoveAdobeMargins` ->
    /// `EmbedFonts` -> `SubsetFonts` -> `ManifestTrimmer` ->
    /// `toc.rationalize_play_orders()`, using `self.opts` (real upstream
    /// default values absent any CLI override -- see
    /// [`ConversionOptions`]'s own module doc).
    ///
    /// **Real, disclosed narrowings, not silently dropped**:
    /// `user_metadata` (real upstream's `read_user_metadata`) is always
    /// [`crate::metadata::meta::MetaInformation::default`] here -- real
    /// upstream's own default (absent `--read-metadata-from-opf` or any
    /// `--title`/`--authors`/etc. CLI override) is exactly the same
    /// blank `MetaInformation(None, [])`, so this is faithful for the
    /// no-CLI-yet case, not a shortcut. The non-EPUB/KePub "remove the
    /// TOC's own reference to the HTML cover" step
    /// (`item_that_refers_to_cover`) has no Rust equivalent on
    /// `crate::oeb::toc::TOC` yet -- a real, narrow, cosmetic gap (a
    /// stray NCX entry pointing at the cover page for non-EPUB
    /// outputs), not attempted here. No named output/input profile
    /// catalog exists (see [`ConversionOptions`]'s own module doc) --
    /// every profile-derived value uses the real *default* profile's
    /// own constants. `mi.cover`-is-a-URL/on-disk-path handling
    /// (`download_cover`, explicit `--cover` CLI flag) doesn't apply
    /// since `user_metadata` is always blank here.
    fn run_transforms(&self, book: &mut OEBBook, output_ext: &str) -> Result<()> {
        use crate::metadata::meta::MetaInformation;
        use crate::oeb::transforms::alt_text::AddAltText;
        use crate::oeb::transforms::data_url::DataURL;
        use crate::oeb::transforms::embed_fonts::EmbedFonts;
        use crate::oeb::transforms::flatcss::CSSFlattener;
        use crate::oeb::transforms::guide::Clean;
        use crate::oeb::transforms::jacket::{JacketTransform, RemoveFirstImage};
        use crate::oeb::transforms::linearize_tables::LinearizeTables;
        use crate::oeb::transforms::metadata::MergeMetadata;
        use crate::oeb::transforms::page_margin::{RemoveAdobeMargins, RemoveFakeMargins};
        use crate::oeb::transforms::structure::DetectStructure;
        use crate::oeb::transforms::subset::SubsetFonts;
        use crate::oeb::transforms::trimmanifest::ManifestTrimmer;
        use crate::oeb::transforms::unsmarten::UnsmartenPunctuation;

        let opts = &self.opts;
        let mut report = |msg: &str| println!("{msg}");
        let user_metadata = MetaInformation::default();
        let jacket_opts = opts.jacket_options();

        DataURL.call(book);
        Clean.call(book);
        RemoveFirstImage.call(book, &jacket_opts, &mut report);
        MergeMetadata.call(book, &user_metadata, opts.prefer_metadata_cover, output_ext, false);
        DetectStructure.call(book, &opts.structure);
        JacketTransform.call(book, &jacket_opts, &user_metadata, &mut report)?;

        if opts.add_alt_text_to_img {
            AddAltText.call(book);
        }
        if opts.linearize_tables && !matches!(output_ext, "mobi" | "lrf") {
            LinearizeTables.call(book);
        }
        if opts.unsmarten_punctuation {
            UnsmartenPunctuation.call(book);
        }

        let flattener = CSSFlattener::new(opts.flattener_options(output_ext));
        let ctx = opts.flatten_context(output_ext);
        flattener.call(book, &ctx, &mut report)?;

        RemoveFakeMargins.call(book, opts.remove_fake_margins);
        RemoveAdobeMargins.call(book);

        if opts.embed_all_fonts {
            EmbedFonts::new().call(book, &mut report)?;
        }
        if opts.subset_embedded_fonts && output_ext != "pdf" {
            SubsetFonts::new().call(book, &mut report)?;
        }

        ManifestTrimmer.call(book);
        book.toc.rationalize_play_orders();
        Ok(())
    }

    fn write_output(&self, mut book: crate::oeb::book::OEBBook) -> Result<()> {
        println!("Writing output...");

        let output_ext = self
            .output_path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        if output_ext == "epub" {
            use crate::output::epub_output::EPUBOutput;
            // Create parent directory if needed
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = EPUBOutput::new();
            output_plugin.convert(&mut book, &self.output_path, &self.opts)?;
        } else if output_ext == "docx" {
            use crate::output::docx_output::DOCXOutput;
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = DOCXOutput::new();
            output_plugin.convert(&book, &self.output_path, &self.opts)?;
        } else if ["mobi", "azw", "prc"].contains(&output_ext.as_str()) {
            use crate::output::mobi_output::MOBIOutput;
            // Ensure dir exists
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = MOBIOutput::new();
            output_plugin.convert(&book, &self.output_path, &self.opts)?;
        } else if output_ext == "rb" {
            use crate::output::rb_output::RBOutput;
            // Ensure dir exists
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = RBOutput::new();
            output_plugin.convert(&book, &self.output_path, &self.opts)?;
        } else if output_ext == "lit" {
            use crate::output::lit_output::LitOutput;
            // Ensure dir exists
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = LitOutput::new();
            for warning in output_plugin.convert(&mut book, &self.output_path, &self.opts)? {
                eprintln!("Warning: {warning}");
            }
        } else if ["txt", "md", "markdown", "text"].contains(&output_ext.as_str()) {
            use crate::output::txt_output::TXTOutput;
            // Ensure dir exists
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = TXTOutput::new();
            output_plugin.convert(&mut book, &self.output_path, &self.opts)?;
        } else if output_ext == "snb" {
            use crate::output::snb_output::SnbOutput;
            // Ensure dir exists
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = SnbOutput::new();
            output_plugin.convert(&book, &self.output_path, &self.opts)?;
        } else if output_ext == "rtf" {
            use crate::output::rtf_output::RTFOutput;
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = RTFOutput::new();
            output_plugin.convert(&book, &self.output_path, &self.opts)?;
        } else if output_ext == "fb2" {
            use crate::output::fb2_output::FB2Output;
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = FB2Output::new();
            output_plugin.convert(&book, &self.output_path, &self.opts)?;
        } else if output_ext == "pdf" {
            use crate::output::pdf_output::PDFOutput;
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = PDFOutput::new();
            output_plugin.convert(&book, &self.output_path, &self.opts)?;
        } else if output_ext == "lrf" {
            use crate::output::lrf_output::LRFOutput;
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = LRFOutput::new();
            output_plugin.convert(&book, &self.output_path, &self.opts)?;
        } else if output_ext == "oeb" {
            use crate::output::oeb_output::OEBOutput;
            let output_plugin = OEBOutput::new();
            output_plugin.convert(&mut book, &self.output_path, &self.opts)?;
        } else if output_ext == "pdb" {
            use crate::output::pdb_output::PDBOutput;
            // Ensure parent exists
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = PDBOutput::new();
            output_plugin.convert(&book, &self.output_path, &self.opts)?;
        } else if output_ext == "odt" {
            use crate::output::odt_output::ODTOutput;
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = ODTOutput::new();
            output_plugin.convert(&book, &self.output_path, &self.opts)?;
        } else if output_ext == "tcr" {
            use crate::output::tcr_output::TCROutput;
            // Ensure parent exists
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = TCROutput::new();
            output_plugin.convert(&book, &self.output_path, &self.opts)?;
        } else {
            // Default to OEB Directory Output
            if !self.output_path.exists() {
                fs::create_dir_all(&self.output_path)?;
            }
            let writer = crate::oeb::writer::OEBWriter::new();
            writer.write_book(&mut book, &self.output_path)?;
        }

        println!("Done.");
        Ok(())
    }
}

#[cfg(test)]
mod dump_input_tests {
    use super::*;

    #[test]
    fn dump_input_writes_a_real_opf_and_content_directory() {
        let src = tempdir().unwrap();
        let html_path = src.path().join("index.html");
        fs::write(
            &html_path,
            "<html><head><title>A Loose Page</title></head><body><h1>Hello</h1></body></html>",
        )
        .unwrap();

        let dump = tempdir().unwrap();
        let dump_dir = dump.path().join("input");
        dump_input(&html_path, &dump_dir).unwrap();

        let entries: Vec<String> = fs::read_dir(&dump_dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        assert!(entries.iter().any(|e| e.ends_with(".opf")), "{entries:?}");

        let opf_text = fs::read_to_string(dump_dir.join("content.opf")).unwrap();
        assert!(opf_text.contains("<manifest>"));
        assert!(opf_text.contains("<spine>"));
    }
}

#[cfg(test)]
mod run_transforms_tests {
    use super::*;
    use base64::Engine;
    use crate::oeb::transforms::test_support::Builder;

    /// Exercises `Plumber::run_transforms`'s real wired-in prefix
    /// end-to-end against one `OEBBook`: a `data:` URI image (DataURL),
    /// an unrecognized guide reference type (Clean), and an orphaned
    /// manifest item unreachable from metadata/guide/spine/links
    /// (ManifestTrimmer) should all be handled for real, not left
    /// untouched by a no-op placeholder.
    #[test]
    fn run_transforms_actually_runs_the_wired_in_pipeline_prefix() {
        let png_1x1_transparent = base64::engine::general_purpose::STANDARD.decode(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
        ).unwrap();
        let data_uri = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png_1x1_transparent)
        );
        let mut oeb = Builder::new()
            .page("a.html", &format!(r#"<img src="{data_uri}"/>"#))
            .part("orphan.txt", "text/plain", b"unreachable", false)
            .build();
        oeb.guide.add("some-unrecognized-type", None, "a.html");

        let plumber = Plumber::new("in.html", "out.epub");
        plumber.run_transforms(&mut oeb, "epub").unwrap();

        // DataURL: the inline data: URI became a real manifest item.
        let raw = oeb.container.read("a.html").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(!html.contains("data:image"), "{html}");
        assert!(oeb.manifest.iter().any(|i| i.media_type == "image/png"));

        // Clean: the unrecognized guide type is gone.
        assert!(!oeb.guide.references.contains_key("some-unrecognized-type"));

        // ManifestTrimmer: the orphaned item was dropped.
        assert!(!oeb.manifest.iter().any(|i| i.href == "orphan.txt"));
    }

    /// A genuine end-to-end conversion through the public `Plumber::run`
    /// entry point (HTML -> EPUB, on real files on disk), proving the
    /// full wired-in pipeline -- not just `run_transforms` in isolation
    /// -- actually flattens CSS for real. Before this issue, this
    /// conversion produced an EPUB with the *unmodified* input markup
    /// (no `stylesheet.css`, no `calibre*` classes); this test would
    /// have failed against that pre-existing behavior.
    #[test]
    fn a_real_html_to_epub_conversion_flattens_css_end_to_end() {
        let src = tempdir().unwrap();
        let html_path = src.path().join("book.html");
        fs::write(&html_path, "<html><head><title>A Book</title></head><body><h1>Ch 1</h1><p>Hello.</p></body></html>").unwrap();

        let out_dir = tempdir().unwrap();
        let epub_path = out_dir.path().join("book.epub");
        Plumber::new(&html_path, &epub_path).run().unwrap();

        let file = fs::File::open(&epub_path).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let names: Vec<String> = (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect();
        assert!(names.iter().any(|n| n.ends_with(".css")), "{names:?}");

        let html_entry = names.iter().find(|n| n.ends_with(".html")).cloned().unwrap();
        let mut html_bytes = Vec::new();
        std::io::Read::read_to_end(&mut zip.by_name(&html_entry).unwrap(), &mut html_bytes).unwrap();
        let html = String::from_utf8_lossy(&html_bytes);
        assert!(html.contains("class=\"calibre"), "CSSFlattener should have added real classes: {html}");
    }
}

#[cfg(test)]
mod write_output_threads_opts_tests {
    use super::*;

    /// Issue #690's own definition of done: `Plumber::write_output`
    /// really threads `&self.opts` through to the output plugin it
    /// dispatches to, exercised here through the public `Plumber` API
    /// end-to-end (not by calling `MOBIOutput::convert` directly, the
    /// way `mobi_output_test.rs`'s own option test does) -- a real
    /// `.mobi` conversion with `opts.mobi.dont_compress = true` set on
    /// the `Plumber` itself produces real uncompressed output.
    #[test]
    fn plumber_threads_a_real_mobi_option_from_conversion_options_through_to_the_output_file() {
        let src = tempdir().unwrap();
        let html_path = src.path().join("book.html");
        let paragraph = "The quick brown fox jumps over the lazy dog. ".repeat(200);
        fs::write(&html_path, format!("<html><body><p>{paragraph}</p></body></html>")).unwrap();

        let out_dir = tempdir().unwrap();
        let mobi_path = out_dir.path().join("book.mobi");

        let mut opts = ConversionOptions::default();
        opts.mobi.dont_compress = true;
        Plumber::with_options(&html_path, &mobi_path, opts).run().unwrap();

        let bytes = fs::read(&mobi_path).unwrap();
        let record0_offset = u32::from_be_bytes(bytes[78..82].try_into().unwrap()) as usize;
        let compression = u16::from_be_bytes(bytes[record0_offset..record0_offset + 2].try_into().unwrap());
        const UNCOMPRESSED: u16 = 1;
        assert_eq!(compression, UNCOMPRESSED, "Plumber's own opts.mobi.dont_compress should have really reached MOBIOutput::convert, not a default MobiWriterOpts");
    }
}
