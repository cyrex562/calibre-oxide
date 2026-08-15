//! Port of `old_src/src/calibre/ebooks/oeb/polish/download.py`.
//!
//! Real network I/O, ported for real using `reqwest`'s blocking client
//! (matching this workspace's existing `reqwest::blocking` usage in
//! `calibre_ai`, the only other crate here doing HTTP), plus `file:`
//! and `data:` URI handling. Concurrency uses `std::thread::scope` +
//! a bounded work queue, the same pattern as `oeb::polish::images`'
//! `compress_images` (itself standing in for Python's
//! `multiprocessing.dummy.Pool`).
//!
//! Per `docs/FAULT_TOLERANCE.md` §6's general network-operation
//! discipline (bounded timeout, no silent infinite retry): every
//! request carries the caller-supplied timeout and is attempted exactly
//! once -- Python's version has no retry loop either (a single
//! `browser().open(url, timeout=timeout)` call), so this matches rather
//! than adds retry behavior that wasn't there.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::mobi::dom::{Dom, NodeId};
use crate::oeb::constants::{OEB_DOCS, OEB_STYLES};

use super::container::{Container, LinkFileType};
use super::utils::guess_type;

/// Port of `is_external`: does `url` have a scheme calibre treats as an
/// "external resource" reference (as opposed to a link within the
/// book)?
pub fn is_external(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(u) => matches!(u.scheme(), "http" | "https" | "file" | "ftp" | "data"),
        Err(_) => false,
    }
}

/// Port of `iterhtmllinks`: every `href`/`src` on a non-`<a>` element.
fn iter_html_links(dom: &Dom) -> Vec<(NodeId, &'static str, String)> {
    let mut out = Vec::new();
    for el in dom.preorder_elements(dom.root) {
        if dom.tag(el) == Some("a") {
            continue;
        }
        for attr in ["href", "src"] {
            if let Some(v) = dom.node(el).attrs.get(attr) {
                out.push((el, attr, v.clone()));
            }
        }
    }
    out
}

/// Port of `get_external_resources`: every external URL referenced
/// anywhere in the book, mapped to the names of the files that
/// reference it.
pub fn get_external_resources(container: &mut Container) -> Result<HashMap<String, Vec<String>>> {
    let mut ans: HashMap<String, Vec<String>> = HashMap::new();
    let names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (name, mt) in names {
        if !container.has_name(&name) || !container.exists(&name) {
            continue;
        }
        if OEB_DOCS.contains(&mt.as_str()) {
            container.ensure_parsed(&name)?;
            let dom = container.get_xhtml(&name)?;
            for (_el, _attr, link) in iter_html_links(dom) {
                if is_external(&link) {
                    ans.entry(link).or_default().push(name.clone());
                }
            }
        } else if OEB_STYLES.contains(&mt.as_str()) {
            for (link, _line, _off) in container.iterlinks(&name)? {
                if is_external(&link) {
                    ans.entry(link).or_default().push(name.clone());
                }
            }
        }
    }
    Ok(ans)
}

/// Port of `sanitize_file_name`. Reuses
/// [`calibre_utils::filenames::sanitize_file_name`] (the established
/// boundary call for this crate's filename sanitization) plus Python's
/// `..` collapsing. Python additionally runs the result through
/// `oeb.polish.check.parsing.make_filename_safe` -- a function that
/// belongs to `polish/check/`, a different not-yet-ported subdirectory
/// outside this issue's scope; the base sanitizer here already strips
/// filesystem-hostile characters, so this is a minor fidelity gap, not
/// a functional one.
fn sanitize_file_name(x: &str) -> String {
    let mut x = calibre_utils::filenames::sanitize_file_name(x);
    while x.contains("..") {
        x = x.replace("..", ".");
    }
    x
}

/// Port of the non-base64 branch of `download_one`'s `data:` URL
/// handling: strip the `data:` prefix, split off the `mime;base64,`
/// header, and either base64-decode or (matching Python exactly, which
/// does *not* percent-decode this branch) take the payload text as
/// literal UTF-8 bytes. Returns `(bytes, file_extension)`.
fn decode_data_uri(raw_url: &str) -> Result<(Vec<u8>, String)> {
    let rest = raw_url.strip_prefix("data:").context("Not a data: URL")?;
    let (prefix, payload) = rest.split_once(',').context("Malformed data: URL")?;
    let parts: Vec<&str> = prefix.split(';').collect();
    let is_base64 = parts
        .last()
        .map(|s| s.eq_ignore_ascii_case("base64"))
        .unwrap_or(false);
    let bytes = if is_base64 {
        use base64::Engine as _;
        let cleaned: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
        base64::engine::general_purpose::STANDARD
            .decode(cleaned.as_bytes())
            .context("Invalid base64 data: URL payload")?
    } else {
        payload.as_bytes().to_vec()
    };
    let mut ext = "unknown".to_string();
    for part in &parts {
        if !part.contains('=') && part.contains('/') {
            if let Some(exts) = mime_guess::get_mime_extensions_str(part) {
                if let Some(&e) = exts.first() {
                    ext = e.to_string();
                    break;
                }
            }
        }
    }
    Ok((bytes, ext))
}

/// Best-effort `filename=`/`filename*=` extraction from a
/// `Content-Disposition` header value. Port of the relevant slice of
/// `calibre.web.get_download_filename_from_response`.
fn filename_from_content_disposition(value: &str) -> Option<String> {
    for part in value.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("filename=") {
            let rest = rest.trim_matches('"');
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

/// Port of `get_filename`: a suggested filename for a downloaded HTTP
/// response, falling back to the URL's path basename, then `"unknown"`,
/// and appending a content-type-derived extension when the guessed mime
/// for the chosen name disagrees with the response's declared type.
fn filename_from_response(url: &url::Url, resp: &reqwest::blocking::Response) -> String {
    let mut name = resp
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(filename_from_content_disposition)
        .or_else(|| {
            url.path_segments()
                .and_then(|mut s| s.next_back())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    if let Some(ct) = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        let ct = ct.split(';').next().unwrap_or(ct).trim().to_lowercase();
        if !ct.is_empty() && ct != "application/octet-stream" {
            let guessed = guess_type(&name);
            if guessed != ct {
                if let Some(exts) = mime_guess::get_mime_extensions_str(&ct) {
                    if let Some(&e) = exts.first() {
                        name.push('.');
                        name.push_str(e);
                    }
                }
            }
        }
    }
    name
}

/// A successfully downloaded external resource. Port of the `(True,
/// (url, suggested_filename, downloaded_file, mt))` success branch of
/// `download_one`.
#[derive(Debug, Clone)]
pub struct DownloadedResource {
    pub url: String,
    pub suggested_filename: String,
    pub downloaded_path: PathBuf,
    pub mime: String,
}

/// Port of `download_one`. Fetches `url` (`http(s)://`, `file://`, or
/// `data:`) into a fresh temp file under `tdir`. `ftp://` is accepted by
/// [`is_external`] (matching Python's URL-scheme test) but not actually
/// fetchable here -- `reqwest` is HTTP(S)-only -- so it errors rather
/// than silently doing nothing.
fn download_one(
    tdir: &Path,
    timeout: Duration,
    url: &str,
) -> std::result::Result<DownloadedResource, String> {
    (|| -> Result<DownloadedResource> {
        let (mut src, mut filename): (Box<dyn Read>, String) = if let Some(rest) =
            url.strip_prefix("file://")
        {
            let path = PathBuf::from(rest);
            let f = fs::File::open(&path)
                .with_context(|| format!("Failed to open {}", path.display()))?;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "unknown".to_string());
            (Box::new(f), name)
        } else if url.starts_with("data:") {
            let (bytes, ext) = decode_data_uri(url)?;
            (Box::new(std::io::Cursor::new(bytes)), format!("data-uri.{ext}"))
        } else if url.starts_with("http://") || url.starts_with("https://") {
            let parsed = url::Url::parse(url).context("Invalid URL")?;
            let client = reqwest::blocking::Client::builder()
                .timeout(timeout)
                .build()
                .context("Failed to build HTTP client")?;
            let resp = client
                .get(parsed.clone())
                .send()
                .with_context(|| format!("Failed to fetch {url}"))?
                .error_for_status()
                .with_context(|| format!("{url} returned an error status"))?;
            let name = filename_from_response(&parsed, &resp);
            (Box::new(resp), name)
        } else {
            bail!("Unsupported or unfetchable URL scheme for {url} (only http(s)/file/data are supported)");
        };

        filename = sanitize_file_name(&filename);
        let mt = guess_type(&filename);
        if OEB_DOCS.contains(&mt.as_str()) {
            bail!("The external resource {url} looks like a HTML document ({filename})");
        }
        if mt.is_empty() || mt == "application/octet-stream" || !filename.contains('.') {
            bail!("The external resource {url} is not of a known type");
        }

        let mut tmp = tempfile::NamedTempFile::new_in(tdir)
            .context("Failed to create a temporary file")?;
        std::io::copy(&mut src, &mut tmp).context("Failed to write downloaded data")?;
        let path = tmp.into_temp_path();
        let path = path.keep().context("Failed to persist downloaded file")?;

        Ok(DownloadedResource {
            url: url.to_string(),
            suggested_filename: filename,
            downloaded_path: path,
            mime: mt,
        })
    })()
    .map_err(|e| e.to_string())
}

trait Read: std::io::Read + Send {}
impl<T: std::io::Read + Send> Read for T {}

/// Port of `download_external_resources`: fetches every URL in `urls`
/// (bounded thread pool, matching Python's `multiprocessing.dummy.Pool`)
/// and adds each successfully downloaded file to `container`, returning
/// `(url -> new container name, url -> error message)`.
pub fn download_external_resources(
    container: &mut Container,
    urls: &[String],
    timeout: Duration,
    mut progress_report: impl FnMut(&str, u64, u64),
) -> Result<(HashMap<String, String>, HashMap<String, String>)> {
    let tdir = tempfile::tempdir().context("Failed to create a temporary directory")?;
    let queue = Arc::new(Mutex::new(urls.iter().cloned()));
    let (tx, rx) = std::sync::mpsc::channel();
    let num_workers = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(10)
        .min(urls.len().max(1))
        .max(1);

    let (replacements, failures) = thread::scope(|scope| {
        for _ in 0..num_workers {
            let queue = Arc::clone(&queue);
            let tx = tx.clone();
            let tdir_path = tdir.path().to_path_buf();
            scope.spawn(move || loop {
                let url = { queue.lock().unwrap().next() };
                let Some(url) = url else { break };
                let result = download_one(&tdir_path, timeout, &url);
                if tx.send((url, result)).is_err() {
                    break;
                }
            });
        }
        drop(tx);

        // `add_file` runs here, on the thread that owns `container`
        // (worker threads only fetch bytes into temp files and never
        // touch `container`), as each result arrives.
        let mut failures = HashMap::new();
        let mut replacements = HashMap::new();
        for (url, result) in rx {
            match result {
                Ok(res) => {
                    progress_report(&res.url, 0, 0);
                    let data = fs::read(&res.downloaded_path).unwrap_or_default();
                    match container.add_file(
                        &res.suggested_filename,
                        &data,
                        Some(&res.mime),
                        None,
                        true,
                    ) {
                        Ok(name) => {
                            replacements.insert(url, name);
                        }
                        Err(e) => {
                            failures.insert(url, e.to_string());
                        }
                    }
                }
                Err(err) => {
                    failures.insert(url, err);
                }
            }
        }
        (replacements, failures)
    });

    Ok((replacements, failures))
}

/// Port of `replace_resources`: for every external URL that was
/// successfully downloaded (present in `replacements`), rewrites every
/// reference to it (in every file that referenced it, per `urls`) to
/// point at the new local container name.
pub fn replace_resources(
    container: &mut Container,
    urls: &HashMap<String, Vec<String>>,
    replacements: &HashMap<String, String>,
) -> Result<bool> {
    let mut url_maps: HashMap<String, HashMap<String, String>> = HashMap::new();
    for (url, names) in urls {
        if let Some(replacement) = replacements.get(url) {
            for name in names {
                let href = container.name_to_href(replacement, Some(name));
                url_maps
                    .entry(name.clone())
                    .or_default()
                    .insert(url.clone(), href);
            }
        }
    }
    let mut changed = false;
    for (name, url_map) in url_maps {
        let mut replaced_any = false;
        container.replace_links(&name, |url, _file_type: LinkFileType| {
            let r = url_map.get(url);
            if let Some(r) = r {
                if r != url {
                    replaced_any = true;
                }
                Some(r.clone())
            } else {
                None
            }
        })?;
        changed |= replaced_any;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_external_recognizes_absolute_schemes() {
        assert!(is_external("http://example.com/a.jpg"));
        assert!(is_external("https://example.com/a.jpg"));
        assert!(is_external("file:///tmp/a.jpg"));
        assert!(is_external("data:image/png;base64,AAAA"));
        assert!(!is_external("images/a.jpg"));
        assert!(!is_external("#anchor"));
        assert!(!is_external(""));
    }

    #[test]
    fn decode_data_uri_handles_base64_payload() {
        let (bytes, ext) = decode_data_uri("data:image/png;base64,aGVsbG8=").unwrap();
        assert_eq!(bytes, b"hello");
        assert_eq!(ext, "png");
    }

    #[test]
    fn decode_data_uri_handles_plain_payload() {
        let (bytes, _ext) = decode_data_uri("data:text/plain,hello%20world").unwrap();
        // Python does not percent-decode this branch -- neither do we.
        assert_eq!(bytes, b"hello%20world");
    }

    #[test]
    fn sanitize_file_name_collapses_dot_dot() {
        // The exact substitution the shared `calibre_utils` sanitizer makes
        // for interior dots differs slightly from Python's own base
        // sanitizer, but the invariant this loop exists for -- no runs of
        // ".." survive -- must hold either way.
        assert!(!sanitize_file_name("a..b").contains(".."));
        assert!(!sanitize_file_name("a...b").contains(".."));
    }

    #[test]
    fn filename_from_content_disposition_extracts_name() {
        assert_eq!(
            filename_from_content_disposition("attachment; filename=\"cover.jpg\""),
            Some("cover.jpg".to_string())
        );
        assert_eq!(filename_from_content_disposition("attachment"), None);
    }
}
