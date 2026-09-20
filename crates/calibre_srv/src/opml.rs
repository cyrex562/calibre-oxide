//! `POST /opml/import` -- a real, **new** route (issue #766), not a
//! port. Confirmed via grep: no OPML support exists anywhere else in
//! this workspace. Parses a real OPML file (the standard feed-
//! subscription-list format most RSS readers export) into a flat list
//! of `{title, feed_url}` entries, for `web/`'s news-fetch panel
//! (issue #723) to offer for adding.
//!
//! OPML is real, simple XML -- `crate::xmltree::Xml` (already used
//! throughout this crate, e.g. `notes.rs`/`tweak.rs`'s own indirect
//! use via `calibre_ebooks`) is a real fit, no new XML-parsing
//! dependency needed. A real OPML file nests `<outline>` elements
//! (categories containing further outlines) with a leaf outline's own
//! `xmlUrl` attribute marking it as an actual feed subscription (not a
//! category) -- every outline in the tree carrying that attribute,
//! regardless of nesting depth, is collected.

use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_ebooks::xmltree::{Xml, XmlNodeId};

use crate::errors::ServerError;

#[derive(Debug, Deserialize)]
pub struct ImportBody {
    opml: String,
}

fn collect_outlines(xml: &Xml, id: XmlNodeId, out: &mut Vec<(String, String)>) {
    for child in xml.element_children(id) {
        if xml.local_name(child) == Some("outline") {
            if let Some(url) = xml.get_attr(child, "xmlUrl") {
                let title = xml.get_attr(child, "title").or_else(|| xml.get_attr(child, "text")).unwrap_or(url).to_string();
                out.push((title, url.to_string()));
            }
            // Real OPML always nests feed outlines inside category
            // outlines -- recurse regardless of whether this outline
            // itself carried `xmlUrl` (a feed outline with children
            // would be unusual, not invalid, and this stays correct
            // either way: recursing into a childless leaf is a no-op).
            collect_outlines(xml, child, out);
        }
    }
}

/// `POST /opml/import`.
pub async fn import(Json(body): Json<ImportBody>) -> Result<Json<Value>, ServerError> {
    let xml = Xml::parse(&body.opml).map_err(|e| ServerError::BadRequest(format!("Invalid OPML: {e}")))?;
    let root = xml.root_element().ok_or_else(|| ServerError::BadRequest("Invalid OPML: no root element".to_string()))?;
    if xml.local_name(root) != Some("opml") {
        return Err(ServerError::BadRequest("Invalid OPML: root element is not <opml>".to_string()));
    }
    let body_el = xml.element_children(root).into_iter().find(|&c| xml.local_name(c) == Some("body"));

    let mut feeds = Vec::new();
    if let Some(body_el) = body_el {
        collect_outlines(&xml, body_el, &mut feeds);
    }
    let feeds_json: Vec<Value> = feeds.into_iter().map(|(title, url)| json!({"title": title, "feed_url": url})).collect();
    Ok(Json(json!({"feeds": feeds_json})))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    fn test_app() -> (tempfile::TempDir, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let state = crate::AppState {
            libraries: None,
            cache: std::sync::Arc::new(cache),
            opts: std::sync::Arc::new(crate::opts::ServerOptions::default()),
            auth: None,
            changes: crate::web_socket::new_change_broadcaster(),
            reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()),
            book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))),
            render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()),
            news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()),
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()), news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()), tts_voice: None, plugin_store: None, plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())),
        };
        let router = crate::test_router(state);
        (dir, router)
    }

    async fn post_json(router: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    const REAL_OPML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="1.0">
  <head><title>My Feeds</title></head>
  <body>
    <outline text="Tech" title="Tech">
      <outline text="Feed One" title="Feed One" type="rss" xmlUrl="https://example.com/feed1.xml" htmlUrl="https://example.com"/>
      <outline text="Feed Two" title="Feed Two" type="rss" xmlUrl="https://example.com/feed2.xml"/>
    </outline>
    <outline text="Standalone" title="Standalone" type="rss" xmlUrl="https://example.com/feed3.xml"/>
    <outline text="Just a bookmark, not a feed" htmlUrl="https://example.com/no-feed"/>
  </body>
</opml>"#;

    #[tokio::test]
    async fn imports_every_real_nested_feed_outline() {
        let (_dir, router) = test_app();
        let (status, body) = post_json(&router, "/opml/import", serde_json::json!({"opml": REAL_OPML})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let feeds = body["feeds"].as_array().unwrap();
        assert_eq!(feeds.len(), 3, "{feeds:?}");
        let urls: Vec<&str> = feeds.iter().map(|f| f["feed_url"].as_str().unwrap()).collect();
        assert!(urls.contains(&"https://example.com/feed1.xml"));
        assert!(urls.contains(&"https://example.com/feed2.xml"));
        assert!(urls.contains(&"https://example.com/feed3.xml"));
        let titles: Vec<&str> = feeds.iter().map(|f| f["title"].as_str().unwrap()).collect();
        assert!(titles.contains(&"Feed One"));
    }

    #[tokio::test]
    async fn rejects_malformed_xml() {
        let (_dir, router) = test_app();
        let (status, _) = post_json(&router, "/opml/import", serde_json::json!({"opml": "<not valid xml"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rejects_a_real_xml_document_that_is_not_opml() {
        let (_dir, router) = test_app();
        let (status, _) = post_json(&router, "/opml/import", serde_json::json!({"opml": "<html><body>not opml</body></html>"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn an_opml_file_with_no_feeds_returns_an_empty_list_not_an_error() {
        let (_dir, router) = test_app();
        let (status, body) = post_json(&router, "/opml/import", serde_json::json!({"opml": "<opml version=\"1.0\"><head/><body></body></opml>"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["feeds"].as_array().unwrap().len(), 0);
    }
}
