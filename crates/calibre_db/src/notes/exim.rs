//! Port of `old_src/src/calibre/db/notes/exim.py` (issue #228, a
//! #201 follow-up): converting a note's stored HTML between its
//! on-disk form (`<img src="resource://scheme/digest">` placeholders
//! pointing at [`crate::notes::connection::NotesConnection`]
//! resources) and a self-contained form suitable for export/import
//! (real image bytes, either inline `data:` URLs or local files).
//!
//! Originally stubbed with the explicit comment "we lack full HTML
//! parsing capabilities" -- that's no longer true:
//! `calibre_ebooks::mobi::dom::Dom` (an html5ever-backed, mutable DOM
//! tree, built for the MOBI reader's own lxml-shaped HTML mutation
//! needs) is reused here exactly the way
//! `calibre_ebooks::oeb::transforms::data_url` already uses it for a
//! near-identical img-src-rewriting job.
//!
//! # Scope of this pass
//!
//! Real, matching `exim.py`: [`export_note`] walks every `<img>`,
//! expands a `resource://` placeholder into a real `data:` URL (via
//! [`calibre_ebooks::oeb::transforms::rasterize::data_url`], MIME
//! type guessed from the resource's stored name). [`import_note`]
//! walks every `<img>` and, for a base64 `data:` URL or a local file
//! path (relative to `basedir`, or absolute, with the same real
//! path-traversal containment check upstream has -- a resolved path
//! must stay inside `basedir`), stores the real bytes via
//! `add_resource` and rewrites `src` to a `resource://` placeholder;
//! anything else (e.g. a remote `http(s)://` URL, or a `data:` URL
//! that isn't base64) is left untouched. Returns the rewritten HTML,
//! a plain-text rendering (via `calibre_utils::html2text`, #201-era
//! port), and the set of resource hashes actually used.
//!
//! # Disclosed simplifications
//!
//! - Upstream's `import_note` builds and then throws away a
//!   `del img.attrib['src']` pass over its in-memory tree *after*
//!   already serializing the returned HTML string -- that loop has no
//!   effect on either return value (the serialized string and the
//!   plaintext render both already reflect the tree as it stood right
//!   after the main per-`<img>` loop). This port skips reproducing
//!   that dead code; the observable behavior -- an unhandled `<img>`
//!   keeps its original `src` in the returned HTML -- is the same.
//! - `html2text` here has no `default_image_alt` parameter (unlike
//!   upstream's `html2text(shtml, default_image_alt=' ')`) -- this
//!   crate's `calibre_utils::html2text::html2text` doesn't expose one;
//!   image alt-text rendering falls back to whatever the underlying
//!   `html2text` crate does by default.
//! - MIME-type-to-extension guessing (for a base64 `data:` URL image
//!   with no `data-filename` hint) uses `mime_guess`, not upstream's
//!   `guess_extension` table -- the established boundary call this
//!   crate already uses elsewhere for the reverse direction
//!   (`guess_type`, see `oeb::polish::utils`).

use crate::constants::RESOURCE_URL_SCHEME;
use base64::Engine;
use calibre_ebooks::mobi::dom::Dom;
use calibre_ebooks::oeb::polish::utils::guess_type;
use calibre_ebooks::oeb::transforms::rasterize::data_url;
use calibre_utils::html2text::html2text;
use std::collections::HashSet;
use std::path::Path;

/// The bytes and stored filename of a resource -- what
/// [`export_note`]'s `get_resource` callback needs to hand back for
/// each `resource://` placeholder it's asked to resolve.
pub struct ExportResource {
    pub name: String,
    pub data: Vec<u8>,
}

/// Port of `export_note`. `get_resource` is
/// [`crate::notes::connection::NotesConnection::get_resource_data`]
/// (or a test double), keyed by the same `"scheme:digest"` hash
/// string [`crate::notes::connection::NotesConnection::add_resource`]
/// returns.
pub fn export_note(
    note_doc: &str,
    get_resource: impl Fn(&str) -> Option<ExportResource>,
) -> String {
    let mut dom = Dom::parse(note_doc);
    for img in dom.find_all_tag_global("img") {
        dom.node_mut(img).attrs.shift_remove("data-pre-import-src");
        let Some(src) = dom.node(img).attrs.get("src").cloned() else {
            continue;
        };
        let Some(rhash) = parse_resource_url(&src) else {
            continue;
        };
        if let Some(res) = get_resource(&rhash) {
            let mime = guess_type(&res.name);
            let url = data_url(&mime, &res.data);
            let node = dom.node_mut(img);
            node.attrs.insert("src".to_string(), url);
            node.attrs.insert("data-filename".to_string(), res.name);
        }
    }
    dom.serialize(dom.root)
}

/// Parses a `resource://<scheme>/<digest>` URL into this crate's
/// `"<scheme>:<digest>"` resource-hash string, or `None` if `src`
/// isn't one.
fn parse_resource_url(src: &str) -> Option<String> {
    let rest = src.strip_prefix(&format!("{RESOURCE_URL_SCHEME}://"))?;
    let (scheme, digest) = rest.split_once('/')?;
    Some(format!("{scheme}:{digest}"))
}

/// Port of `import_note`. `add_resource` is
/// [`crate::notes::connection::NotesConnection::add_resource`] (or a
/// test double); `basedir` is the directory local `<img src="...">`
/// paths are resolved relative to (matching upstream, a resolved path
/// that escapes `basedir` is rejected rather than read).
pub fn import_note(
    shtml: &str,
    basedir: &Path,
    mut add_resource: impl FnMut(&[u8], &str) -> String,
) -> (String, String, HashSet<String>) {
    let mut dom = Dom::parse(shtml);
    let mut resources = HashSet::new();

    for img in dom.find_all_tag_global("img") {
        let Some(src) = dom.node(img).attrs.get("src").cloned() else {
            continue;
        };
        dom.node_mut(img)
            .attrs
            .insert("data-pre-import-src".to_string(), src.clone());

        let stored = if let Some(rest) = src.strip_prefix("data:") {
            decode_data_url(rest, dom.node(img).attrs.get("data-filename").cloned())
        } else {
            resolve_local_file(&src, basedir)
        };

        if let Some((bytes, name)) = stored {
            let rhash = add_resource(&bytes, &name);
            let (scheme, digest) = rhash.split_once(':').unwrap_or(("raw", rhash.as_str()));
            let node = dom.node_mut(img);
            node.attrs.insert(
                "src".to_string(),
                format!("{RESOURCE_URL_SCHEME}://{scheme}/{digest}"),
            );
            resources.insert(rhash);
        }
    }

    let doc = dom.serialize(dom.root);
    let text = html2text(&doc);
    (doc, text, resources)
}

/// `data:<mimetype>;base64,<payload>` -- anything else (no `;base64`
/// marker, or undecodable payload) is rejected, matching upstream.
fn decode_data_url(rest: &str, data_filename: Option<String>) -> Option<(Vec<u8>, String)> {
    let (menc, payload) = rest.split_once(',')?;
    let (mime, enc) = menc.split_once(';').unwrap_or((menc, ""));
    if enc != "base64" {
        return None;
    }
    let cleaned: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cleaned)
        .ok()?;
    let name = data_filename.unwrap_or_else(|| {
        let ext = mime_guess::get_mime_extensions_str(mime)
            .and_then(|exts| exts.first())
            .copied()
            .unwrap_or("bin");
        format!("image.{ext}")
    });
    Some((bytes, name))
}

/// Resolves `src` (empty/`file` scheme only -- a real `http(s)://` or
/// other remote scheme is left alone) against `basedir`, rejecting any
/// path that escapes it once canonicalized (upstream's
/// `q.startswith(basedir)` check).
fn resolve_local_file(src: &str, basedir: &Path) -> Option<(Vec<u8>, String)> {
    if let Some(idx) = src.find("://") {
        let scheme = &src[..idx];
        if !scheme.is_empty() && !scheme.eq_ignore_ascii_case("file") {
            return None;
        }
    }
    let raw_path = src.strip_prefix("file://").unwrap_or(src);
    let decoded = urlencoding::decode(raw_path).ok()?.into_owned();
    let candidate = Path::new(&decoded);
    let resolved = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        basedir.join(candidate)
    };

    let canonical_base = basedir.canonicalize().ok()?;
    let canonical = resolved.canonicalize().ok()?;
    if !canonical.starts_with(&canonical_base) {
        return None;
    }

    let bytes = std::fs::read(&canonical).ok()?;
    let name = canonical.file_name()?.to_string_lossy().into_owned();
    Some((bytes, name))
}
