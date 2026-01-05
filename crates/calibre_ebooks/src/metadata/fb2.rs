use crate::metadata::MetaInformation;
use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use roxmltree::Document;
use std::io::{Read, Seek};
use zip::ZipArchive;

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    // Check if zip
    let start_pos = stream.stream_position()?;
    let mut xml_content = String::new();
    let mut is_zip = false;

    if let Ok(mut archive) = ZipArchive::new(&mut stream) {
        // Find .fb2 file
        let mut file_name = String::new();
        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            if file.name().to_lowercase().ends_with(".fb2") {
                file_name = file.name().to_string();
                break;
            }
        }
        if !file_name.is_empty() {
            let mut file = archive.by_name(&file_name)?;
            file.read_to_string(&mut xml_content)?;
            is_zip = true;
        }
    }

    if !is_zip {
        stream.seek(std::io::SeekFrom::Start(start_pos))?;
        stream.read_to_string(&mut xml_content)?;
    }

    parse_fb2(&xml_content)
}

fn parse_fb2(xml: &str) -> Result<MetaInformation> {
    let doc = Document::parse(xml)?;
    let root = doc.root_element();
    // FB2 namespace usually http://www.gribuser.ru/xml/fictionbook/2.0
    // But we iterate descendants so we can just check name

    let description = root
        .descendants()
        .find(|n| n.tag_name().name().eq_ignore_ascii_case("description"))
        .ok_or_else(|| anyhow::anyhow!("No description in FB2"))?;

    let title_info = description
        .descendants()
        .find(|n| n.tag_name().name().eq_ignore_ascii_case("title-info"));

    let mut mi = MetaInformation::default();

    if let Some(ti) = title_info {
        // Title
        if let Some(bt) = ti
            .descendants()
            .find(|n| n.tag_name().name().eq_ignore_ascii_case("book-title"))
        {
            if let Some(t) = bt.text() {
                mi.title = t.trim().to_string();
            }
        }
        // Authors
        let authors: Vec<String> = ti
            .descendants()
            .filter(|n| n.tag_name().name().eq_ignore_ascii_case("author"))
            .map(|n| {
                // Compose author name from first-name, middle-name, last-name
                let first = n
                    .children()
                    .find(|c| c.tag_name().name() == "first-name")
                    .and_then(|c| c.text())
                    .unwrap_or("");
                let middle = n
                    .children()
                    .find(|c| c.tag_name().name() == "middle-name")
                    .and_then(|c| c.text())
                    .unwrap_or("");
                let last = n
                    .children()
                    .find(|c| c.tag_name().name() == "last-name")
                    .and_then(|c| c.text())
                    .unwrap_or("");

                let full = format!("{} {} {}", first, middle, last);
                full.split_whitespace().collect::<Vec<_>>().join(" ") // normalize spaces
            })
            .filter(|s| !s.is_empty())
            .collect();

        if !authors.is_empty() {
            mi.authors = authors;
        }

        // Series
        if let Some(seq) = ti
            .descendants()
            .find(|n| n.tag_name().name().eq_ignore_ascii_case("sequence"))
        {
            if let Some(name) = seq.attribute("name") {
                mi.series = Some(name.to_string());
            }
            if let Some(num) = seq.attribute("number") {
                if let Ok(idx) = num.parse::<f64>() {
                    mi.series_index = idx;
                }
            }
        }

        // Tags/Genres
        for genre in ti
            .descendants()
            .filter(|n| n.tag_name().name().eq_ignore_ascii_case("genre"))
        {
            if let Some(t) = genre.text() {
                mi.tags.push(t.trim().to_string());
            }
        }

        // Comment/Annotation
        if let Some(annot) = ti
            .descendants()
            .find(|n| n.tag_name().name().eq_ignore_ascii_case("annotation"))
        {
            // Annotation can contain HTML-like tags. roxmltree check text content recursively?
            // Just take text for now.
            if let Some(t) = annot.text() {
                mi.comments = Some(t.trim().to_string());
            }
        }

        // Cover
        // <coverpage><image l:href="#cover.jpg"/></coverpage>
        if let Some(cp) = ti
            .descendants()
            .find(|n| n.tag_name().name().eq_ignore_ascii_case("coverpage"))
        {
            if let Some(img) = cp
                .descendants()
                .find(|n| n.tag_name().name().eq_ignore_ascii_case("image"))
            {
                // href attribute. Might be 'href' or 'l:href' or 'xlink:href'
                // roxmltree handles namespaces.
                // We check all attrs.
                let href = img
                    .attributes()
                    .find(|a| a.name().contains("href"))
                    .map(|a| a.value());
                if let Some(h) = href {
                    let id = h.trim_start_matches('#');
                    // Find <binary id="...">
                    let binary = root
                        .descendants()
                        .find(|n| n.tag_name().name() == "binary" && n.attribute("id") == Some(id));

                    if let Some(bin) = binary {
                        if let Some(content) = bin.text() {
                            // Decode Base64
                            let content = content.split_whitespace().collect::<String>();
                            if let Ok(data) = general_purpose::STANDARD.decode(content) {
                                mi.cover_data = (Some("jpg".to_string()), data);
                                // Assume jpg or check content-type
                            }
                        }
                    }
                }
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
    fn test_fb2_metadata() {
        let xml = r##"
        <FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
        <description>
            <title-info>
                <genre>sf</genre>
                <author>
                    <first-name>John</first-name>
                    <last-name>Doe</last-name>
                </author>
                <book-title>The Book</book-title>
                <sequence name="The Series" number="1"/>
                <coverpage>
                     <image l:href="#cover.jpg"/>
                </coverpage>
            </title-info>
        </description>
        <binary id="cover.jpg" content-type="image/jpeg">
            SGVsbG8=
        </binary>
        </FictionBook>
        "##;

        let mut stream = Cursor::new(xml);
        let mi = get_metadata(&mut stream).unwrap();

        assert_eq!(mi.title, "The Book");
        assert_eq!(mi.authors, vec!["John Doe"]);
        assert_eq!(mi.tags, vec!["sf"]);
        assert_eq!(mi.series, Some("The Series".to_string()));
        assert_eq!(mi.series_index, 1.0);

        // "SGVsbG8=" -> "Hello"
        assert!(mi.cover_data.1.starts_with(b"Hello"));
    }
}
