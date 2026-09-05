use crate::html::input::get_filelist;
use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use crate::oeb::spine::SpineItem;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub struct HTMLInput;

impl HTMLInput {
    pub fn new() -> Self {
        HTMLInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        println!("Crawling HTML file: {:?}", input_path);

        // Depth-first, which is calibre's default ordering.
        let file_list = get_filelist(input_path, 5, None, false)
            .map_err(|e| anyhow::anyhow!("Failed to traverse HTML: {}", e.message))?;
        println!("Found {} files", file_list.len());

        fs::create_dir_all(output_dir)?;
        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);

        // Map absolute paths to hrefs
        let mut path_to_href = HashMap::new();

        // 1. Process files and add to Manifest
        for (i, html_file) in file_list.iter().enumerate() {
            let stem = html_file.path.file_stem().unwrap().to_string_lossy();
            let href = format!("{}_{}.html", stem, i); // Ensure unique href
            let id = format!("item_{}", i);

            // Copy content to output dir
            let dest_path = output_dir.join(&href);
            fs::copy(&html_file.path, &dest_path)?;

            // Add to map
            path_to_href.insert(html_file.path.clone(), href.clone());

            // Add to Manifest
            book.manifest.items.insert(
                id.clone(),
                ManifestItem {
                    id: id.clone(),
                    href: href.clone(),
                    media_type: "application/xhtml+xml".to_string(),
                    fallback: None,
                    linear: true,
                },
            );

            // Add primary file to Spine
            if i == 0 {
                // Root file is first
                book.spine.items.push(SpineItem {
                    idref: id.clone(),
                    linear: true,
                });
            } else {
                book.spine.items.push(SpineItem {
                    idref: id.clone(),
                    linear: true,
                });
            }
        }

        // 2. Rewrite Links (TODO: Implement robust rewriting using html parser)
        // For now, we just copied files. Links might be broken if they were relative.
        // A robust implementation would parse HTML, find links, resolve to `path`, lookup `href` in map, and rewrite.
        // Leaving this as TODO for iterative refinement.

        // 3. Metadata (issue #536): the real book title, from the root
        // file's own `<title>` (`get_filelist`'s `HtmlFile::title`
        // already extracts it, falling back to the file's stem when
        // there's no `<title>` tag -- see that struct's own doc).
        // Real upstream's own title chain is `metadata_from_filename`
        // updated by `get_metadata`'s real HTML-derived value,
        // falling back to "Unknown" -- narrowed here to just the
        // per-file `<title>`/stem `get_filelist` already computes,
        // not a separate filename-pattern parser.
        let title = file_list.first().map(|f| f.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| "Unknown".to_string());
        book.metadata.add("title", &title);

        println!(
            "Created OEBBook with {} manifest items.",
            book.manifest.items.len()
        );
        Ok(book)
    }
}
