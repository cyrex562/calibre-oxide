//! Port of `old_src/src/calibre/ebooks/textile/unsmarten.py`.
//!
//! A straightforward sequential find-replace table that turns HTML
//! entities and their literal Unicode typographic-character
//! equivalents back into Textile's own `{...}` escape notation (e.g.
//! `&copy;`/`©` -> `{(c)}`). No lookaround is needed anywhere in this
//! table -- every pattern is a plain alternation of literal forms --
//! so this uses the plain `regex` crate throughout (unlike
//! `functions.rs`, which needs `fancy_regex` for several of its
//! patterns).
//!
//! The substitutions run in the same order as the Python's sequence of
//! `re.sub` calls. That order doesn't actually matter for correctness
//! here (every pattern's character ranges are disjoint from every
//! other pattern's), but it's preserved anyway for a direct,
//! easy-to-audit 1:1 correspondence with the original. Literal Unicode
//! characters are used directly in the pattern/replacement strings
//! (rather than `\u{...}` escapes), matching the Python source file.

use lazy_static::lazy_static;
use regex::Regex;

/// One `(pattern, replacement)` entry from `unsmarten.py`. `pattern` is
/// compiled with the plain `regex` crate; `replacement` uses `regex`'s
/// `$1`-style syntax (none of these patterns actually capture groups,
/// so replacements here are always a literal string).
struct Rule {
    regex: Regex,
    replacement: &'static str,
}

macro_rules! rule {
    ($pattern:expr, $replacement:expr) => {
        Rule {
            regex: Regex::new($pattern).expect("static regex"),
            replacement: $replacement,
        }
    };
}

lazy_static! {
    /// Port of the sequence of `re.sub()` calls in `unsmarten()`.
    static ref RULES: Vec<Rule> = vec![
        // -- Latin-1 supplement --------------------------------------
        rule!(r"&#162;|&cent;|¢",     r"{c\}"),  // cent
        rule!(r"&#163;|&pound;|£",    r"{L-}"),  // pound
        rule!(r"&#165;|&yen;|¥",      r"{Y=}"),  // yen
        rule!(r"&#169;|&copy;|©",     r"{(c)}"), // copyright
        rule!(r"&#174;|&reg;|®",      r"{(r)}"), // registered
        rule!(r"&#188;|&frac14;|¼",   r"{1/4}"), // quarter
        rule!(r"&#189;|&frac12;|½",   r"{1/2}"), // half
        rule!(r"&#190;|&frac34;|¾",   r"{3/4}"), // three-quarter
        rule!(r"&#192;|&Agrave;|À",   r"{A`)}"), // A-grave
        rule!(r"&#193;|&Aacute;|Á",   r"{A'}"),  // A-acute
        rule!(r"&#194;|&Acirc;|Â",    r"{A^}"),  // A-circumflex
        rule!(r"&#195;|&Atilde;|Ã",   r"{A~}"),  // A-tilde
        rule!(r#"&#196;|&Auml;|Ä"#,   r#"{A"}"#), // A-umlaut
        rule!(r"&#197;|&Aring;|Å",    r"{Ao}"),  // A-ring
        rule!(r"&#198;|&AElig;|Æ",    r"{AE}"),  // AE
        rule!(r"&#199;|&Ccedil;|Ç",   r"{C,}"),  // C-cedilla
        rule!(r"&#200;|&Egrave;|È",   r"{E`}"),  // E-grave
        rule!(r"&#201;|&Eacute;|É",   r"{E'}"),  // E-acute
        rule!(r"&#202;|&Ecirc;|Ê",    r"{E^}"),  // E-circumflex
        rule!(r#"&#203;|&Euml;|Ë"#,   r#"{E"}"#), // E-umlaut
        rule!(r"&#204;|&Igrave;|Ì",   r"{I`}"),  // I-grave
        rule!(r"&#205;|&Iacute;|Í",   r"{I'}"),  // I-acute
        rule!(r"&#206;|&Icirc;|Î",    r"{I^}"),  // I-circumflex
        rule!(r#"&#207;|&Iuml;|Ï"#,   r#"{I"}"#), // I-umlaut
        rule!(r"&#208;|&ETH;|Ð",      r"{D-}"),  // ETH
        rule!(r"&#209;|&Ntilde;|Ñ",   r"{N~}"),  // N-tilde
        rule!(r"&#210;|&Ograve;|Ò",   r"{O`}"),  // O-grave
        rule!(r"&#211;|&Oacute;|Ó",   r"{O'}"),  // O-acute
        rule!(r"&#212;|&Ocirc;|Ô",    r"{O^}"),  // O-circumflex
        rule!(r"&#213;|&Otilde;|Õ",   r"{O~}"),  // O-tilde
        rule!(r#"&#214;|&Ouml;|Ö"#,   r#"{O"}"#), // O-umlaut
        rule!(r"&#215;|&times;|×",    r"{x}"),   // dimension
        rule!(r"&#216;|&Oslash;|Ø",   r"{O/}"),  // O-slash
        rule!(r"&#217;|&Ugrave;|Ù",   r"{U`}"),  // U-grave
        rule!(r"&#218;|&Uacute;|Ú",   r"{U'}"),  // U-acute
        rule!(r"&#219;|&Ucirc;|Û",    r"{U^}"),  // U-circumflex
        rule!(r#"&#220;|&Uuml;|Ü"#,   r#"{U"}"#), // U-umlaut
        rule!(r"&#221;|&Yacute;|Ý",   r"{Y'}"),  // Y-grave
        rule!(r"&#223;|&szlig;|ß",    r"{sz}"),  // sharp-s
        rule!(r"&#224;|&agrave;|à",   r"{a`}"),  // a-grave
        rule!(r"&#225;|&aacute;|á",   r"{a'}"),  // a-acute
        rule!(r"&#226;|&acirc;|â",    r"{a^}"),  // a-circumflex
        rule!(r"&#227;|&atilde;|ã",   r"{a~}"),  // a-tilde
        rule!(r#"&#228;|&auml;|ä"#,   r#"{a"}"#), // a-umlaut
        rule!(r"&#229;|&aring;|å",    r"{ao}"),  // a-ring
        rule!(r"&#230;|&aelig;|æ",    r"{ae}"),  // ae
        rule!(r"&#231;|&ccedil;|ç",   r"{c,}"),  // c-cedilla
        rule!(r"&#232;|&egrave;|è",   r"{e`}"),  // e-grave
        rule!(r"&#233;|&eacute;|é",   r"{e'}"),  // e-acute
        rule!(r"&#234;|&ecirc;|ê",    r"{e^}"),  // e-circumflex
        rule!(r#"&#235;|&euml;|ë"#,   r#"{e"}"#), // e-umlaut
        rule!(r"&#236;|&igrave;|ì",   r"{i`}"),  // i-grave
        rule!(r"&#237;|&iacute;|í",   r"{i'}"),  // i-acute
        rule!(r"&#238;|&icirc;|î",    r"{i^}"),  // i-circumflex
        rule!(r#"&#239;|&iuml;|ï"#,   r#"{i"}"#), // i-umlaut
        rule!(r"&#240;|&eth;|ð",      r"{d-}"),  // eth
        rule!(r"&#241;|&ntilde;|ñ",   r"{n~}"),  // n-tilde
        rule!(r"&#242;|&ograve;|ò",   r"{o`}"),  // o-grave
        rule!(r"&#243;|&oacute;|ó",   r"{o'}"),  // o-acute
        rule!(r"&#244;|&ocirc;|ô",    r"{o^}"),  // o-circumflex
        rule!(r"&#245;|&otilde;|õ",   r"{o~}"),  // o-tilde
        rule!(r#"&#246;|&ouml;|ö"#,   r#"{o"}"#), // o-umlaut
        rule!(r"&#248;|&oslash;|ø",   r"{o/}"),  // o-stroke
        rule!(r"&#249;|&ugrave;|ù",   r"{u`}"),  // u-grave
        rule!(r"&#250;|&uacute;|ú",   r"{u'}"),  // u-acute
        rule!(r"&#251;|&ucirc;|û",    r"{u^}"),  // u-circumflex
        rule!(r#"&#252;|&uuml;|ü"#,   r#"{u"}"#), // u-umlaut
        rule!(r"&#253;|&yacute;|ý",   r"{y'}"),  // y-acute
        rule!(r#"&#255;|&yuml;|ÿ"#,   r#"{y"}"#), // y-umlaut

        // -- Latin extended-A / caron letters -------------------------
        rule!(r"&#268;|&Ccaron;|Č",   r"{Cˇ}"),  // C-caron
        rule!(r"&#269;|&ccaron;|č",   r"{cˇ}"),  // c-caron
        rule!(r"&#270;|&Dcaron;|Ď",   r"{Dˇ}"),  // D-caron
        rule!(r"&#271;|&dcaron;|ď",   r"{dˇ}"),  // d-caron
        rule!(r"&#282;|&Ecaron;|Ě",   r"{Eˇ}"),  // E-caron
        rule!(r"&#283;|&ecaron;|ě",   r"{eˇ}"),  // e-caron
        rule!(r"&#313;|&Lacute;|Ĺ",   r"{L'}"),  // L-acute
        rule!(r"&#314;|&lacute;|ĺ",   r"{l'}"),  // l-acute
        rule!(r"&#317;|&Lcaron;|Ľ",   r"{Lˇ}"),  // L-caron
        rule!(r"&#318;|&lcaron;|ľ",   r"{lˇ}"),  // l-caron
        rule!(r"&#327;|&Ncaron;|Ň",   r"{Nˇ}"),  // N-caron
        rule!(r"&#328;|&ncaron;|ň",   r"{nˇ}"),  // n-caron

        rule!(r"&#338;|&OElig;|Œ",    r"{OE}"),  // OE
        rule!(r"&#339;|&oelig;|œ",    r"{oe}"),  // oe

        rule!(r"&#340;|&Racute;|Ŕ",   r"{R'}"),  // R-acute
        rule!(r"&#341;|&racute;|ŕ",   r"{r'}"),  // r-acute
        rule!(r"&#344;|&Rcaron;|Ř",   r"{Rˇ}"),  // R-caron
        rule!(r"&#345;|&rcaron;|ř",   r"{rˇ}"),  // r-caron
        rule!(r"&#348;|Ŝ",            r"{S^}"),  // S-circumflex
        rule!(r"&#349;|ŝ",            r"{s^}"),  // s-circumflex
        rule!(r"&#352;|&Scaron;|Š",   r"{Sˇ}"),  // S-caron
        rule!(r"&#353;|&scaron;|š",   r"{sˇ}"),  // s-caron
        rule!(r"&#356;|&Tcaron;|Ť",   r"{Tˇ}"),  // T-caron
        rule!(r"&#357;|&tcaron;|ť",   r"{tˇ}"),  // t-caron
        rule!(r"&#366;|&Uring;|Ů",    r"{U°}"),  // U-ring
        rule!(r"&#367;|&uring;|ů",    r"{u°}"),  // u-ring
        rule!(r"&#381;|&Zcaron;|Ž",   r"{Zˇ}"),  // Z-caron
        rule!(r"&#382;|&zcaron;|ž",   r"{zˇ}"),  // z-caron

        // -- Currency, punctuation, card suits ------------------------
        rule!(r"&#8226;|&bull;|•",    r"{*}"),        // bullet
        rule!(r"&#8355;|₣",           r"{Fr}"),       // Franc
        rule!(r"&#8356;|₤",           r"{L=}"),       // Lira
        rule!(r"&#8360;|₨",           r"{Rs}"),       // Rupee
        rule!(r"&#8364;|&euro;|€",    r"{C=}"),       // euro
        rule!(r"&#8482;|&trade;|™",   r"{tm}"),       // trademark
        rule!(r"&#9824;|&spades;|♠",  r"{spade}"),    // spade
        rule!(r"&#9827;|&clubs;|♣",   r"{club}"),     // club
        rule!(r"&#9829;|&hearts;|♥",  r"{heart}"),    // heart
        rule!(r"&#9830;|&diams;|♦",   r"{diamond}"),  // diamond
    ];
}

/// Port of `unsmarten(txt)`: reverses HTML-entity/Unicode typographic
/// characters back into Textile's own `{...}` escape notation.
///
/// Three lines at the very end of the Python (blank-paragraph handling
/// for `\xa0`, `\n\n\n\n`, `\n  \n`) are commented out upstream (behind
/// a `# Move into main code?` note) and were never active -- they are
/// not ported, matching the Python's actual behavior.
pub fn unsmarten(txt: &str) -> String {
    let mut txt = txt.to_string();
    for rule in RULES.iter() {
        txt = rule.regex.replace_all(&txt, rule.replacement).into_owned();
    }
    txt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_forms_convert() {
        assert_eq!(unsmarten("&#162; cent"), "{c\\} cent");
        assert_eq!(unsmarten("&cent; cent"), "{c\\} cent");
        assert_eq!(unsmarten("¢ cent"), "{c\\} cent");
    }

    #[test]
    fn copyright_forms_convert() {
        assert_eq!(unsmarten("&#169; copy"), "{(c)} copy");
        assert_eq!(unsmarten("&copy; copy"), "{(c)} copy");
        assert_eq!(unsmarten("© copy"), "{(c)} copy");
    }

    #[test]
    fn trademark_converts() {
        assert_eq!(unsmarten("&#8482; tm"), "{tm} tm");
        assert_eq!(unsmarten("&trade; tm"), "{tm} tm");
        assert_eq!(unsmarten("™ tm"), "{tm} tm");
    }

    #[test]
    fn caron_letters_convert() {
        assert_eq!(unsmarten("&Scaron;"), "{Sˇ}");
        assert_eq!(unsmarten("š"), "{sˇ}");
    }

    #[test]
    fn card_suits_convert() {
        assert_eq!(unsmarten("&spades;"), "{spade}");
        assert_eq!(unsmarten("♦"), "{diamond}");
    }

    #[test]
    fn plain_text_is_unaffected() {
        assert_eq!(
            unsmarten("plain ascii text unaffected"),
            "plain ascii text unaffected"
        );
        assert_eq!(
            unsmarten("a & b < c > d \"quote\" "),
            "a & b < c > d \"quote\" "
        );
    }
}
