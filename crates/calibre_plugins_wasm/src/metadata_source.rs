//! The metadata-source ABI (issue #800) — the first plugin type that
//! **cannot work without a host function**.
//!
//! # Why this type is the one that proves the capability model
//!
//! A sandboxed plugin has no ambient network (see
//! [`crate::host`]'s `with_wasi(false)`), but a metadata source's
//! entire job is to make HTTP calls. Every other plugin type so far
//! computes over bytes it was handed; this one has to reach outside,
//! which makes it the real test of whether "default deny, explicit
//! grant" actually holds.
//!
//! Upstream treats metadata sources as uniquely churn-prone:
//! `customize/ui.py` has a `patch_metadata_plugins()` path that exists
//! *solely* so `Source` plugins can be hot-updated ahead of the
//! release cycle — special treatment given to no other plugin type.
//! Third parties replace these often, so they are a strong candidate
//! for being user-installable.
//!
//! # Why the plugin does NOT get Extism's built-in HTTP
//!
//! Extism can grant HTTP via `allowed_hosts`, but that is **hostname
//! matching only**. A permitted hostname whose DNS resolves to an
//! internal address would sail straight through — exactly the
//! SSRF-via-DNS hole that `calibre_srv`'s own routes guard against,
//! and a close relative of the redirect-TOCTOU bug fixed in #792.
//!
//! So this module supplies its own [`HTTP_GET_EXPORT`] host function,
//! and every request through it is checked **per request**:
//!
//! 1. scheme must be http/https;
//! 2. the host must appear in the plugin's declared `allowed_hosts` —
//!    an empty list therefore denies everything;
//! 3. the host is DNS-resolved and every resulting address checked
//!    against the shared SSRF policy
//!    ([`calibre_utils::net_guard`], the same policy `calibre_srv`
//!    uses);
//! 4. redirects are **not** followed — a redirect would re-open the
//!    same TOCTOU hole, and a metadata API that needs one can be
//!    followed by the plugin itself, which puts each hop back through
//!    this check.
//!
//! # The search ABI
//!
//! A plugin exports [`SEARCH_EXPORT`], receiving a JSON
//! [`MetadataQuery`] and returning a JSON array of
//! [`CandidateDto`] — the wire form of
//! `calibre_ebooks::metadata::sources::MetadataCandidate`.
//!
//! That struct is deliberately *not* imported here: this crate must
//! not depend on `calibre_ebooks` (it would drag a WASM runtime into
//! the ebook crate's dependents, the very thing #798's crate split
//! avoids). The DTO is a structural mirror, and
//! `calibre_srv` — which depends on both — converts. A test there
//! pins the two shapes together so they cannot drift silently.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::host::{LoadedPlugin, PluginPackage, WasmPluginError};
use crate::manifest::PluginType;

/// The export a metadata-source plugin must provide.
pub const SEARCH_EXPORT: &str = "search_metadata";

/// The host function this crate provides to metadata-source plugins.
pub const HTTP_GET_EXPORT: &str = "calibre_http_get";

/// What the host asks a metadata source to look up.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataQuery {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub authors: Option<String>,
    #[serde(default)]
    pub isbn: Option<String>,
}

/// Wire form of `calibre_ebooks::metadata::sources::MetadataCandidate`.
/// See the module doc for why it is mirrored rather than imported.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CandidateDto {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(default)]
    pub pubdate: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub identifiers: BTreeMap<String, String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub cover_url: Option<String>,
    #[serde(default)]
    pub rating: Option<f64>,
}

#[derive(Debug, thiserror::Error)]
pub enum MetadataAbiError {
    #[error("plugin {name:?} is a {actual:?} plugin, not a metadata-source plugin")]
    WrongPluginType { name: String, actual: PluginType },
    #[error(transparent)]
    Wasm(#[from] WasmPluginError),
    #[error("plugin {name:?} returned a malformed candidate list: {reason}")]
    MalformedResult { name: String, reason: String },
}

/// A loaded WASM metadata-source plugin.
pub struct WasmMetadataSource {
    name: String,
    plugin: Mutex<LoadedPlugin>,
}

impl WasmMetadataSource {
    pub fn load(package: &PluginPackage) -> Result<WasmMetadataSource, MetadataAbiError> {
        if package.manifest.plugin_type != PluginType::MetadataSource {
            return Err(MetadataAbiError::WrongPluginType { name: package.manifest.name.clone(), actual: package.manifest.plugin_type });
        }
        let loaded = package.load_with_host_functions(http_host_functions(&package.manifest.capabilities.allowed_hosts))?;
        Ok(WasmMetadataSource { name: package.manifest.name.clone(), plugin: Mutex::new(loaded) })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Runs the plugin's search and returns its candidates.
    ///
    /// The plugin's own `source` field is overwritten with the
    /// installed plugin's name, so a plugin cannot claim its results
    /// came from a different (e.g. more trusted-looking) source than
    /// the one the user installed.
    pub fn search(&self, query: &MetadataQuery) -> Result<Vec<CandidateDto>, MetadataAbiError> {
        let input = serde_json::to_vec(query).expect("MetadataQuery always serializes");
        let raw = {
            let mut guard = self.plugin.lock().expect("plugin mutex poisoned by a previous panic");
            guard.call(SEARCH_EXPORT, &input)?
        };

        let mut candidates: Vec<CandidateDto> =
            serde_json::from_slice(&raw).map_err(|e| MetadataAbiError::MalformedResult { name: self.name.clone(), reason: e.to_string() })?;
        for c in &mut candidates {
            c.source = self.name.clone();
        }
        Ok(candidates)
    }
}

/// Builds the guarded HTTP host function, closed over the plugin's own
/// declared `allowed_hosts`.
fn http_host_functions(allowed_hosts: &[String]) -> Vec<extism::Function> {
    let allowed: Vec<String> = allowed_hosts.to_vec();

    let f = extism::Function::new(
        HTTP_GET_EXPORT,
        [extism::PTR],
        [extism::PTR],
        extism::UserData::new(allowed),
        |plugin, inputs, outputs, user_data: extism::UserData<Vec<String>>| {
            let url: String = plugin.memory_get_val(&inputs[0])?;
            let allowed = user_data.get()?;
            let allowed = allowed.lock().expect("allowed-hosts lock poisoned");

            // A denied or failed request is surfaced to the plugin as an
            // `ERROR: ...` body rather than a host-side trap: the plugin
            // should be able to try another source or return no results,
            // exactly as it would for an HTTP error from a real server.
            let body = guarded_http_get(&url, &allowed).unwrap_or_else(|e| format!("ERROR: {e}").into_bytes());
            let handle = plugin.memory_new(body)?;
            outputs[0] = extism::Val::I64(handle.offset() as i64);
            Ok(())
        },
    );
    vec![f]
}

/// The whole point of this module: every plugin HTTP request is
/// checked here, per request. Public so it can be tested directly
/// against a real server -- it is the security-critical function in
/// this crate and deserves direct coverage, not only coverage through
/// a plugin. See the module doc for the four checks
/// and why each exists.
pub fn guarded_http_get(url: &str, allowed_hosts: &[String]) -> Result<Vec<u8>, String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("{url}: invalid URL ({e})"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!("{url}: only http/https is allowed"));
    }
    let host = parsed.host_str().ok_or_else(|| format!("{url}: no host"))?.to_string();

    if !host_is_allowed(&host, allowed_hosts) {
        return Err(format!("{host}: not in this plugin's declared allowed_hosts"));
    }

    let port = parsed.port_or_known_default().unwrap_or(443);
    calibre_utils::net_guard::resolve_and_check_blocking_with(&host, port, super::loopback_aware_disallowed_ip)?;

    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("upstream returned HTTP {}", resp.status().as_u16()));
    }
    Ok(resp.bytes().map_err(|e| e.to_string())?.to_vec())
}

/// Exact host match, or a `*.example.com` suffix match.
///
/// Deliberately does **not** support a bare `*`: a plugin that could
/// declare "any host" would make the allowlist meaningless, and the
/// management UI (#801) could not show the user anything useful about
/// what it wants to reach.
fn host_is_allowed(host: &str, allowed: &[String]) -> bool {
    let host = host.to_lowercase();
    allowed.iter().any(|pattern| {
        let pattern = pattern.trim().to_lowercase();
        if pattern == "*" || pattern.is_empty() {
            return false;
        }
        match pattern.strip_prefix("*.") {
            Some(suffix) => host == suffix || host.ends_with(&format!(".{suffix}")),
            None => host == pattern,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_allowlist_denies_everything() {
        assert!(!host_is_allowed("example.com", &[]));
    }

    #[test]
    fn an_exact_host_matches_only_itself() {
        let allowed = vec!["www.googleapis.com".to_string()];
        assert!(host_is_allowed("www.googleapis.com", &allowed));
        assert!(host_is_allowed("WWW.GOOGLEAPIS.COM", &allowed), "matching must be case-insensitive");
        assert!(!host_is_allowed("googleapis.com", &allowed));
        assert!(!host_is_allowed("evil.com", &allowed));
    }

    #[test]
    fn a_wildcard_suffix_matches_subdomains_but_not_a_lookalike() {
        let allowed = vec!["*.openlibrary.org".to_string()];
        assert!(host_is_allowed("covers.openlibrary.org", &allowed));
        assert!(host_is_allowed("openlibrary.org", &allowed));
        // The classic bypass: a domain that merely *ends with* the text.
        assert!(!host_is_allowed("evil-openlibrary.org", &allowed));
        assert!(!host_is_allowed("openlibrary.org.evil.com", &allowed));
    }

    #[test]
    fn a_bare_wildcard_is_refused_rather_than_allowing_everything() {
        assert!(!host_is_allowed("anything.com", &["*".to_string()]));
    }

    #[test]
    fn a_non_http_scheme_is_refused_before_any_request() {
        let err = guarded_http_get("file:///etc/passwd", &["*.anything".to_string()]).unwrap_err();
        assert!(err.contains("only http/https"), "{err}");
    }

    #[test]
    fn a_host_outside_the_allowlist_is_refused_before_dns() {
        let err = guarded_http_get("https://evil.example/", &["www.googleapis.com".to_string()]).unwrap_err();
        assert!(err.contains("allowed_hosts"), "{err}");
    }

    #[test]
    fn the_cloud_metadata_address_is_refused_even_if_somehow_allowlisted() {
        // Defence in depth: the allowlist is author-declared, so the
        // SSRF policy must still apply after it passes.
        let err = guarded_http_get("http://169.254.169.254/latest/meta-data/", &["169.254.169.254".to_string()]).unwrap_err();
        assert!(err.contains("disallowed address"), "{err}");
    }

    #[test]
    fn a_query_round_trips_through_json() {
        let q = MetadataQuery { title: Some("Dune".into()), authors: Some("Frank Herbert".into()), isbn: None };
        let back: MetadataQuery = serde_json::from_slice(&serde_json::to_vec(&q).unwrap()).unwrap();
        assert_eq!(back, q);
    }

    #[test]
    fn a_candidate_dto_tolerates_a_plugin_sending_only_some_fields() {
        // A third-party plugin must not have to fill in every field to
        // return a usable result.
        let c: CandidateDto = serde_json::from_str(r#"{"title": "Dune", "authors": ["Frank Herbert"]}"#).unwrap();
        assert_eq!(c.title.as_deref(), Some("Dune"));
        assert!(c.tags.is_empty());
        assert_eq!(c.rating, None);
    }
}
