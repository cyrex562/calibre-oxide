use crate::metadata::{string_to_authors, MetaInformation};
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use std::io::{Read, Seek};

lazy_static! {
    static ref META_PAT: Regex =
        Regex::new(r#"(?i)<meta\s+name=["']([^"']+)["']\s+content=["']([^"']+)["']"#).unwrap();
    static ref TITLE_PAT: Regex = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap();
    static ref COMMENT_PAT: Regex =
        Regex::new(r"<!--\s*([A-Z_]+)=['\x22](.*?)['\x22]\s*-->").unwrap();
}

pub fn get_metadata<R: Read + Seek>(stream: R) -> Result<MetaInformation> {
    // Read up to 150KB (Python limit) or full file if smaller
    let mut buf = Vec::with_capacity(150_000);
    stream.take(150_000).read_to_end(&mut buf)?;
    let content = String::from_utf8_lossy(&buf);

    let mut mi = MetaInformation::default();

    // Parse Checks
    // 1. Comments <!-- TITLE="Foo" -->
    for caps in COMMENT_PAT.captures_iter(&content) {
        let key = caps.get(1).unwrap().as_str();
        let val = caps.get(2).unwrap().as_str().trim();

        match key {
            "TITLE" => mi.title = val.to_string(),
            "AUTHOR" => mi.authors = string_to_authors(val),
            "PUBLISHER" => mi.publisher = Some(val.to_string()),
            "ISBN" => mi.set_identifier("isbn", val),
            "TAGS" => mi.tags = val.split(',').map(|s| s.trim().to_string()).collect(),
            "COMMENTS" => mi.comments = Some(val.to_string()),
            "SERIES" => mi.series = Some(val.to_string()),
            "SERIESNUMBER" => {
                if let Ok(idx) = val.parse::<f64>() {
                    mi.series_index = idx;
                }
            }
            "RATING" => {
                if let Ok(r) = val.parse::<f64>() {
                    mi.rating = Some(r);
                }
            }
            _ => {}
        }
    }

    // 2. Meta tags
    for caps in META_PAT.captures_iter(&content) {
        let name = caps.get(1).unwrap().as_str().to_lowercase();
        let content = caps.get(2).unwrap().as_str().trim();

        if content.is_empty() {
            continue;
        }

        if name.contains("title") && mi.title == "Unknown" {
            mi.title = content.to_string();
        } else if name.contains("author") || name.contains("creator") {
            if mi.authors.len() == 1 && mi.authors[0] == "Unknown" {
                mi.authors = string_to_authors(content);
            } else {
                // append? Python code appends.
                let authors = string_to_authors(content);
                for a in authors {
                    if !mi.authors.contains(&a) {
                        mi.authors.push(a);
                    }
                }
                // Remove "Unknown" if present
                if let Some(pos) = mi.authors.iter().position(|x| x == "Unknown") {
                    mi.authors.remove(pos);
                }
            }
        } else if name.contains("publisher") {
            mi.publisher = Some(content.to_string());
        } else if name == "isbn" {
            mi.set_identifier("isbn", content);
        } else if name.contains("description") || name == "comments" {
            mi.comments = Some(content.to_string());
        } else if name == "tags" || name == "subject" {
            for t in content.split(',') {
                mi.tags.push(t.trim().to_string());
            }
        }
    }

    // 3. Title tag (lowest priority or fallback?)
    // Python: get('title') or title_tag
    if mi.title == "Unknown" {
        if let Some(cap) = TITLE_PAT.captures(&content) {
            let t = cap.get(1).unwrap().as_str().trim();
            if !t.is_empty() {
                mi.title = t.to_string();
            }
        }
    }

    Ok(mi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_html_metadata() {
        let html = r#"
        <html>
        <head>
            <!-- TITLE="My HTML Book" -->
            <!-- AUTHOR="Jane Doe" -->
            <meta name="dc.language" content="en" />
            <meta name="description" content="A description" />
            <title>Ignored Title</title>
        </head>
        </html>
        "#;
        let mut stream = Cursor::new(html);
        let mi = get_metadata(&mut stream).unwrap();

        assert_eq!(mi.title, "My HTML Book");
        assert_eq!(mi.authors, vec!["Jane Doe"]);
        assert_eq!(mi.comments, Some("A description".to_string()));
    }

    #[test]
    fn test_html_meta_tags() {
        let html = r#"
        <html>
        <head>
            <meta name="dc.title" content="Meta Title" />
            <meta name="dc.creator" content="Meta Author" />
        </head>
        </html>
        "#;
        let mut stream = Cursor::new(html);
        let mi = get_metadata(&mut stream).unwrap();

        assert_eq!(mi.title, "Meta Title");
        assert_eq!(mi.authors, vec!["Meta Author"]);
    }
}
