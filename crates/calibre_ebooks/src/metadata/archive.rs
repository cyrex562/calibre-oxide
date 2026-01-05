use crate::metadata::meta::MetaInformation;
use anyhow::{Context, Result};
use serde_json::Value;
use std::io::Read;

/// Check if a list of filenames represents a comic book (only image files)
pub fn is_comic(list_of_names: &[String]) -> bool {
    let extensions: std::collections::HashSet<String> = list_of_names
        .iter()
        .filter(|name| name.contains('.') && !name.to_lowercase().ends_with("thumbs.db"))
        .filter_map(|name| name.rsplitn(2, '.').next().map(|ext| ext.to_lowercase()))
        .collect();

    let comic_extensions: std::collections::HashSet<String> = ["jpg", "jpeg", "png"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    !extensions.is_empty() && extensions.is_subset(&comic_extensions)
}

/// Detect archive type from stream header
pub fn archive_type(stream: &mut dyn Read) -> Result<Option<String>> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header)?;

    // ZIP signature: PK\x03\x04 or PK\x05\x06
    if header[0] == 0x50 && header[1] == 0x4B {
        return Ok(Some("zip".to_string()));
    }

    // RAR signature: Rar!
    if &header == b"Rar!" || (header[0] == 0x52 && header[1] == 0x61 && header[2] == 0x72) {
        return Ok(Some("rar".to_string()));
    }

    Ok(None)
}

/// Extract comic book metadata from ComicBookInfo JSON structure
pub fn get_comic_book_info(data: &Value, mi: &mut MetaInformation, series_index: &str) {
    // Extract series
    if let Some(series) = data.get("series").and_then(|v| v.as_str()) {
        if !series.trim().is_empty() {
            mi.series = Some(series.to_string());

            // Try to get series index
            let si = data.get(series_index).or_else(|| {
                // Fallback to alternate index field
                if series_index == "volume" {
                    data.get("issue")
                } else {
                    data.get("volume")
                }
            });

            if let Some(idx) = si {
                if let Some(idx_num) = idx.as_f64() {
                    mi.series_index = idx_num;
                } else if let Some(idx_str) = idx.as_str() {
                    if let Ok(idx_num) = idx_str.parse::<f64>() {
                        mi.series_index = idx_num;
                    }
                }
            }
        }
    }

    // Extract language
    if let Some(lang) = data.get("language").and_then(|v| v.as_str()) {
        // TODO: Use canonicalize_lang from calibre_utils when available
        if !lang.is_empty() {
            mi.languages = vec![lang.to_string()];
        }
    }

    // Extract rating
    if let Some(rating) = data.get("rating").and_then(|v| v.as_f64()) {
        if rating >= 0.0 {
            mi.rating = Some(rating);
        }
    }

    // Extract title
    if let Some(title) = data.get("title").and_then(|v| v.as_str()) {
        if !title.trim().is_empty() {
            mi.title = title.to_string();
        }
    }

    // Extract publisher
    if let Some(publisher) = data.get("publisher").and_then(|v| v.as_str()) {
        if !publisher.trim().is_empty() {
            mi.publisher = Some(publisher.to_string());
        }
    }

    // Extract tags
    if let Some(tags) = data.get("tags").and_then(|v| v.as_array()) {
        let tag_strings: Vec<String> = tags
            .iter()
            .filter_map(|t| t.as_str().map(|s| s.to_string()))
            .collect();
        if !tag_strings.is_empty() {
            mi.tags = tag_strings;
        }
    }

    // Extract authors from credits
    if let Some(credits) = data.get("credits").and_then(|v| v.as_array()) {
        let mut authors = Vec::new();
        for credit in credits {
            if let Some(role) = credit.get("role").and_then(|v| v.as_str()) {
                if ["Writer", "Artist", "Cartoonist", "Creator"].contains(&role) {
                    if let Some(person) = credit.get("person").and_then(|v| v.as_str()) {
                        if !person.is_empty() {
                            // Reverse "Last, First" format to "First Last"
                            let name = if person.contains(", ") {
                                let parts: Vec<&str> = person.split(", ").collect();
                                if parts.len() == 2 {
                                    format!("{} {}", parts[1], parts[0])
                                } else {
                                    person.to_string()
                                }
                            } else {
                                person.to_string()
                            };
                            authors.push(name);
                        }
                    }
                }
            }
        }
        if !authors.is_empty() {
            mi.authors = authors;
        }
    }

    // Extract comments
    if let Some(comments) = data.get("comments").and_then(|v| v.as_str()) {
        if !comments.trim().is_empty() {
            mi.comments = Some(comments.trim().to_string());
        }
    }

    // Extract publication date
    if let Some(puby) = data.get("publicationYear").and_then(|v| v.as_i64()) {
        use chrono::NaiveDate;
        let pubm = data
            .get("publicationMonth")
            .and_then(|v| v.as_i64())
            .unwrap_or(6) as u32;

        if let Some(date) = NaiveDate::from_ymd_opt(puby as i32, pubm, 15) {
            mi.pubdate = Some(date.and_hms_opt(0, 0, 0).unwrap().and_utc());
        }
    }
}

/// Parse comic comment JSON to extract metadata
pub fn parse_comic_comment(comment: &[u8], series_index: &str) -> Result<MetaInformation> {
    let mut mi = MetaInformation::default();

    // Parse JSON
    let json_str = std::str::from_utf8(comment).context("Failed to parse comment as UTF-8")?;

    if json_str.trim().is_empty() || json_str == "{}" {
        return Ok(mi);
    }

    let data: Value = serde_json::from_str(json_str).context("Failed to parse comment as JSON")?;

    // Look for ComicBookInfo structure
    if let Some(obj) = data.as_object() {
        for (key, value) in obj {
            if key.starts_with("ComicBookInfo") {
                get_comic_book_info(value, &mut mi, series_index);
                break;
            }
        }
    }

    Ok(mi)
}

/// Extract metadata from comic book archive (CBZ/CBR)
pub fn get_comic_metadata<R: Read + std::io::Seek>(
    stream: &mut R,
    stream_type: &str,
    series_index: &str,
) -> Result<MetaInformation> {
    let comment = match stream_type {
        "cbz" => {
            use zip::ZipArchive;
            let archive = ZipArchive::new(stream).context("Failed to open ZIP archive")?;
            archive.comment().to_vec()
        }
        "cbr" => {
            // RAR comment extraction would require unrar library
            // For now, return empty comment
            // TODO: Implement RAR comment extraction when unrar bindings are available
            Vec::new()
        }
        _ => Vec::new(),
    };

    parse_comic_comment(&comment, series_index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_comic() {
        // All images - should be comic
        assert!(is_comic(&[
            "page1.jpg".to_string(),
            "page2.png".to_string(),
            "page3.jpeg".to_string(),
        ]));

        // Mixed with non-images - should not be comic
        assert!(!is_comic(&[
            "page1.jpg".to_string(),
            "metadata.xml".to_string(),
        ]));

        // Empty list - should not be comic
        assert!(!is_comic(&[]));

        // Thumbs.db should be ignored
        assert!(is_comic(&[
            "page1.jpg".to_string(),
            "Thumbs.db".to_string(),
        ]));
    }

    #[test]
    fn test_archive_type() {
        // ZIP signature
        let mut zip_header = std::io::Cursor::new(vec![0x50, 0x4B, 0x03, 0x04]);
        assert_eq!(
            archive_type(&mut zip_header).unwrap(),
            Some("zip".to_string())
        );

        // RAR signature
        let mut rar_header = std::io::Cursor::new(b"Rar!");
        assert_eq!(
            archive_type(&mut rar_header).unwrap(),
            Some("rar".to_string())
        );

        // Unknown signature
        let mut unknown = std::io::Cursor::new(vec![0x00, 0x00, 0x00, 0x00]);
        assert_eq!(archive_type(&mut unknown).unwrap(), None);
    }

    #[test]
    fn test_parse_comic_comment() {
        let json = r#"{
            "ComicBookInfo/1.0": {
                "series": "Test Series",
                "volume": 1,
                "title": "Test Title",
                "publisher": "Test Publisher",
                "rating": 4.5,
                "tags": ["Action", "Adventure"],
                "credits": [
                    {"role": "Writer", "person": "Doe, John"},
                    {"role": "Artist", "person": "Smith, Jane"}
                ],
                "comments": "Test comments",
                "publicationYear": 2020,
                "publicationMonth": 3
            }
        }"#;

        let mi = parse_comic_comment(json.as_bytes(), "volume").unwrap();
        assert_eq!(mi.series, Some("Test Series".to_string()));
        assert_eq!(mi.series_index, 1.0);
        assert_eq!(mi.title, "Test Title");
        assert_eq!(mi.publisher, Some("Test Publisher".to_string()));
        assert_eq!(mi.rating, Some(4.5));
        assert_eq!(mi.tags, vec!["Action", "Adventure"]);
        assert_eq!(mi.authors, vec!["John Doe", "Jane Smith"]);
        assert_eq!(mi.comments, Some("Test comments".to_string()));
    }

    #[test]
    fn test_get_comic_book_info_series_index_fallback() {
        let json = serde_json::json!({
            "series": "Test",
            "issue": 5
        });

        let mut mi = MetaInformation::default();
        get_comic_book_info(&json, &mut mi, "volume");

        // Should fallback to "issue" when looking for "volume"
        assert_eq!(mi.series_index, 5.0);
    }
}
