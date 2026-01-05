pub mod archive;
pub mod author_mapper;
pub mod authors;
pub mod azw4;
pub mod chm;
pub mod docx;
pub mod epub;
pub mod ereader;
pub mod extz;
pub mod fb2;
pub mod haodoo;
pub mod html;
pub mod imp;
pub mod kfx;
pub mod lit;
pub mod lrf;
pub mod lrx;
pub mod meta;
pub mod mobi;
pub mod odt;
pub mod pdb;
pub mod pdf;
pub mod plucker;
pub mod pml;
pub mod rar;
pub mod rb;
pub mod rtf;
pub mod search_internet;
pub mod snb;
pub mod tag_mapper;
pub mod toc;
pub mod topaz;
pub mod txt;
pub mod utils;
pub mod xmp;
pub mod zip;

// Re-export commonly used items
pub use archive::{archive_type, get_comic_metadata, is_comic, parse_comic_comment};
pub use author_mapper::{cap_author_token, compile_rules, map_authors, Rule};
pub use authors::{author_to_author_sort, authors_to_string, string_to_authors};
pub use meta::{check_isbn, title_sort, MetaInformation};

use anyhow::{bail, Result};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub fn get_metadata<P: AsRef<Path>>(path: P) -> Result<MetaInformation> {
    let path = path.as_ref();
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    let file = File::open(path)?;
    let stream = BufReader::new(file);

    match ext.as_str() {
        "epub" => epub::get_metadata(stream),
        "mobi" | "prc" | "azw" | "azw3" => mobi::get_metadata(stream),
        "fb2" => fb2::get_metadata(stream),
        "lit" => lit::get_metadata(stream),
        "pdf" => pdf::get_metadata(stream),
        "rb" => rb::get_metadata(stream),
        "imp" => imp::get_metadata(stream),
        "lrf" | "lrx" => lrx::get_metadata(stream),
        "azw4" => azw4::get_metadata(stream),
        "chm" => chm::get_metadata(stream),
        "docx" => docx::get_metadata(stream),
        "odt" => odt::get_metadata(stream),
        "snb" => snb::get_metadata(stream),
        "pdb" | "updb" => pdb::get_metadata(stream), // PDB dispatcher?
        "txt" => txt::get_metadata(stream),
        "rtf" => rtf::get_metadata(stream),
        "html" | "htm" | "xhtml" => html::get_metadata(stream),
        "zip" | "cbz" => zip::get_metadata(stream),
        "rar" | "cbr" => rar::get_metadata(stream),
        // "xmp" => xmp::get_metadata(stream), // XMP usually sidecar?
        _ => bail!("Unsupported format: {}", ext),
    }
}
