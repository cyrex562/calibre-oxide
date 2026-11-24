//! Port of `old_src/src/calibre/ebooks/rtf2xml/get_char_map.py`
//! (`GetCharMap`).
//!
//! Parses one named section out of [`super::char_set::CHAR_SET`] into a
//! `key -> replacement` map. This is a straight line-oriented scan, not
//! a pre-built index: the Python re-scans the whole string on every
//! call (`self.__char_file.seek(0)`), and this port does the same
//! (`str::lines()` over the shared `&'static str` is cheap enough that
//! there is no reason to diverge and cache).

use std::collections::HashMap;

use thiserror::Error;

use super::char_set::CHAR_SET;

/// Port of `raise self.__bug_handler(msg)` in `GetCharMap.get_char_map`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("no map found\nmap is \"{0}\"\n")]
pub struct MapNotFoundError(pub String);

/// Port of `GetCharMap.get_char_map`.
///
/// Scans [`CHAR_SET`] for a `<map>` ... `</map>` section (matched by
/// substring, exactly as the Python's `begin_element in line` /
/// `end_element in line` checks do -- not by requiring the tag to be
/// the entire line) and returns a map of field 1 -> field 3 for every
/// non-blank `:`-delimited record inside it.
///
/// # Preserved upstream quirks
///
/// - **`fields[1].replace('\\colon', ':')` is a no-op in the Python**:
///   the result of `str.replace` (which returns a new string) is
///   never assigned back to anything, so keys that spell out the
///   literal text `\colon` (used in the source table in place of a
///   raw `:`, which would otherwise be parsed as a field delimiter)
///   keep that literal text rather than being unescaped to `:`. Ported
///   as-is: this function does not perform the replacement either.
/// - **A source-level backslash-newline splice in the `bottom_128`
///   table** (see [`super::char_set`]'s module docs) means the
///   `bottom_128` map's `"'5C"` entry (REVERSE SOLIDUS / backslash) has
///   already-wrong content, and it has no `"'5D"` entry at all (RIGHT
///   SQUARE BRACKET's own record was swallowed into the previous
///   line). Both are verified in the tests below.
/// - **Lines with fewer than 4 `:`-delimited fields inside a matched
///   section would panic-equivalent in the Python** (an uncaught
///   `IndexError` reading `fields[3]`). No real record in any tagged
///   section is this malformed (verified: the only two `char_set.py`
///   lines with fewer than 4 fields, `#mac_roman` and `#unused
///   character maps`, are both comment lines that sit *between*
///   sections, never inside one -- see `super::char_set`'s module
///   docs), so this is not expected to trigger on real data. Per this
///   crate's fault-tolerance convention of never panicking on
///   malformed input, such a line is skipped rather than crashing.
pub fn get_char_map(map: &str) -> Result<HashMap<String, String>, MapNotFoundError> {
    let begin_element = format!("<{map}>");
    let end_element = format!("</{map}>");

    let mut found_map = false;
    let mut map_dict = HashMap::new();

    for line in CHAR_SET.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if !found_map {
            if line.contains(&begin_element) {
                found_map = true;
            }
            continue;
        }
        if line.contains(&end_element) {
            break;
        }
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() < 4 {
            // See "Preserved upstream quirks" above: not expected on
            // real data, skipped rather than panicking.
            continue;
        }
        map_dict.insert(fields[1].to_string(), fields[3].to_string());
    }

    if !found_map {
        return Err(MapNotFoundError(map.to_string()));
    }
    Ok(map_dict)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_on_unknown_map_name() {
        let err = get_char_map("does_not_exist").unwrap_err();
        assert_eq!(err, MapNotFoundError("does_not_exist".to_string()));
    }

    // ---- ms_standard / ms_symbol / ms_dingbats / ms_wingdings ----

    #[test]
    fn ms_standard_maps_named_entities_to_xml_numeric_refs() {
        let map = get_char_map("ms_standard").unwrap();
        assert_eq!(map.get("ldblquote"), Some(&"&#x201C;".to_string()));
        assert_eq!(map.get("rdblquote"), Some(&"&#x201D;".to_string()));
        assert_eq!(map.get("bullet"), Some(&"&#x00B7;".to_string()));
        assert_eq!(map.get("tab"), Some(&"&#x009;".to_string()));
    }

    #[test]
    fn ms_symbol_overrides_same_keys_with_different_codepoints() {
        // Same *keys* as ms_standard (ldblquote, rquote, ...) but
        // different Symbol-font glyphs -- verifies section boundaries
        // are respected, not just "does the key exist anywhere".
        let map = get_char_map("ms_symbol").unwrap();
        assert_eq!(map.get("ldblquote"), Some(&"&#x00AE;".to_string()));
        assert_eq!(map.get("rquote"), Some(&"&#x220F;".to_string()));
    }

    #[test]
    fn ms_dingbats_and_ms_wingdings_spot_check() {
        let dingbats = get_char_map("ms_dingbats").unwrap();
        assert_eq!(dingbats.get("rquote"), Some(&"&#x2192;".to_string()));

        let wingdings = get_char_map("ms_wingdings").unwrap();
        assert_eq!(wingdings.get("endash"), Some(&"&amp;".to_string()));
        assert_eq!(wingdings.get("bullet"), Some(&"&#x2609;".to_string()));
    }

    // ---- not_unicode / bottom_128 control + ASCII chars ----

    #[test]
    fn not_unicode_maps_control_char_names_to_numeric_refs() {
        let map = get_char_map("not_unicode").unwrap();
        assert_eq!(map.get("'00"), Some(&"&#x0000;".to_string()));
        assert_eq!(map.get("'1B"), Some(&"&#x001B;".to_string()));
    }

    #[test]
    fn bottom_128_maps_printable_ascii() {
        let map = get_char_map("bottom_128").unwrap();
        assert_eq!(map.get("'41"), Some(&"A".to_string()));
        assert_eq!(map.get("'7A"), Some(&"z".to_string()));
    }

    #[test]
    fn bottom_128_reverse_solidus_quirk_is_preserved() {
        // See super::char_set's module docs and this module's own
        // "Preserved upstream quirks": a source-level backslash-newline
        // splice corrupts the REVERSE SOLIDUS record and swallows the
        // RIGHT SQUARE BRACKET record that followed it.
        let map = get_char_map("bottom_128").unwrap();
        assert_eq!(
            map.get("'5C"),
            Some(&"RIGHT SQUARE BRACKET".to_string()),
            "expected the corrupted value from the upstream splice"
        );
        assert_eq!(
            map.get("'5D"),
            None,
            "RIGHT SQUARE BRACKET's own record should have been swallowed"
        );
    }

    // ---- ansicpgNNNN codepages ----

    #[test]
    fn ansicpg1252_has_entries() {
        let map = get_char_map("ansicpg1252").unwrap();
        assert!(!map.is_empty());
    }

    #[test]
    fn ansicpg950_has_entries() {
        let map = get_char_map("ansicpg950").unwrap();
        assert!(!map.is_empty());
    }

    // ---- SYMBOL / wingdings / dingbats font maps ----

    #[test]
    fn symbol_font_map_has_entries() {
        let map = get_char_map("SYMBOL").unwrap();
        assert!(!map.is_empty());
    }

    #[test]
    fn wingdings_and_dingbats_font_maps_have_entries() {
        assert!(!get_char_map("wingdings").unwrap().is_empty());
        assert!(!get_char_map("dingbats").unwrap().is_empty());
    }

    #[test]
    fn caps_uni_has_entries() {
        let map = get_char_map("caps_uni").unwrap();
        assert!(!map.is_empty());
    }

    // ---- the "\colon no-op replace" quirk ----

    #[test]
    fn colon_literal_keys_are_not_unescaped() {
        // Port of the `fields[1].replace('\\colon', ':')` no-op quirk
        // documented above: `ascii_to_hex`'s COLON record spells its
        // key field as the literal text "\colon" (a stand-in to avoid
        // being parsed as a `:` field delimiter), and it is never
        // unescaped back to a bare ':'.
        let map = get_char_map("ascii_to_hex").unwrap();
        assert_eq!(map.get("\\colon"), Some(&"'3A".to_string()));
        assert_eq!(map.get(":"), None);
    }
}
