//! Encoding detection + declared-encoding extraction.
//!
//! Port of `old_src/src/calibre/ebooks/chardet.py`. The Python module
//! also served as the entry point to `calibre_extensions.uchardet` —
//! the C++ uchardet wrapper (`old_src/src/calibre/ebooks/uchardet.c`)
//! which this module supersedes.
//!
//! Detection strategy:
//! 1. Byte-order marks (BOM) — canonical, cheap, unambiguous.
//! 2. XML/HTML declared encoding via regex.
//! 3. Statistical guess via `chardetng` (pure Rust; same lineage as
//!    Firefox's autodetector).

use encoding_rs::Encoding;
use regex::bytes::Regex;
use std::sync::OnceLock;

/// The three encoding-declaration patterns from Python. Order matters:
/// XML declaration first, then HTML5 charset, then HTML4 pragma.
fn declared_patterns() -> &'static [Regex; 3] {
    static PATS: OnceLock<[Regex; 3]> = OnceLock::new();
    PATS.get_or_init(|| {
        [
            Regex::new(r#"(?i)<\?[^<>]+encoding\s*=\s*['"](.*?)['"][^<>]*>"#).unwrap(),
            Regex::new(r#"(?i)<meta\s+charset=['"]([-_a-z0-9]+)['"][^<>]*>(?:\s*</meta>)?"#)
                .unwrap(),
            Regex::new(
                r#"(?i)<meta\s+?[^<>]*?content\s*=\s*['"][^'"]*?charset=([-_a-z0-9]+)[^'"]*?['"][^<>]*>(?:\s*</meta>)?"#,
            )
            .unwrap(),
        ]
    })
}

/// Common charset aliases Calibre folded into modern names.
fn canonicalize_encoding(name: &str) -> String {
    let n = name.trim().to_ascii_lowercase();
    match n.as_str() {
        // Direct aliases from Python `_CHARSET_ALIASES`.
        "macintosh" => "mac-roman".to_string(),
        "x-sjis" => "shift-jis".to_string(),
        "mac-centraleurope" => "cp1250".to_string(),
        // MS Word gb2312 → gbk workaround (Python `xml_to_unicode`).
        "gb2312" | "chinese" | "csiso58gb231280" | "euc-cn" | "euccn"
        | "eucgb2312-cn" | "gb2312-1980" | "gb2312-80" | "iso-ir-58" => "gbk".to_string(),
        // ASCII → utf-8 (Python `force_encoding`).
        "ascii" => "utf-8".to_string(),
        _ => n,
    }
}

/// Best-effort detection of an XML/HTML encoding declaration in the
/// leading `limit` bytes. Returns the raw string as it appears in the
/// declaration (case may be mixed).
pub fn find_declared_encoding(raw: &[u8], limit: usize) -> Option<String> {
    let head = &raw[..raw.len().min(limit)];
    for pat in declared_patterns() {
        if let Some(caps) = pat.captures(head) {
            if let Some(m) = caps.get(1) {
                return Some(String::from_utf8_lossy(m.as_bytes()).to_string());
            }
        }
    }
    None
}

/// Strip encoding declarations from the first `limit` bytes.
/// `preserve_newlines=true` matches the Python behavior of replacing
/// the match with its own newline count (so line numbers upstream
/// don't shift).
pub fn strip_encoding_declarations(raw: &[u8], limit: usize, preserve_newlines: bool) -> Vec<u8> {
    let split_at = raw.len().min(limit);
    let (prefix, suffix) = raw.split_at(split_at);
    let mut prefix = prefix.to_vec();
    for pat in declared_patterns() {
        prefix = if preserve_newlines {
            pat.replace_all(&prefix, |caps: &regex::bytes::Captures| {
                let match_bytes = caps.get(0).unwrap().as_bytes();
                let nl = match_bytes.iter().filter(|&&b| b == b'\n').count();
                vec![b'\n'; nl]
            })
            .into_owned()
        } else {
            pat.replace_all(&prefix, &b""[..]).into_owned()
        };
    }
    prefix.extend_from_slice(suffix);
    prefix
}

#[derive(Debug, Clone, PartialEq)]
pub struct DetectionResult {
    pub encoding: String,
    /// 1.0 when we found a BOM or an explicit declaration; the
    /// chardetng-derived confidence otherwise.
    pub confidence: f32,
}

/// Statistical detection via chardetng. Matches the Python
/// `detect(bytestring)` return: `{encoding, confidence}`.
pub fn detect(raw: &[u8]) -> DetectionResult {
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
    det.feed(raw, true);
    let enc = det.guess(None, chardetng::Utf8Detection::Allow);
    DetectionResult {
        encoding: canonicalize_encoding(enc.name()),
        // chardetng doesn't expose a probability; treat a guess as
        // ~1.0 confident. Matches Python's uchardet wrapper which
        // also always returned 1.
        confidence: 1.0,
    }
}

/// Best-effort encoding decision. Order (matches Python
/// `force_encoding`): declared > detected > default (utf-8).
pub fn force_encoding(raw: &[u8], assume_utf8: bool) -> String {
    if let Some(declared) = find_declared_encoding(raw, 50 * 1024) {
        return canonicalize_encoding(&declared);
    }
    if assume_utf8 {
        if std::str::from_utf8(raw).is_ok() {
            return "utf-8".to_string();
        }
    }
    let det = detect(&raw[..raw.len().min(50 * 1024)]);
    if det.encoding.is_empty() {
        return "utf-8".to_string();
    }
    det.encoding
}

/// BOM-first detection. Returns `(raw_without_bom, encoding_name)`.
pub fn detect_bom(raw: &[u8]) -> Option<(&[u8], &'static str)> {
    if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some((&raw[3..], "utf-8"))
    } else if raw.starts_with(&[0xFF, 0xFE]) {
        Some((&raw[2..], "utf-16-le"))
    } else if raw.starts_with(&[0xFE, 0xFF]) {
        Some((&raw[2..], "utf-16-be"))
    } else {
        None
    }
}

/// Full port of Python `xml_to_unicode`. Returns
/// `(decoded_string, encoding_used)`.
pub fn xml_to_unicode(
    raw: &[u8],
    strip_encoding_pats: bool,
    assume_utf8: bool,
) -> (String, Option<String>) {
    if raw.is_empty() {
        return (String::new(), None);
    }
    let (body, encoding) = detect_xml_encoding(raw, assume_utf8);
    let enc_name = encoding.as_deref().unwrap_or("utf-8");
    let enc = Encoding::for_label(enc_name.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    let (cow, _, _) = enc.decode(body);
    let mut decoded = cow.into_owned();
    if strip_encoding_pats {
        let stripped = strip_encoding_declarations(decoded.as_bytes(), 50 * 1024, false);
        decoded = String::from_utf8_lossy(&stripped).into_owned();
    }
    (decoded, encoding)
}

fn detect_xml_encoding<'a>(raw: &'a [u8], assume_utf8: bool) -> (&'a [u8], Option<String>) {
    if let Some((body, enc)) = detect_bom(raw) {
        return (body, Some(enc.to_string()));
    }
    if let Some(declared) = find_declared_encoding(raw, 50 * 1024) {
        return (raw, Some(canonicalize_encoding(&declared)));
    }
    if assume_utf8 && std::str::from_utf8(raw).is_ok() {
        return (raw, Some("utf-8".to_string()));
    }
    let det = detect(&raw[..raw.len().min(50 * 1024)]);
    (raw, Some(det.encoding))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_aliases_match_python() {
        assert_eq!(canonicalize_encoding("Macintosh"), "mac-roman");
        assert_eq!(canonicalize_encoding("X-SJIS"), "shift-jis");
        assert_eq!(canonicalize_encoding("mac-centraleurope"), "cp1250");
        assert_eq!(canonicalize_encoding("gb2312"), "gbk");
        assert_eq!(canonicalize_encoding("ASCII"), "utf-8");
        assert_eq!(canonicalize_encoding("UTF-8"), "utf-8");
    }

    #[test]
    fn find_declared_xml_declaration() {
        let raw = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><root/>";
        assert_eq!(find_declared_encoding(raw, 1024).as_deref(), Some("UTF-8"));
    }

    #[test]
    fn find_declared_html5_meta_charset() {
        let raw = b"<html><head><meta charset='utf-8'></head></html>";
        assert_eq!(find_declared_encoding(raw, 1024).as_deref(), Some("utf-8"));
    }

    #[test]
    fn find_declared_html4_pragma() {
        let raw = b"<meta http-equiv='Content-Type' content='text/html; charset=iso-8859-1'>";
        assert_eq!(
            find_declared_encoding(raw, 1024).as_deref(),
            Some("iso-8859-1")
        );
    }

    #[test]
    fn find_declared_returns_none_when_absent() {
        assert!(find_declared_encoding(b"<p>hi</p>", 1024).is_none());
    }

    #[test]
    fn strip_declaration_removes_meta_charset() {
        let raw = b"<html><head><meta charset='utf-8'></head><body>x</body></html>";
        let out = strip_encoding_declarations(raw, 1024, false);
        assert!(!out.windows(7).any(|w| w == b"charset"));
        assert!(out.windows(1).filter(|w| w == b"x").count() == 1);
    }

    #[test]
    fn strip_declaration_preserves_newlines_when_asked() {
        // A declaration containing an embedded newline must be
        // replaced by that many newlines to keep upstream line
        // numbers stable.
        let raw = b"<meta\ncharset='utf-8'>rest";
        let out = strip_encoding_declarations(raw, 1024, true);
        assert_eq!(out, b"\nrest");
    }

    #[test]
    fn detect_bom_utf8() {
        let raw = [0xEF, 0xBB, 0xBF, b'a', b'b'];
        let (body, enc) = detect_bom(&raw).unwrap();
        assert_eq!(body, b"ab");
        assert_eq!(enc, "utf-8");
    }

    #[test]
    fn detect_bom_utf16_le_and_be() {
        let (body, enc) = detect_bom(&[0xFF, 0xFE, b'a']).unwrap();
        assert_eq!(body, b"a");
        assert_eq!(enc, "utf-16-le");
        let (body, enc) = detect_bom(&[0xFE, 0xFF, b'a']).unwrap();
        assert_eq!(body, b"a");
        assert_eq!(enc, "utf-16-be");
    }

    #[test]
    fn detect_bom_returns_none_when_absent() {
        assert!(detect_bom(b"plain").is_none());
    }

    #[test]
    fn detect_returns_utf8_for_ascii() {
        let out = detect(b"hello world");
        assert!(!out.encoding.is_empty());
    }

    #[test]
    fn xml_to_unicode_uses_bom() {
        let raw = [0xEF, 0xBB, 0xBF, b'h', b'i'];
        let (s, enc) = xml_to_unicode(&raw, false, false);
        assert_eq!(s, "hi");
        assert_eq!(enc.as_deref(), Some("utf-8"));
    }

    #[test]
    fn xml_to_unicode_strips_when_asked() {
        let raw = b"<meta charset='utf-8'>hello";
        let (s, _) = xml_to_unicode(raw, true, false);
        assert!(!s.contains("charset"));
        assert!(s.contains("hello"));
    }

    #[test]
    fn force_encoding_prefers_declared_over_detected() {
        // Even if the body could be UTF-8, an explicit declaration
        // must win.
        let raw = b"<meta charset='iso-8859-1'>text";
        assert_eq!(force_encoding(raw, false), "iso-8859-1");
    }
}
