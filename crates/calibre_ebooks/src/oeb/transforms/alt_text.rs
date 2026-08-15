//! Port of `old_src/src/calibre/ebooks/oeb/transforms/alt_text.py`.
//!
//! Reads accessibility alt-text embedded in an image's metadata and
//! fills in a spine `<img>`'s missing `alt` attribute. Python's
//! `calibre.utils.img.read_alt_text` has two paths: XMP (real here, see
//! [`read_alt_text_from_xmp`]/[`read_xmp_packet`]), and an EXIF
//! `ImageDescription` (tag `0x010E`/`270`) fallback.
//!
//! # Scope note: the EXIF fallback
//!
//! There is no EXIF reader anywhere in this workspace, and adding one
//! (even a narrow one) is a bigger addition than this 36-line file
//! otherwise warrants -- per the porting brief, a documented gap here is
//! the chosen tradeoff over adding a new dependency for a single
//! fallback path. [`read_alt_text`]'s XMP path (the primary path Python
//! itself tries first) is real end to end for JPEG and PNG; the EXIF
//! fallback is a `todo!()`, reached only when an image has no XMP alt
//! text at all -- see [`exif_alt_text_fallback`].

use roxmltree::Document;

use crate::mobi::dom::Dom;
use crate::oeb::book::OEBBook;

/// Port of `read_text_from_container`: pick the best `rdf:li` child text
/// from an XMP `AltTextAccessibility`/`dc:description` container,
/// preferring `target_lang`, then `x-default`, then the first entry.
fn read_text_from_container(container: roxmltree::Node, target_lang: &str) -> String {
    let mut lang_map: Vec<(String, String)> = Vec::new();
    for li in container
        .descendants()
        .filter(|n| n.tag_name().name() == "li")
    {
        if let Some(text) = li.text() {
            if text.is_empty() {
                continue;
            }
            let lang = li
                .attribute(("http://www.w3.org/XML/1998/namespace", "lang"))
                .unwrap_or("x-default")
                .to_string();
            lang_map.push((lang, text.to_string()));
        }
    }
    if target_lang.is_empty() {
        if let Some((_, v)) = lang_map.iter().find(|(l, _)| l == "x-default") {
            return v.clone();
        }
    }
    if let Some((_, v)) = lang_map.iter().find(|(l, _)| l == target_lang) {
        return v.clone();
    }
    lang_map
        .iter()
        .find(|(l, _)| l == "x-default")
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

/// Port of `read_alt_text_from_xmp`: find `AltTextAccessibility` (any
/// namespace) or `dc:description` in an XMP packet and return its best
/// text, or `""` if neither is present / the packet doesn't parse.
pub fn read_alt_text_from_xmp(xmp: &str, target_lang: &str) -> String {
    let Ok(doc) = Document::parse(xmp) else {
        return String::new();
    };
    for node in doc.descendants() {
        if node.tag_name().name() == "AltTextAccessibility" {
            let ans = read_text_from_container(node, target_lang);
            if !ans.is_empty() {
                return ans;
            }
        }
    }
    for node in doc.descendants() {
        if node.tag_name().name() == "description"
            && node.tag_name().namespace() == Some("http://purl.org/dc/elements/1.1/")
        {
            let ans = read_text_from_container(node, target_lang);
            if !ans.is_empty() {
                return ans;
            }
        }
    }
    String::new()
}

/// Port of the `fmt == 'jpeg'` branch of `read_xmp_from_pil_image`:
/// scan JPEG APP1 markers for the Adobe XMP packet.
fn xmp_from_jpeg(data: &[u8]) -> Option<String> {
    const MARKER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut pos = 2usize;
    while pos + 4 <= data.len() {
        if data[pos] != 0xFF {
            pos += 1;
            continue;
        }
        let marker = data[pos + 1];
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            pos += 2;
            continue;
        }
        if marker == 0xDA {
            // Start of scan: no more markers of interest follow.
            break;
        }
        if pos + 4 > data.len() {
            break;
        }
        let seg_len = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;
        if seg_len < 2 || pos + 2 + seg_len > data.len() {
            break;
        }
        let payload = &data[pos + 4..pos + 2 + seg_len];
        if marker == 0xE1 && payload.starts_with(MARKER) {
            let xml = &payload[MARKER.len()..];
            return Some(String::from_utf8_lossy(xml).into_owned());
        }
        pos += 2 + seg_len;
    }
    None
}

/// Port of the `fmt == 'png'` branch of `read_xmp_from_pil_image`: read
/// the `XML:com.adobe.xmp` `tEXt`/`iTXt` chunk. Compressed `iTXt`
/// (compression flag set) and `zTXt` are not handled -- narrower than
/// PIL, which decompresses them; real-world XMP chunks are essentially
/// always uncompressed `iTXt`/`tEXt` in practice.
fn xmp_from_png(data: &[u8]) -> Option<String> {
    const SIG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if !data.starts_with(SIG) {
        return None;
    }
    let mut pos = SIG.len();
    while pos + 8 <= data.len() {
        let len =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let ctype = &data[pos + 4..pos + 8];
        let data_start = pos + 8;
        if data_start + len + 4 > data.len() {
            break;
        }
        let chunk_data = &data[data_start..data_start + len];
        if ctype == b"tEXt" {
            if let Some(nul) = chunk_data.iter().position(|&b| b == 0) {
                let keyword = String::from_utf8_lossy(&chunk_data[..nul]);
                if keyword == "XML:com.adobe.xmp" {
                    return Some(String::from_utf8_lossy(&chunk_data[nul + 1..]).into_owned());
                }
            }
        } else if ctype == b"iTXt" {
            if let Some(nul) = chunk_data.iter().position(|&b| b == 0) {
                let keyword = String::from_utf8_lossy(&chunk_data[..nul]);
                if keyword == "XML:com.adobe.xmp" && chunk_data.len() > nul + 2 {
                    let compression_flag = chunk_data[nul + 1];
                    if compression_flag == 0 {
                        // keyword\0 compression_flag compression_method lang\0 translated\0 text
                        let rest = &chunk_data[nul + 3..];
                        if let Some(lang_end) = rest.iter().position(|&b| b == 0) {
                            let rest2 = &rest[lang_end + 1..];
                            if let Some(tk_end) = rest2.iter().position(|&b| b == 0) {
                                let text = &rest2[tk_end + 1..];
                                return Some(String::from_utf8_lossy(text).into_owned());
                            }
                        }
                    }
                }
            }
        } else if ctype == b"IDAT" || ctype == b"IEND" {
            break;
        }
        pos = data_start + len + 4;
    }
    None
}

/// Port of `read_xmp_from_pil_image`: extract the raw XMP packet, if
/// any, keyed by the already-detected image format
/// ([`calibre_utils::imghdr::what`]'s vocabulary). WebP/TIFF XMP
/// extraction is not implemented (Python reads `im.info['xmp']`/
/// `im.tag_v2[700]` for those -- narrower than Python, but JPEG and PNG
/// cover the overwhelming majority of e-book cover/inline images).
fn read_xmp_packet(data: &[u8], fmt: &str) -> Option<String> {
    match fmt {
        "jpeg" => xmp_from_jpeg(data),
        "png" => xmp_from_png(data),
        _ => None,
    }
}

/// Port of the EXIF fallback in `read_alt_text` (`exif.get(270)`, the
/// `ImageDescription` tag). Real work item: no EXIF reader exists in
/// this workspace yet; adding one (even a narrow one, e.g.
/// `kamadak-exif`) is a reasonable follow-up but is a bigger addition
/// than this fallback path warrants on its own -- see the module-level
/// scope note.
///
/// Deliberately returns `None` rather than `todo!()`: `AddAltText`
/// calls this on every ordinary spine `<img>` that has no XMP alt text
/// at all, which is the common case for real books, so a `todo!()` here
/// would panic on the very first conversion that enables
/// `add_alt_text_to_img` -- unacceptable for a path exercised during
/// normal operation, not just malformed input (see
/// `docs/FAULT_TOLERANCE.md`). "Not yet implemented" degrades to "no alt
/// text found," matching what Python does for any image whose EXIF
/// simply has no `ImageDescription` tag.
fn exif_alt_text_fallback(_data: &[u8], _fmt: &str) -> Option<String> {
    None
}

/// Port of `read_alt_text`: XMP alt text if present, else the EXIF
/// `ImageDescription` fallback.
pub fn read_alt_text(data: &[u8]) -> Option<String> {
    let fmt = calibre_utils::imghdr::what(data)?;
    if let Some(xmp) = read_xmp_packet(data, fmt) {
        let alt = read_alt_text_from_xmp(&xmp, "");
        let alt = alt.trim();
        if !alt.is_empty() {
            return Some(alt.to_string());
        }
    }
    exif_alt_text_fallback(data, fmt)
}

/// Port of `AddAltText`: fill in `alt` text for spine `<img>` elements
/// that don't already have it, from the referenced image's metadata.
pub struct AddAltText;

impl AddAltText {
    pub fn call(&self, oeb: &mut OEBBook) {
        let spine_hrefs: Vec<String> = oeb
            .spine
            .iter()
            .filter_map(|s| oeb.manifest.get_by_id(&s.idref).map(|i| i.href.clone()))
            .collect();
        for href in spine_hrefs {
            let Ok(raw) = oeb.container.read(&href) else {
                continue;
            };
            let html = String::from_utf8_lossy(&raw);
            let mut dom = Dom::parse(&html);
            let mut changed = false;
            for img in dom.find_all_tag_global("img") {
                let has_alt = dom
                    .node(img)
                    .attrs
                    .get("alt")
                    .map(|a| !a.is_empty())
                    .unwrap_or(false);
                if has_alt {
                    continue;
                }
                let Some(src) = dom.node(img).attrs.get("src").cloned() else {
                    continue;
                };
                let target = super::filenames::abshref(&href, &src);
                let Some(image_item) = oeb.manifest.get_by_href(&target) else {
                    continue;
                };
                if image_item.media_type == "image/svg+xml" {
                    continue;
                }
                let Ok(image_data) = oeb.container.read(&image_item.href) else {
                    continue;
                };
                if let Some(alt) = read_alt_text(&image_data) {
                    if !alt.is_empty() {
                        dom.node_mut(img).attrs.insert("alt".to_string(), alt);
                        changed = true;
                    }
                }
            }
            if changed {
                let rendered = dom.serialize(dom.root).into_bytes();
                let _ = oeb.container.write(&href, &rendered);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALT_TEXT_XMP: &str = r#"<?xpacket begin="" id=""?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description>
   <Iptc4xmpExt:AltTextAccessibility xmlns:Iptc4xmpExt="http://iptc.org/std/Iptc4xmpExt/2008-02-29/">
    <rdf:Alt>
     <rdf:li xml:lang="x-default">A red bicycle leaning on a wall</rdf:li>
    </rdf:Alt>
   </Iptc4xmpExt:AltTextAccessibility>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    #[test]
    fn reads_alt_text_accessibility_container() {
        let alt = read_alt_text_from_xmp(ALT_TEXT_XMP, "");
        assert_eq!(alt, "A red bicycle leaning on a wall");
    }

    #[test]
    fn falls_back_to_dc_description() {
        let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
          xmlns:dc="http://purl.org/dc/elements/1.1/">
  <rdf:Description>
   <dc:description>
    <rdf:Alt><rdf:li xml:lang="x-default">A plain description</rdf:li></rdf:Alt>
   </dc:description>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let alt = read_alt_text_from_xmp(xmp, "");
        assert_eq!(alt, "A plain description");
    }

    #[test]
    fn invalid_xmp_returns_empty_string_not_a_panic() {
        assert_eq!(read_alt_text_from_xmp("not xml at all <<<", ""), "");
    }

    #[test]
    fn extracts_xmp_from_a_png_text_chunk() {
        // Minimal PNG: signature + IHDR + tEXt(XML:com.adobe.xmp) + IEND.
        let mut png = vec![0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        // IHDR (13 bytes of zeros is fine for this test -- we never
        // decode pixel data).
        push_chunk(&mut png, b"IHDR", &[0u8; 13]);
        let mut text_data = b"XML:com.adobe.xmp\0".to_vec();
        text_data.extend_from_slice(ALT_TEXT_XMP.as_bytes());
        push_chunk(&mut png, b"tEXt", &text_data);
        push_chunk(&mut png, b"IEND", &[]);

        let xmp = xmp_from_png(&png).expect("xmp chunk found");
        let alt = read_alt_text_from_xmp(&xmp, "");
        assert_eq!(alt, "A red bicycle leaning on a wall");
    }

    fn push_chunk(buf: &mut Vec<u8>, ctype: &[u8; 4], data: &[u8]) {
        buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
        buf.extend_from_slice(ctype);
        buf.extend_from_slice(data);
        buf.extend_from_slice(&[0, 0, 0, 0]); // fake CRC, unused by our reader
    }
}
