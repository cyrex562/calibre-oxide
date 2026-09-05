//! Sony PRS periodical metadata.
//!
//! Port of `old_src/src/calibre/ebooks/epub/periodical.py`.
//!
//! Sony's e-ink readers show a newspaper differently from a book —
//! sections, articles, bylines, a date on the home screen — and they
//! get that from two extra files alongside the EPUB's own metadata: an
//! RDF description of the issue and an Atom feed of its sections and
//! articles. [`sony_metadata`] builds both from a converted book's
//! metadata and table of contents. The EPUB output plugin writes them
//! into the package when converting a periodical for a Sony device.
//!
//! # Disclosed divergences from upstream (issues #24/#139/#142)
//!
//! - **Escaping.** In the interest of producing a file that parses:
//!   calibre escapes the section summary but not the article summary,
//!   and escapes neither `href`. A description containing `&` or `<`
//!   — ordinary in a news feed — therefore yields non-well-formed XML.
//!   This port escapes every interpolated value. See issue #142 for
//!   the one open question about this (whether real Sony hardware
//!   actually requires calibre's unescaped form).
//! - **Article summaries.** Real calibre reads the *section's*
//!   description for every article's `<summary>` instead of the
//!   article's own, so every article in a section shows the same
//!   blurb and the article's real description is dropped entirely —
//!   a plain bug (issue #139, #5), fixed here to use `article.description`.

use chrono::{DateTime, Datelike, Utc};

use crate::oeb::book::OEBBook;
use crate::oeb::toc::TOCNode;

/// The RDF issue description, `<periodical>.rdf` on the device.
const SONY_METADATA: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
    xmlns:dcterms="http://purl.org/dc/terms/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:prs="http://xmlns.sony.net/e-book/prs/">
    <rdf:Description rdf:about="">
        <dc:title>{title}</dc:title>
        <dc:publisher>{publisher}</dc:publisher>
        <dcterms:alternative>{short_title}</dcterms:alternative>
        <dcterms:issued>{issue_date}</dcterms:issued>
        <dc:language>{language}</dc:language>
        <dcterms:conformsTo rdf:resource="http://xmlns.sony.net/e-book/prs/periodicals/1.0/newspaper/1.0"/>
        <dcterms:type rdf:resource="http://xmlns.sony.net/e-book/prs/datatype/newspaper"/>
        <dcterms:type rdf:resource="http://xmlns.sony.net/e-book/prs/datatype/periodical"/>
    </rdf:Description>
</rdf:RDF>
"#;

const SONY_ATOM: &str = r#"<?xml version="1.0" encoding="utf-8" ?>
<feed xmlns="http://www.w3.org/2005/Atom"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:dcterms="http://purl.org/dc/terms/"
    xmlns:prs="http://xmlns.sony.net/e-book/prs/"
    xmlns:media="http://video.search.yahoo.com/mrss"
    xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
    xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">

<title>{short_title}</title>
<updated>{updated}</updated>
<id>{id}</id>
{entries}
</feed>
"#;

const SONY_ATOM_SECTION: &str = r#"<entry rdf:ID="{title}">
  <title>{title}</title>
  <link href="{href}"/>
  <id>{id}</id>
  <updated>{updated}</updated>
  <summary>{desc}</summary>
  <category term="{short_title}/{title}"
      scheme="http://xmlns.sony.net/e-book/terms/" label="{title}"/>
  <dc:type xsi:type="prs:datatype">newspaper/section</dc:type>
  <dcterms:isReferencedBy rdf:resource=""/>
</entry>
"#;

const SONY_ATOM_ENTRY: &str = r##"<entry>
  <title>{title}</title>
  <author><name>{author}</name></author>
  <link href="{href}"/>
  <id>{id}</id>
  <updated>{updated}</updated>
  <summary>{desc}</summary>
  <category term="{short_title}/{section_title}"
      scheme="http://xmlns.sony.net/e-book/terms/" label="{section_title}"/>
  <dcterms:extent xsi:type="prs:word-count">{word_count}</dcterms:extent>
  <dc:type xsi:type="prs:datatype">newspaper/article</dc:type>
  <dcterms:isReferencedBy rdf:resource="#{section_title}"/>
</entry>
"##;

/// Re-exported so the FB2 and EPUB writers share one escaper.
pub use crate::xml_util::prepare_string_for_xml;

/// Substitute `{name}` placeholders in a template.
///
/// The templates are fixed strings in this file, so this is a literal
/// stand-in for Python's `str.format` rather than a general formatter:
/// an unknown placeholder is left as written.
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (name, value) in values {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// Inputs that would otherwise come from the clock or a random source.
///
/// Split out so the output is reproducible: two runs over the same book
/// with the same [`Clock`] produce identical bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clock {
    /// Stands in for `time.gmtime()`, used for `<updated>` and as the
    /// fallback issue date.
    pub now: DateTime<Utc>,
    /// Stands in for `uuid4()`, used when the book carries no calibre
    /// UUID identifier.
    pub fallback_id: String,
}

impl Clock {
    /// The real clock, with a caller-supplied fallback id.
    pub fn now(fallback_id: impl Into<String>) -> Self {
        Self {
            now: Utc::now(),
            fallback_id: fallback_id.into(),
        }
    }
}

/// The two files Sony's readers want alongside the EPUB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SonyMetadata {
    /// The RDF issue description.
    pub metadata: String,
    /// The Atom feed of sections and articles.
    pub atom: String,
}

/// Build the Sony periodical metadata for a converted book.
///
/// Port of the Python `sony_metadata`, which returns the same pair.
pub fn sony_metadata(oeb: &OEBBook, clock: &Clock) -> SonyMetadata {
    let m = &oeb.metadata;
    let first =
        |term: &str| -> Option<String> { m.get(term).first().map(|item| item.value.clone()) };

    let title = first("title").unwrap_or_default();
    // calibre stamps its own name and version here; this port stamps
    // the crate's.
    let publisher = format!("calibre-oxide {}", env!("CARGO_PKG_VERSION"));

    // A publication type reads `periodical:magazine:The Economist`, and
    // everything from the third field on is the short title.
    let short_title = match first("publication_type") {
        Some(pt) => {
            let parts: Vec<&str> = pt.split(':').collect();
            if parts.len() > 2 {
                parts[2..].join(":")
            } else {
                title.clone()
            }
        }
        None => title.clone(),
    };

    let date = first("date")
        .and_then(|d| calibre_utils::date::parse_date(&d, false))
        .map(|d| format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day()))
        .unwrap_or_else(|| {
            format!(
                "{:04}-{:02}-{:02}",
                clock.now.year(),
                clock.now.month(),
                clock.now.day()
            )
        });

    let language = first("language")
        .map(|l| l.replace('_', "-"))
        .unwrap_or_else(|| "en".to_string());

    let short_title = prepare_string_for_xml(&short_title, true);

    let metadata = fill(
        SONY_METADATA,
        &[
            ("title", &prepare_string_for_xml(&title, false)),
            ("short_title", &short_title),
            ("publisher", &prepare_string_for_xml(&publisher, false)),
            ("issue_date", &prepare_string_for_xml(&date, false)),
            ("language", &prepare_string_for_xml(&language, false)),
        ],
    );

    let updated = clock.now.format("%Y-%m-%dT%H:%M:%SZ").to_string();

    // calibre's own book id is the identifier whose scheme is `uuid`.
    let base_id = m
        .get("identifier")
        .iter()
        .find(|item| {
            item.attrib
                .iter()
                .any(|(k, v)| k.ends_with("scheme") && v == "uuid")
        })
        .map(|item| item.value.clone())
        .unwrap_or_else(|| clock.fallback_id.clone());
    let base_id = prepare_string_for_xml(&base_id, false);

    let mut entries: Vec<String> = Vec::new();
    let mut seen_titles: Vec<String> = Vec::new();

    for (i, section) in oeb.toc.root.children.iter().enumerate() {
        let Some(href) = non_empty(&section.href) else {
            continue;
        };
        let secid = format!("section{i}");
        let sectitle = disambiguate(
            section.title.clone().unwrap_or_default(),
            "Unknown",
            &mut seen_titles,
            true,
        );
        let sectitle = prepare_string_for_xml(&sectitle, true);
        let secdesc = prepare_string_for_xml(section.description.as_deref().unwrap_or(""), false);

        entries.push(fill(
            SONY_ATOM_SECTION,
            &[
                ("title", &sectitle),
                ("href", &prepare_string_for_xml(href, true)),
                ("id", &format!("{base_id}/{secid}")),
                ("short_title", &short_title),
                ("desc", &secdesc),
                ("updated", &updated),
            ],
        ));

        for (j, article) in section.children.iter().enumerate() {
            let Some(ahref) = non_empty(&article.href) else {
                continue;
            };
            // The article title is disambiguated against the same set
            // but — as in calibre — is not added to it, so several
            // articles sharing a title all become "<title> 1".
            let atitle = disambiguate(
                article.title.clone().unwrap_or_default(),
                "",
                &mut seen_titles,
                false,
            );
            let author = article.author.clone().unwrap_or_default();
            // Issue #139 (#5): real calibre reads the *section's*
            // description here instead of the article's own, so every
            // article in a section carries the same blurb and the
            // article's real description is dropped entirely -- a
            // plain bug, not a rendering choice worth reproducing.
            let desc = article.description.clone().unwrap_or_default();
            let aid = format!("article{j}");

            entries.push(fill(
                SONY_ATOM_ENTRY,
                &[
                    ("title", &prepare_string_for_xml(&atitle, false)),
                    ("author", &prepare_string_for_xml(&author, false)),
                    ("updated", &updated),
                    ("desc", &prepare_string_for_xml(&desc, false)),
                    ("short_title", &short_title),
                    ("section_title", &sectitle),
                    ("href", &prepare_string_for_xml(ahref, true)),
                    ("word_count", "1"),
                    ("id", &format!("{base_id}/{secid}/{aid}")),
                ],
            ));
        }
    }

    let atom = fill(
        SONY_ATOM,
        &[
            ("short_title", &short_title),
            ("entries", &entries.join("\n\n")),
            ("updated", &updated),
            ("id", &base_id),
        ],
    );

    SonyMetadata { metadata, atom }
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|v| !v.is_empty())
}

/// Append ` 1`, ` 2`, ... until the title is not already taken.
///
/// `record` mirrors calibre: section titles join the seen set, article
/// titles do not.
fn disambiguate(title: String, default: &str, seen: &mut Vec<String>, record: bool) -> String {
    let base = if title.is_empty() {
        default.to_string()
    } else {
        title
    };
    let mut candidate = base.clone();
    let mut n = 1;
    while seen.contains(&candidate) {
        candidate = format!("{base} {n}");
        n += 1;
    }
    if record {
        seen.push(candidate.clone());
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::DirContainer;
    use roxmltree::Document;
    use std::collections::HashMap;

    fn clock() -> Clock {
        Clock {
            now: DateTime::parse_from_rfc3339("2010-06-15T09:30:00Z")
                .unwrap()
                .with_timezone(&Utc),
            fallback_id: "fallback-uuid".to_string(),
        }
    }

    fn section(title: &str, href: &str, desc: Option<&str>) -> TOCNode {
        let mut node = TOCNode::new(Some(title.to_string()), Some(href.to_string()));
        node.description = desc.map(str::to_string);
        node
    }

    fn article(title: &str, href: &str, author: Option<&str>) -> TOCNode {
        article_with_desc(title, href, author, None)
    }

    fn article_with_desc(title: &str, href: &str, author: Option<&str>, desc: Option<&str>) -> TOCNode {
        let mut node = TOCNode::new(Some(title.to_string()), Some(href.to_string()));
        node.author = author.map(str::to_string);
        node.description = desc.map(str::to_string);
        node
    }

    fn book() -> OEBBook {
        let mut oeb = OEBBook::new(Box::new(DirContainer::new(std::path::Path::new("."))));
        oeb.metadata.add("title", "The Daily Chronicle");
        oeb.metadata.add("language", "en_GB");
        oeb.metadata.add("date", "2010-06-14T00:00:00+00:00");
        oeb.metadata
            .add("publication_type", "periodical:magazine:Chronicle Weekly");
        let mut attrib = HashMap::new();
        attrib.insert("scheme".to_string(), "uuid".to_string());
        oeb.metadata
            .add_with_attrib("identifier", "book-uuid-1234", attrib);

        let mut news = section("News", "news.html", Some("What happened"));
        news.add(article_with_desc(
            "Council votes",
            "news.html#a1",
            Some("A. Reporter"),
            Some("The council voted 5-2"),
        ));
        news.add(article("Rain expected", "news.html#a2", None));
        let mut sport = section("Sport", "sport.html", None);
        sport.add(article("United win", "sport.html#a1", None));
        oeb.toc.root.add(news);
        oeb.toc.root.add(sport);
        oeb
    }

    #[test]
    fn both_documents_are_well_formed_xml() {
        let out = sony_metadata(&book(), &clock());
        Document::parse(&out.metadata).expect("the RDF parses");
        Document::parse(&out.atom).expect("the Atom feed parses");
    }

    #[test]
    fn the_rdf_carries_the_issue_details() {
        let out = sony_metadata(&book(), &clock());
        let doc = Document::parse(&out.metadata).unwrap();
        let text_of = |name: &str| -> Option<String> {
            doc.descendants()
                .find(|n| n.tag_name().name() == name)
                .and_then(|n| n.text())
                .map(str::to_string)
        };
        assert_eq!(text_of("title").as_deref(), Some("The Daily Chronicle"));
        // The short title comes from the third field of the
        // publication type onwards.
        assert_eq!(text_of("alternative").as_deref(), Some("Chronicle Weekly"));
        assert_eq!(text_of("issued").as_deref(), Some("2010-06-14"));
        // Underscores in a language tag become hyphens.
        assert_eq!(text_of("language").as_deref(), Some("en-GB"));
        assert!(text_of("publisher").unwrap().starts_with("calibre-oxide "));
    }

    #[test]
    fn a_publication_type_with_too_few_fields_leaves_the_title_alone() {
        let mut oeb = book();
        oeb.metadata = crate::oeb::metadata::Metadata::new();
        oeb.metadata.add("title", "Plain Book");
        oeb.metadata.add("publication_type", "periodical:magazine");
        let out = sony_metadata(&oeb, &clock());
        assert!(
            out.metadata.contains("<dcterms:alternative>Plain Book<"),
            "{}",
            out.metadata
        );
    }

    #[test]
    fn missing_metadata_falls_back_rather_than_failing() {
        let oeb = OEBBook::new(Box::new(DirContainer::new(std::path::Path::new("."))));
        let out = sony_metadata(&oeb, &clock());
        let doc = Document::parse(&out.metadata).expect("still well formed");
        let language = doc
            .descendants()
            .find(|n| n.tag_name().name() == "language")
            .and_then(|n| n.text());
        assert_eq!(language, Some("en"));
        // No date metadata: today's date, per the injected clock.
        assert!(
            out.metadata.contains("<dcterms:issued>2010-06-15<"),
            "{}",
            out.metadata
        );
        // No calibre UUID identifier: the fallback id is used.
        assert!(out.atom.contains("fallback-uuid"), "{}", out.atom);
    }

    #[test]
    fn the_book_uuid_identifier_becomes_the_feed_id() {
        let out = sony_metadata(&book(), &clock());
        let doc = Document::parse(&out.atom).unwrap();
        let feed_id = doc
            .root_element()
            .children()
            .find(|n| n.tag_name().name() == "id")
            .and_then(|n| n.text());
        assert_eq!(feed_id, Some("book-uuid-1234"));
    }

    #[test]
    fn identifiers_without_the_uuid_scheme_are_ignored() {
        let mut oeb = book();
        oeb.metadata = crate::oeb::metadata::Metadata::new();
        let mut isbn = HashMap::new();
        isbn.insert("scheme".to_string(), "ISBN".to_string());
        oeb.metadata
            .add_with_attrib("identifier", "9780000000000", isbn);
        let out = sony_metadata(&oeb, &clock());
        assert!(!out.atom.contains("9780000000000"), "{}", out.atom);
        assert!(out.atom.contains("fallback-uuid"));
    }

    #[test]
    fn sections_and_articles_are_numbered_by_position() {
        let out = sony_metadata(&book(), &clock());
        let doc = Document::parse(&out.atom).unwrap();
        let ids: Vec<String> = doc
            .descendants()
            .filter(|n| n.tag_name().name() == "entry")
            .filter_map(|e| {
                e.children()
                    .find(|c| c.tag_name().name() == "id")
                    .and_then(|c| c.text())
                    .map(str::to_string)
            })
            .collect();
        assert_eq!(
            ids,
            vec![
                "book-uuid-1234/section0",
                "book-uuid-1234/section0/article0",
                "book-uuid-1234/section0/article1",
                "book-uuid-1234/section1",
                "book-uuid-1234/section1/article0",
            ]
        );
    }

    #[test]
    fn an_article_carries_its_author_and_its_own_summary() {
        // Issue #139 (#5) fixed a real bug where every article in a
        // section carried the *section's* blurb instead of its own.
        let out = sony_metadata(&book(), &clock());
        let doc = Document::parse(&out.atom).unwrap();
        let first_article = doc
            .descendants()
            .filter(|n| n.tag_name().name() == "entry")
            .nth(1)
            .expect("an article entry");
        let child_text = |name: &str| -> Option<String> {
            first_article
                .descendants()
                .find(|c| c.tag_name().name() == name)
                .and_then(|c| c.text())
                .map(str::to_string)
        };
        assert_eq!(child_text("title").as_deref(), Some("Council votes"));
        assert_eq!(child_text("name").as_deref(), Some("A. Reporter"));
        assert_eq!(child_text("summary").as_deref(), Some("The council voted 5-2"));
    }

    #[test]
    fn an_article_with_no_description_of_its_own_gets_an_empty_summary() {
        // The second article in the fixture has no description set --
        // it must NOT fall back to the section's, matching the fix for
        // issue #139 (#5).
        let out = sony_metadata(&book(), &clock());
        let doc = Document::parse(&out.atom).unwrap();
        let second_article = doc
            .descendants()
            .filter(|n| n.tag_name().name() == "entry")
            .nth(2)
            .expect("a second article entry");
        let summary = second_article.descendants().find(|c| c.tag_name().name() == "summary").and_then(|c| c.text()).unwrap_or("");
        assert_eq!(summary, "");
    }

    #[test]
    fn nodes_without_an_href_are_skipped() {
        let mut oeb = book();
        oeb.toc = crate::oeb::toc::TOC::new();
        oeb.toc.root.add(TOCNode::new(Some("No link".into()), None));
        let mut linked = section("Linked", "s.html", None);
        linked.add(TOCNode::new(Some("Article".into()), Some(String::new())));
        oeb.toc.root.add(linked);

        let out = sony_metadata(&oeb, &clock());
        let doc = Document::parse(&out.atom).unwrap();
        let entries: Vec<_> = doc
            .descendants()
            .filter(|n| n.tag_name().name() == "entry")
            .collect();
        assert_eq!(entries.len(), 1, "only the linked section survives");
        // Section numbering still follows the TOC index, so the
        // skipped node's slot is left unused.
        assert!(out.atom.contains("/section1"), "{}", out.atom);
    }

    #[test]
    fn duplicate_section_titles_are_disambiguated() {
        let mut oeb = book();
        oeb.toc = crate::oeb::toc::TOC::new();
        for _ in 0..3 {
            oeb.toc.root.add(section("News", "news.html", None));
        }
        let out = sony_metadata(&oeb, &clock());
        let doc = Document::parse(&out.atom).unwrap();
        let titles: Vec<String> = doc
            .descendants()
            .filter(|n| n.tag_name().name() == "entry")
            .filter_map(|e| {
                e.children()
                    .find(|c| c.tag_name().name() == "title")
                    .and_then(|c| c.text())
                    .map(str::to_string)
            })
            .collect();
        assert_eq!(titles, vec!["News", "News 1", "News 2"]);
    }

    #[test]
    fn a_section_without_a_title_is_called_unknown() {
        let mut oeb = book();
        oeb.toc = crate::oeb::toc::TOC::new();
        oeb.toc
            .root
            .add(TOCNode::new(None, Some("s.html".to_string())));
        let out = sony_metadata(&oeb, &clock());
        assert!(out.atom.contains("<title>Unknown</title>"), "{}", out.atom);
    }

    #[test]
    fn duplicate_article_titles_all_get_the_same_suffix() {
        // calibre never adds an article title to the seen set, so the
        // second and third copies both become "Repeat 1". Reproduced.
        let mut oeb = book();
        oeb.toc = crate::oeb::toc::TOC::new();
        let mut s = section("Repeat", "s.html", None);
        for _ in 0..3 {
            s.add(article("Repeat", "s.html#a", None));
        }
        oeb.toc.root.add(s);
        let out = sony_metadata(&oeb, &clock());
        assert_eq!(out.atom.matches("<title>Repeat 1</title>").count(), 3);
    }

    #[test]
    fn markup_in_titles_and_summaries_is_escaped() {
        // The divergence from calibre this module documents: every
        // interpolated value is escaped, so a description containing
        // markup characters cannot produce a broken feed.
        let mut oeb = book();
        oeb.toc = crate::oeb::toc::TOC::new();
        let mut s = section(
            "Q & A <live>",
            "s.html?a=1&b=2",
            Some("Tom & Jerry <cartoon>"),
        );
        s.add(article("5 > 3", "a.html?x=1&y=2", Some("A & B")));
        oeb.toc.root.add(s);

        let out = sony_metadata(&oeb, &clock());
        let doc = Document::parse(&out.atom).expect("still parses with markup in the data");
        let titles: Vec<String> = doc
            .descendants()
            .filter(|n| n.tag_name().name() == "title")
            .filter_map(|n| n.text())
            .map(str::to_string)
            .collect();
        assert!(titles.contains(&"Q & A <live>".to_string()), "{titles:?}");
        assert!(titles.contains(&"5 > 3".to_string()), "{titles:?}");
        let hrefs: Vec<&str> = doc
            .descendants()
            .filter(|n| n.tag_name().name() == "link")
            .filter_map(|n| n.attribute("href"))
            .collect();
        assert!(hrefs.contains(&"s.html?a=1&b=2"), "{hrefs:?}");
    }

    #[test]
    fn entities_in_metadata_are_resolved_before_escaping() {
        // prepare_string_for_xml resolves entity references first, so
        // `&amp;` in the source does not become `&amp;amp;`.
        assert_eq!(
            prepare_string_for_xml("Tom &amp; Jerry", false),
            "Tom &amp; Jerry"
        );
        assert_eq!(prepare_string_for_xml("a < b", false), "a &lt; b");
        assert_eq!(prepare_string_for_xml("a > b", false), "a &gt; b");
        assert_eq!(prepare_string_for_xml(r#"say "hi""#, false), r#"say "hi""#);
        assert_eq!(
            prepare_string_for_xml(r#"say "hi""#, true),
            "say &quot;hi&quot;"
        );
        assert_eq!(prepare_string_for_xml("it's", true), "it&apos;s");
    }

    #[test]
    fn the_output_is_reproducible() {
        let oeb = book();
        assert_eq!(sony_metadata(&oeb, &clock()), sony_metadata(&oeb, &clock()));
    }

    #[test]
    fn an_empty_toc_still_produces_a_valid_feed() {
        let mut oeb = book();
        oeb.toc = crate::oeb::toc::TOC::new();
        let out = sony_metadata(&oeb, &clock());
        let doc = Document::parse(&out.atom).expect("parses");
        assert_eq!(
            doc.descendants()
                .filter(|n| n.tag_name().name() == "entry")
                .count(),
            0
        );
    }
}
