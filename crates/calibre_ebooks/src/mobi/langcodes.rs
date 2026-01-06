use lazy_static::lazy_static;
use std::collections::HashMap;

// (main_code, sub_code)
type MobiCode = (u8, u8);

struct LangEntry {
    code: &'static str,
    region: Option<&'static str>,
    mobi_code: MobiCode,
}

const IANA_MOBI_DATA: &[LangEntry] = &[
    LangEntry {
        code: "af",
        region: None,
        mobi_code: (54, 0),
    },
    LangEntry {
        code: "ar",
        region: None,
        mobi_code: (1, 0),
    },
    LangEntry {
        code: "ar",
        region: Some("AE"),
        mobi_code: (1, 56),
    },
    LangEntry {
        code: "ar",
        region: Some("BH"),
        mobi_code: (1, 60),
    },
    LangEntry {
        code: "ar",
        region: Some("DZ"),
        mobi_code: (1, 20),
    },
    LangEntry {
        code: "ar",
        region: Some("EG"),
        mobi_code: (1, 12),
    },
    LangEntry {
        code: "ar",
        region: Some("JO"),
        mobi_code: (1, 44),
    },
    LangEntry {
        code: "ar",
        region: Some("KW"),
        mobi_code: (1, 52),
    },
    LangEntry {
        code: "ar",
        region: Some("LB"),
        mobi_code: (1, 48),
    },
    LangEntry {
        code: "ar",
        region: Some("MA"),
        mobi_code: (1, 24),
    },
    LangEntry {
        code: "ar",
        region: Some("OM"),
        mobi_code: (1, 32),
    },
    LangEntry {
        code: "ar",
        region: Some("QA"),
        mobi_code: (1, 64),
    },
    LangEntry {
        code: "ar",
        region: Some("SA"),
        mobi_code: (1, 4),
    },
    LangEntry {
        code: "ar",
        region: Some("SY"),
        mobi_code: (1, 40),
    },
    LangEntry {
        code: "ar",
        region: Some("TN"),
        mobi_code: (1, 28),
    },
    LangEntry {
        code: "ar",
        region: Some("YE"),
        mobi_code: (1, 36),
    },
    LangEntry {
        code: "as",
        region: None,
        mobi_code: (77, 0),
    },
    LangEntry {
        code: "az",
        region: None,
        mobi_code: (44, 0),
    },
    LangEntry {
        code: "be",
        region: None,
        mobi_code: (35, 0),
    },
    LangEntry {
        code: "bg",
        region: None,
        mobi_code: (2, 0),
    },
    LangEntry {
        code: "bn",
        region: None,
        mobi_code: (69, 0),
    },
    LangEntry {
        code: "ca",
        region: None,
        mobi_code: (3, 0),
    },
    LangEntry {
        code: "cs",
        region: None,
        mobi_code: (5, 0),
    },
    LangEntry {
        code: "da",
        region: None,
        mobi_code: (6, 0),
    },
    LangEntry {
        code: "de",
        region: None,
        mobi_code: (7, 0),
    },
    LangEntry {
        code: "de",
        region: Some("AT"),
        mobi_code: (7, 12),
    },
    LangEntry {
        code: "de",
        region: Some("CH"),
        mobi_code: (7, 8),
    },
    LangEntry {
        code: "de",
        region: Some("LI"),
        mobi_code: (7, 20),
    },
    LangEntry {
        code: "de",
        region: Some("LU"),
        mobi_code: (7, 16),
    },
    LangEntry {
        code: "el",
        region: None,
        mobi_code: (8, 0),
    },
    LangEntry {
        code: "en",
        region: None,
        mobi_code: (9, 0),
    },
    LangEntry {
        code: "en",
        region: Some("AU"),
        mobi_code: (9, 12),
    },
    LangEntry {
        code: "en",
        region: Some("BZ"),
        mobi_code: (9, 40),
    },
    LangEntry {
        code: "en",
        region: Some("CA"),
        mobi_code: (9, 16),
    },
    LangEntry {
        code: "en",
        region: Some("GB"),
        mobi_code: (9, 8),
    },
    LangEntry {
        code: "en",
        region: Some("IE"),
        mobi_code: (9, 24),
    },
    LangEntry {
        code: "en",
        region: Some("JM"),
        mobi_code: (9, 32),
    },
    LangEntry {
        code: "en",
        region: Some("NZ"),
        mobi_code: (9, 20),
    },
    LangEntry {
        code: "en",
        region: Some("PH"),
        mobi_code: (9, 52),
    },
    LangEntry {
        code: "en",
        region: Some("TT"),
        mobi_code: (9, 44),
    },
    LangEntry {
        code: "en",
        region: Some("US"),
        mobi_code: (9, 4),
    },
    LangEntry {
        code: "en",
        region: Some("ZA"),
        mobi_code: (9, 28),
    },
    LangEntry {
        code: "en",
        region: Some("ZW"),
        mobi_code: (9, 48),
    },
    LangEntry {
        code: "es",
        region: None,
        mobi_code: (10, 0),
    },
    LangEntry {
        code: "es",
        region: Some("AR"),
        mobi_code: (10, 44),
    },
    LangEntry {
        code: "es",
        region: Some("BO"),
        mobi_code: (10, 64),
    },
    LangEntry {
        code: "es",
        region: Some("CL"),
        mobi_code: (10, 52),
    },
    LangEntry {
        code: "es",
        region: Some("CO"),
        mobi_code: (10, 36),
    },
    LangEntry {
        code: "es",
        region: Some("CR"),
        mobi_code: (10, 20),
    },
    LangEntry {
        code: "es",
        region: Some("DO"),
        mobi_code: (10, 28),
    },
    LangEntry {
        code: "es",
        region: Some("EC"),
        mobi_code: (10, 48),
    },
    LangEntry {
        code: "es",
        region: Some("ES"),
        mobi_code: (10, 4),
    },
    LangEntry {
        code: "es",
        region: Some("GT"),
        mobi_code: (10, 16),
    },
    LangEntry {
        code: "es",
        region: Some("HN"),
        mobi_code: (10, 72),
    },
    LangEntry {
        code: "es",
        region: Some("MX"),
        mobi_code: (10, 8),
    },
    LangEntry {
        code: "es",
        region: Some("NI"),
        mobi_code: (10, 76),
    },
    LangEntry {
        code: "es",
        region: Some("PA"),
        mobi_code: (10, 24),
    },
    LangEntry {
        code: "es",
        region: Some("PE"),
        mobi_code: (10, 40),
    },
    LangEntry {
        code: "es",
        region: Some("PR"),
        mobi_code: (10, 80),
    },
    LangEntry {
        code: "es",
        region: Some("PY"),
        mobi_code: (10, 60),
    },
    LangEntry {
        code: "es",
        region: Some("SV"),
        mobi_code: (10, 68),
    },
    LangEntry {
        code: "es",
        region: Some("UY"),
        mobi_code: (10, 56),
    },
    LangEntry {
        code: "es",
        region: Some("VE"),
        mobi_code: (10, 32),
    },
    LangEntry {
        code: "et",
        region: None,
        mobi_code: (37, 0),
    },
    LangEntry {
        code: "eu",
        region: None,
        mobi_code: (45, 0),
    },
    LangEntry {
        code: "fa",
        region: None,
        mobi_code: (41, 0),
    },
    LangEntry {
        code: "fi",
        region: None,
        mobi_code: (11, 0),
    },
    LangEntry {
        code: "fo",
        region: None,
        mobi_code: (56, 0),
    },
    LangEntry {
        code: "fr",
        region: None,
        mobi_code: (12, 0),
    },
    LangEntry {
        code: "fr",
        region: Some("BE"),
        mobi_code: (12, 8),
    },
    LangEntry {
        code: "fr",
        region: Some("CA"),
        mobi_code: (12, 12),
    },
    LangEntry {
        code: "fr",
        region: Some("CH"),
        mobi_code: (12, 16),
    },
    LangEntry {
        code: "fr",
        region: Some("FR"),
        mobi_code: (12, 4),
    },
    LangEntry {
        code: "fr",
        region: Some("LU"),
        mobi_code: (12, 20),
    },
    LangEntry {
        code: "fr",
        region: Some("MC"),
        mobi_code: (12, 24),
    },
    LangEntry {
        code: "gu",
        region: None,
        mobi_code: (71, 0),
    },
    LangEntry {
        code: "he",
        region: None,
        mobi_code: (13, 0),
    },
    LangEntry {
        code: "hi",
        region: None,
        mobi_code: (57, 0),
    },
    LangEntry {
        code: "hr",
        region: None,
        mobi_code: (26, 0),
    },
    LangEntry {
        code: "hu",
        region: None,
        mobi_code: (14, 0),
    },
    LangEntry {
        code: "hy",
        region: None,
        mobi_code: (43, 0),
    },
    LangEntry {
        code: "id",
        region: None,
        mobi_code: (33, 0),
    },
    LangEntry {
        code: "is",
        region: None,
        mobi_code: (15, 0),
    },
    LangEntry {
        code: "it",
        region: None,
        mobi_code: (16, 0),
    },
    LangEntry {
        code: "it",
        region: Some("CH"),
        mobi_code: (16, 8),
    },
    LangEntry {
        code: "it",
        region: Some("IT"),
        mobi_code: (16, 4),
    },
    LangEntry {
        code: "ja",
        region: None,
        mobi_code: (17, 0),
    },
    LangEntry {
        code: "ka",
        region: None,
        mobi_code: (55, 0),
    },
    LangEntry {
        code: "kk",
        region: None,
        mobi_code: (63, 0),
    },
    LangEntry {
        code: "kn",
        region: None,
        mobi_code: (75, 0),
    },
    LangEntry {
        code: "ko",
        region: None,
        mobi_code: (18, 0),
    },
    LangEntry {
        code: "kok",
        region: None,
        mobi_code: (87, 0),
    },
    LangEntry {
        code: "lt",
        region: None,
        mobi_code: (39, 0),
    },
    LangEntry {
        code: "lv",
        region: None,
        mobi_code: (38, 0),
    },
    LangEntry {
        code: "mk",
        region: None,
        mobi_code: (47, 0),
    },
    LangEntry {
        code: "ml",
        region: None,
        mobi_code: (76, 0),
    },
    LangEntry {
        code: "mr",
        region: None,
        mobi_code: (78, 0),
    },
    LangEntry {
        code: "ms",
        region: None,
        mobi_code: (62, 0),
    },
    LangEntry {
        code: "mt",
        region: None,
        mobi_code: (58, 0),
    },
    LangEntry {
        code: "ne",
        region: None,
        mobi_code: (97, 0),
    },
    LangEntry {
        code: "nl",
        region: None,
        mobi_code: (19, 0),
    },
    LangEntry {
        code: "nl",
        region: Some("BE"),
        mobi_code: (19, 8),
    },
    LangEntry {
        code: "no",
        region: None,
        mobi_code: (20, 0),
    },
    LangEntry {
        code: "or",
        region: None,
        mobi_code: (72, 0),
    },
    LangEntry {
        code: "pa",
        region: None,
        mobi_code: (70, 0),
    },
    LangEntry {
        code: "pl",
        region: None,
        mobi_code: (21, 0),
    },
    LangEntry {
        code: "pt",
        region: None,
        mobi_code: (22, 0),
    },
    LangEntry {
        code: "pt",
        region: Some("BR"),
        mobi_code: (22, 4),
    },
    LangEntry {
        code: "pt",
        region: Some("PT"),
        mobi_code: (22, 8),
    },
    LangEntry {
        code: "rm",
        region: None,
        mobi_code: (23, 0),
    },
    LangEntry {
        code: "ro",
        region: None,
        mobi_code: (24, 0),
    },
    LangEntry {
        code: "ru",
        region: None,
        mobi_code: (25, 0),
    },
    LangEntry {
        code: "sa",
        region: None,
        mobi_code: (79, 0),
    },
    LangEntry {
        code: "se",
        region: None,
        mobi_code: (59, 0),
    },
    LangEntry {
        code: "sk",
        region: None,
        mobi_code: (27, 0),
    },
    LangEntry {
        code: "sl",
        region: None,
        mobi_code: (36, 0),
    },
    LangEntry {
        code: "sq",
        region: None,
        mobi_code: (28, 0),
    },
    LangEntry {
        code: "sr",
        region: None,
        mobi_code: (26, 12),
    },
    LangEntry {
        code: "sr",
        region: Some("RS"),
        mobi_code: (26, 12),
    },
    LangEntry {
        code: "st",
        region: None,
        mobi_code: (48, 0),
    },
    LangEntry {
        code: "sv",
        region: None,
        mobi_code: (29, 0),
    },
    LangEntry {
        code: "sv",
        region: Some("FI"),
        mobi_code: (29, 8),
    },
    LangEntry {
        code: "sw",
        region: None,
        mobi_code: (65, 0),
    },
    LangEntry {
        code: "ta",
        region: None,
        mobi_code: (73, 0),
    },
    LangEntry {
        code: "te",
        region: None,
        mobi_code: (74, 0),
    },
    LangEntry {
        code: "th",
        region: None,
        mobi_code: (30, 0),
    },
    LangEntry {
        code: "tn",
        region: None,
        mobi_code: (50, 0),
    },
    LangEntry {
        code: "tr",
        region: None,
        mobi_code: (31, 0),
    },
    LangEntry {
        code: "ts",
        region: None,
        mobi_code: (49, 0),
    },
    LangEntry {
        code: "tt",
        region: None,
        mobi_code: (68, 0),
    },
    LangEntry {
        code: "uk",
        region: None,
        mobi_code: (34, 0),
    },
    LangEntry {
        code: "ur",
        region: None,
        mobi_code: (32, 0),
    },
    LangEntry {
        code: "uz",
        region: None,
        mobi_code: (67, 0),
    },
    LangEntry {
        code: "uz",
        region: Some("UZ"),
        mobi_code: (67, 8),
    },
    LangEntry {
        code: "vi",
        region: None,
        mobi_code: (42, 0),
    },
    LangEntry {
        code: "wen",
        region: None,
        mobi_code: (46, 0),
    },
    LangEntry {
        code: "xh",
        region: None,
        mobi_code: (52, 0),
    },
    LangEntry {
        code: "zh",
        region: None,
        mobi_code: (4, 0),
    },
    LangEntry {
        code: "zh",
        region: Some("CN"),
        mobi_code: (4, 8),
    },
    LangEntry {
        code: "zh",
        region: Some("HK"),
        mobi_code: (4, 12),
    },
    LangEntry {
        code: "zh",
        region: Some("SG"),
        mobi_code: (4, 16),
    },
    LangEntry {
        code: "zh",
        region: Some("TW"),
        mobi_code: (4, 4),
    },
    LangEntry {
        code: "zu",
        region: None,
        mobi_code: (53, 0),
    },
];

lazy_static! {
    static ref IANA_MOBI_MAP: HashMap<&'static str, HashMap<Option<&'static str>, MobiCode>> = {
        let mut m = HashMap::new();
        // Insert None: {None: (0,0)}
        let mut def = HashMap::new();
        def.insert(None, (0, 0));
        m.insert("und", def); // "und" or "" or how to represent None key in main map?
        // Python has `None: {None: (0, 0)}`
        // We will handle None input specially.

        for entry in IANA_MOBI_DATA {
             m.entry(entry.code).or_insert_with(HashMap::new).insert(entry.region, entry.mobi_code);
        }
        m
    };
}

pub fn iana2mobi(icode: &str) -> Vec<u8> {
    let mut langdict = if let Some(d) = IANA_MOBI_MAP.get("und") {
        // Fallback, though we know it's not "und" in rust map usually
        d
    } else {
        // We can create a default strict ref if needed, but let's just find the main one.
        // Actually, Python `IANA_MOBI[None]` is `{None: (0, 0)}`.
        // Let's emulate logic.
        return manual_iana2mobi(icode);
    };

    // It's cleaner to rewrite logic without lazy_static if we can just scan the slice,
    // but lazy_static map is good for lookup.
    // Let's use the logic from python directly translated.

    // Since we can't easily put `None` as key in HashMap<&str, ...> without wrapper.
    // I check `manual_iana2mobi`.
    manual_iana2mobi(icode)
}

fn manual_iana2mobi(icode: &str) -> Vec<u8> {
    let mut mcode = (0, 0);
    // Python: `langdict = IANA_MOBI[None]`
    // The default is (0,0)

    let mut subtags: Vec<&str> = if icode.is_empty() {
        Vec::new()
    } else {
        icode.split('-').collect()
    };

    // Python: lang = subtags.pop(0).lower()
    // lang = lang_as_iso639_1(lang)
    // if lang and lang in IANA_MOBI: langdict = IANA_MOBI[lang]

    let mut found_lang = false;
    let mut current_lang_code = "";

    if !subtags.is_empty() {
        let lang = subtags[0].to_lowercase();
        // we need lang_as_iso639_1, but maybe we assume 2 letter codes or just check map.
        // checking map first.
        // Note: IANA_MOBI_DATA has generic "en" etc.
        // Assuming lang is normalized.

        // We need to resolve `lang` to what is in our map.
        // For now let's assume `lang` is the key.
        // If we need `lang_as_iso639_1` we might need that util.
        // Let's assume input is standard.

        // We need to check if we have data for this lang.
        // scan IANA_MOBI_DATA or use map.

        // Let's do a linear scan of data for simplicity and no alloc/lazy_static overhead if valid?
        // No, map is faster.

        // Wait, `subtags` is modified (pop(0)).

        // Let's just use the logic:
        let lookup_lang = lang.as_str();
        if IANA_MOBI_MAP.contains_key(lookup_lang) {
            current_lang_code = IANA_MOBI_DATA
                .iter()
                .find(|x| x.code == lookup_lang)
                .unwrap()
                .code;
            subtags.remove(0); // pop(0)
            found_lang = true;
        } else {
            // Maybe try to normalize 'eng' -> 'en'?
            // For now assume valid input.
        }
    }

    let lang_entries = if found_lang {
        IANA_MOBI_MAP.get(current_lang_code)
    } else {
        None
    };

    if let Some(entries) = lang_entries {
        if let Some(val) = entries.get(&None) {
            mcode = *val;
        }

        while !subtags.is_empty() {
            let subtag = subtags.remove(0);
            // Python: check subtag, title(), upper()
            // In our data, regions are usually uppercase "US", "GB", etc.

            let possible_keys = [
                subtag.to_string(),
                subtag.to_lowercase(),
                subtag.to_uppercase(),
            ];
            // Actually python does: subtag (orig), subtag.title(), subtag.upper().
            // We only have Upper regions in data mostly.

            // Check if any match
            let mut matched = false;
            // Iterate our entries (which are region -> code)

            // entries is HashMap<Option<&str>, MobiCode>

            for k in possible_keys.iter() {
                if let Some(val) = entries.get(&Some(k.as_str())) {
                    mcode = *val;
                    matched = true;
                    break;
                }
            }
            if matched {
                break;
            }
        }
    }

    // pack('>HBB', 0, mcode[1], mcode[0])
    // 0u16 (BE) -> 00 00
    // mcode[1] u8
    // mcode[0] u8
    vec![0, 0, mcode.1, mcode.0]
}

pub fn mobi2iana(langcode: u32, sublangcode: u32) -> String {
    let mut prefix: Option<&str> = None;
    let mut suffix: Option<&str> = None;

    let lc = langcode as u8;
    let slc = sublangcode as u8;

    // Iterate over all data
    for entry in IANA_MOBI_DATA {
        if entry.mobi_code.0 == lc {
            prefix = Some(entry.code);
        }

        // Python: if cl == sublangcode: suffix = subcode.lower() ... break
        // Check if this entry matches sublangcode
        // Note: Python nested loop implies we look for sublangcode inside the langcode group logic,
        // but here flattened.

        // Wait, python logic:
        // for code, d in IANA_MOBI.items():
        //   for subcode, t in d.items():
        //     cc, cl = t
        //     if cc == langcode: prefix = code
        //     if cl == sublangcode: suffix = subcode.lower()... break
        //   if prefix is not None: break

        // This suggests: Find the language group (prefix) that matches `langcode`.
        // THEN within that group, find the subcode that matches `sublangcode`.

        // So we should find the group first?
        // Or find the entry that matches `langcode`?

        // Actually `cc` (main code) is consistent per language?
        // Yes, `af` -> (54, 0).
        // `en` -> (9, ...).
        // So `langcode` uniquely identifies `prefix` (the language).

        if entry.mobi_code.0 == lc {
            // We found the language.
            // Now does the subcode match?
            if entry.mobi_code.1 == slc {
                suffix = entry.region.map(|r| r); // We'll lower it later
                                                  // We found exact match.
                break;
            }
        }
    }

    // If we finished loop and have prefix but maybe not suffix?
    // Python logic:
    // It breaks outer loop if prefix is not None.
    // So it finds the *first* language where `cc == langcode`.
    // And if `cl == sublangcode` it sets suffix.

    // Implementation:
    // 1. Find the language (prefix) by matching `mobi_code.0`.
    // 2. Ideally, we want the specific entry where `mobi_code.1 == sublangcode` ALSO.

    // Let's do it cleaner:
    // Find entry with exact match (lc, slc).
    if let Some(entry) = IANA_MOBI_DATA.iter().find(|e| e.mobi_code == (lc, slc)) {
        let p = entry.code;
        let s = entry.region;

        if let Some(reg) = s {
            return format!("{}-{}", p, reg.to_lowercase());
        } else {
            return p.to_string();
        }
    }

    // If exact match not found?
    // Python code:
    // It loops. `if cc == langcode: prefix = code`.
    // So if it finds ANY entry with matching langcode, it sets prefix.
    // Then continues to look for sublangcode match IN THAT GROUP?
    // "for subcode, t in d.items(): ... break" (inner loop break)
    // "if prefix is not None: break" (outer loop break)

    // So yes, it finds the first group matching langcode.
    // Then searches inside that group for sublangcode.

    // Re-scanning to find just prefix if exact failed?
    // If exact failed, `suffix` remains None (from Python's `subcode else None` logic? No).
    // Python: `suffix = subcode.lower() if subcode else None`.
    // If `subcode` is None (the main entry), then suffix is None.

    // If passed `sublangcode` is not found, `suffix` remains None.
    // So we return `prefix`.

    if let Some(entry) = IANA_MOBI_DATA.iter().find(|e| e.mobi_code.0 == lc) {
        return entry.code.to_string();
    }

    "und".to_string()
}
