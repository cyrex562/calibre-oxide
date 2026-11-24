use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use crate::oeb::spine::SpineItem;
use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use zip::ZipArchive;

pub struct ComicInput;

impl ComicInput {
    pub fn new() -> Self {
        ComicInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        println!("Converting Comic (CBZ): {:?}", input_path);

        // 1. Unzip
        let file = File::open(input_path).context("Failed to open CBZ")?;
        let mut archive = ZipArchive::new(file).context("Failed to open Zip archive")?;

        // Extract all to output_dir
        archive
            .extract(output_dir)
            .context("Failed to extract CBZ")?;

        // 2. Identify Images
        let mut images: Vec<PathBuf> = Vec::new();
        let valid_exts = ["jpg", "jpeg", "png", "gif", "webp"];

        // Recursive walk or flat? CBZ usually flat or nested?
        // Let's do a recursive walk of output_dir to find images.
        for entry in walkdir::WalkDir::new(output_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                if let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) {
                    if valid_exts.contains(&ext.to_lowercase().as_str()) {
                        images.push(entry.path().to_path_buf());
                    }
                }
            }
        }

        // 3. Sort Images
        // Natural sort matches Calibre behavior best, but alphanumeric is okay for now.
        images.sort();

        // 4. Generate Index HTML
        let mut html = String::from("<html><head><title>Comic</title><style>img { max-width: 100%; display: block; margin: 0 auto; }</style></head><body>");

        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);

        // We need paths relative to output_dir for hrefs
        let mut image_hrefs = Vec::new();

        for (i, img_path) in images.iter().enumerate() {
            let rel_path = pathdiff::diff_paths(img_path, output_dir).unwrap_or(img_path.clone());
            let rel_path_str = rel_path.to_string_lossy().replace("\\", "/");

            // Add to HTML
            html.push_str(&format!(
                "<div class=\"page\"><img src=\"{}\" alt=\"Page {}\" /></div>",
                rel_path_str,
                i + 1
            ));

            // Add to Manifest
            let id = format!("img_{}", i);
            let media_type = mime_guess::from_path(img_path)
                .first_or_octet_stream()
                .to_string();

            book.manifest.items.insert(
                id.clone(),
                ManifestItem {
                    id,
                    href: rel_path_str.clone(),
                    media_type,
                    fallback: None,
                    linear: false, // Images are not linear flow text usually, but here they are content?
                },
            );

            image_hrefs.push(rel_path_str);
        }

        html.push_str("</body></html>");

        let index_path = output_dir.join("index.html");
        fs::write(&index_path, html)?;

        // Add index to Manifest and Spine
        let index_id = "index".to_string();
        book.manifest.items.insert(
            index_id.clone(),
            ManifestItem {
                id: index_id.clone(),
                href: "index.html".to_string(),
                media_type: "application/xhtml+xml".to_string(),
                fallback: None,
                linear: true,
            },
        );

        book.spine.items.push(SpineItem {
            idref: index_id,
            linear: true,
        });

        // Metadata
        book.metadata.add("title", "Converted Comic");

        Ok(book)
    }
}
