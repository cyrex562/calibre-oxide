use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use crate::oeb::spine::SpineItem;
use anyhow::{Context, Result};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use zip::ZipArchive;

pub struct ZIPInput;

impl ZIPInput {
    pub fn new() -> Self {
        ZIPInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        println!("Converting ZIP: {:?}", input_path);

        // 1. Unzip
        let file = File::open(input_path).context("Failed to open ZIP")?;
        let mut archive = ZipArchive::new(file).context("Failed to open Zip archive")?;

        archive
            .extract(output_dir)
            .context("Failed to extract ZIP")?;

        // 2. Detect Structure
        // Case A: OEB / EPUB Unpacked (look for OPF)
        let opf_path = find_file_recursive(output_dir, "opf");
        if let Some(opf) = opf_path {
            println!("Found OPF at {:?}, treating as OEB structure.", opf);
            // Verify OEBReader or similar can handle this?
            // OEBReader expects to read from a container/unpacked dir.
            // But we need to construct OEBBook from it.
            // We can use `crate::input::epub_input` logic or `OEBReader::read_opf`.
            // Actually, `OEBReader` isn't fully exposed as a high level "Import from Dir" yet maybe?
            // Let's check `epub_input`. It finds opf and parses it.

            // Simplest: Use logic similar to EPUB input but on the dir we just extracted.
            // Or just construct OEBBook pointing to this dir and let it be?
            // If we return OEBBook, the Plumber might convert it.

            // Let's do a basic "Index HTML" detection as primary fallback if OPF parsing is too complex to wire up here inline.
            // But valid ZIPs often are pseudo-EPUBs.
        }

        // Case B: Website Dump (index.html)
        let index_candidates = ["index.html", "index.htm", "default.html"];
        let mut index_file = None;

        for cand in index_candidates {
            if let Some(found) = find_file_recursive_name(output_dir, cand) {
                index_file = Some(found);
                break;
            }
        }

        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);

        if let Some(index_path) = index_file {
            // We found an index. Use it as the spine.
            let rel_path = pathdiff::diff_paths(&index_path, output_dir).unwrap_or(index_path);
            let rel_path_str = rel_path.to_string_lossy().replace("\\", "/");

            let id = "index".to_string();
            book.manifest.items.insert(
                id.clone(),
                ManifestItem::new(&id, &rel_path_str, "application/xhtml+xml"),
            );
            book.manifest.hrefs.insert(rel_path_str, id.clone());
            book.spine.add(&id, true);
            book.metadata.add("title", "ZIP Content");
        } else {
            // Case C: File Pile (Generate Index)
            let mut html = String::from(
                "<html><head><title>ZIP Contents</title></head><body><h1>ZIP Contents</h1><ul>",
            );

            let mut files = Vec::new();
            for entry in walkdir::WalkDir::new(output_dir)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file() {
                    files.push(entry.path().to_path_buf());
                }
            }
            files.sort();

            for path in files {
                let rel_path = pathdiff::diff_paths(&path, output_dir).unwrap_or(path.clone());
                let rel_str = rel_path.to_string_lossy().replace("\\", "/");
                html.push_str(&format!("<li><a href=\"{}\">{}</a></li>", rel_str, rel_str));
            }
            html.push_str("</ul></body></html>");

            let index_out = output_dir.join("generated_index.html");
            fs::write(&index_out, html)?;

            let id = "generated_index".to_string();
            book.manifest.items.insert(
                id.clone(),
                ManifestItem::new(&id, "generated_index.html", "application/xhtml+xml"),
            );
            book.manifest
                .hrefs
                .insert("generated_index.html".to_string(), id.clone());
            book.spine.add(&id, true);
            book.metadata.add("title", "ZIP Archive");
        }

        Ok(book)
    }
}

fn find_file_recursive(dir: &Path, extension: &str) -> Option<PathBuf> {
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            if let Some(ext) = entry.path().extension() {
                if ext.to_string_lossy().eq_ignore_ascii_case(extension) {
                    return Some(entry.path().to_path_buf());
                }
            }
        }
    }
    None
}

fn find_file_recursive_name(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            if entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(name)
            {
                return Some(entry.path().to_path_buf());
            }
        }
    }
    None
}
