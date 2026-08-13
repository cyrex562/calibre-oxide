//! Support for Microsoft LIT files.
//!
//! Port of `src/calibre/ebooks/lit/`. `LitError` here is the exception
//! defined in `lit/__init__.py`; the LZX codec and the LIT-flavoured DES
//! live in `calibre_utils`, mirroring `src/calibre/utils/{lzx,msdes}/`.

pub mod lzx;
pub mod maps;
pub mod mssha1;
pub mod reader;
pub mod writer;

/// `LitError` in `src/calibre/ebooks/lit/__init__.py`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LitError {
    /// A malformed or unsupported LIT file.
    #[error("{0}")]
    Lit(String),
    /// `DRMError` in `src/calibre/ebooks/__init__.py` — the book is
    /// protected and cannot be opened.
    #[error("this book is DRM protected: {0}")]
    Drm(String),
}

impl LitError {
    /// Build a [`LitError::Lit`] from anything printable.
    pub fn msg<S: Into<String>>(s: S) -> Self {
        LitError::Lit(s.into())
    }
}

/// Result alias for the LIT modules.
pub type Result<T> = std::result::Result<T, LitError>;

/// The characters `oeb.base.urlquote` leaves alone.
fn url_safe(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'/' | b'~')
}

/// Percent-decode a string, leaving invalid escapes alone.
///
/// `polyglot.urllib.unquote`, as used on manifest paths and container
/// names.
pub fn urlunquote(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &s[i + 1..i + 3];
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `oeb.base.urlquote` — quote URL-unsafe characters, allowing
/// IRI-safe (non-ASCII) ones through untouched.
pub fn urlquote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii() && !url_safe(ch as u8) {
            out.push_str(&format!("%{:02x}", ch as u32));
        } else {
            out.push(ch);
        }
    }
    out
}

/// The scheme at the start of `href`, if it has one.
///
/// `urlparse` only recognises a scheme made of a letter followed by
/// letters, digits, `+`, `-` or `.`, and only before the first `/`.
fn url_scheme(href: &str) -> Option<&str> {
    let colon = href.find(':')?;
    let scheme = &href[..colon];
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    Some(scheme)
}

/// Requote one URL component: normalize separators, decode what is
/// already escaped, then escape exactly what needs it.
fn requote(part: &str) -> String {
    urlquote(&urlunquote(&part.replace('\\', "/")))
}

/// `oeb.base.urlnormalize` — normalize a URL so that exactly the
/// URL-unsafe characters are quoted.
///
/// Components are requoted individually, so the delimiters that give a
/// URL its structure — the `:` after a scheme, the `//` before an
/// authority — survive rather than being escaped. Most hrefs the LIT
/// code passes through here are relative paths within the book, but
/// documents do carry absolute links out to the web.
pub fn urlnormalize(href: &str) -> String {
    let scheme = url_scheme(href).filter(|s| !s.eq_ignore_ascii_case("file"));
    let Some(scheme) = scheme else {
        // No scheme (or `file:`): path and fragment only, as
        // `urlnormalize` reduces those cases to.
        let (path, frag) = match href.split_once('#') {
            Some((p, f)) => (p, Some(f)),
            None => (href, None),
        };
        let mut out = requote(path);
        if let Some(f) = frag {
            out.push('#');
            out.push_str(&requote(f));
        }
        return out;
    };

    let rest = &href[scheme.len() + 1..];
    let mut out = format!("{scheme}:");
    let rest = match rest.strip_prefix("//") {
        Some(after) => {
            out.push_str("//");
            let end = after.find(['/', '?', '#']).unwrap_or(after.len());
            out.push_str(&requote(&after[..end]));
            &after[end..]
        }
        None => rest,
    };

    let (rest, frag) = match rest.split_once('#') {
        Some((r, f)) => (r, Some(f)),
        None => (rest, None),
    };
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    out.push_str(&requote(path));
    if let Some(q) = query {
        out.push('?');
        out.push_str(&requote(q));
    }
    if let Some(f) = frag {
        out.push('#');
        out.push_str(&requote(f));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquote_decodes_percent_escapes() {
        assert_eq!(urlunquote("a%20b"), "a b");
        assert_eq!(urlunquote("100%25"), "100%");
        assert_eq!(urlunquote("plain"), "plain");
    }

    #[test]
    fn unquote_leaves_malformed_escapes_alone() {
        assert_eq!(urlunquote("50%"), "50%");
        assert_eq!(urlunquote("%zz"), "%zz");
        assert_eq!(urlunquote("%2"), "%2");
    }

    #[test]
    fn quote_escapes_only_unsafe_ascii() {
        assert_eq!(urlquote("a/b-c_d.e~f"), "a/b-c_d.e~f");
        assert_eq!(urlquote("a b"), "a%20b");
        assert_eq!(urlquote("q?x=1&y=2"), "q%3fx%3d1%26y%3d2");
        // Non-ASCII is IRI-safe and passes through.
        assert_eq!(urlquote("caf\u{e9}"), "caf\u{e9}");
    }

    #[test]
    fn normalize_leaves_absolute_urls_structurally_intact() {
        assert_eq!(urlnormalize("http://example.com/a"), "http://example.com/a");
        assert_eq!(
            urlnormalize("https://example.com/a b?q=1 2#f g"),
            "https://example.com/a%20b?q%3d1%202#f%20g"
        );
        assert_eq!(urlnormalize("mailto:a@b.com"), "mailto:a%40b.com");
        // `file:` is treated as schemeless, as `urlnormalize` does.
        assert_eq!(urlnormalize("file:a b.html"), "file%3aa%20b.html");
    }

    #[test]
    fn scheme_detection_matches_urlparse() {
        assert_eq!(url_scheme("http://x"), Some("http"));
        assert_eq!(url_scheme("a+b-c.d:x"), Some("a+b-c.d"));
        assert_eq!(url_scheme("no-colon"), None);
        assert_eq!(url_scheme("1http://x"), None);
        assert_eq!(url_scheme("ch1.htm#frag"), None);
    }

    #[test]
    fn normalize_requotes_and_keeps_the_fragment() {
        assert_eq!(urlnormalize("a b.html"), "a%20b.html");
        assert_eq!(urlnormalize("a%20b.html"), "a%20b.html");
        assert_eq!(urlnormalize("dir\\file.html"), "dir/file.html");
        assert_eq!(urlnormalize("a b.html#frag ment"), "a%20b.html#frag%20ment");
        assert_eq!(urlnormalize(""), "");
    }

    #[test]
    fn errors_render_readably() {
        assert_eq!(LitError::msg("boom").to_string(), "boom");
        assert!(LitError::Drm("EUL".into())
            .to_string()
            .contains("DRM protected"));
    }
}
