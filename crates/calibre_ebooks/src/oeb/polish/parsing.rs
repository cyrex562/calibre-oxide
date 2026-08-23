//! Port of `old_src/src/calibre/ebooks/oeb/polish/parsing.py`.
//!
//! # Design decision: always the tag-soup path
//!
//! Python's `parse()` first tries a strict XML parse
//! (`safe_xml_fromstring(raw, recover=False)`) and only falls back to
//! `parse_html5`'s lenient HTML5 tag-soup parser if that raises. Both
//! paths return the same type (an `lxml.etree` tree), so the fallback is
//! cheap to express in Python.
//!
//! In this port, the two paths would return genuinely different Rust
//! types (a strict-XML tree vs. [`crate::dom::Dom`]'s HTML5-arena
//! -- see `crate::xmltree`'s module docs for why OPF-family XML
//! needs its own strict tree type and content documents don't), and
//! unifying them is exactly the kind of "second tree abstraction" this
//! project's scope note says not to build. Since HTML5 tag-soup parsing
//! is a strict superset of what it can successfully consume (it never
//! "fails" the way a strict parser does) and calibre's own pipeline only
//! ever feeds this well-formed-enough XHTML, this port always takes the
//! tag-soup path via [`crate::dom::Dom::parse`]. `force_html5_parse`
//! is kept as a parameter for signature parity with Python but is a
//! documented no-op (the behavior is unconditional).

use unicode_normalization::UnicodeNormalization;

use crate::chardet::{detect_bom as chardet_detect_bom, xml_to_unicode};
use crate::dom::Dom;
use crate::html_entities::xml_replace_entities;

pub const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Port of `decode_xml`. BOM-aware decode of `data` to a `String`, with
/// `\r\n`/`\r` normalized to `\n`. Returns `(text, used_encoding)` --
/// `used_encoding` is empty when decoding a `str` directly needed no
/// detection (Python's `isinstance(data, str)` early return).
pub fn decode_xml(data: &[u8], normalize_to_nfc: bool) -> (String, String) {
    fn fix_newlines(s: String) -> String {
        s.replace("\r\n", "\n").replace('\r', "\n")
    }

    // UTF-32 BOMs first (`chardet::detect_bom` only recognizes UTF-8/16,
    // matching what `xml_to_unicode` needs; `decode_xml` additionally
    // recognizes UTF-32 the way Python's version does).
    let (body, bom_enc): (&[u8], Option<&str>) = if data.starts_with(&[0, 0, 0xfe, 0xff]) {
        (&data[4..], Some("utf-32-be"))
    } else if data.starts_with(&[0xff, 0xfe, 0, 0]) {
        (&data[4..], Some("utf-32-le"))
    } else {
        (data, None)
    };

    if let Some(enc) = bom_enc {
        let decoded = decode_utf32(body, enc == "utf-32-be");
        if let Some(text) = decoded {
            let text = if normalize_to_nfc {
                text.nfc().collect()
            } else {
                text
            };
            return (fix_newlines(text), enc.to_string());
        }
        // Fall through to generic detection on decode failure, matching
        // Python's `except UnicodeDecodeError: pass`.
    }

    if let Some((body16, enc16)) = chardet_detect_bom(data) {
        if enc16 != "utf-8" {
            // utf-16-le/be: decode via encoding_rs through xml_to_unicode
            // machinery for consistency with the rest of the crate.
            let (text, _) = xml_to_unicode(data, false, false);
            let text = if normalize_to_nfc {
                text.nfc().collect()
            } else {
                text
            };
            return (fix_newlines(text), enc16.to_string());
        }
        if let Ok(text) = std::str::from_utf8(body16) {
            let text = if normalize_to_nfc {
                text.nfc().collect()
            } else {
                text.to_string()
            };
            return (fix_newlines(text), "utf-8".to_string());
        }
    }

    if let Ok(text) = std::str::from_utf8(data) {
        let text = if normalize_to_nfc {
            text.nfc().collect()
        } else {
            text.to_string()
        };
        return (fix_newlines(text), "utf-8".to_string());
    }

    let (text, used) = xml_to_unicode(data, false, true);
    let text = if normalize_to_nfc {
        text.nfc().collect()
    } else {
        text
    };
    (fix_newlines(text), used.unwrap_or_default())
}

fn decode_utf32(body: &[u8], big_endian: bool) -> Option<String> {
    if !body.len().is_multiple_of(4) {
        return None;
    }
    let mut out = String::new();
    for chunk in body.chunks_exact(4) {
        let code = if big_endian {
            u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        } else {
            u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        };
        out.push(char::from_u32(code)?);
    }
    Some(out)
}

/// Port of `handle_private_entities`: resolves any `<!ENTITY name
/// "value">` declarations found in a `<!DOCTYPE ...>` preamble before
/// the `<html`/`<HTML` tag, substituting `&name;` references in the
/// rest of the document, then drops the preamble (preserving line
/// numbers by replacing it with blank lines).
pub fn handle_private_entities(data: &str) -> String {
    let idx = data.find("<html").or_else(|| data.find("<HTML"));
    let Some(idx) = idx else {
        return data.to_string();
    };
    let pre = &data[..idx];
    if !pre.contains("<!DOCTYPE") {
        return data.to_string();
    }
    let num_of_nl_in_pre = pre.matches('\n').count();

    let mut user_entities: Vec<(String, String)> = Vec::new();
    let entity_re = entity_decl_re();
    for caps in entity_re.captures_iter(pre) {
        let name = caps.get(1).unwrap().as_str().to_string();
        let mut val = caps.get(2).unwrap().as_str().trim().to_string();
        if val.starts_with('"') && val.ends_with('"') && val.len() >= 2 {
            val = val[1..val.len() - 1].to_string();
        }
        user_entities.push((name, val));
    }
    if user_entities.is_empty() {
        return data.to_string();
    }

    let mut out = "\n".repeat(num_of_nl_in_pre);
    out.push_str(&data[idx..]);

    let names: Vec<&str> = user_entities.iter().map(|(n, _)| n.as_str()).collect();
    let pat = regex::Regex::new(&format!(r"&({});", names.join("|"))).unwrap();
    let out = pat
        .replace_all(&out, |caps: &regex::Captures| {
            let name = &caps[1];
            user_entities
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| caps[0].to_string())
        })
        .into_owned();
    out
}

fn entity_decl_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"<!ENTITY\s+(\S+)\s+([^>]+)").unwrap())
}

/// Port of `parse_html5`: HTML5 tag-soup parse via
/// [`crate::dom::Dom::parse`] -- see `crate::dom`'s module docs for
/// why that arena (built on a real `html5ever` parse) is the right tool
/// here, and this module's own docs for why it's used unconditionally
/// rather than only as a fallback.
pub fn parse_html5(raw: &str, replace_entities: bool, fix_newlines: bool) -> Dom {
    let mut raw = raw.to_string();
    if replace_entities {
        raw = xml_replace_entities(&raw);
    }
    if fix_newlines {
        raw = raw.replace("\r\n", "\n").replace('\r', "\n");
    }
    Dom::parse(&raw)
}

/// Port of `parse`: strips any pre-`<html>` preamble (preserving line
/// numbers), resolves private DOCTYPE entities, strips encoding
/// declarations, then parses. See the module docs for why this always
/// takes the tag-soup path (`force_html5_parse` is accepted for
/// signature parity but has no effect).
pub fn parse(raw: &str, replace_entities: bool, _force_html5_parse: bool) -> Dom {
    let mut raw = handle_private_entities(raw);
    if replace_entities {
        raw = xml_replace_entities(&raw);
    }
    raw = raw.replace("\r\n", "\n").replace('\r', "\n");

    // Remove any preamble before the opening <html> tag (within the
    // first 2048 chars, matching Python), preserving line numbers.
    let scan_len = raw.len().min(2048);
    if let Some(m) = html_open_tag_re().find(&raw[..scan_len]) {
        let newlines = raw[..m.start()].matches('\n').count();
        raw = "\n".repeat(newlines) + &raw[m.start()..];
    }

    let stripped = crate::chardet::strip_encoding_declarations(raw.as_bytes(), 10 * 1024, true);
    let raw = String::from_utf8_lossy(&stripped).into_owned();

    parse_html5(&raw, false, false)
}

fn html_open_tag_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(?i)<\s*html").unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_xml_plain_utf8() {
        let (text, enc) = decode_xml("<html>hi</html>".as_bytes(), true);
        assert_eq!(text, "<html>hi</html>");
        assert_eq!(enc, "utf-8");
    }

    #[test]
    fn decode_xml_strips_bom_and_fixes_newlines() {
        let mut data = vec![0xef, 0xbb, 0xbf];
        data.extend_from_slice(b"line1\r\nline2\r");
        let (text, _) = decode_xml(&data, true);
        assert_eq!(text, "line1\nline2\n");
    }

    #[test]
    fn parse_html5_builds_a_dom() {
        let dom = parse_html5("<html><body><p>hi</p></body></html>", true, true);
        let ps = dom.find_all_tag_global("p");
        assert_eq!(ps.len(), 1);
    }

    #[test]
    fn parse_strips_preamble_before_html_tag() {
        let raw = "<?xml version=\"1.0\"?>\n<!DOCTYPE html>\n<html><body><p id=\"x\">hi</p></body></html>";
        let dom = parse(raw, true, false);
        let p = dom.find_by_id("x");
        assert!(p.is_some());
    }

    #[test]
    fn handle_private_entities_substitutes_and_strips_preamble() {
        let raw = "<!DOCTYPE html [\n<!ENTITY foo \"bar\">\n]>\n<html>&foo;</html>";
        let out = handle_private_entities(raw);
        assert!(out.contains("bar"));
        assert!(!out.contains("<!DOCTYPE"));
    }

    #[test]
    fn handle_private_entities_noop_without_doctype() {
        let raw = "<html>hi</html>";
        assert_eq!(handle_private_entities(raw), raw);
    }
}
