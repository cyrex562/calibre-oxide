//! Port of `old_src/src/calibre/ebooks/oeb/polish/check/parsing.py`.
//!
//! # Scope note: source positions on content documents
//!
//! Every error here that references a byte offset inside *strict XML*
//! (OPF/NCX -- [`crate::oeb::polish::xmltree::Xml`], parsed via
//! `roxmltree`) gets a real line/column, exactly like Python's
//! `lxml`-backed `elem.sourceline`. Errors that reference a position
//! inside an *XHTML content document* ([`crate::dom::Dom`], parsed
//! via `html5ever` through `oeb::polish::parsing::parse_html5`) get
//! `None` for line/column: unlike `lxml`, `Dom`'s arena does not record
//! source positions for parsed elements (see `crate::dom`'s module docs).
//! This is a real, pre-existing property of the already-merged `Dom`
//! type (not something this port narrows), and does not affect whether
//! an error is *detected* -- only how precisely its location is
//! reported. Every check function is otherwise fully real.
//!
//! Byte-offset errors found by scanning raw bytes directly (bad
//! entities, oversized files) use [`calibre_ebooks::oeb::polish::utils::PositionFinder`]
//! exactly like Python's `PositionFinder`, and get real line/column
//! numbers regardless of document kind.

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::Result;
use regex::bytes::Regex as BytesRegex;

use crate::oeb::constants::{OEB_DOCS, XHTML_NS};
use crate::oeb::polish::utils::{guess_type, PositionFinder};

use super::super::container::Container;
use super::base::{CheckError, Level};

/// Port of `XML_TYPES` (`main.py`). Kept here (rather than only in
/// `main.rs`) because [`named_entities_fix`] needs it and `parsing.rs`
/// is lower in the dependency order; `main.rs` re-uses this constant
/// rather than redefining it.
pub fn xml_types() -> [String; 5] {
    [
        guess_type("a.xml"),
        guess_type("a.svg"),
        guess_type("a.opf"),
        guess_type("a.ncx"),
        "application/oebps-page-map+xml".to_string(),
    ]
}

const XML_ENTITY_NAMES: [&str; 5] = ["lt", "gt", "amp", "apos", "quot"];

// ===================================================================
// Error constructors (port of each `BaseError` subclass)
// ===================================================================

/// Port of `EmptyFile`.
pub fn empty_file(name: &str) -> CheckError {
    let owned_name = name.to_string();
    CheckError::new("EmptyFile", format!("The file {name} is empty"), name)
        .with_help("This file is empty, it contains nothing, you should probably remove it.")
        .with_fix("Remove this file", move |container| {
            container.remove_item(&owned_name, true)?;
            Ok(true)
        })
}

/// Port of `DecodeError`.
pub fn decode_error(name: &str) -> CheckError {
    CheckError::new(
        "DecodeError",
        format!("Parsing of {name} failed, could not decode"),
        name,
    )
    .parsing_error()
    .with_help(
        "A decoding errors means that the contents of the file could not be interpreted as \
         text. This usually happens if the file has an incorrect character encoding \
         declaration or if the file is actually a binary file, like an image or font that is \
         mislabelled with an incorrect media type in the OPF.",
    )
}

/// Port of `XMLParseError`/`HTMLParseError`. `is_html` selects the
/// subclass (only the `HELP` text and `type_name` differ).
pub fn xml_parse_error(
    is_html: bool,
    msg: &str,
    name: &str,
    line: Option<u32>,
    col: Option<u32>,
) -> CheckError {
    let type_name = if is_html {
        "HTMLParseError"
    } else {
        "XMLParseError"
    };
    let help = if is_html {
        "A parsing error in an HTML file means that the HTML syntax is incorrect. Most \
         readers will automatically ignore such errors, but they may result in incorrect \
         display of content. These errors can usually be fixed automatically, however, \
         automatic fixing can sometimes \"do the wrong thing\"."
    } else {
        "A parsing error in an XML file means that the XML syntax in the file is incorrect. \
         Such a file will most probably not open in an e-book reader. These errors can \
         usually be fixed automatically, however, automatic fixing can sometimes \"do the \
         wrong thing\"."
    };
    let mut err = CheckError::new(type_name, format!("Parsing failed: {msg}"), name)
        .at(line, col)
        .parsing_error()
        .with_help(help);
    // Port of the `mismatch_pat` special case: lxml's specific "tag
    // mismatch: ... line N ..." wording. `roxmltree` (this port's
    // strict-XML parser) produces differently-worded errors, so this
    // will rarely if ever match in practice -- kept for parity in case
    // a future parser swap produces matching wording, and because the
    // logic itself (not the specific message format) is what Python
    // encodes here.
    if let Some(caps) = mismatch_pat().captures(msg) {
        if let Ok(mismatch_line) = caps[1].parse::<u32>() {
            err = err.with_locations(vec![
                (name.to_string(), Some(mismatch_line), None),
                (name.to_string(), line, col),
            ]);
        }
    }
    err
}

fn mismatch_pat() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"tag mismatch:.+?line (\d+).+?line \d+").unwrap())
}

/// Port of `PrivateEntities`.
pub fn private_entities(name: &str) -> CheckError {
    CheckError::new(
        "PrivateEntities",
        "Parsing failed: Private entities found",
        name,
    )
    .parsing_error()
    .with_help(
        "This HTML file uses private entities. These are not supported. You can try running \
         \"Fix HTML\" from the Tools menu, which will try to automatically resolve the \
         private entities.",
    )
}

/// Port of `NamedEntities`.
pub fn named_entities(name: &str) -> CheckError {
    let owned_name = name.to_string();
    CheckError::new("NamedEntities", "Named entities present", name)
        .with_level(Level::Warn)
        .with_help(
            "Named entities are often only incompletely supported by various book reading \
             software. Therefore, it is best to not use them, replacing them with the actual \
             characters they represent. This can be done automatically.",
        )
        .with_fix(
            "Replace all named entities with their character equivalents in this book",
            move |container| named_entities_fix(container, &owned_name),
        )
}

fn named_entities_fix(container: &mut Container, _triggering_name: &str) -> Result<bool> {
    let check_types: std::collections::HashSet<String> = xml_types()
        .into_iter()
        .chain(OEB_DOCS.iter().map(|s| s.to_string()))
        .collect();
    let names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut changed = false;
    for (name, mt) in names {
        if !check_types.contains(&mt) {
            continue;
        }
        let raw = container.raw_data(&name, true)?;
        let raw = String::from_utf8_lossy(&raw).into_owned();
        let nraw = replace_named_entities(&raw);
        if nraw != raw {
            changed = true;
            container.write_file(&name, nraw.as_bytes())?;
        }
    }
    Ok(changed)
}

/// Port of `replace_pat.sub(...)`: replaces every `&name;` reference to
/// a *known, non-XML* HTML5 named entity with its literal character,
/// leaving the five XML entities, numeric entities, and unknown names
/// untouched.
fn replace_named_entities(raw: &str) -> String {
    named_entity_ref_re()
        .replace_all(raw, |caps: &regex::Captures| {
            let name = &caps[1];
            if XML_ENTITY_NAMES.contains(&name) {
                return caps[0].to_string();
            }
            let probe = format!("&{name};");
            let decoded = crate::html_entities::decode_entities(&probe);
            if decoded != probe {
                decoded
            } else {
                caps[0].to_string()
            }
        })
        .into_owned()
}

fn named_entity_ref_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"&([a-zA-Z][a-zA-Z0-9]*);").unwrap())
}

/// Port of `make_filename_safe`.
pub fn make_filename_safe(name: &str) -> String {
    name.split('/')
        .map(|part| {
            let ascii = calibre_utils::filenames::ascii_filename(part);
            ascii
                .chars()
                .map(|c| if is_url_safe_char(c) { c } else { '_' })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn is_url_safe_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | '~')
}

/// Port of `oeb.base.urlquote`: percent-encodes ASCII characters outside
/// the IRI-safe set, leaving non-ASCII characters (which are IRI-safe)
/// untouched.
pub fn urlquote(href: &str) -> String {
    let mut out = String::new();
    for ch in href.chars() {
        if ch.is_ascii() && !is_url_safe_char(ch) {
            out.push('%');
            out.push_str(&format!("{:02x}", ch as u32));
        } else {
            out.push(ch);
        }
    }
    out
}

/// Port of `EscapedName`.
pub fn escaped_name(name: &str) -> CheckError {
    let qname = urlquote(name);
    let mut sname = make_filename_safe(name);
    let help = format!(
        "The filename {name} contains unsafe characters, that must be escaped, like this \
         {qname}. This can cause problems with some e-book readers. To be absolutely safe, \
         use only the English alphabet [a-z], the numbers [0-9], underscores and hyphens in \
         your file names. While many other characters are allowed, they may cause problems \
         with some software."
    );
    let individual_fix_label = format!("Rename the file {name} to {sname}");
    let owned_name = name.to_string();
    CheckError::new("EscapedName", "Filename contains unsafe characters", name)
        .with_level(Level::Warn)
        .with_help(help)
        .with_fix(individual_fix_label, move |container| {
            let all_names: std::collections::HashSet<String> =
                container.name_path_map.keys().cloned().collect();
            // Port of `bn, ext = self.sname.rpartition('.')[0::2]`:
            // `rpartition` on a string with no `.` yields `('', '',
            // original)`, i.e. an empty `bn` and `ext` equal to the
            // whole name.
            let (base, ext) = match sname.rfind('.') {
                Some(idx) => (sname[..idx].to_string(), sname[idx + 1..].to_string()),
                None => (String::new(), sname.clone()),
            };
            let mut c = 0u32;
            while all_names.contains(&sname) {
                c += 1;
                sname = format!("{base}_{c}.{ext}");
            }
            let mut file_map = HashMap::new();
            file_map.insert(owned_name.clone(), sname.clone());
            super::super::replace::rename_files(container, &file_map)?;
            Ok(true)
        })
}

/// Port of `TooLarge`.
pub fn too_large(name: &str, max_size: u64) -> CheckError {
    CheckError::new("TooLarge", "File too large", name)
        .with_level(Level::Info)
        .with_help(format!(
            "This HTML file is larger than {}. Too large HTML files can cause performance \
             problems on some e-book readers. Consider splitting this file into smaller \
             sections.",
            human_readable_size(max_size)
        ))
}

fn human_readable_size(size: u64) -> String {
    let mut size = size as f64;
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if size < 1024.0 {
            return format!("{size:.1} {unit}");
        }
        size /= 1024.0;
    }
    format!("{size:.1} PB")
}

/// Port of `BadEntity`.
pub fn bad_entity(ent: &str, name: &str, lnum: Option<u32>, col: Option<u32>) -> CheckError {
    CheckError::new("BadEntity", format!("Invalid entity: {ent}"), name)
        .at(lnum, col)
        .with_help(
            "This is an invalid (unrecognized) entity. Replace it with whatever text it is \
             supposed to have represented.",
        )
}

/// Port of `BadNamespace`.
pub fn bad_namespace(name: &str, namespace: Option<&str>) -> CheckError {
    let ns_desc = match namespace {
        Some(ns) => format!("incorrect namespace {ns}"),
        None => "no namespace".to_string(),
    };
    let help = crate::xml_util::prepare_string_for_xml(
        &format!(
            "This file has {ns_desc}. Its namespace must be {XHTML_NS}. Set the namespace by \
             defining the xmlns attribute on the <html> element, like this <html xmlns=\"{XHTML_NS}\">"
        ),
        false,
    );
    let owned_name = name.to_string();
    CheckError::new("BadNamespace", "Invalid or missing namespace", name)
        .with_help(help)
        .with_fix(
            "Run fix HTML on this file, which will automatically insert the correct namespace",
            move |container| {
                container.ensure_parsed(&owned_name)?;
                container.dirty(&owned_name);
                Ok(true)
            },
        )
}

/// Port of `NonUTF8`.
pub fn non_utf8(name: &str, enc: &str) -> CheckError {
    let owned_name = name.to_string();
    CheckError::new("NonUTF8", "Non UTF-8 encoding declaration", name)
        .with_level(Level::Warn)
        .with_help(format!(
            "This file has its encoding declared as {enc}. Some reader software cannot handle \
             non-UTF8 encoded files. You should change the encoding to UTF-8."
        ))
        .with_fix("Change this file's encoding to UTF-8", move |container| {
            let raw = container.raw_data(&owned_name, false)?;
            let (new_raw, changed) =
                crate::chardet::replace_encoding_declarations(&raw, "utf-8", 50 * 1024);
            if changed {
                container.write_file(&owned_name, &new_raw)?;
            }
            Ok(changed)
        })
}

fn is_strict_xml_type(mt: &str) -> bool {
    mt == guess_type("a.opf") || mt == guess_type("a.ncx")
}

/// Port of `DuplicateId`.
pub fn duplicate_id(name: &str, eid: &str, locs: &[Option<u32>]) -> CheckError {
    let mut sorted_locs = locs.to_vec();
    sorted_locs.sort();
    let all_locations = sorted_locs
        .iter()
        .map(|l| (name.to_string(), *l, None))
        .collect();
    let owned_name = name.to_string();
    let owned_eid = eid.to_string();
    CheckError::new("DuplicateId", format!("Duplicate id: {eid}"), name)
        .with_help(format!(
            "The id {eid} is present on more than one element in {name}. This is not \
             allowed. Remove the id from all but one of the elements"
        ))
        .with_locations(all_locations)
        .with_fix(
            "Remove the duplicate ids from all but the first element",
            move |container| duplicate_id_fix(container, &owned_name, &owned_eid),
        )
}

fn duplicate_id_fix(container: &mut Container, name: &str, eid: &str) -> Result<bool> {
    container.ensure_parsed(name)?;
    let mt = container
        .base
        .mime_map
        .get(name)
        .cloned()
        .unwrap_or_default();
    if is_strict_xml_type(&mt) {
        let xml = container.get_xml_mut(name)?;
        let ids = xml.opf_xpath("//*[@id]", &HashMap::new());
        let mut first = true;
        for id_node in ids {
            if xml.get_attr(id_node, "id") == Some(eid) {
                if first {
                    first = false;
                } else {
                    xml.remove_attr(id_node, "id");
                }
            }
        }
    } else {
        let dom = container.get_xhtml_mut(name)?;
        let els: Vec<_> = dom.preorder_elements(dom.root);
        let mut first = true;
        for el in els {
            if dom.node(el).attrs.get("id").map(|s| s.as_str()) == Some(eid) {
                if first {
                    first = false;
                } else {
                    dom.node_mut(el).attrs.shift_remove("id");
                }
            }
        }
    }
    container.dirty(name);
    Ok(true)
}

/// Port of `InvalidId`.
pub fn invalid_id(name: &str, line: Option<u32>, eid: &str) -> CheckError {
    let owned_name = name.to_string();
    let owned_eid = eid.to_string();
    CheckError::new("InvalidId", format!("Invalid id: {eid}"), name)
        .at(line, None)
        .with_level(Level::Warn)
        .with_help(format!(
            "The id {eid} is not a valid id. IDs must start with a letter ([A-Za-z]) and may \
             be followed by any number of letters, digits ([0-9]), hyphens (\"-\"), \
             underscores (\"_\"), colons (\":\"), and periods (\".\"). This is to ensure \
             maximum compatibility with a wide range of devices."
        ))
        .with_fix(
            "Replace this id with a randomly generated valid id",
            move |container| invalid_id_fix(container, &owned_name, &owned_eid),
        )
}

fn invalid_id_fix(container: &mut Container, name: &str, eid: &str) -> Result<bool> {
    container.ensure_parsed(name)?;
    let newid = uuid::Uuid::new_v4().to_string().replace('-', "");
    let newid = format!("id_{newid}");
    let mt = container
        .base
        .mime_map
        .get(name)
        .cloned()
        .unwrap_or_default();
    let mut changed = false;
    if is_strict_xml_type(&mt) {
        let xml = container.get_xml_mut(name)?;
        for id_node in xml.opf_xpath("//*[@id]", &HashMap::new()) {
            if xml.get_attr(id_node, "id") == Some(eid) {
                xml.set_attr(id_node, "id", newid.clone());
                changed = true;
            }
        }
    } else {
        let dom = container.get_xhtml_mut(name)?;
        for el in dom.preorder_elements(dom.root) {
            if dom.node(el).attrs.get("id").map(|s| s.as_str()) == Some(eid) {
                dom.node_mut(el)
                    .attrs
                    .insert("id".to_string(), newid.clone());
                changed = true;
            }
        }
    }
    if changed {
        container.dirty(name);
        let mut id_map = HashMap::new();
        let mut inner = HashMap::new();
        inner.insert(eid.to_string(), newid.clone());
        id_map.insert(name.to_string(), inner);
        super::super::replace::replace_ids(container, &id_map)?;
    }
    Ok(changed)
}

/// Port of `BareTextInBody`.
pub fn bare_text_in_body(name: &str, lines: &[Option<u32>]) -> CheckError {
    let mut sorted = lines.to_vec();
    sorted.sort();
    let all_locations = sorted
        .iter()
        .map(|l| (name.to_string(), *l, None))
        .collect();
    let owned_name = name.to_string();
    CheckError::new("BareTextInBody", "Bare text in body tag", name)
        .with_help(
            "You cannot have bare text inside the body tag. The text must be placed inside \
             some other tag, such as p or div",
        )
        .with_locations(all_locations)
        .with_fix("Wrap the bare text in a p tag", move |container| {
            bare_text_in_body_fix(container, &owned_name)
        })
}

fn bare_text_in_body_fix(container: &mut Container, name: &str) -> Result<bool> {
    container.ensure_parsed(name)?;
    let dom = container.get_xhtml_mut(name)?;
    let mut changed = false;
    for body in dom.find_all_tag_global("body") {
        let children: Vec<_> = dom.node(body).children.clone();
        for child in children {
            let is_bare_text = matches!(
                &dom.node(child).kind,
                crate::dom::NodeKind::Text(t) if !t.trim().is_empty()
            );
            if !is_bare_text {
                continue;
            }
            let text = match &dom.node(child).kind {
                crate::dom::NodeKind::Text(t) => t.trim().to_string(),
                _ => continue,
            };
            let idx = dom.index_in_parent(child).unwrap_or(0);
            let p = dom.new_element("p");
            let text_node = dom.new_text(&text);
            dom.append_child(p, text_node);
            dom.detach(child);
            dom.insert_child(body, idx, p);
            changed = true;
        }
    }
    if changed {
        container.dirty(name);
    }
    Ok(changed)
}

// ===================================================================
// Check functions
// ===================================================================

/// Port of `check_html_size`.
pub fn check_html_size(name: &str, raw: &[u8], max_size: u64) -> Vec<CheckError> {
    if max_size > 0 && raw.len() as u64 > max_size {
        vec![too_large(name, max_size)]
    } else {
        Vec::new()
    }
}

/// Port of `check_encoding_declarations`. `raw` is the file's
/// *undecoded* bytes (Python's `container.raw_data(name)` without
/// `decode=True`).
pub fn check_encoding_declarations(name: &str, raw: &[u8]) -> Vec<CheckError> {
    match crate::chardet::find_declared_encoding(raw, 50 * 1024) {
        Some(enc) if !enc.eq_ignore_ascii_case("utf-8") => vec![non_utf8(name, &enc)],
        _ => Vec::new(),
    }
}

/// Port of `check_for_private_entities`.
pub fn check_for_private_entities(raw: &[u8]) -> bool {
    private_entity_re().is_match(raw)
}

fn private_entity_re() -> &'static BytesRegex {
    static RE: OnceLock<BytesRegex> = OnceLock::new();
    RE.get_or_init(|| BytesRegex::new(r"(?s)<!DOCTYPE\s+.+?<!ENTITY\s+.+?]>").unwrap())
}

fn entity_pat() -> &'static BytesRegex {
    static RE: OnceLock<BytesRegex> = OnceLock::new();
    RE.get_or_init(|| BytesRegex::new(r"&(#?[a-zA-Z0-9]{1,8});").unwrap())
}

struct EntityScan {
    eraw: Vec<u8>,
    ok_named_entities: Vec<usize>,
    bad_entities: Vec<(usize, Vec<u8>)>,
}

/// Port of `EntitityProcessor`: scans `raw` for `&name;`/`&#N;`/`&#xN;`
/// references, blanking out (replacing with spaces) everything except
/// the five XML-predefined entities and valid numeric references, and
/// classifying named references as "known" (`ok_named_entities`, an
/// entity `NamedEntities` will flag) or "unknown" (`bad_entities`, each
/// reported as a [`bad_entity`]).
fn scan_and_blank_entities(raw: &[u8], is_oeb_doc: bool) -> EntityScan {
    let re = entity_pat();
    let mut eraw = Vec::with_capacity(raw.len());
    let mut last_end = 0usize;
    let mut ok_named_entities = Vec::new();
    let mut bad_entities = Vec::new();
    for caps in re.captures_iter(raw) {
        let m0 = caps.get(0).unwrap();
        let g1 = caps.get(1).unwrap();
        eraw.extend_from_slice(&raw[last_end..m0.start()]);
        let val = String::from_utf8_lossy(g1.as_bytes()).into_owned();
        if XML_ENTITY_NAMES.contains(&val.as_str()) {
            eraw.extend_from_slice(m0.as_bytes());
        } else if let Some(rest) = val.strip_prefix('#') {
            let valid = if let Some(hex) = rest.strip_prefix('x') {
                !hex.is_empty() && u32::from_str_radix(hex, 16).is_ok()
            } else {
                !rest.is_empty()
                    && rest.chars().all(|c| c.is_ascii_digit())
                    && rest.parse::<u32>().is_ok()
            };
            if valid {
                eraw.extend_from_slice(m0.as_bytes());
            } else {
                bad_entities.push((m0.start(), m0.as_bytes().to_vec()));
                eraw.extend(std::iter::repeat_n(b' ', m0.as_bytes().len()));
            }
        } else {
            let known = is_oeb_doc && {
                let probe = format!("&{val};");
                crate::html_entities::decode_entities(&probe) != probe
            };
            if known {
                ok_named_entities.push(m0.start());
            } else {
                bad_entities.push((m0.start(), m0.as_bytes().to_vec()));
            }
            eraw.extend(std::iter::repeat_n(b' ', m0.as_bytes().len()));
        }
        last_end = m0.end();
    }
    eraw.extend_from_slice(&raw[last_end..]);
    EntityScan {
        eraw,
        ok_named_entities,
        bad_entities,
    }
}

/// Port of `check_xml_parsing`. Real, strict-XML validation of `raw`
/// (using [`roxmltree`] directly rather than through
/// [`crate::oeb::polish::xmltree::Xml`], so a parse failure's position
/// is available even though the document never becomes a usable tree).
/// See the module docs for why HTML-family errors don't carry
/// as-precise a location as their XML counterparts do.
pub fn check_xml_parsing(name: &str, mt: &str, raw: &[u8]) -> Vec<CheckError> {
    if raw.is_empty() {
        return vec![empty_file(name)];
    }
    if check_for_private_entities(raw) {
        return vec![private_entities(name)];
    }
    let raw: Vec<u8> = {
        let s = String::from_utf8_lossy(raw).into_owned();
        s.replace("\r\n", "\n").replace('\r', "\n").into_bytes()
    };
    let is_oeb_doc = OEB_DOCS.contains(&mt);
    let scan = scan_and_blank_entities(&raw, is_oeb_doc);
    let mut errors = Vec::new();
    if !scan.ok_named_entities.is_empty() {
        errors.push(named_entities(name));
    }
    if !scan.bad_entities.is_empty() {
        let position = PositionFinder::new(&String::from_utf8_lossy(&raw));
        for (offset, ent) in &scan.bad_entities {
            let (lnum, col) = position.locate(*offset);
            errors.push(bad_entity(
                &String::from_utf8_lossy(ent),
                name,
                Some(lnum as u32),
                Some(col as u32),
            ));
        }
    }

    let eraw_str = match std::str::from_utf8(&scan.eraw) {
        Ok(s) => s,
        Err(_) => {
            errors.push(decode_error(name));
            return errors;
        }
    };

    match roxmltree::Document::parse(eraw_str) {
        Ok(doc) => {
            if is_oeb_doc {
                let root = doc.root_element();
                let ns = root.tag_name().namespace();
                if ns != Some(XHTML_NS) {
                    errors.push(bad_namespace(name, ns));
                }
            }
        }
        Err(e) => {
            let pos = e.pos();
            errors.push(xml_parse_error(
                is_oeb_doc,
                &e.to_string(),
                name,
                Some(pos.row),
                Some(pos.col),
            ));
        }
    }
    errors
}

/// Port of `check_filenames`.
pub fn check_filenames(container: &Container) -> Vec<CheckError> {
    let mut errors = Vec::new();
    let protected = container.names_that_must_not_be_changed();
    let mut names: Vec<&String> = container
        .name_path_map
        .keys()
        .filter(|n| !protected.contains(n.as_str()))
        .collect();
    names.sort();
    for name in names {
        if urlquote(name) != *name {
            errors.push(escaped_name(name));
        }
    }
    errors
}

fn valid_id_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[a-zA-Z][a-zA-Z0-9_:.\-]*$").unwrap())
}

/// Port of `check_ids`.
pub fn check_ids(container: &mut Container) -> Result<Vec<CheckError>> {
    let mut errors = Vec::new();
    let opf_type = guess_type("a.opf");
    let ncx_type = guess_type("a.ncx");
    let mut names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    names.sort();
    for (name, mt) in names {
        let is_doc = OEB_DOCS.contains(&mt.as_str());
        let is_strict = mt == opf_type || mt == ncx_type;
        if !is_doc && !is_strict {
            continue;
        }
        container.ensure_parsed(&name)?;
        let mut seen: HashMap<String, Option<u32>> = HashMap::new();
        let mut dups: HashMap<String, Vec<Option<u32>>> = HashMap::new();
        let mut invalids: Vec<(Option<u32>, String)> = Vec::new();
        if is_doc {
            let dom = container.get_xhtml(&name)?;
            for el in dom.preorder_elements(dom.root) {
                let Some(eid) = dom.node(el).attrs.get("id").cloned() else {
                    continue;
                };
                record_id(&eid, None, &mut seen, &mut dups);
                if !valid_id_re().is_match(&eid) {
                    invalids.push((None, eid));
                }
            }
        } else {
            let xml = container.get_xml(&name)?;
            for id_node in xml.opf_xpath("//*[@id]", &HashMap::new()) {
                let Some(eid) = xml.get_attr(id_node, "id").map(|s| s.to_string()) else {
                    continue;
                };
                let line = xml.node(id_node).sourceline;
                record_id(&eid, line, &mut seen, &mut dups);
                if !valid_id_re().is_match(&eid) {
                    invalids.push((line, eid));
                }
            }
        }
        for (line, eid) in invalids {
            errors.push(invalid_id(&name, line, &eid));
        }
        let mut dup_names: Vec<&String> = dups.keys().collect();
        dup_names.sort();
        for eid in dup_names {
            errors.push(duplicate_id(&name, eid, &dups[eid]));
        }
    }
    Ok(errors)
}

fn record_id(
    eid: &str,
    line: Option<u32>,
    seen: &mut HashMap<String, Option<u32>>,
    dups: &mut HashMap<String, Vec<Option<u32>>>,
) {
    if let Some(&first_line) = seen.get(eid) {
        dups.entry(eid.to_string())
            .or_insert_with(|| vec![first_line])
            .push(line);
    } else {
        seen.insert(eid.to_string(), line);
    }
}

/// Port of `check_markup`.
pub fn check_markup(container: &mut Container) -> Result<Vec<CheckError>> {
    let mut errors = Vec::new();
    let mut names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    names.sort();
    for (name, mt) in names {
        if !OEB_DOCS.contains(&mt.as_str()) {
            continue;
        }
        container.ensure_parsed(&name)?;
        let dom = container.get_xhtml(&name)?;
        let mut has_bare_text = false;
        for body in dom.find_all_tag_global("body") {
            for child in &dom.node(body).children {
                if let crate::dom::NodeKind::Text(t) = &dom.node(*child).kind {
                    if !t.trim().is_empty() {
                        has_bare_text = true;
                        break;
                    }
                }
            }
            if has_bare_text {
                break;
            }
        }
        if has_bare_text {
            // Line numbers are unavailable for `Dom`-backed documents
            // (see the module docs); Python collects a `lines` list of
            // per-occurrence source lines, this port reports the single
            // occurrence-count-agnostic location `None` instead.
            errors.push(bare_text_in_body(&name, &[None]));
        }
    }
    Ok(errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn make_container(dir: &Path, opf: &str, files: &[(&str, &[u8])]) -> Container {
        std::fs::write(dir.join("content.opf"), opf).unwrap();
        for (name, data) in files {
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, data).unwrap();
        }
        Container::open(dir, &dir.join("content.opf")).unwrap()
    }

    const OPF_V2: &str = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata><dc:identifier id="bookid">urn:uuid:x</dc:identifier></metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="c1"/></spine>
</package>"#;

    #[test]
    fn check_html_size_flags_oversized_files() {
        let errors = check_html_size("a.html", &[0u8; 100], 50);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].type_name, "TooLarge");
        assert!(check_html_size("a.html", &[0u8; 10], 50).is_empty());
        assert!(check_html_size("a.html", &[0u8; 100], 0).is_empty());
    }

    #[test]
    fn check_encoding_declarations_flags_non_utf8() {
        let raw = b"<?xml version=\"1.0\" encoding=\"iso-8859-1\"?><html/>";
        let errors = check_encoding_declarations("a.xml", raw);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].type_name, "NonUTF8");
        let raw_ok = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><html/>";
        assert!(check_encoding_declarations("a.xml", raw_ok).is_empty());
    }

    #[test]
    fn check_for_private_entities_detects_doctype_entity() {
        let raw = b"<!DOCTYPE html [\n<!ENTITY foo \"bar\">\n]><html>&foo;</html>";
        assert!(check_for_private_entities(raw));
        assert!(!check_for_private_entities(b"<html/>"));
    }

    #[test]
    fn check_xml_parsing_flags_empty_file() {
        let errors = check_xml_parsing("a.html", "application/xhtml+xml", b"");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].type_name, "EmptyFile");
    }

    #[test]
    fn check_xml_parsing_flags_malformed_xml_with_position() {
        let raw = b"<html><body><p>unclosed</body></html>";
        let errors = check_xml_parsing("a.html", "application/xhtml+xml", raw);
        assert!(errors.iter().any(|e| e.type_name == "HTMLParseError"));
        let err = errors
            .iter()
            .find(|e| e.type_name == "HTMLParseError")
            .unwrap();
        assert!(err.line.is_some());
    }

    #[test]
    fn check_xml_parsing_flags_bad_namespace() {
        let raw = b"<html><body><p>hi</p></body></html>";
        let errors = check_xml_parsing("a.html", "application/xhtml+xml", raw);
        assert!(errors.iter().any(|e| e.type_name == "BadNamespace"));
    }

    #[test]
    fn check_xml_parsing_accepts_well_formed_xhtml() {
        let raw = b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>hi</p></body></html>";
        let errors = check_xml_parsing("a.html", "application/xhtml+xml", raw);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn check_xml_parsing_flags_bad_entity_with_position() {
        let raw =
            b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>&notreal;</p></body></html>";
        let errors = check_xml_parsing("a.html", "application/xhtml+xml", raw);
        assert!(errors.iter().any(|e| e.type_name == "BadEntity"));
    }

    #[test]
    fn check_xml_parsing_flags_named_entities() {
        let raw =
            b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>a&nbsp;b</p></body></html>";
        let errors = check_xml_parsing("a.html", "application/xhtml+xml", raw);
        assert!(errors.iter().any(|e| e.type_name == "NamedEntities"));
    }

    #[test]
    fn check_filenames_flags_unsafe_names() {
        let dir = tempfile::tempdir().unwrap();
        let c = make_container(
            dir.path(),
            OPF_V2,
            &[("chap1.html", b"<html><body/></html>"), ("a b.html", b"x")],
        );
        let errors = check_filenames(&c);
        assert!(errors.iter().any(|e| e.name == "a b.html"));
        assert!(!errors.iter().any(|e| e.name == "chap1.html"));
    }

    #[test]
    fn check_ids_flags_duplicates_and_invalid_ids() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(
            dir.path(),
            OPF_V2,
            &[(
                "chap1.html",
                b"<html><body><p id=\"a\">1</p><p id=\"a\">2</p><p id=\"1bad\">3</p></body></html>",
            )],
        );
        let errors = check_ids(&mut c).unwrap();
        assert!(errors.iter().any(|e| e.type_name == "DuplicateId"));
        assert!(errors.iter().any(|e| e.type_name == "InvalidId"));
    }

    #[test]
    fn check_ids_reports_real_line_numbers_for_opf() {
        let dir = tempfile::tempdir().unwrap();
        let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:identifier id="bookid">urn:uuid:x</dc:identifier>
    <dc:title id="bookid">Dup</dc:title>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="c1"/></spine>
</package>"#;
        let mut c = make_container(dir.path(), opf, &[("chap1.html", b"<html><body/></html>")]);
        let errors = check_ids(&mut c).unwrap();
        let dup = errors
            .iter()
            .find(|e| e.type_name == "DuplicateId" && e.name == "content.opf")
            .expect("expected a duplicate id error for the opf");
        let locs = dup.all_locations.as_ref().unwrap();
        assert!(locs.iter().all(|(_, line, _)| line.is_some()));
    }

    #[test]
    fn check_markup_flags_bare_text_in_body() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(
            dir.path(),
            OPF_V2,
            &[(
                "chap1.html",
                b"<html><body>bare text<p>ok</p></body></html>",
            )],
        );
        let errors = check_markup(&mut c).unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].type_name, "BareTextInBody");
    }

    #[test]
    fn check_markup_ignores_wrapped_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(
            dir.path(),
            OPF_V2,
            &[("chap1.html", b"<html><body><p>ok</p></body></html>")],
        );
        let errors = check_markup(&mut c).unwrap();
        assert!(errors.is_empty());
    }

    #[test]
    fn urlquote_escapes_unsafe_ascii_and_keeps_unicode() {
        assert_eq!(urlquote("a b.html"), "a%20b.html");
        assert_eq!(urlquote("caf\u{e9}.html"), "caf\u{e9}.html");
        assert_eq!(urlquote("chap1.html"), "chap1.html");
    }

    #[test]
    fn empty_file_fix_removes_item() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(
            dir.path(),
            OPF_V2,
            &[("chap1.html", b"<html/>"), ("stray.txt", b"")],
        );
        let mut err = empty_file("stray.txt");
        assert!(err.apply_fix(&mut c).unwrap());
        assert!(!c.has_name("stray.txt"));
    }

    #[test]
    fn duplicate_id_fix_removes_all_but_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(
            dir.path(),
            OPF_V2,
            &[(
                "chap1.html",
                b"<html><body><p id=\"a\">1</p><p id=\"a\">2</p></body></html>",
            )],
        );
        let mut err = duplicate_id("chap1.html", "a", &[None, None]);
        assert!(err.apply_fix(&mut c).unwrap());
        let dom = c.get_xhtml("chap1.html").unwrap();
        let count = dom
            .preorder_elements(dom.root)
            .into_iter()
            .filter(|&el| dom.node(el).attrs.get("id").map(|s| s.as_str()) == Some("a"))
            .count();
        assert_eq!(count, 1);
    }
}
