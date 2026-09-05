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

pub struct Plumber {
    input_path: PathBuf,
    output_path: PathBuf,
}

impl Plumber {
    pub fn new<P: AsRef<Path>>(input: P, output: P) -> Self {
        Self {
            input_path: input.as_ref().to_path_buf(),
            output_path: output.as_ref().to_path_buf(),
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
        let book = convert_to_oebbook(&self.input_path, &extract_path)?;

        // 3. Transforms (Placeholder)
        // Processing steps would go here (metadata merge, style flattening, etc.)
        println!("Processed {} manifest items.", book.manifest.items.len());

        // 4. Output Plugin
        self.write_output(book)?;

        println!("Done.");
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
            output_plugin.convert(&mut book, &self.output_path)?;
        } else if output_ext == "docx" {
            use crate::output::docx_output::DOCXOutput;
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = DOCXOutput::new();
            output_plugin.convert(&book, &self.output_path)?;
        } else if ["mobi", "azw", "prc"].contains(&output_ext.as_str()) {
            use crate::output::mobi_output::MOBIOutput;
            // Ensure dir exists
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = MOBIOutput::new();
            output_plugin.convert(&book, &self.output_path)?;
        } else if output_ext == "rb" {
            use crate::output::rb_output::RBOutput;
            // Ensure dir exists
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = RBOutput::new();
            output_plugin.convert(&book, &self.output_path)?;
        } else if output_ext == "lit" {
            use crate::output::lit_output::LitOutput;
            // Ensure dir exists
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = LitOutput::new();
            for warning in output_plugin.convert(&mut book, &self.output_path)? {
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
            output_plugin.convert(&mut book, &self.output_path)?;
        } else if output_ext == "snb" {
            use crate::output::snb_output::SnbOutput;
            // Ensure dir exists
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = SnbOutput::new();
            output_plugin.convert(&book, &self.output_path)?;
        } else if output_ext == "rtf" {
            use crate::output::rtf_output::RTFOutput;
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = RTFOutput::new();
            output_plugin.convert(&book, &self.output_path)?;
        } else if output_ext == "fb2" {
            use crate::output::fb2_output::FB2Output;
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = FB2Output::new();
            output_plugin.convert(&book, &self.output_path)?;
        } else if output_ext == "pdf" {
            use crate::output::pdf_output::PDFOutput;
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = PDFOutput::new();
            output_plugin.convert(&book, &self.output_path)?;
        } else if output_ext == "lrf" {
            use crate::output::lrf_output::LRFOutput;
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = LRFOutput::new();
            output_plugin.convert(&book, &self.output_path)?;
        } else if output_ext == "oeb" {
            use crate::output::oeb_output::OEBOutput;
            let output_plugin = OEBOutput::new();
            output_plugin.convert(&mut book, &self.output_path)?;
        } else if output_ext == "pdb" {
            use crate::output::pdb_output::PDBOutput;
            // Ensure parent exists
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = PDBOutput::new();
            output_plugin.convert(&book, &self.output_path)?;
        } else if output_ext == "odt" {
            use crate::output::odt_output::ODTOutput;
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = ODTOutput::new();
            output_plugin.convert(&book, &self.output_path)?;
        } else if output_ext == "tcr" {
            use crate::output::tcr_output::TCROutput;
            // Ensure parent exists
            if let Some(parent) = self.output_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let output_plugin = TCROutput::new();
            output_plugin.convert(&book, &self.output_path)?;
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
