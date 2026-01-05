use crate::input::html_input::HTMLInput;
use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use anyhow::{Context, Result};
use std::fs::File;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct HTMLZInput;

impl HTMLZInput {
    pub fn new() -> Self {
        HTMLZInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        println!("Converting HTMLZ: {:?}", input_path);

        // 1. Unzip to output_dir
        if !output_dir.exists() {
            std::fs::create_dir_all(output_dir)?;
        }

        let file = File::open(input_path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        archive.extract(output_dir)?;

        // 2. Check for OPF
        // Common names: metadata.opf, content.opf
        let opf_path = self.find_opf(output_dir);

        if let Some(opf) = opf_path {
            println!("Found OPF: {:?}", opf);

            // Construct OEBBook from OPF
            // Note: OEBReader mostly uses Container trait.
            // We need a DirContainer at output_dir.
            let container = Box::new(DirContainer::new(output_dir));
            let mut book = OEBBook::new(container);

            // Need a way to read OPF properly.
            // OEBReader::read_opf(path) logic is embedded or separate?
            // Checking `src/opf/read.rs` usage in `epub_input.rs`.
            // `epub_input` calls `OEBReader::read_opf(&mut book, &full_path_in_container)`.
            // We need relative path of OPF to container root.

            let opf_path_buf = opf.clone();
            // let relative_opf = opf.strip_prefix(output_dir).unwrap_or(&opf);

            // Read OPF content and parse metadata
            let opf_content = std::fs::read_to_string(&opf_path_buf)?;
            if let Ok(meta) = crate::opf::parse_opf(&opf_content) {
                // Populate metadata from OPF
                if !meta.title.is_empty() {
                    book.metadata.add("title", &meta.title);
                }
                for author in meta.authors {
                    book.metadata.add("creator", &author);
                }
            } else {
                println!("Failed to parse OPF");
            }

            // Fallback: Assume index.html is content
            if output_dir.join("index.html").exists() {
                book.manifest
                    .add("content", "index.html", "application/xhtml+xml");
                book.spine.add("content", true);
            }
            // Ideally parse OPF manifest

            Ok(book)
        } else {
            println!("No OPF found, using HTMLInput logic");
            // Delegates to HTMLInput
            // HTMLInput expects input_path. We give it output_dir (where we extracted).
            // It will scan for index.html.
            // CAUTION: HTMLInput might try to copy files to output_dir.
            // If input == output, it might be fine or fail.
            // `HTMLInput::convert(input, output)`
            // If we pass `output_dir` as `input`, and `output_dir` as `output`?
            // HTMLInput creates `DirContainer(output)`.
            // It reads from `DirContainer(input)`.
            // It copies missing files.
            // It should be safe-ish.

            let html_input = HTMLInput::new();
            // We want HTMLInput to assemble book from our extracted directory.
            // We reuse the extracted dir as both source and destination (in-place).
            html_input.convert(output_dir, output_dir)
        }
    }

    fn find_opf(&self, dir: &Path) -> Option<PathBuf> {
        for entry in WalkDir::new(dir).max_depth(2) {
            if let Ok(e) = entry {
                if e.file_type().is_file() {
                    if let Some(ext) = e.path().extension() {
                        if ext == "opf" {
                            return Some(e.path().to_path_buf());
                        }
                    }
                }
            }
        }
        None
    }
}
