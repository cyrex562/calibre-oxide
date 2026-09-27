//! `fetch-ebook-metadata` — the console front end to
//! `metadata::sources` (#813).
//!
//! Google Books and Open Library have both been merged and tested since
//! #785-#788; their only caller was `POST /metadata-search` in the
//! content server. This is the wrapper.
//!
//! Both sources are queried, and a failure in one is reported without
//! losing the other's results — a search that returns half an answer is
//! far more useful than one that refuses because a single service was
//! unreachable.

use anyhow::{bail, Result};
use calibre_ebooks::metadata::sources::{google_books, open_library, MetadataCandidate};
use calibre_ebooks::scraper::Browser;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "fetch-ebook-metadata")]
#[command(about = "Look up book metadata on Google Books and Open Library", long_about = None)]
struct Args {
    #[arg(short, long)]
    title: Option<String>,

    /// May be given more than once.
    #[arg(short, long = "author")]
    authors: Vec<String>,

    #[arg(short, long)]
    isbn: Option<String>,

    /// Print the candidates as JSON instead of for reading.
    #[arg(long)]
    json: bool,

    /// Show at most this many candidates. 0 means all.
    #[arg(short = 'n', long, default_value_t = 5)]
    limit: usize,
}

/// Greedy word wrap.
///
/// Hand-rolled rather than pulling in a dependency for one call site: a
/// description is the only long field, and breaking on whitespace is all
/// this needs. A word longer than `width` is left over-long rather than
/// split, since splitting an ISBN or a URL mid-token is worse than a
/// ragged edge.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn describe(candidate: &MetadataCandidate) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Source     : {}", candidate.source));
    if let Some(title) = &candidate.title {
        lines.push(format!("Title      : {title}"));
    }
    if !candidate.authors.is_empty() {
        lines.push(format!("Author(s)  : {}", candidate.authors.join(" & ")));
    }
    if let Some(publisher) = &candidate.publisher {
        lines.push(format!("Publisher  : {publisher}"));
    }
    if let Some(pubdate) = &candidate.pubdate {
        lines.push(format!("Published  : {pubdate}"));
    }
    if let Some(language) = &candidate.language {
        lines.push(format!("Language   : {language}"));
    }
    if !candidate.tags.is_empty() {
        lines.push(format!("Tags       : {}", candidate.tags.join(", ")));
    }
    if !candidate.identifiers.is_empty() {
        let ids: Vec<String> = candidate.identifiers.iter().map(|(k, v)| format!("{k}:{v}")).collect();
        lines.push(format!("Identifiers: {}", ids.join(" ")));
    }
    if let Some(rating) = candidate.rating {
        lines.push(format!("Rating     : {rating}"));
    }
    if let Some(url) = &candidate.cover_url {
        lines.push(format!("Cover      : {url}"));
    }
    if let Some(description) = &candidate.description {
        // Wrapped at a readable width rather than dumped: a Google Books
        // description can be several hundred characters on one line.
        lines.push("Comments   :".to_string());
        for chunk in wrap(description, 76) {
            lines.push(format!("  {chunk}"));
        }
    }
    lines.join("\n")
}

fn as_json(candidates: &[MetadataCandidate]) -> serde_json::Value {
    serde_json::json!(candidates
        .iter()
        .map(|c| serde_json::json!({
            "source": c.source,
            "title": c.title,
            "authors": c.authors,
            "description": c.description,
            "publisher": c.publisher,
            "pubdate": c.pubdate,
            "tags": c.tags,
            "identifiers": c.identifiers,
            "language": c.language,
            "cover_url": c.cover_url,
            "rating": c.rating,
        }))
        .collect::<Vec<_>>())
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.title.is_none() && args.authors.is_empty() && args.isbn.is_none() {
        bail!("nothing to search for: pass --title, --author or --isbn");
    }

    // The sources take one author string, not a list -- joined the same
    // way `field_for` renders multiple authors, so a repeated --author
    // behaves the way a user would expect.
    let authors = if args.authors.is_empty() { None } else { Some(args.authors.join(" & ")) };
    let google_query = google_books::GoogleBooksQuery { title: args.title.clone(), authors: authors.clone(), isbn: args.isbn.clone() };
    let open_library_query = open_library::OpenLibraryQuery { title: args.title.clone(), authors, isbn: args.isbn.clone() };

    let mut candidates: Vec<MetadataCandidate> = Vec::new();
    // Reported to stderr, not fatal: one source being down should not
    // throw away what the other found.
    let mut failures: Vec<String> = Vec::new();

    match google_books::search(&Browser::new("", &[], true), &google_query) {
        Ok(found) => candidates.extend(found),
        Err(e) => failures.push(format!("Google Books: {e}")),
    }
    match open_library::search(&Browser::new("", &[], true), &open_library_query) {
        Ok(found) => candidates.extend(found),
        Err(e) => failures.push(format!("Open Library: {e}")),
    }

    for failure in &failures {
        eprintln!("warning: {failure}");
    }

    if args.limit > 0 && candidates.len() > args.limit {
        candidates.truncate(args.limit);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&as_json(&candidates))?);
    } else if candidates.is_empty() {
        println!("No matches found.");
    } else {
        for (i, candidate) in candidates.iter().enumerate() {
            if i > 0 {
                println!();
            }
            println!("{}", describe(candidate));
        }
    }

    // A search that found nothing because every source failed is a
    // failure, not an empty result -- the exit status has to say so or a
    // script cannot tell the difference.
    if candidates.is_empty() && !failures.is_empty() {
        bail!("every metadata source failed");
    }
    Ok(())
}
