//! Windows LCID → ISO language code.
//!
//! Port of `old_src/src/calibre/ebooks/docx/lcid.py`, which is a plain
//! lookup table. DOCX stores a run's language as a Windows locale id
//! (`<w:lang w:val="0409"/>`, hex), and the HTML output wants a
//! language code, so the table collapses every regional variant onto
//! its base language — all twenty-odd Arabic locales map to `ar`.
//!
//! The table is sorted by code so lookups can binary-search it.

/// `(lcid, language code)`, sorted by lcid.
static LCID_TABLE: &[(u32, &str)] = &[
    (1025, "ar"),  // Arabic - Saudi Arabia
    (1026, "bg"),  // Bulgarian
    (1027, "ca"),  // Catalan
    (1028, "zh"),  // Chinese - Taiwan
    (1029, "cs"),  // Czech
    (1030, "da"),  // Danish
    (1031, "de"),  // German - Germany
    (1032, "el"),  // Greek
    (1033, "en"),  // English - United States
    (1034, "es"),  // Spanish - Spain (Traditional Sort)
    (1035, "fi"),  // Finnish
    (1036, "fr"),  // French - France
    (1037, "he"),  // Hebrew
    (1038, "hu"),  // Hungarian
    (1039, "is"),  // Icelandic
    (1040, "it"),  // Italian - Italy
    (1041, "ja"),  // Japanese
    (1042, "ko"),  // Korean
    (1043, "nl"),  // Dutch - Netherlands
    (1044, "no"),  // Norwegian (Bokmål)
    (1045, "pl"),  // Polish
    (1046, "pt"),  // Portuguese - Brazil
    (1047, "rm"),  // Rhaeto-Romanic
    (1048, "ro"),  // Romanian
    (1049, "ru"),  // Russian
    (1050, "hr"),  // Croatian
    (1051, "sk"),  // Slovak
    (1052, "sq"),  // Albanian - Albania
    (1053, "sv"),  // Swedish
    (1054, "th"),  // Thai
    (1055, "tr"),  // Turkish
    (1056, "ur"),  // Urdu
    (1057, "id"),  // Indonesian
    (1058, "uk"),  // Ukrainian
    (1059, "be"),  // Belarusian
    (1060, "sl"),  // Slovenian
    (1061, "et"),  // Estonian
    (1062, "lv"),  // Latvian
    (1063, "lt"),  // Lithuanian
    (1064, "tg"),  // Tajik
    (1066, "vi"),  // Vietnamese
    (1067, "hy"),  // Armenian - Armenia
    (1068, "az"),  // Azeri (Latin)
    (1069, "eu"),  // Basque
    (1070, "wen"), // Sorbian
    (1071, "mk"),  // FYRO Macedonian
    (1073, "ts"),  // Tsonga
    (1074, "tn"),  // Tswana
    (1075, "ve"),  // Venda
    (1076, "xh"),  // Xhosa
    (1077, "zu"),  // Zulu
    (1078, "af"),  // Afrikaans - South Africa
    (1079, "ka"),  // Georgian
    (1080, "fo"),  // Faroese
    (1081, "hi"),  // Hindi
    (1082, "mt"),  // Maltese
    (1083, "se"),  // Sami (Lappish)
    (1084, "gd"),  // Gaelic (Scotland)
    (1085, "yi"),  // Yiddish
    (1086, "ms"),  // Malay - Malaysia
    (1087, "kk"),  // Kazakh
    (1088, "ky"),  // Kyrgyz (Cyrillic)
    (1089, "sw"),  // Swahili
    (1090, "tk"),  // Turkmen
    (1091, "uz"),  // Uzbek (Latin)
    (1092, "tt"),  // Tatar
    (1093, "bn"),  // Bengali (India)
    (1094, "pa"),  // Punjabi
    (1095, "gu"),  // Gujarati
    (1096, "or"),  // Oriya
    (1097, "ta"),  // Tamil
    (1098, "te"),  // Telugu
    (1099, "kn"),  // Kannada
    (1100, "ml"),  // Malayalam
    (1101, "as"),  // Assamese
    (1102, "mr"),  // Marathi
    (1103, "sa"),  // Sanskrit
    (1104, "mn"),  // Mongolian (Cyrillic)
    (1105, "bo"),  // Tibetan - People's Republic of China
    (1106, "cy"),  // Welsh
    (1107, "km"),  // Khmer
    (1108, "lo"),  // Lao
    (1109, "my"),  // Burmese
    (1110, "gl"),  // Galician
    (1111, "kok"), // Konkani
    (1112, "mni"), // Manipuri
    (1113, "sd"),  // Sindhi - India
    (1114, "syr"), // Syriac
    (1115, "si"),  // Sinhalese - Sri Lanka
    (1116, "chr"), // Cherokee - United States
    (1117, "iu"),  // Inuktitut
    (1118, "am"),  // Amharic - Ethiopia
    (1120, "ks"),  // Kashmiri (Arabic)
    (1121, "ne"),  // Nepali
    (1122, "fy"),  // Frisian - Netherlands
    (1123, "ps"),  // Pashto
    (1124, "fil"), // Filipino
    (1125, "dv"),  // Divehi
    (1126, "bin"), // Edo
    (1128, "ha"),  // Hausa - Nigeria
    (1130, "yo"),  // Yoruba
    (1131, "qu"),  // Quecha - Bolivia
    (1132, "nso"), // Sepedi
    (1136, "ig"),  // Igbo - Nigeria
    (1137, "kr"),  // Kanuri - Nigeria
    (1138, "om"),  // Oromo
    (1139, "ti"),  // Tigrigna - Ethiopia
    (1140, "gn"),  // Guarani - Paraguay
    (1141, "haw"), // Hawaiian - United States
    (1142, "la"),  // Latin
    (1143, "so"),  // Somali
    (1144, "ii"),  // Yi
    (1145, "pap"), // Papiamentu
    (1152, "ug"),  // Uighur - China
    (1153, "mi"),  // Maori - New Zealand
    (2049, "ar"),  // Arabic - Iraq
    (2052, "zh"),  // Chinese - People's Republic of China
    (2055, "de"),  // German - Switzerland
    (2057, "en"),  // English - United Kingdom
    (2058, "es"),  // Spanish - Mexico
    (2060, "fr"),  // French - Belgium
    (2064, "it"),  // Italian - Switzerland
    (2067, "nl"),  // Dutch - Belgium
    (2068, "no"),  // Norwegian (Nynorsk)
    (2070, "pt"),  // Portuguese - Portugal
    (2072, "ro"),  // Romanian - Moldava
    (2073, "ru"),  // Russian - Moldava
    (2074, "sr"),  // Serbian (Latin)
    (2077, "sv"),  // Swedish - Finland
    (2080, "ur"),  // Urdu - India
    (2092, "az"),  // Azeri (Cyrillic)
    (2108, "ga"),  // Gaelic (Ireland)
    (2110, "ms"),  // Malay - Brunei Darussalam
    (2115, "uz"),  // Uzbek (Cyrillic)
    (2117, "bn"),  // Bengali (Bangladesh)
    (2118, "pa"),  // Punjabi (Pakistan)
    (2128, "mn"),  // Mongolian (Mongolian)
    (2129, "bo"),  // Tibetan - Bhutan
    (2137, "sd"),  // Sindhi - Pakistan
    (2144, "ks"),  // Kashmiri
    (2145, "ne"),  // Nepali - India
    (2155, "qu"),  // Quecha - Ecuador
    (2163, "ti"),  // Tigrigna - Eritrea
    (3073, "ar"),  // Arabic - Egypt
    (3076, "zh"),  // Chinese - Hong Kong SAR
    (3079, "de"),  // German - Austria
    (3081, "en"),  // English - Australia
    (3082, "es"),  // Spanish - Spain (Modern Sort)
    (3084, "fr"),  // French - Canada
    (3098, "sr"),  // Serbian (Cyrillic)
    (3179, "qu"),  // Quecha - Peru
    (4097, "ar"),  // Arabic - Libya
    (4100, "zh"),  // Chinese - Singapore
    (4103, "de"),  // German - Luxembourg
    (4105, "en"),  // English - Canada
    (4106, "es"),  // Spanish - Guatemala
    (4108, "fr"),  // French - Switzerland
    (4122, "hr"),  // Croatian (Bosnia/Herzegovina)
    (5121, "ar"),  // Arabic - Algeria
    (5124, "zh"),  // Chinese - Macao SAR
    (5127, "de"),  // German - Liechtenstein
    (5129, "en"),  // English - New Zealand
    (5130, "es"),  // Spanish - Costa Rica
    (5132, "fr"),  // French - Luxembourg
    (5146, "bs"),  // Bosnian (Bosnia/Herzegovina)
    (6145, "ar"),  // Arabic - Morocco
    (6153, "en"),  // English - Ireland
    (6154, "es"),  // Spanish - Panama
    (6156, "fr"),  // French - Monaco
    (7169, "ar"),  // Arabic - Tunisia
    (7177, "en"),  // English - South Africa
    (7178, "es"),  // Spanish - Dominican Republic
    (7180, "fr"),  // French - West Indies
    (8193, "ar"),  // Arabic - Oman
    (8201, "en"),  // English - Jamaica
    (8202, "es"),  // Spanish - Venezuela
    (8204, "fr"),  // French - Reunion
    (9217, "ar"),  // Arabic - Yemen
    (9225, "en"),  // English - Caribbean
    (9226, "es"),  // Spanish - Colombia
    (9228, "fr"),  // French - Democratic Rep. of Congo
    (10241, "ar"), // Arabic - Syria
    (10249, "en"), // English - Belize
    (10250, "es"), // Spanish - Peru
    (10252, "fr"), // French - Senegal
    (11265, "ar"), // Arabic - Jordan
    (11273, "en"), // English - Trinidad
    (11274, "es"), // Spanish - Argentina
    (11276, "fr"), // French - Cameroon
    (12289, "ar"), // Arabic - Lebanon
    (12297, "en"), // English - Zimbabwe
    (12298, "es"), // Spanish - Ecuador
    (12300, "fr"), // French - Cote d'Ivoire
    (13313, "ar"), // Arabic - Kuwait
    (13321, "en"), // English - Philippines
    (13322, "es"), // Spanish - Chile
    (13324, "fr"), // French - Mali
    (14337, "ar"), // Arabic - U.A.E.
    (14345, "en"), // English - Indonesia
    (14346, "es"), // Spanish - Uruguay
    (14348, "fr"), // French - Morocco
    (15361, "ar"), // Arabic - Bahrain
    (15369, "en"), // English - Hong Kong SAR
    (15370, "es"), // Spanish - Paraguay
    (15372, "fr"), // French - Haiti
    (16385, "ar"), // Arabic - Qatar
    (16393, "en"), // English - India
    (16394, "es"), // Spanish - Bolivia
    (17417, "en"), // English - Malaysia
    (17418, "es"), // Spanish - El Salvador
    (18441, "en"), // English - Singapore
    (18442, "es"), // Spanish - Honduras
    (19466, "es"), // Spanish - Nicaragua
    (20490, "es"), // Spanish - Puerto Rico
    (21514, "es"), // Spanish - United States
    (58378, "es"), // Spanish - Latin America
    (58380, "fr"), // French - North Africa
];

/// The ISO language code for a Windows locale id, if known.
///
/// Port of the Python `lcid` dict lookup.
pub fn language_for_lcid(code: u32) -> Option<&'static str> {
    LCID_TABLE
        .binary_search_by_key(&code, |(k, _)| *k)
        .ok()
        .map(|i| LCID_TABLE[i].1)
}

/// Every mapping, in lcid order.
pub fn all() -> &'static [(u32, &'static str)] {
    LCID_TABLE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_the_common_locales() {
        assert_eq!(language_for_lcid(1033), Some("en")); // en-US
        assert_eq!(language_for_lcid(2057), Some("en")); // en-GB
        assert_eq!(language_for_lcid(1036), Some("fr")); // fr-FR
        assert_eq!(language_for_lcid(1031), Some("de")); // de-DE
        assert_eq!(language_for_lcid(1041), Some("ja"));
        assert_eq!(language_for_lcid(2052), Some("zh")); // zh-CN
    }

    #[test]
    fn regional_variants_collapse_onto_the_base_language() {
        // Every Arabic locale in the table resolves to `ar`.
        for code in [
            1025, 5121, 15361, 3073, 2049, 11265, 13313, 12289, 4097, 6145,
        ] {
            assert_eq!(language_for_lcid(code), Some("ar"), "lcid {code}");
        }
    }

    #[test]
    fn unknown_codes_resolve_to_nothing() {
        assert_eq!(language_for_lcid(0), None);
        assert_eq!(language_for_lcid(4095), None);
        assert_eq!(language_for_lcid(u32::MAX), None);
    }

    #[test]
    fn the_table_is_sorted_and_complete() {
        // Sortedness is what makes the binary search correct, and the
        // count guards against a truncated transcription of the Python
        // table.
        assert_eq!(all().len(), 217);
        assert!(
            all().windows(2).all(|w| w[0].0 < w[1].0),
            "table must be strictly sorted by lcid"
        );
        assert!(all().iter().all(|(_, lang)| !lang.is_empty()));
    }
}
