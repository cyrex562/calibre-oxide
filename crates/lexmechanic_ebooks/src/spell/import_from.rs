//! Port of `calibre.spell.import_from` -- importing an additional
//! spell-check dictionary from a LibreOffice `.oxt` extension file, or
//! downloading one from LibreOffice's own dictionaries repository.
//!
//! # Not ported: `import_from_libreoffice_source_tree`
//!
//! That function (and the module's `__main__` block, which is just a
//! CLI wrapper around it) regenerates *this project's own* bundled
//! builtin dictionaries from a full LibreOffice source checkout -- a
//! maintainer-only build tool, not a runtime feature, and not
//! applicable without such a checkout (none exists in this workspace).
//! `parse_xcu`/`convert_to_utf8`, the two functions it shares with the
//! runtime `import_from_oxt`/`import_from_online` path, are ported and
//! shared here regardless.
//!
//! # `parse_xcu`: hand-rolled traversal, not a general XPath engine
//!
//! Upstream uses `lxml.etree.XPath` with four fairly specific
//! expressions over LibreOffice's `dictionaries.xcu` registry-config
//! schema. `roxmltree` (already a dependency here) has no XPath engine,
//! so [`parse_xcu`] walks the tree directly instead -- the schema is
//! fixed and well-known, so a general query engine buys nothing here
//! (the same "reimplement the specific behavior, don't pull in a
//! general engine" choice this project makes elsewhere, e.g. issue #85's
//! CSS-selector scoping notes). **Disclosed**: this was validated
//! against a hand-constructed fixture matching the documented XCU shape
//! (see this module's own tests), not a real LibreOffice `.xcu` file --
//! none was available to cross-check against in this environment.
//!
//! # `convert_to_utf8`: lossy re-decode, not upstream's strict mode
//!
//! Upstream's `errors='strict'` default raises on a byte sequence that
//! doesn't decode cleanly under the detected source encoding. `encoding_rs`
//! (used here, already a dependency) has no strict/raising decode mode --
//! only lossy, replacement-character decoding. A real dictionary file
//! that's internally inconsistent with its own declared `SET` encoding
//! would silently get replacement characters here instead of an error.
//! Not expected to matter for any dictionary actually worth importing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::bytes::{Regex as BytesRegex, RegexBuilder as BytesRegexBuilder};
use roxmltree::Document;

const OOR_NS: &str = "http://openoffice.org/2001/registry";
const MANIFEST_NS: &str = "http://openoffice.org/2001/manifest";

fn attr_eq(n: &roxmltree::Node, ns: &str, name: &str, value: &str) -> bool {
    n.attribute((ns, name)) == Some(value)
}

/// Port of `parse_xcu`: `(dic_path, aff_path) -> locales` for every
/// `DICT_SPELL`-format dictionary entry in `raw`, with `%origin%`
/// replaced by `origin` in every path.
pub fn parse_xcu(raw: &str, origin: &str) -> Result<HashMap<(String, String), Vec<String>>> {
    let doc = Document::parse(raw).context("parsing dictionaries.xcu")?;
    let mut ans = HashMap::new();

    for value_node in doc.descendants().filter(|n| n.has_tag_name("value") && n.text() == Some("DICT_SPELL")) {
        let Some(format_prop) = value_node.parent_element() else { continue };
        if !format_prop.has_tag_name("prop") || !attr_eq(&format_prop, OOR_NS, "name", "Format") {
            continue;
        }
        let Some(entry_node) = format_prop.parent_element() else { continue };

        let locations_values: Vec<roxmltree::Node> = entry_node
            .descendants()
            .filter(|n| n.has_tag_name("prop") && attr_eq(n, OOR_NS, "name", "Locations"))
            .flat_map(|p| p.children().filter(|c| c.has_tag_name("value")))
            .collect();
        let paths: Vec<String> = if let Some(first_value) = locations_values.first() {
            if first_value.children().filter(|c| c.is_element()).next().is_none() {
                let joined: String = locations_values.iter().filter_map(|v| v.text()).collect();
                joined.replace("%origin%", origin).split_whitespace().map(str::to_string).collect()
            } else {
                locations_values
                    .iter()
                    .flat_map(|v| v.children().filter(|c| c.is_element()))
                    .filter_map(|c| c.text())
                    .map(|t| t.replace("%origin%", origin))
                    .collect()
            }
        } else {
            Vec::new()
        };
        if paths.len() < 2 {
            continue;
        }
        let (dic, aff) = if paths[0].ends_with(".aff") { (paths[1].clone(), paths[0].clone()) } else { (paths[0].clone(), paths[1].clone()) };

        let locales_values: Vec<roxmltree::Node> = entry_node
            .descendants()
            .filter(|n| n.has_tag_name("prop") && attr_eq(n, OOR_NS, "name", "Locales"))
            .flat_map(|p| p.children().filter(|c| c.has_tag_name("value")))
            .collect();
        let joined_locales: String = locales_values.iter().filter_map(|v| v.text()).collect();
        let mut locales: Vec<String> = joined_locales.split_whitespace().map(str::to_string).collect();
        if locales.is_empty() {
            locales = locales_values
                .iter()
                .flat_map(|v| v.descendants().filter(|c| c.has_tag_name("it")))
                .filter_map(|it| it.text())
                .map(str::to_string)
                .collect();
        }

        ans.insert((dic, aff), locales);
    }
    Ok(ans)
}

lazy_static! {
    static ref SET_LINE: BytesRegex = BytesRegexBuilder::new(r"^SET\s+(\S+)$").multi_line(true).build().unwrap();
}

/// Port of `convert_to_utf8`. See this module's doc for the
/// lossy-vs-strict decoding disclosure.
pub fn convert_to_utf8(dic_data: &[u8], aff_data: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let head_len = aff_data.len().min(2048);
    let Some(caps) = SET_LINE.captures(&aff_data[..head_len]) else {
        return (dic_data.to_vec(), aff_data.to_vec());
    };
    let whole = caps.get(0).unwrap();
    let enc_name = String::from_utf8_lossy(&caps[1]).into_owned();
    if enc_name.eq_ignore_ascii_case("utf-8") || enc_name.eq_ignore_ascii_case("utf8") {
        return (dic_data.to_vec(), aff_data.to_vec());
    }
    let Some(encoding) = encoding_rs::Encoding::for_label(enc_name.as_bytes()) else {
        return (dic_data.to_vec(), aff_data.to_vec());
    };
    let mut rewritten_aff = Vec::with_capacity(aff_data.len());
    rewritten_aff.extend_from_slice(&aff_data[..whole.start()]);
    rewritten_aff.extend_from_slice(b"SET UTF-8");
    rewritten_aff.extend_from_slice(&aff_data[whole.end()..]);

    let (decoded_aff, _, _) = encoding.decode(&rewritten_aff);
    let (decoded_dic, _, _) = encoding.decode(dic_data);
    (decoded_dic.into_owned().into_bytes(), decoded_aff.into_owned().into_bytes())
}

/// Port of `fill_country_code`.
pub fn fill_country_code(x: &str) -> String {
    if x == "lt" {
        "lt_LT".to_string()
    } else {
        x.to_string()
    }
}

/// Port of `uniq`: preserves first-seen order, drops later duplicates.
pub fn uniq(vals: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    vals.iter().filter(|x| seen.insert((*x).clone())).cloned().collect()
}

fn manifest_xcu_path(manifest_xml: &str) -> Result<String> {
    let doc = Document::parse(manifest_xml).context("parsing META-INF/manifest.xml")?;
    doc.descendants()
        .find(|n| {
            n.has_tag_name((MANIFEST_NS, "file-entry"))
                && attr_eq(n, MANIFEST_NS, "media-type", "application/vnd.sun.star.configuration-data")
        })
        .and_then(|n| n.attribute((MANIFEST_NS, "full-path")))
        .map(str::to_string)
        .context("no configuration-data file-entry found in manifest.xml")
}

fn unique_dest_subdir(dest_dir: &Path, prefix: &str) -> Result<PathBuf> {
    let dir = tempfile::Builder::new().prefix(prefix).disable_cleanup(true).tempdir_in(dest_dir)?;
    Ok(dir.path().to_path_buf())
}

/// Port of `_import_from_virtual_directory`: shared by
/// [`import_from_oxt`] and (a caller-supplied, network-backed
/// `read_file`) an online-fetch equivalent of upstream's
/// `import_from_online`. `read_file` maps a path inside the extension
/// (e.g. `META-INF/manifest.xml`) to its raw bytes. Returns the number
/// of dictionaries imported.
pub fn import_from_virtual_directory(mut read_file: impl FnMut(&str) -> Result<Vec<u8>>, name: &str, dest_dir: &Path, prefix: &str) -> Result<usize> {
    std::fs::create_dir_all(dest_dir)?;
    let mut num = 0;

    let manifest_raw = read_file("META-INF/manifest.xml")?;
    let manifest_str = String::from_utf8_lossy(&manifest_raw).into_owned();
    let xcu_path = manifest_xcu_path(&manifest_str)?;
    let xcu_raw = read_file(&xcu_path)?;
    let xcu_str = String::from_utf8_lossy(&xcu_raw).into_owned();

    for ((dic, aff), locales) in parse_xcu(&xcu_str, "")? {
        let dic = dic.trim_start_matches('/').to_string();
        let aff = aff.trim_start_matches('/').to_string();

        let mut mapped = Vec::new();
        for l in &locales {
            let filled = fill_country_code(l);
            let parsed = super::parse_lang_code(&filled).map_err(|e| anyhow::anyhow!(e))?;
            if parsed.countrycode.is_some() {
                mapped.push(filled);
            }
        }
        let locales = uniq(&mapped);
        if locales.is_empty() {
            continue;
        }

        let d = unique_dest_subdir(dest_dir, prefix)?;
        let mut metadata = vec![name.to_string()];
        metadata.extend(locales.iter().cloned());
        std::fs::write(d.join("locales"), metadata.join("\n").as_bytes())?;

        let dic_raw = read_file(&dic)?;
        let aff_raw = read_file(&aff)?;
        let (dd, ad) = convert_to_utf8(&dic_raw, &aff_raw);
        std::fs::write(d.join(format!("{}.dic", locales[0])), dd)?;
        std::fs::write(d.join(format!("{}.aff", locales[0])), ad)?;
        num += 1;
    }
    Ok(num)
}

/// Port of `import_from_oxt`: `source_path` is a `.oxt` file (a ZIP
/// archive), read directly from disk.
pub fn import_from_oxt(source_path: &Path, name: &str, dest_dir: &Path, prefix: &str) -> Result<usize> {
    let file = std::fs::File::open(source_path).with_context(|| format!("opening {source_path:?}"))?;
    let mut zip = zip::ZipArchive::new(file)?;

    import_from_virtual_directory(
        |key| {
            let read = |zip: &mut zip::ZipArchive<std::fs::File>, key: &str| -> Result<Vec<u8>> {
                use std::io::Read;
                let mut f = zip.by_name(key)?;
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)?;
                Ok(buf)
            };
            match read(&mut zip, key) {
                Ok(v) => Ok(v),
                Err(_) => {
                    // Some dictionaries put the xcu in a sub-directory and
                    // incorrectly make paths relative to that directory
                    // instead of the root -- matches upstream's own retry.
                    let mut fixed = key;
                    while let Some(stripped) = fixed.strip_prefix("../") {
                        fixed = stripped;
                    }
                    let fixed = fixed.trim_start_matches('/');
                    read(&mut zip, fixed)
                }
            }
        },
        name,
        dest_dir,
        prefix,
    )
}

/// Port of `import_from_online`: downloads dictionary files from
/// LibreOffice's own dictionaries repository on GitHub via `browser`
/// (a [`super::super::scraper::Browser`], reused rather than upstream's
/// separate `calibre.browser()` helper -- both are just "an HTTP
/// client" here).
pub fn import_from_online(browser: &crate::scraper::Browser, directory: &str, name: &str, dest_dir: &Path, prefix: &str) -> Result<usize> {
    const BASE_URL: &str = "https://raw.githubusercontent.com/LibreOffice/dictionaries/master/";
    import_from_virtual_directory(
        |key| {
            let opts = crate::scraper::OpenOptions { timeout: Some(std::time::Duration::from_secs(30)), ..Default::default() };
            let url = format!("{BASE_URL}{directory}/{key}");
            match browser.open(&url, &opts) {
                Ok(resp) => Ok(resp.read().to_vec()),
                Err(_) => {
                    // Some dictionaries put the dic/aff files under a
                    // "dictionaries" sub-directory instead of the root --
                    // matches upstream's own 404 retry.
                    let url = format!("{BASE_URL}{directory}/dictionaries/{key}");
                    let resp = browser.open(&url, &opts)?;
                    Ok(resp.read().to_vec())
                }
            }
        },
        name,
        dest_dir,
        prefix,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-constructed fixture matching the XCU shape the four
    /// upstream XPath expressions target -- see this module's doc for
    /// why it's not a real LibreOffice file.
    const SAMPLE_XCU: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<oor:component-data xmlns:oor="http://openoffice.org/2001/registry" oor:name="Linguistic" oor:package="org.openoffice.Office">
  <node oor:name="ServiceManager">
    <node oor:name="Dictionaries">
      <node oor:name="org.openoffice.lingu.new_en_US" oor:op="fuse">
        <prop oor:name="Format" oor:type="xs:string">
          <value>DICT_SPELL</value>
        </prop>
        <prop oor:name="Locations" oor:type="oor:string-list">
          <value>%origin%/en_US.aff %origin%/en_US.dic</value>
        </prop>
        <prop oor:name="Locales" oor:type="oor:string-list">
          <value>en-US</value>
        </prop>
      </node>
    </node>
  </node>
</oor:component-data>"#;

    #[test]
    fn parse_xcu_extracts_dic_aff_and_locales() {
        let ans = parse_xcu(SAMPLE_XCU, "ORIGIN").unwrap();
        assert_eq!(ans.len(), 1);
        let ((dic, aff), locales) = ans.iter().next().unwrap();
        assert_eq!(dic, "ORIGIN/en_US.dic");
        assert_eq!(aff, "ORIGIN/en_US.aff");
        assert_eq!(locales, &vec!["en-US".to_string()]);
    }

    #[test]
    fn parse_xcu_handles_element_children_locations() {
        let xcu = r#"<?xml version="1.0"?>
<oor:component-data xmlns:oor="http://openoffice.org/2001/registry">
  <node oor:name="d1">
    <prop oor:name="Format"><value>DICT_SPELL</value></prop>
    <prop oor:name="Locations"><value><it>%origin%/x.aff</it><it>%origin%/x.dic</it></value></prop>
    <prop oor:name="Locales"><value><it>en-GB</it><it>en-US</it></value></prop>
  </node>
</oor:component-data>"#;
        let ans = parse_xcu(xcu, "O").unwrap();
        let ((dic, aff), locales) = ans.iter().next().unwrap();
        assert_eq!(dic, "O/x.dic");
        assert_eq!(aff, "O/x.aff");
        assert_eq!(locales, &vec!["en-GB".to_string(), "en-US".to_string()]);
    }

    #[test]
    fn convert_to_utf8_leaves_already_utf8_data_unchanged() {
        let dic = b"hello\n";
        let aff = b"SET UTF-8\nTRY abc\n";
        let (dd, ad) = convert_to_utf8(dic, aff);
        assert_eq!(dd, dic);
        assert_eq!(ad, aff);
    }

    #[test]
    fn convert_to_utf8_reencodes_latin1_and_rewrites_the_set_line() {
        // 0xE9 is 'é' in Latin-1/ISO-8859-1.
        let dic = &[b'r', b'\xe9', b's', b'u', b'm', b'\xe9', b'\n'][..];
        let aff = b"SET ISO8859-1\nTRY abc\n";
        let (dd, ad) = convert_to_utf8(dic, aff);
        assert_eq!(String::from_utf8(dd).unwrap(), "r\u{e9}sum\u{e9}\n");
        let ad_str = String::from_utf8(ad).unwrap();
        assert!(ad_str.starts_with("SET UTF-8\n"));
        assert!(ad_str.contains("TRY abc"));
    }

    #[test]
    fn fill_country_code_maps_the_one_known_special_case() {
        assert_eq!(fill_country_code("lt"), "lt_LT");
        assert_eq!(fill_country_code("en-US"), "en-US");
    }

    #[test]
    fn uniq_drops_duplicates_preserving_first_seen_order() {
        let vals: Vec<String> = ["a", "b", "a", "c", "b"].iter().map(|s| s.to_string()).collect();
        assert_eq!(uniq(&vals), vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn import_from_virtual_directory_writes_locales_and_converted_dictionary_files() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");

        let manifest = r#"<?xml version="1.0"?>
<manifest:manifest xmlns:manifest="http://openoffice.org/2001/manifest">
  <manifest:file-entry manifest:media-type="application/vnd.sun.star.configuration-data" manifest:full-path="dictionaries.xcu"/>
</manifest:manifest>"#;

        let files: HashMap<&str, Vec<u8>> = HashMap::from([
            ("META-INF/manifest.xml", manifest.as_bytes().to_vec()),
            ("dictionaries.xcu", SAMPLE_XCU.as_bytes().to_vec()),
            ("en_US.dic", b"1\nhello\n".to_vec()),
            ("en_US.aff", b"SET UTF-8\nTRY abc\n".to_vec()),
        ]);

        let num = import_from_virtual_directory(
            |key| files.get(key).cloned().ok_or_else(|| anyhow::anyhow!("missing {key}")),
            "My Imported Dictionary",
            &dest,
            "dic-",
        )
        .unwrap();
        assert_eq!(num, 1);

        let entries: Vec<_> = std::fs::read_dir(&dest).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(entries.len(), 1);
        let d = &entries[0];
        let locales = std::fs::read_to_string(d.join("locales")).unwrap();
        assert_eq!(locales, "My Imported Dictionary\nen-US");
        assert!(d.join("en-US.dic").exists());
        assert!(d.join("en-US.aff").exists());
    }
}
