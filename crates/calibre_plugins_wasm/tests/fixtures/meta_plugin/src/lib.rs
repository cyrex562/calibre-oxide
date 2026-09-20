//! A realistic third-party metadata-source plugin (issue #800).
//!
//! Shaped like a plugin an outside author would really write: it
//! declares the host it needs, calls out through the host's guarded
//! HTTP function, and maps the response into candidates.

use extism_pdk::*;

#[host_fn]
extern "ExtismHost" {
    /// The host's SSRF-guarded HTTP getter. A plugin has no ambient
    /// network; this is the only way out, and every call is checked
    /// against the plugin's own declared `allowed_hosts`.
    fn calibre_http_get(url: String) -> String;
}

/// Searches, via the host's guarded HTTP function.
///
/// The host passes a JSON query and expects a JSON array of
/// candidates back.
#[plugin_fn]
pub fn search_metadata(input: String) -> FnResult<String> {
    let query: serde_json::Value = serde_json::from_str(&input)?;
    let title = query.get("title").and_then(|v| v.as_str()).unwrap_or("");

    let url = format!("http://metadata.test.invalid/search?q={title}");
    let body = unsafe { calibre_http_get(url)? };

    // The host reports a denied or failed request as an `ERROR:` body,
    // so a well-behaved plugin returns no candidates rather than
    // fabricating them.
    if body.starts_with("ERROR:") {
        return Ok("[]".to_string());
    }

    let candidates = serde_json::json!([{
        "source": "ignored -- the host overwrites this",
        "title": title,
        "authors": ["From The Network"],
        "description": body.trim(),
        "identifiers": {"isbn": "9780000000001"},
    }]);
    Ok(candidates.to_string())
}

/// Searches without any network access at all, to show a plugin that
/// needs no capabilities still works.
#[plugin_fn]
pub fn search_offline(input: String) -> FnResult<String> {
    let query: serde_json::Value = serde_json::from_str(&input)?;
    let title = query.get("title").and_then(|v| v.as_str()).unwrap_or("");
    Ok(serde_json::json!([{"title": title, "authors": ["Offline Author"]}]).to_string())
}
