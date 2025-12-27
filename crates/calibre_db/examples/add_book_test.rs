use calibre_db::Library;
use calibre_ebooks::epub::read_epub_metadata;
use calibre_utils::logging;
use std::env;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    logging::init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <library_path> <epub_path>", args[0]);
        std::process::exit(1);
    }

    let library_path = PathBuf::from(&args[1]);
    let epub_path = PathBuf::from(&args[2]);

    println!("Opening library at {:?}", library_path);
    let mut library = Library::open(library_path)?;

    println!("Reading metadata from {:?}", epub_path);
    let metadata = read_epub_metadata(&epub_path)?;
    println!("Found title: {}", metadata.title);

    println!("Adding book...");
    let id = library.add_book(&epub_path, &metadata)?;
    println!("Book added with ID: {}", id);

    Ok(())
}
