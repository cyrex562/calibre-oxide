//! Port of `calibre.utils.fonts.sfnt.cff.constants` (issue #563, split
//! from #554/#65). Pure data: the 391 CFF Standard Strings (Adobe
//! Technical Note #5176 v1.0, 18 March 1998) and the 3 CFF predefined
//! charsets (ISOAdobe/Expert/Expert Subset), generated directly from
//! the real Python source rather than hand-transcribed (391+ string
//! literals is exactly the shape of data this project's own
//! established codegen-from-live-Python-data technique exists for).

pub const CFF_STANDARD_STRINGS: &[&str] = &[
    ".notdef", "space", "exclam", "quotedbl", "numbersign", "dollar", "percent", "ampersand", 
    "quoteright", "parenleft", "parenright", "asterisk", "plus", "comma", "hyphen", "period", 
    "slash", "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", 
    "colon", "semicolon", "less", "equal", "greater", "question", "at", "A", "B", "C", "D", "E", 
    "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", 
    "Y", "Z", "bracketleft", "backslash", "bracketright", "asciicircum", "underscore", "quoteleft", 
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", 
    "t", "u", "v", "w", "x", "y", "z", "braceleft", "bar", "braceright", "asciitilde", 
    "exclamdown", "cent", "sterling", "fraction", "yen", "florin", "section", "currency", 
    "quotesingle", "quotedblleft", "guillemotleft", "guilsinglleft", "guilsinglright", "fi", "fl", 
    "endash", "dagger", "daggerdbl", "periodcentered", "paragraph", "bullet", "quotesinglbase", 
    "quotedblbase", "quotedblright", "guillemotright", "ellipsis", "perthousand", "questiondown", 
    "grave", "acute", "circumflex", "tilde", "macron", "breve", "dotaccent", "dieresis", "ring", 
    "cedilla", "hungarumlaut", "ogonek", "caron", "emdash", "AE", "ordfeminine", "Lslash", 
    "Oslash", "OE", "ordmasculine", "ae", "dotlessi", "lslash", "oslash", "oe", "germandbls", 
    "onesuperior", "logicalnot", "mu", "trademark", "Eth", "onehalf", "plusminus", "Thorn", 
    "onequarter", "divide", "brokenbar", "degree", "thorn", "threequarters", "twosuperior", 
    "registered", "minus", "eth", "multiply", "threesuperior", "copyright", "Aacute", 
    "Acircumflex", "Adieresis", "Agrave", "Aring", "Atilde", "Ccedilla", "Eacute", "Ecircumflex", 
    "Edieresis", "Egrave", "Iacute", "Icircumflex", "Idieresis", "Igrave", "Ntilde", "Oacute", 
    "Ocircumflex", "Odieresis", "Ograve", "Otilde", "Scaron", "Uacute", "Ucircumflex", "Udieresis", 
    "Ugrave", "Yacute", "Ydieresis", "Zcaron", "aacute", "acircumflex", "adieresis", "agrave", 
    "aring", "atilde", "ccedilla", "eacute", "ecircumflex", "edieresis", "egrave", "iacute", 
    "icircumflex", "idieresis", "igrave", "ntilde", "oacute", "ocircumflex", "odieresis", "ograve", 
    "otilde", "scaron", "uacute", "ucircumflex", "udieresis", "ugrave", "yacute", "ydieresis", 
    "zcaron", "exclamsmall", "Hungarumlautsmall", "dollaroldstyle", "dollarsuperior", 
    "ampersandsmall", "Acutesmall", "parenleftsuperior", "parenrightsuperior", "twodotenleader", 
    "onedotenleader", "zerooldstyle", "oneoldstyle", "twooldstyle", "threeoldstyle", 
    "fouroldstyle", "fiveoldstyle", "sixoldstyle", "sevenoldstyle", "eightoldstyle", 
    "nineoldstyle", "commasuperior", "threequartersemdash", "periodsuperior", "questionsmall", 
    "asuperior", "bsuperior", "centsuperior", "dsuperior", "esuperior", "isuperior", "lsuperior", 
    "msuperior", "nsuperior", "osuperior", "rsuperior", "ssuperior", "tsuperior", "ff", "ffi", 
    "ffl", "parenleftinferior", "parenrightinferior", "Circumflexsmall", "hyphensuperior", 
    "Gravesmall", "Asmall", "Bsmall", "Csmall", "Dsmall", "Esmall", "Fsmall", "Gsmall", "Hsmall", 
    "Ismall", "Jsmall", "Ksmall", "Lsmall", "Msmall", "Nsmall", "Osmall", "Psmall", "Qsmall", 
    "Rsmall", "Ssmall", "Tsmall", "Usmall", "Vsmall", "Wsmall", "Xsmall", "Ysmall", "Zsmall", 
    "colonmonetary", "onefitted", "rupiah", "Tildesmall", "exclamdownsmall", "centoldstyle", 
    "Lslashsmall", "Scaronsmall", "Zcaronsmall", "Dieresissmall", "Brevesmall", "Caronsmall", 
    "Dotaccentsmall", "Macronsmall", "figuredash", "hypheninferior", "Ogoneksmall", "Ringsmall", 
    "Cedillasmall", "questiondownsmall", "oneeighth", "threeeighths", "fiveeighths", 
    "seveneighths", "onethird", "twothirds", "zerosuperior", "foursuperior", "fivesuperior", 
    "sixsuperior", "sevensuperior", "eightsuperior", "ninesuperior", "zeroinferior", "oneinferior", 
    "twoinferior", "threeinferior", "fourinferior", "fiveinferior", "sixinferior", "seveninferior", 
    "eightinferior", "nineinferior", "centinferior", "dollarinferior", "periodinferior", 
    "commainferior", "Agravesmall", "Aacutesmall", "Acircumflexsmall", "Atildesmall", 
    "Adieresissmall", "Aringsmall", "AEsmall", "Ccedillasmall", "Egravesmall", "Eacutesmall", 
    "Ecircumflexsmall", "Edieresissmall", "Igravesmall", "Iacutesmall", "Icircumflexsmall", 
    "Idieresissmall", "Ethsmall", "Ntildesmall", "Ogravesmall", "Oacutesmall", "Ocircumflexsmall", 
    "Otildesmall", "Odieresissmall", "OEsmall", "Oslashsmall", "Ugravesmall", "Uacutesmall", 
    "Ucircumflexsmall", "Udieresissmall", "Yacutesmall", "Thornsmall", "Ydieresissmall", "001.000", 
    "001.001", "001.002", "001.003", "Black", "Bold", "Book", "Light", "Medium", "Regular", 
    "Roman", "Semibold", 
];

// 391 standard strings

pub const STANDARD_CHARSETS: &[&[&str]] = &[
    &[
        ".notdef", "space", "exclam", "quotedbl", "numbersign", "dollar", "percent", "ampersand", 
        "quoteright", "parenleft", "parenright", "asterisk", "plus", "comma", "hyphen", "period", 
        "slash", "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", 
        "colon", "semicolon", "less", "equal", "greater", "question", "at", "A", "B", "C", "D", 
        "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", 
        "W", "X", "Y", "Z", "bracketleft", "backslash", "bracketright", "asciicircum", 
        "underscore", "quoteleft", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", 
        "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z", "braceleft", "bar", 
        "braceright", "asciitilde", "exclamdown", "cent", "sterling", "fraction", "yen", "florin", 
        "section", "currency", "quotesingle", "quotedblleft", "guillemotleft", "guilsinglleft", 
        "guilsinglright", "fi", "fl", "endash", "dagger", "daggerdbl", "periodcentered", 
        "paragraph", "bullet", "quotesinglbase", "quotedblbase", "quotedblright", "guillemotright", 
        "ellipsis", "perthousand", "questiondown", "grave", "acute", "circumflex", "tilde", 
        "macron", "breve", "dotaccent", "dieresis", "ring", "cedilla", "hungarumlaut", "ogonek", 
        "caron", "emdash", "AE", "ordfeminine", "Lslash", "Oslash", "OE", "ordmasculine", "ae", 
        "dotlessi", "lslash", "oslash", "oe", "germandbls", "onesuperior", "logicalnot", "mu", 
        "trademark", "Eth", "onehalf", "plusminus", "Thorn", "onequarter", "divide", "brokenbar", 
        "degree", "thorn", "threequarters", "twosuperior", "registered", "minus", "eth", 
        "multiply", "threesuperior", "copyright", "Aacute", "Acircumflex", "Adieresis", "Agrave", 
        "Aring", "Atilde", "Ccedilla", "Eacute", "Ecircumflex", "Edieresis", "Egrave", "Iacute", 
        "Icircumflex", "Idieresis", "Igrave", "Ntilde", "Oacute", "Ocircumflex", "Odieresis", 
        "Ograve", "Otilde", "Scaron", "Uacute", "Ucircumflex", "Udieresis", "Ugrave", "Yacute", 
        "Ydieresis", "Zcaron", "aacute", "acircumflex", "adieresis", "agrave", "aring", "atilde", 
        "ccedilla", "eacute", "ecircumflex", "edieresis", "egrave", "iacute", "icircumflex", 
        "idieresis", "igrave", "ntilde", "oacute", "ocircumflex", "odieresis", "ograve", "otilde", 
        "scaron", "uacute", "ucircumflex", "udieresis", "ugrave", "yacute", "ydieresis", "zcaron", 
    ],
    &[
        "notdef", "space", "exclamsmall", "Hungarumlautsmall", "dollaroldstyle", "dollarsuperior", 
        "ampersandsmall", "Acutesmall", "parenleftsuperior", "parenrightsuperior", 
        "twodotenleader", "onedotenleader", "comma", "hyphen", "period", "fraction", 
        "zerooldstyle", "oneoldstyle", "twooldstyle", "threeoldstyle", "fouroldstyle", 
        "fiveoldstyle", "sixoldstyle", "sevenoldstyle", "eightoldstyle", "nineoldstyle", "colon", 
        "semicolon", "commasuperior", "threequartersemdash", "periodsuperior", "questionsmall", 
        "asuperior", "bsuperior", "centsuperior", "dsuperior", "esuperior", "isuperior", 
        "lsuperior", "msuperior", "nsuperior", "osuperior", "rsuperior", "ssuperior", "tsuperior", 
        "ff", "fi", "fl", "ffi", "ffl", "parenleftinferior", "parenrightinferior", 
        "Circumflexsmall", "hyphensuperior", "Gravesmall", "Asmall", "Bsmall", "Csmall", "Dsmall", 
        "Esmall", "Fsmall", "Gsmall", "Hsmall", "Ismall", "Jsmall", "Ksmall", "Lsmall", "Msmall", 
        "Nsmall", "Osmall", "Psmall", "Qsmall", "Rsmall", "Ssmall", "Tsmall", "Usmall", "Vsmall", 
        "Wsmall", "Xsmall", "Ysmall", "Zsmall", "colonmonetary", "onefitted", "rupiah", 
        "Tildesmall", "exclamdownsmall", "centoldstyle", "Lslashsmall", "Scaronsmall", 
        "Zcaronsmall", "Dieresissmall", "Brevesmall", "Caronsmall", "Dotaccentsmall", 
        "Macronsmall", "figuredash", "hypheninferior", "Ogoneksmall", "Ringsmall", "Cedillasmall", 
        "onequarter", "onehalf", "threequarters", "questiondownsmall", "oneeighth", "threeeighths", 
        "fiveeighths", "seveneighths", "onethird", "twothirds", "zerosuperior", "onesuperior", 
        "twosuperior", "threesuperior", "foursuperior", "fivesuperior", "sixsuperior", 
        "sevensuperior", "eightsuperior", "ninesuperior", "zeroinferior", "oneinferior", 
        "twoinferior", "threeinferior", "fourinferior", "fiveinferior", "sixinferior", 
        "seveninferior", "eightinferior", "nineinferior", "centinferior", "dollarinferior", 
        "periodinferior", "commainferior", "Agravesmall", "Aacutesmall", "Acircumflexsmall", 
        "Atildesmall", "Adieresissmall", "Aringsmall", "AEsmall", "Ccedillasmall", "Egravesmall", 
        "Eacutesmall", "Ecircumflexsmall", "Edieresissmall", "Igravesmall", "Iacutesmall", 
        "Icircumflexsmall", "Idieresissmall", "Ethsmall", "Ntildesmall", "Ogravesmall", 
        "Oacutesmall", "Ocircumflexsmall", "Otildesmall", "Odieresissmall", "OEsmall", 
        "Oslashsmall", "Ugravesmall", "Uacutesmall", "Ucircumflexsmall", "Udieresissmall", 
        "Yacutesmall", "Thornsmall", "Ydieresissmall", 
    ],
    &[
        ".notdef", "space", "dollaroldstyle", "dollarsuperior", "parenleftsuperior", 
        "parenrightsuperior", "twodotenleader", "onedotenleader", "comma", "hyphen", "period", 
        "fraction", "zerooldstyle", "oneoldstyle", "twooldstyle", "threeoldstyle", "fouroldstyle", 
        "fiveoldstyle", "sixoldstyle", "sevenoldstyle", "eightoldstyle", "nineoldstyle", "colon", 
        "semicolon", "commasuperior", "threequartersemdash", "periodsuperior", "asuperior", 
        "bsuperior", "centsuperior", "dsuperior", "esuperior", "isuperior", "lsuperior", 
        "msuperior", "nsuperior", "osuperior", "rsuperior", "ssuperior", "tsuperior", "ff", "fi", 
        "fl", "ffi", "ffl", "parenleftinferior", "parenrightinferior", "hyphensuperior", 
        "colonmonetary", "onefitted", "rupiah", "centoldstyle", "figuredash", "hypheninferior", 
        "onequarter", "onehalf", "threequarters", "oneeighth", "threeeighths", "fiveeighths", 
        "seveneighths", "onethird", "twothirds", "zerosuperior", "onesuperior", "twosuperior", 
        "threesuperior", "foursuperior", "fivesuperior", "sixsuperior", "sevensuperior", 
        "eightsuperior", "ninesuperior", "zeroinferior", "oneinferior", "twoinferior", 
        "threeinferior", "fourinferior", "fiveinferior", "sixinferior", "seveninferior", 
        "eightinferior", "nineinferior", "centinferior", "dollarinferior", "periodinferior", 
        "commainferior", 
    ],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cff_standard_strings_has_the_real_391_entries_in_the_real_order() {
        assert_eq!(CFF_STANDARD_STRINGS.len(), 391);
        assert_eq!(CFF_STANDARD_STRINGS[0], ".notdef");
        assert_eq!(CFF_STANDARD_STRINGS[1], "space");
        assert_eq!(CFF_STANDARD_STRINGS[34], "A");
        assert_eq!(CFF_STANDARD_STRINGS[390], "Semibold");
    }

    #[test]
    fn standard_charsets_has_the_real_iso_adobe_expert_and_expert_subset_sizes() {
        assert_eq!(STANDARD_CHARSETS.len(), 3);
        assert_eq!(STANDARD_CHARSETS[0].len(), 229, "ISOAdobe");
        assert_eq!(STANDARD_CHARSETS[1].len(), 166, "Expert");
        assert_eq!(STANDARD_CHARSETS[2].len(), 87, "Expert Subset");
        assert_eq!(STANDARD_CHARSETS[0][0], ".notdef");
        assert_eq!(STANDARD_CHARSETS[1][0], "notdef", "real upstream's Expert charset really does omit the leading dot");
        assert_eq!(STANDARD_CHARSETS[2].last(), Some(&"commainferior"));
    }
}
