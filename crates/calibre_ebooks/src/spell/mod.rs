//! Port of `old_src/src/calibre/spell/` -- calibre's spell-checking
//! subsystem: locale parsing (`__init__.py`), word/sentence
//! segmentation (`break_iterator.py`), the dictionary-lookup engine
//! itself (`dictionary.py`), and importing additional dictionaries from
//! LibreOffice sources (`import_from.py`).
//!
//! # Locale parsing: narrow `parse_lang_code`, not the full ISO tables
//!
//! `calibre.spell.parse_lang_code` depends on
//! `calibre.utils.localization.canonicalize_lang`/`load_iso3166`, both
//! large generated tables neither this crate nor any other in the
//! workspace has ported. [`parse_lang_code`] ports the function's
//! actual algorithm (split on `-`/`_`, drop `Cyrl`/`Latn` script
//! subtags, canonicalize the primary subtag, treat a two-letter second
//! subtag as a country code) against a narrow, hand-picked ISO 639-1 ->
//! ISO 639-2/T table covering the common languages real books are
//! written in, rather than the full multi-hundred-entry table -- the
//! same "narrow slice, not a full port" boundary this project draws
//! elsewhere (see `oeb::polish::opf`'s own `canonicalize_lang`).
//!
//! This type and function used to live privately in
//! `oeb::polish::spell` (which only needed `parse_lang_code`, not the
//! rest of this module); they're canonical here now, and that module
//! imports them from here instead of keeping its own copy.

pub mod break_iterator;
pub mod dictionary;
pub mod import_from;

/// Port of `calibre.spell.DictionaryLocale`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DictionaryLocale {
    pub langcode: String,
    pub countrycode: Option<String>,
}

impl DictionaryLocale {
    pub fn new(langcode: impl Into<String>, countrycode: Option<String>) -> Self {
        DictionaryLocale { langcode: langcode.into(), countrycode }
    }
}

/// Narrow ISO 639-1 -> ISO 639-2/T table. See the module docs.
const LANG_MAP: &[(&str, &str)] = &[
    ("en", "eng"),
    ("fr", "fra"),
    ("de", "deu"),
    ("es", "spa"),
    ("it", "ita"),
    ("pt", "por"),
    ("nl", "nld"),
    ("sv", "swe"),
    ("da", "dan"),
    ("no", "nor"),
    ("fi", "fin"),
    ("pl", "pol"),
    ("ru", "rus"),
    ("ja", "jpn"),
    ("zh", "zho"),
    ("ko", "kor"),
    ("ar", "ara"),
    ("he", "heb"),
    ("tr", "tur"),
    ("el", "ell"),
    ("cs", "ces"),
    ("hu", "hun"),
    ("ro", "ron"),
    ("uk", "ukr"),
    ("bg", "bul"),
    ("hr", "hrv"),
    ("sr", "srp"),
    ("sk", "slk"),
    ("sl", "slv"),
    ("lt", "lit"),
    ("lv", "lav"),
    ("et", "est"),
    ("is", "isl"),
    ("ga", "gle"),
    ("cy", "cym"),
    ("ca", "cat"),
    ("eu", "eus"),
    ("gl", "glg"),
    ("hi", "hin"),
    ("bn", "ben"),
    ("ta", "tam"),
    ("te", "tel"),
    ("vi", "vie"),
    ("th", "tha"),
    ("id", "ind"),
    ("ms", "msa"),
    ("fa", "fas"),
    ("ur", "urd"),
];

fn narrow_canonicalize_lang(code: &str) -> Option<String> {
    let c = code.trim().to_lowercase();
    if c.is_empty() {
        return None;
    }
    if c.len() == 3 && c.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return Some(c);
    }
    LANG_MAP.iter().find(|(k, _)| *k == c).map(|(_, v)| v.to_string())
}

/// Port of `calibre.spell.parse_lang_code`. See the module docs for the
/// table-size tradeoff.
pub fn parse_lang_code(raw: &str) -> std::result::Result<DictionaryLocale, String> {
    let normalized = raw.replace('_', "-");
    let mut parts: Vec<&str> = normalized.split('-').collect();
    if parts.is_empty() {
        parts.push("");
    }
    let lc = narrow_canonicalize_lang(parts[0]).ok_or_else(|| format!("Invalid language code: {raw:?}"))?;
    parts.retain(|p| *p != "Cyrl" && *p != "Latn");
    let cc = if parts.len() > 1 {
        let q = parts[1].to_uppercase();
        if q.len() == 2 && q.chars().all(|c| c.is_ascii_alphabetic()) {
            Some(q)
        } else {
            None
        }
    } else {
        None
    };
    Ok(DictionaryLocale::new(lc, cc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lang_code_handles_common_forms() {
        assert_eq!(parse_lang_code("en").unwrap(), DictionaryLocale::new("eng", None));
        assert_eq!(parse_lang_code("en-US").unwrap(), DictionaryLocale::new("eng", Some("US".to_string())));
        assert_eq!(parse_lang_code("fra").unwrap(), DictionaryLocale::new("fra", None));
        assert!(parse_lang_code("").is_err());
        assert!(parse_lang_code("xx-zz-not-a-lang").is_err());
    }

    #[test]
    fn parse_lang_code_drops_script_subtags() {
        assert_eq!(parse_lang_code("sr-Cyrl-RS").unwrap(), DictionaryLocale::new("srp", Some("RS".to_string())));
    }
}
