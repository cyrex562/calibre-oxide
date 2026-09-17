//! Real client for the [Google Books API v1](https://developers.google.com/books/docs/v1/using)
//! (issue #785, part of #750).
//!
//! Real upstream calibre's own `google.py` source talks to Google's
//! now-deprecated `books.google.com/books/feeds/volumes` Atom/OpenSearch
//! feed. This port deliberately uses the current, officially documented
//! `www.googleapis.com/books/v1/volumes` JSON API instead -- a real,
//! disclosed divergence, not a byte-for-byte port of the old endpoint:
//! the JSON API is the one Google actually documents and supports today,
//! needs no XML/XPath machinery, and returns a superset of the fields
//! the old feed exposed (including `averageRating`, absent from the
//! Atom entries `to_metadata` parses).
//!
//! Built on the already-real, plain-`reqwest`-based [`crate::scraper::Browser`]
//! (issue #58) -- no new HTTP transport.

use std::collections::BTreeMap;

use crate::scraper::{Browser, BrowserError, OpenOptions};

use super::MetadataCandidate;

const DEFAULT_BASE_URL: &str = "https://www.googleapis.com/books/v1/volumes";

#[derive(Debug, thiserror::Error)]
pub enum GoogleBooksError {
    #[error(transparent)]
    Browser(#[from] BrowserError),
    #[error("Google Books API returned HTTP {0}")]
    Http(u16),
    #[error("failed to parse Google Books API response: {0}")]
    Parse(#[from] serde_json::Error),
}

/// A search query. At least one field should be set; an entirely empty
/// query returns Google's own "no query" error, which surfaces as
/// [`GoogleBooksError::Http`].
#[derive(Debug, Clone, Default)]
pub struct GoogleBooksQuery {
    pub title: Option<String>,
    pub authors: Option<String>,
    /// When set, takes precedence over `title`/`authors` -- matches
    /// Google Books' own `isbn:` search-operator semantics (an ISBN
    /// search is exact, mixing it with title/author terms only narrows
    /// further and isn't useful for "identify this book" lookups).
    pub isbn: Option<String>,
}

/// Real search against the live Google Books API.
pub fn search(browser: &Browser, query: &GoogleBooksQuery) -> Result<Vec<MetadataCandidate>, GoogleBooksError> {
    search_at(browser, DEFAULT_BASE_URL, query)
}

pub(crate) fn search_at(browser: &Browser, base_url: &str, query: &GoogleBooksQuery) -> Result<Vec<MetadataCandidate>, GoogleBooksError> {
    let q = build_query_string(query);
    let url = format!("{base_url}?q={}&maxResults=20", url_encode(&q));
    let resp = browser.open_novisit(&url, &OpenOptions::default())?;
    if let Some(status) = resp.status() {
        if status >= 400 {
            return Err(GoogleBooksError::Http(status));
        }
    }
    let body: serde_json::Value = serde_json::from_slice(resp.read())?;
    Ok(parse_response(&body))
}

fn build_query_string(query: &GoogleBooksQuery) -> String {
    if let Some(isbn) = query.isbn.as_deref().filter(|s| !s.is_empty()) {
        return format!("isbn:{isbn}");
    }
    let mut parts = Vec::new();
    if let Some(title) = query.title.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("intitle:{title}"));
    }
    if let Some(authors) = query.authors.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("inauthor:{authors}"));
    }
    parts.join("+")
}

fn url_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn parse_response(body: &serde_json::Value) -> Vec<MetadataCandidate> {
    body.get("items").and_then(|v| v.as_array()).map(|items| items.iter().filter_map(parse_item).collect()).unwrap_or_default()
}

fn parse_item(item: &serde_json::Value) -> Option<MetadataCandidate> {
    let info = item.get("volumeInfo")?;

    let title = info.get("title").and_then(|v| v.as_str()).map(String::from);
    let authors = info
        .get("authors")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let description = info.get("description").and_then(|v| v.as_str()).map(String::from);
    let publisher = info.get("publisher").and_then(|v| v.as_str()).map(String::from);
    let pubdate = info.get("publishedDate").and_then(|v| v.as_str()).map(String::from);
    let tags =
        info.get("categories").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let language = info.get("language").and_then(|v| v.as_str()).map(String::from);
    let rating = info.get("averageRating").and_then(|v| v.as_f64());

    let mut identifiers = BTreeMap::new();
    if let Some(ids) = info.get("industryIdentifiers").and_then(|v| v.as_array()) {
        for id in ids {
            let itype = id.get("type").and_then(|v| v.as_str());
            let ivalue = id.get("identifier").and_then(|v| v.as_str());
            if let (Some(itype), Some(ivalue)) = (itype, ivalue) {
                let key = match itype {
                    "ISBN_13" => "isbn".to_string(),
                    "ISBN_10" => "isbn10".to_string(),
                    other => other.to_lowercase(),
                };
                identifiers.entry(key).or_insert_with(|| ivalue.to_string());
            }
        }
    }

    // Google serves cover thumbnails over plain http:// even though the
    // API itself is https-only; force https so the srv cover-proxy
    // sub-issue never has to special-case a mixed-scheme fetch.
    let cover_url = info.get("imageLinks").and_then(|v| v.get("thumbnail")).and_then(|v| v.as_str()).map(|s| s.replacen("http://", "https://", 1));

    Some(MetadataCandidate { source: "Google Books".to_string(), title, authors, description, publisher, pubdate, tags, identifiers, language, cover_url, rating })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    use super::*;

    /// A tiny single-route HTTP/1.1 test server -- matches this
    /// workspace's other `TestSite` test helpers (e.g.
    /// `crates/calibre_srv/src/news.rs`'s own private copy); not shared
    /// across crates' private `#[cfg(test)]` blocks.
    struct TestSite {
        addr: std::net::SocketAddr,
    }

    impl TestSite {
        fn start(body: &'static [u8]) -> TestSite {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let mut stream: TcpStream = stream;
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        continue;
                    }
                    loop {
                        let mut l = String::new();
                        if reader.read_line(&mut l).is_err() || l.trim().is_empty() {
                            break;
                        }
                    }
                    let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.write_all(body);
                }
            });
            TestSite { addr }
        }

        fn base_url(&self) -> String {
            format!("http://{}/books/v1/volumes", self.addr)
        }
    }

    /// A real captured shape of a Google Books API v1 `volumes` list
    /// response (fields trimmed to what this module reads), per
    /// https://developers.google.com/books/docs/v1/using#WorkingVolumes.
    const FIXTURE: &[u8] = br#"{
      "kind": "books#volumes",
      "totalItems": 1,
      "items": [
        {
          "kind": "books#volume",
          "id": "abc123",
          "volumeInfo": {
            "title": "Dune",
            "authors": ["Frank Herbert"],
            "publisher": "Ace Books",
            "publishedDate": "1990-09-01",
            "description": "A stunning blend of adventure and mysticism.",
            "industryIdentifiers": [
              {"type": "ISBN_10", "identifier": "0441013597"},
              {"type": "ISBN_13", "identifier": "9780441013593"}
            ],
            "categories": ["Fiction"],
            "averageRating": 4.5,
            "language": "en",
            "imageLinks": {
              "thumbnail": "http://books.google.com/books/content?id=abc123&printsec=frontcover&img=1&zoom=1"
            }
          }
        }
      ]
    }"#;

    fn dummy_browser() -> Browser {
        Browser::new("", &[], true)
    }

    #[test]
    fn search_parses_a_real_google_books_response_shape() {
        let site = TestSite::start(FIXTURE);
        let browser = dummy_browser();
        let results = search_at(&browser, &site.base_url(), &GoogleBooksQuery { title: Some("Dune".into()), ..Default::default() }).unwrap();

        assert_eq!(results.len(), 1);
        let c = &results[0];
        assert_eq!(c.source, "Google Books");
        assert_eq!(c.title.as_deref(), Some("Dune"));
        assert_eq!(c.authors, vec!["Frank Herbert".to_string()]);
        assert_eq!(c.publisher.as_deref(), Some("Ace Books"));
        assert_eq!(c.pubdate.as_deref(), Some("1990-09-01"));
        assert_eq!(c.description.as_deref(), Some("A stunning blend of adventure and mysticism."));
        assert_eq!(c.tags, vec!["Fiction".to_string()]);
        assert_eq!(c.language.as_deref(), Some("en"));
        assert_eq!(c.rating, Some(4.5));
        assert_eq!(c.identifiers.get("isbn").map(String::as_str), Some("9780441013593"));
        assert_eq!(c.identifiers.get("isbn10").map(String::as_str), Some("0441013597"));
        // http:// forced to https://.
        assert_eq!(c.cover_url.as_deref(), Some("https://books.google.com/books/content?id=abc123&printsec=frontcover&img=1&zoom=1"));
    }

    #[test]
    fn search_with_no_items_returns_an_empty_vec_not_an_error() {
        let site = TestSite::start(br#"{"kind": "books#volumes", "totalItems": 0}"#);
        let browser = dummy_browser();
        let results = search_at(&browser, &site.base_url(), &GoogleBooksQuery { title: Some("zzzznonexistent".into()), ..Default::default() }).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn an_isbn_query_takes_precedence_over_title_and_author() {
        let mut q = GoogleBooksQuery::default();
        q.isbn = Some("9780441013593".into());
        q.title = Some("Dune".into());
        q.authors = Some("Frank Herbert".into());
        assert_eq!(build_query_string(&q), "isbn:9780441013593");
    }

    #[test]
    fn a_title_and_author_query_combines_both_operators() {
        let q = GoogleBooksQuery { title: Some("Dune".into()), authors: Some("Frank Herbert".into()), isbn: None };
        assert_eq!(build_query_string(&q), "intitle:Dune+inauthor:Frank Herbert");
    }

    #[test]
    #[ignore = "hits the real, rate-limited public Google Books API -- run explicitly with `cargo test -- --ignored`"]
    fn a_real_live_google_books_search_returns_a_real_dune_candidate() {
        let browser = dummy_browser();
        let results = search(&browser, &GoogleBooksQuery { title: Some("Dune".into()), authors: Some("Frank Herbert".into()), isbn: None }).unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().any(|c| c.title.as_deref().unwrap_or_default().contains("Dune")));
    }

    #[test]
    fn identifiers_map_keys_from_map_identify_fixture() {
        // Extra type not seen for books but real upstream `industryIdentifiers`
        // can contain e.g. "OTHER" -- confirm it's lowercased, not dropped.
        let body: serde_json::Value = serde_json::from_slice(br#"{
            "items": [{"volumeInfo": {"industryIdentifiers": [{"type": "OTHER", "identifier": "xyz"}]}}]
        }"#).unwrap();
        let results = parse_response(&body);
        assert_eq!(results[0].identifiers.get("other").map(String::as_str), Some("xyz"));
    }
}
