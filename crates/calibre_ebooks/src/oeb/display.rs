//! Port of `calibre.ebooks.oeb.display.webview`.
//!
//! In Python this module prepares markup for calibre's legacy Qt
//! book-viewer widget before handing it to a `QWebView`: substituting
//! `<!ENTITY ...>` declarations, closing self-closing non-void tags
//! (WebKit's XML parser chokes on `<div/>`), and deciding whether the
//! browser should load the document as HTML or as strict XML/XHTML.
//!
//! [`load_html`] here keeps that decision logic real and testable but
//! takes a [`WebViewSink`] instead of talking to a `QWebPage` directly —
//! this crate has no GUI/webview dependency, so the actual "put this
//! content in a browser widget" step is left to the caller (e.g. the
//! `wry`-backed viewer in `app-iced-prototype`). See [`WebViewSink`]'s
//! docs for the one piece of Python behavior that has no clean
//! equivalent here.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::{Captures, Regex};

lazy_static! {
    static ref ENTITY_DECL_PAT: Regex = Regex::new(r"<!\s*ENTITY\s+([^>]+)>").unwrap();
    static ref SELF_CLOSING_PAT: Regex = Regex::new(r"<\s*([:A-Za-z0-9-]+)([^>]*)/\s*>").unwrap();
    static ref XML_DETECT_PAT: Regex = Regex::new(r"<!(?:\[CDATA\[|ENTITY)").unwrap();
    static ref SVG_NS_TAG_PAT: Regex = Regex::new(r"<[a-zA-Z0-9-]+:svg").unwrap();
}

/// Port of `EntityDeclarationProcessor`: finds `<!ENTITY name "value">`
/// declarations in `html` and substitutes `&name;` references with
/// `value` throughout the document.
pub struct EntityDeclarationProcessor {
    pub declared_entities: HashMap<String, String>,
    pub processed_html: String,
}

impl EntityDeclarationProcessor {
    pub fn new(html: &str) -> Self {
        let mut declared_entities = HashMap::new();
        for caps in ENTITY_DECL_PAT.captures_iter(html) {
            let tokens: Vec<&str> = caps[1].split_whitespace().collect();
            if tokens.len() > 1 {
                declared_entities.insert(
                    tokens[0].trim().to_string(),
                    tokens[1].trim().replace('"', ""),
                );
            }
        }
        let mut processed_html = html.to_string();
        for (key, val) in &declared_entities {
            processed_html = processed_html.replace(&format!("&{key};"), val);
        }
        EntityDeclarationProcessor {
            declared_entities,
            processed_html,
        }
    }
}

fn self_closing_sub(caps: &Captures) -> String {
    let tag = &caps[1];
    if tag.to_lowercase().trim() == "br" {
        caps[0].to_string()
    } else {
        format!("<{}{}></{}>", &caps[1], &caps[2], &caps[1])
    }
}

/// Port of `cleanup_html`: resolves declared entities, then closes any
/// self-closing non-`<br/>` tag (`<div/>` -> `<div></div>`) since
/// WebKit's HTML parser treats an XML-style self-close on a non-void
/// element as unclosed.
pub fn cleanup_html(html: &str) -> String {
    let html = EntityDeclarationProcessor::new(html).processed_html;
    SELF_CLOSING_PAT
        .replace_all(&html, self_closing_sub)
        .into_owned()
}

/// Port of `load_as_html`: `true` if `html` should be loaded as
/// permissive HTML rather than strict XML — `false` when it contains a
/// namespace-qualified `<ns:svg>` tag or a `<!ENTITY`/`<![CDATA[`
/// declaration, both of which need XML parsing to render correctly.
pub fn load_as_html(html: &str) -> bool {
    SVG_NS_TAG_PAT.find(html).is_none() && XML_DETECT_PAT.find(html).is_none()
}

/// Where [`load_html`] delivers prepared content. Implemented by
/// whatever actual browser/webview widget the caller owns (this crate
/// has no GUI dependency of its own).
pub trait WebViewSink {
    /// Load `html` (already cleaned up) as permissive HTML, with
    /// `base_url` as the document's base for resolving relative links.
    fn set_html(&mut self, html: &str, base_url: &str);

    /// Load `html` as `mime_type` (e.g. `application/xhtml+xml`) with
    /// strict XML parsing, `base_url` as the document's base.
    ///
    /// Python's `load_html` follows this call with a WebKit-specific
    /// check —
    /// `view.page().mainFrame().findFirstElement('parsererror')` — to
    /// detect whether `QWebFrame`'s XML parser rejected the document
    /// (WebKit renders a `<parsererror>` element on failure) and
    /// reports that back to the caller as a `bool`. No webview binding
    /// in this workspace (`wry`, which backs the viewer prototype in
    /// `app-iced-prototype`) exposes an equivalent "did XML parsing
    /// fail" signal — it either renders the document or shows the
    /// browser engine's own error page, with no programmatic hook. If a
    /// future webview binding *does* expose this, wire it up here and
    /// have [`load_html`] surface it instead of always returning `true`
    /// for the XML path.
    fn set_content(&mut self, html: &str, mime_type: &str, base_url: &str);
}

/// Port of `load_html`. Reads `path` (unless `html_source` is given, in
/// which case that's used as the markup directly — Python's
/// `path_is_html` case), runs [`cleanup_html`], picks HTML vs. XML
/// loading via [`load_as_html`] (unless `force_as_html` overrides it),
/// and hands the result to `sink`.
///
/// Returns `Ok(true)` on success. Python can also return `False` (not
/// raise) when the WebKit XML parser reports a `parsererror`; see
/// [`WebViewSink::set_content`] for why that signal isn't available
/// here — this always returns `Ok(true)` once content is handed off.
#[allow(clippy::too_many_arguments)]
pub fn load_html(
    path: &Path,
    sink: &mut dyn WebViewSink,
    html_source: Option<&str>,
    codec: &str,
    mime_type: Option<&str>,
    force_as_html: bool,
    loading_url: Option<&str>,
) -> Result<bool> {
    let raw_html = match html_source {
        Some(html) => html.to_string(),
        None => {
            let bytes = fs::read(path).with_context(|| format!("Failed to read {:?}", path))?;
            decode_with_codec(&bytes, codec)
        }
    };

    let mime_type = mime_type.map(|m| m.to_string()).unwrap_or_else(|| {
        mime_guess::from_path(path)
            .first()
            .map(|m| m.to_string())
            .unwrap_or_else(|| "text/html".to_string())
    });

    let html = cleanup_html(&raw_html);
    let url = loading_url
        .map(|u| u.to_string())
        .unwrap_or_else(|| format!("file://{}", path.display()));

    if force_as_html || load_as_html(&html) {
        sink.set_html(&html, &url);
    } else {
        sink.set_content(&html, &mime_type, &url);
    }

    Ok(true)
}

fn decode_with_codec(bytes: &[u8], codec: &str) -> String {
    match codec.to_lowercase().as_str() {
        "utf-8" | "utf8" => String::from_utf8_lossy(bytes).into_owned(),
        "cp1252" | "windows-1252" => {
            let (cow, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
            cow.into_owned()
        }
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_declaration_processor_substitutes_declared_entities() {
        // The declaration line itself is left in place -- only `&foo;`
        // references are substituted (matches Python: `processed_html`
        // starts as the full original string, then only `&key;` gets
        // replaced, the `<!ENTITY ...>` text is never stripped).
        let html = r#"<!ENTITY foo "bar">Hello &foo; world"#;
        let proc = EntityDeclarationProcessor::new(html);
        assert_eq!(proc.declared_entities.get("foo"), Some(&"bar".to_string()));
        assert_eq!(proc.processed_html, r#"<!ENTITY foo "bar">Hello bar world"#);
    }

    #[test]
    fn entity_declaration_processor_ignores_malformed_declarations() {
        let html = "<!ENTITY solo>text";
        let proc = EntityDeclarationProcessor::new(html);
        assert!(proc.declared_entities.is_empty());
        assert_eq!(proc.processed_html, html);
    }

    #[test]
    fn cleanup_html_closes_self_closing_non_br_tags() {
        let html = "<div/>text<span class=\"x\" />";
        let out = cleanup_html(html);
        assert_eq!(out, "<div></div>text<span class=\"x\" ></span>");
    }

    #[test]
    fn cleanup_html_leaves_br_self_closed() {
        let html = "line1<br/>line2<BR />end";
        let out = cleanup_html(html);
        assert_eq!(out, "line1<br/>line2<BR />end");
    }

    #[test]
    fn cleanup_html_resolves_entities_before_closing_tags() {
        // Entity substitution runs first, and the self-closing-tag pass
        // then scans the *whole* resulting string -- so a substituted
        // value's own self-closing tags get closed too, same as any
        // other tag in the document (matches Python: one `.replace`
        // pass, then one `self_closing_pat.sub` pass over the result).
        let html = r#"<!ENTITY x "text">before &x; after<img/>"#;
        let out = cleanup_html(html);
        assert_eq!(out, r#"<!ENTITY x "text">before text after<img></img>"#);
    }

    #[test]
    fn load_as_html_true_for_plain_html() {
        assert!(load_as_html("<html><body>hi</body></html>"));
    }

    #[test]
    fn load_as_html_false_for_namespaced_svg() {
        assert!(!load_as_html(r#"<html><ns:svg></ns:svg></html>"#));
    }

    #[test]
    fn load_as_html_false_for_entity_declaration() {
        assert!(!load_as_html(r#"<!ENTITY foo "bar">"#));
    }

    #[test]
    fn load_as_html_false_for_cdata() {
        assert!(!load_as_html("<![CDATA[ raw ]]>"));
    }

    struct RecordingSink {
        html_calls: Vec<(String, String)>,
        content_calls: Vec<(String, String, String)>,
    }

    impl RecordingSink {
        fn new() -> Self {
            RecordingSink {
                html_calls: Vec::new(),
                content_calls: Vec::new(),
            }
        }
    }

    impl WebViewSink for RecordingSink {
        fn set_html(&mut self, html: &str, base_url: &str) {
            self.html_calls
                .push((html.to_string(), base_url.to_string()));
        }

        fn set_content(&mut self, html: &str, mime_type: &str, base_url: &str) {
            self.content_calls.push((
                html.to_string(),
                mime_type.to_string(),
                base_url.to_string(),
            ));
        }
    }

    #[test]
    fn load_html_from_source_routes_plain_html_to_set_html() {
        let mut sink = RecordingSink::new();
        let ok = load_html(
            Path::new("index.html"),
            &mut sink,
            Some("<p>hello</p>"),
            "utf-8",
            None,
            false,
            Some("file:///tmp/index.html"),
        )
        .unwrap();
        assert!(ok);
        assert_eq!(sink.html_calls.len(), 1);
        assert!(sink.content_calls.is_empty());
        assert_eq!(sink.html_calls[0].0, "<p>hello</p>");
        assert_eq!(sink.html_calls[0].1, "file:///tmp/index.html");
    }

    #[test]
    fn load_html_routes_xml_bearing_markup_to_set_content() {
        let mut sink = RecordingSink::new();
        load_html(
            Path::new("index.xhtml"),
            &mut sink,
            Some(r#"<!ENTITY x "y"><p>&x;</p>"#),
            "utf-8",
            Some("application/xhtml+xml"),
            false,
            Some("file:///tmp/index.xhtml"),
        )
        .unwrap();
        assert!(sink.html_calls.is_empty());
        assert_eq!(sink.content_calls.len(), 1);
        assert_eq!(sink.content_calls[0].1, "application/xhtml+xml");
    }

    #[test]
    fn load_html_force_as_html_overrides_xml_detection() {
        let mut sink = RecordingSink::new();
        load_html(
            Path::new("index.xhtml"),
            &mut sink,
            Some(r#"<!ENTITY x "y"><p>&x;</p>"#),
            "utf-8",
            None,
            true,
            None,
        )
        .unwrap();
        assert_eq!(sink.html_calls.len(), 1);
        assert!(sink.content_calls.is_empty());
    }

    #[test]
    fn load_html_reads_from_disk_when_no_source_given() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.html");
        std::fs::write(&path, "<div/>text").unwrap();

        let mut sink = RecordingSink::new();
        load_html(&path, &mut sink, None, "utf-8", None, false, None).unwrap();
        assert_eq!(sink.html_calls[0].0, "<div></div>text");
    }

    #[test]
    fn load_html_errors_on_missing_file() {
        let mut sink = RecordingSink::new();
        let err = load_html(
            Path::new("/nonexistent/path/does-not-exist.html"),
            &mut sink,
            None,
            "utf-8",
            None,
            false,
            None,
        );
        assert!(err.is_err());
    }
}
