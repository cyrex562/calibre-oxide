use calibre_ebooks::epub::read_epub_metadata;
use calibre_ebooks::opf::parse_opf;
use calibre_utils::logging;
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    logging::init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <path_to_epub_or_opf>", args[0]);
        std::process::exit(1);
    }

    let path = PathBuf::from(&args[1]);
    if !path.exists() {
        eprintln!("File not found: {:?}", path);
        std::process::exit(1);
    }

    println!("Reading metadata from: {:?}", path);

    let meta = if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        if ext.eq_ignore_ascii_case("opf") {
            let content = fs::read_to_string(&path)?;
            parse_opf(&content)?
        } else if ext.eq_ignore_ascii_case("epub") {
            read_epub_metadata(&path)?
        } else {
            eprintln!("Unsupported extension: {}", ext);
            std::process::exit(1);
        }
    } else {
        eprintln!("No extension found");
        std::process::exit(1);
    };

    println!("Title: {}", meta.title);
    println!("Authors: {:?}", meta.authors);
    println!("Language: {:?}", meta.languages);
    println!("UUID: {:?}", meta.uuid);
    println!("Cover ID: {:?}", meta.cover_id);
    println!("Description: {:?}", meta.comments);

    Ok(())
}
