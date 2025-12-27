use calibre_db::Library;
use calibre_utils::logging;
use std::env;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    logging::init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <library_path>", args[0]);
        std::process::exit(1);
    }

    let library_path = PathBuf::from(&args[1]);
    let library = Library::open(library_path)?;

    println!("Connected to library.");
    println!("Total books: {}", library.book_count()?);

    println!("{:<5} | {:<40} | {:<20}", "ID", "Title", "Author(s)");
    println!("{:-<5}-+-{:-<40}-+-{:-<20}", "", "", "");

    for book in library.list_books()? {
        let title = if book.title.len() > 40 {
            format!("{}...", &book.title[..37])
        } else {
            book.title.clone()
        };
        
        let author = book.author_sort.as_deref().unwrap_or("Unknown");
        let author = if author.len() > 20 {
             format!("{}...", &author[..17])
        } else {
            author.to_string()
        };

        println!("{:<5} | {:<40} | {:<20}", book.id, title, author);
    }

    Ok(())
}
