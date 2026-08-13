//! Microsoft LIT tag and attribute tables.
//!
//! Port of `src/calibre/ebooks/lit/maps/` (`__init__.py`, `html.py`,
//! `opf.py`), which calibre in turn copied from ConvertLIT.
//!
//! The tables are generated verbatim from the Python; the ordering of
//! the attribute lists is significant, because `writer.py` inverts
//! them into name-to-code maps and a later duplicate name wins.

/// One attribute table: `(code, name)` pairs in source order.
pub type AttrTable = &'static [(u32, &'static str)];

/// A tag/attribute table triple — `(TAGS, ATTRS, TAGS_ATTRS)` in the
/// Python, the `map` argument to `UnBinary` and `ReBinary`.
#[derive(Clone, Copy)]
pub struct TagMap {
    /// Tag names by code; `None` where the code is unassigned.
    pub tags: &'static [Option<&'static str>],
    /// The default attribute table, consulted when the per-tag one
    /// has no entry.
    pub attrs: AttrTable,
    /// Per-tag attribute tables, empty where the Python has `None`.
    pub tag_attrs: &'static [AttrTable],
    /// Whether this is the HTML map. The Python asks `map is HTML_MAP`;
    /// pointer identity is not dependable for `const` tables in Rust,
    /// which may be duplicated at each use, so it is recorded here.
    pub html: bool,
}

impl TagMap {
    /// Look up a tag name by code.
    pub fn tag(&self, code: usize) -> Option<&'static str> {
        self.tags.get(code).copied().flatten()
    }

    /// The attribute table for a tag code, empty if there is none.
    pub fn tag_attrs(&self, code: usize) -> AttrTable {
        self.tag_attrs.get(code).copied().unwrap_or(&[])
    }

    /// Whether this is the HTML map. `map is HTML_MAP` in the Python,
    /// which switches on several behaviours in both directions.
    pub fn is_html(&self) -> bool {
        self.html
    }
}

/// An attribute table with no entries, for tags that declare none.
pub const EMPTY_ATTRS: AttrTable = &[];

/// Look up an attribute name by code in one table.
pub fn attr_name(table: AttrTable, code: u32) -> Option<&'static str> {
    table.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

/// `TAGS` in `maps/html.py`.
pub const HTML_TAGS: &[Option<&str>] = &[
    None,               // 0
    None,               // 1
    None,               // 2
    Some("a"),          // 3
    Some("acronym"),    // 4
    Some("address"),    // 5
    Some("applet"),     // 6
    Some("area"),       // 7
    Some("b"),          // 8
    Some("base"),       // 9
    Some("basefont"),   // 10
    Some("bdo"),        // 11
    Some("bgsound"),    // 12
    Some("big"),        // 13
    Some("blink"),      // 14
    Some("blockquote"), // 15
    Some("body"),       // 16
    Some("br"),         // 17
    Some("button"),     // 18
    Some("caption"),    // 19
    Some("center"),     // 20
    Some("cite"),       // 21
    Some("code"),       // 22
    Some("col"),        // 23
    Some("colgroup"),   // 24
    None,               // 25
    None,               // 26
    Some("dd"),         // 27
    Some("del"),        // 28
    Some("dfn"),        // 29
    Some("dir"),        // 30
    Some("div"),        // 31
    Some("dl"),         // 32
    Some("dt"),         // 33
    Some("em"),         // 34
    Some("embed"),      // 35
    Some("fieldset"),   // 36
    Some("font"),       // 37
    Some("form"),       // 38
    Some("frame"),      // 39
    Some("frameset"),   // 40
    None,               // 41
    Some("h1"),         // 42
    Some("h2"),         // 43
    Some("h3"),         // 44
    Some("h4"),         // 45
    Some("h5"),         // 46
    Some("h6"),         // 47
    Some("head"),       // 48
    Some("hr"),         // 49
    Some("html"),       // 50
    Some("i"),          // 51
    Some("iframe"),     // 52
    Some("img"),        // 53
    Some("input"),      // 54
    Some("ins"),        // 55
    Some("kbd"),        // 56
    Some("label"),      // 57
    Some("legend"),     // 58
    Some("li"),         // 59
    Some("link"),       // 60
    Some("tag61"),      // 61
    Some("map"),        // 62
    Some("tag63"),      // 63
    Some("tag64"),      // 64
    Some("meta"),       // 65
    Some("nextid"),     // 66
    Some("nobr"),       // 67
    Some("noembed"),    // 68
    Some("noframes"),   // 69
    Some("noscript"),   // 70
    Some("object"),     // 71
    Some("ol"),         // 72
    Some("option"),     // 73
    Some("p"),          // 74
    Some("param"),      // 75
    Some("plaintext"),  // 76
    Some("pre"),        // 77
    Some("q"),          // 78
    Some("rp"),         // 79
    Some("rt"),         // 80
    Some("ruby"),       // 81
    Some("s"),          // 82
    Some("samp"),       // 83
    Some("script"),     // 84
    Some("select"),     // 85
    Some("small"),      // 86
    Some("span"),       // 87
    Some("strike"),     // 88
    Some("strong"),     // 89
    Some("style"),      // 90
    Some("sub"),        // 91
    Some("sup"),        // 92
    Some("table"),      // 93
    Some("tbody"),      // 94
    Some("tc"),         // 95
    Some("td"),         // 96
    Some("textarea"),   // 97
    Some("tfoot"),      // 98
    Some("th"),         // 99
    Some("thead"),      // 100
    Some("title"),      // 101
    Some("tr"),         // 102
    Some("tt"),         // 103
    Some("u"),          // 104
    Some("ul"),         // 105
    Some("var"),        // 106
    Some("wbr"),        // 107
    None,               // 108
];

/// `ATTRS0` in `maps/html.py` — the default attribute table.
pub const HTML_ATTRS: &[(u32, &str)] = &[
    (0x8010, "tabindex"),
    (0x8046, "title"),
    (0x804B, "style"),
    (0x804D, "disabled"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x83FE, "datafld"),
    (0x83FF, "datasrc"),
    (0x8400, "dataformatas"),
    (0x87D6, "accesskey"),
    (0x9392, "lang"),
    (0x93ED, "language"),
    (0x93FE, "dir"),
    (0x9771, "onmouseover"),
    (0x9772, "onmouseout"),
    (0x9773, "onmousedown"),
    (0x9774, "onmouseup"),
    (0x9775, "onmousemove"),
    (0x9776, "onkeydown"),
    (0x9777, "onkeyup"),
    (0x9778, "onkeypress"),
    (0x9779, "onclick"),
    (0x977A, "ondblclick"),
    (0x977E, "onhelp"),
    (0x977F, "onfocus"),
    (0x9780, "onblur"),
    (0x9783, "onrowexit"),
    (0x9784, "onrowenter"),
    (0x9786, "onbeforeupdate"),
    (0x9787, "onafterupdate"),
    (0x978A, "onreadystatechange"),
    (0x9790, "onscroll"),
    (0x9794, "ondragstart"),
    (0x9795, "onresize"),
    (0x9796, "onselectstart"),
    (0x9797, "onerrorupdate"),
    (0x9799, "ondatasetchanged"),
    (0x979A, "ondataavailable"),
    (0x979B, "ondatasetcomplete"),
    (0x979C, "onfilterchange"),
    (0x979F, "onlosecapture"),
    (0x97A0, "onpropertychange"),
    (0x97A2, "ondrag"),
    (0x97A3, "ondragend"),
    (0x97A4, "ondragenter"),
    (0x97A5, "ondragover"),
    (0x97A6, "ondragleave"),
    (0x97A7, "ondrop"),
    (0x97A8, "oncut"),
    (0x97A9, "oncopy"),
    (0x97AA, "onpaste"),
    (0x97AB, "onbeforecut"),
    (0x97AC, "onbeforecopy"),
    (0x97AD, "onbeforepaste"),
    (0x97AF, "onrowsdelete"),
    (0x97B0, "onrowsinserted"),
    (0x97B1, "oncellchange"),
    (0x97B2, "oncontextmenu"),
    (0x97B6, "onbeforeeditfocus"),
];

/// `ATTRS3` in `maps/html.py` — attributes of `a`.
pub const HTML_ATTRS_3: &[(u32, &str)] = &[
    (0x0001, "href"),
    (0x03EC, "target"),
    (0x03EE, "rel"),
    (0x03EF, "rev"),
    (0x03F0, "urn"),
    (0x03F1, "methods"),
    (0x8001, "name"),
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS5` in `maps/html.py` — attributes of `address`.
pub const HTML_ATTRS_5: &[(u32, &str)] = &[(0x9399, "clear")];

/// `ATTRS6` in `maps/html.py` — attributes of `applet`.
pub const HTML_ATTRS_6: &[(u32, &str)] = &[
    (0x8001, "name"),
    (0x8006, "width"),
    (0x8007, "height"),
    (0x804A, "align"),
    (0x8BBB, "classid"),
    (0x8BBC, "data"),
    (0x8BBF, "codebase"),
    (0x8BC0, "codetype"),
    (0x8BC1, "code"),
    (0x8BC2, "type"),
    (0x8BC5, "vspace"),
    (0x8BC6, "hspace"),
    (0x978E, "onerror"),
];

/// `ATTRS7` in `maps/html.py` — attributes of `area`.
pub const HTML_ATTRS_7: &[(u32, &str)] = &[
    (0x0001, "href"),
    (0x03EA, "shape"),
    (0x03EB, "coords"),
    (0x03ED, "target"),
    (0x03EE, "alt"),
    (0x03EF, "nohref"),
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS8` in `maps/html.py` — attributes of `b`.
pub const HTML_ATTRS_8: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS9` in `maps/html.py` — attributes of `base`.
pub const HTML_ATTRS_9: &[(u32, &str)] = &[(0x03EC, "href"), (0x03ED, "target")];

/// `ATTRS10` in `maps/html.py` — attributes of `basefont`.
pub const HTML_ATTRS_10: &[(u32, &str)] = &[(0x938B, "color"), (0x939B, "face"), (0x93A3, "size")];

/// `ATTRS12` in `maps/html.py` — attributes of `bgsound`.
pub const HTML_ATTRS_12: &[(u32, &str)] = &[
    (0x03EA, "src"),
    (0x03EB, "loop"),
    (0x03EC, "volume"),
    (0x03ED, "balance"),
];

/// `ATTRS13` in `maps/html.py` — attributes of `big`.
pub const HTML_ATTRS_13: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS15` in `maps/html.py` — attributes of `blockquote`.
pub const HTML_ATTRS_15: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x9399, "clear"),
];

/// `ATTRS16` in `maps/html.py` — attributes of `body`.
pub const HTML_ATTRS_16: &[(u32, &str)] = &[
    (0x07DB, "link"),
    (0x07DC, "alink"),
    (0x07DD, "vlink"),
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x938A, "background"),
    (0x938B, "text"),
    (0x938E, "nowrap"),
    (0x93AE, "topmargin"),
    (0x93AF, "rightmargin"),
    (0x93B0, "bottommargin"),
    (0x93B1, "leftmargin"),
    (0x93B6, "bgproperties"),
    (0x93D8, "scroll"),
    (0x977B, "onselect"),
    (0x9791, "onload"),
    (0x9792, "onunload"),
    (0x9798, "onbeforeunload"),
    (0x97B3, "onbeforeprint"),
    (0x97B4, "onafterprint"),
    (0xFE0C, "bgcolor"),
];

/// `ATTRS17` in `maps/html.py` — attributes of `br`.
pub const HTML_ATTRS_17: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x9399, "clear"),
];

/// `ATTRS18` in `maps/html.py` — attributes of `button`.
pub const HTML_ATTRS_18: &[(u32, &str)] = &[(0x07D1, "type"), (0x8001, "name")];

/// `ATTRS19` in `maps/html.py` — attributes of `caption`.
pub const HTML_ATTRS_19: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x8049, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x93A8, "valign"),
];

/// `ATTRS20` in `maps/html.py` — attributes of `center`.
pub const HTML_ATTRS_20: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x9399, "clear"),
];

/// `ATTRS21` in `maps/html.py` — attributes of `cite`.
pub const HTML_ATTRS_21: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS22` in `maps/html.py` — attributes of `code`.
pub const HTML_ATTRS_22: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS23` in `maps/html.py` — attributes of `col`.
pub const HTML_ATTRS_23: &[(u32, &str)] = &[
    (0x03EA, "span"),
    (0x8006, "width"),
    (0x8049, "align"),
    (0x93A8, "valign"),
    (0xFE0C, "bgcolor"),
];

/// `ATTRS24` in `maps/html.py` — attributes of `colgroup`.
pub const HTML_ATTRS_24: &[(u32, &str)] = &[
    (0x03EA, "span"),
    (0x8006, "width"),
    (0x8049, "align"),
    (0x93A8, "valign"),
    (0xFE0C, "bgcolor"),
];

/// `ATTRS27` in `maps/html.py` — attributes of `dd`.
pub const HTML_ATTRS_27: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x938E, "nowrap"),
];

/// `ATTRS29` in `maps/html.py` — attributes of `dfn`.
pub const HTML_ATTRS_29: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS31` in `maps/html.py` — attributes of `div`.
pub const HTML_ATTRS_31: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x8049, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x938E, "nowrap"),
];

/// `ATTRS32` in `maps/html.py` — attributes of `dl`.
pub const HTML_ATTRS_32: &[(u32, &str)] = &[
    (0x03EA, "compact"),
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS33` in `maps/html.py` — attributes of `dt`.
pub const HTML_ATTRS_33: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x938E, "nowrap"),
];

/// `ATTRS34` in `maps/html.py` — attributes of `em`.
pub const HTML_ATTRS_34: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS35` in `maps/html.py` — attributes of `embed`.
pub const HTML_ATTRS_35: &[(u32, &str)] = &[
    (0x8001, "name"),
    (0x8006, "width"),
    (0x8007, "height"),
    (0x804A, "align"),
    (0x8BBD, "palette"),
    (0x8BBE, "pluginspage"),
    (0x8BBF, "src"),
    (0x8BC1, "units"),
    (0x8BC2, "type"),
    (0x8BC3, "hidden"),
];

/// `ATTRS36` in `maps/html.py` — attributes of `fieldset`.
pub const HTML_ATTRS_36: &[(u32, &str)] = &[(0x804A, "align")];

/// `ATTRS37` in `maps/html.py` — attributes of `font`.
pub const HTML_ATTRS_37: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x938B, "color"),
    (0x939B, "face"),
    (0x939C, "size"),
];

/// `ATTRS38` in `maps/html.py` — attributes of `form`.
pub const HTML_ATTRS_38: &[(u32, &str)] = &[
    (0x03EA, "action"),
    (0x03EC, "enctype"),
    (0x03ED, "method"),
    (0x03EF, "target"),
    (0x03F4, "accept-charset"),
    (0x8001, "name"),
    (0x977C, "onsubmit"),
    (0x977D, "onreset"),
];

/// `ATTRS39` in `maps/html.py` — attributes of `frame`.
pub const HTML_ATTRS_39: &[(u32, &str)] = &[
    (0x8000, "align"),
    (0x8001, "name"),
    (0x8BB9, "src"),
    (0x8BBB, "border"),
    (0x8BBC, "frameborder"),
    (0x8BBD, "framespacing"),
    (0x8BBE, "marginwidth"),
    (0x8BBF, "marginheight"),
    (0x8BC0, "noresize"),
    (0x8BC1, "scrolling"),
    (0x8FA2, "bordercolor"),
];

/// `ATTRS40` in `maps/html.py` — attributes of `frameset`.
pub const HTML_ATTRS_40: &[(u32, &str)] = &[
    (0x03E9, "rows"),
    (0x03EA, "cols"),
    (0x03EB, "border"),
    (0x03EC, "bordercolor"),
    (0x03ED, "frameborder"),
    (0x03EE, "framespacing"),
    (0x8001, "name"),
    (0x9791, "onload"),
    (0x9792, "onunload"),
    (0x9798, "onbeforeunload"),
    (0x97B3, "onbeforeprint"),
    (0x97B4, "onafterprint"),
];

/// `ATTRS42` in `maps/html.py` — attributes of `h1`.
pub const HTML_ATTRS_42: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x8049, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x9399, "clear"),
];

/// `ATTRS43` in `maps/html.py` — attributes of `h2`.
pub const HTML_ATTRS_43: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x8049, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x9399, "clear"),
];

/// `ATTRS44` in `maps/html.py` — attributes of `h3`.
pub const HTML_ATTRS_44: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x8049, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x9399, "clear"),
];

/// `ATTRS45` in `maps/html.py` — attributes of `h4`.
pub const HTML_ATTRS_45: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x8049, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x9399, "clear"),
];

/// `ATTRS46` in `maps/html.py` — attributes of `h5`.
pub const HTML_ATTRS_46: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x8049, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x9399, "clear"),
];

/// `ATTRS47` in `maps/html.py` — attributes of `h6`.
pub const HTML_ATTRS_47: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x8049, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x9399, "clear"),
];

/// `ATTRS49` in `maps/html.py` — attributes of `hr`.
pub const HTML_ATTRS_49: &[(u32, &str)] = &[
    (0x03EA, "noshade"),
    (0x8006, "width"),
    (0x8007, "size"),
    (0x8046, "title"),
    (0x8049, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x938B, "color"),
];

/// `ATTRS51` in `maps/html.py` — attributes of `i`.
pub const HTML_ATTRS_51: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS52` in `maps/html.py` — attributes of `iframe`.
pub const HTML_ATTRS_52: &[(u32, &str)] = &[
    (0x8001, "name"),
    (0x8006, "width"),
    (0x8007, "height"),
    (0x804A, "align"),
    (0x8BB9, "src"),
    (0x8BBB, "border"),
    (0x8BBC, "frameborder"),
    (0x8BBD, "framespacing"),
    (0x8BBE, "marginwidth"),
    (0x8BBF, "marginheight"),
    (0x8BC0, "noresize"),
    (0x8BC1, "scrolling"),
    (0x8FA2, "vspace"),
    (0x8FA3, "hspace"),
];

/// `ATTRS53` in `maps/html.py` — attributes of `img`.
pub const HTML_ATTRS_53: &[(u32, &str)] = &[
    (0x03EB, "alt"),
    (0x03EC, "src"),
    (0x03ED, "border"),
    (0x03EE, "vspace"),
    (0x03EF, "hspace"),
    (0x03F0, "lowsrc"),
    (0x03F1, "vrml"),
    (0x03F2, "dynsrc"),
    (0x03F4, "loop"),
    (0x03F6, "start"),
    (0x07D3, "ismap"),
    (0x07D9, "usemap"),
    (0x8001, "name"),
    (0x8006, "width"),
    (0x8007, "height"),
    (0x8046, "title"),
    (0x804A, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x978D, "onabort"),
    (0x978E, "onerror"),
    (0x9791, "onload"),
];

/// `ATTRS54` in `maps/html.py` — attributes of `input`.
pub const HTML_ATTRS_54: &[(u32, &str)] = &[
    (0x07D1, "type"),
    (0x07D3, "size"),
    (0x07D4, "maxlength"),
    (0x07D6, "readonly"),
    (0x07D8, "indeterminate"),
    (0x07DA, "checked"),
    (0x07DB, "alt"),
    (0x07DC, "src"),
    (0x07DD, "border"),
    (0x07DE, "vspace"),
    (0x07DF, "hspace"),
    (0x07E0, "lowsrc"),
    (0x07E1, "vrml"),
    (0x07E2, "dynsrc"),
    (0x07E4, "loop"),
    (0x07E5, "start"),
    (0x8001, "name"),
    (0x8006, "width"),
    (0x8007, "height"),
    (0x804A, "align"),
    (0x93EE, "value"),
    (0x977B, "onselect"),
    (0x978D, "onabort"),
    (0x978E, "onerror"),
    (0x978F, "onchange"),
    (0x9791, "onload"),
];

/// `ATTRS56` in `maps/html.py` — attributes of `kbd`.
pub const HTML_ATTRS_56: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS57` in `maps/html.py` — attributes of `label`.
pub const HTML_ATTRS_57: &[(u32, &str)] = &[(0x03E9, "for")];

/// `ATTRS58` in `maps/html.py` — attributes of `legend`.
pub const HTML_ATTRS_58: &[(u32, &str)] = &[(0x804A, "align")];

/// `ATTRS59` in `maps/html.py` — attributes of `li`.
pub const HTML_ATTRS_59: &[(u32, &str)] = &[
    (0x03EA, "value"),
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x939A, "type"),
];

/// `ATTRS60` in `maps/html.py` — attributes of `link`.
pub const HTML_ATTRS_60: &[(u32, &str)] = &[
    (0x03EE, "href"),
    (0x03EF, "rel"),
    (0x03F0, "rev"),
    (0x03F1, "type"),
    (0x03F9, "media"),
    (0x03FA, "target"),
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x978E, "onerror"),
    (0x9791, "onload"),
];

/// `ATTRS61` in `maps/html.py` — attributes of `tag61`.
pub const HTML_ATTRS_61: &[(u32, &str)] = &[(0x9399, "clear")];

/// `ATTRS62` in `maps/html.py` — attributes of `map`.
pub const HTML_ATTRS_62: &[(u32, &str)] = &[
    (0x8001, "name"),
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS63` in `maps/html.py` — attributes of `tag63`.
pub const HTML_ATTRS_63: &[(u32, &str)] = &[
    (0x1771, "scrolldelay"),
    (0x1772, "direction"),
    (0x1773, "behavior"),
    (0x1774, "scrollamount"),
    (0x1775, "loop"),
    (0x1776, "vspace"),
    (0x1777, "hspace"),
    (0x1778, "truespeed"),
    (0x8006, "width"),
    (0x8007, "height"),
    (0x9785, "onbounce"),
    (0x978B, "onfinish"),
    (0x978C, "onstart"),
    (0xFE0C, "bgcolor"),
];

/// `ATTRS65` in `maps/html.py` — attributes of `meta`.
pub const HTML_ATTRS_65: &[(u32, &str)] = &[
    (0x03EA, "http-equiv"),
    (0x03EB, "content"),
    (0x03EC, "url"),
    (0x03F6, "charset"),
    (0x8001, "name"),
];

/// `ATTRS66` in `maps/html.py` — attributes of `nextid`.
pub const HTML_ATTRS_66: &[(u32, &str)] = &[(0x03F5, "n")];

/// `ATTRS71` in `maps/html.py` — attributes of `object`.
pub const HTML_ATTRS_71: &[(u32, &str)] = &[
    (0x8000, "usemap"),
    (0x8001, "name"),
    (0x8006, "width"),
    (0x8007, "height"),
    (0x8046, "title"),
    (0x804A, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x8BBB, "classid"),
    (0x8BBC, "data"),
    (0x8BBF, "codebase"),
    (0x8BC0, "codetype"),
    (0x8BC1, "code"),
    (0x8BC2, "type"),
    (0x8BC5, "vspace"),
    (0x8BC6, "hspace"),
    (0x978E, "onerror"),
];

/// `ATTRS72` in `maps/html.py` — attributes of `ol`.
pub const HTML_ATTRS_72: &[(u32, &str)] = &[
    (0x03EB, "compact"),
    (0x03EC, "start"),
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x939A, "type"),
];

/// `ATTRS73` in `maps/html.py` — attributes of `option`.
pub const HTML_ATTRS_73: &[(u32, &str)] = &[(0x03EA, "selected"), (0x03EB, "value")];

/// `ATTRS74` in `maps/html.py` — attributes of `p`.
pub const HTML_ATTRS_74: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x8049, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x9399, "clear"),
];

/// `ATTRS75` in `maps/html.py` — attributes of `param`.
pub const HTML_ATTRS_75: &[(u32, &str)] = &[(0x8000, "type")];

/// `ATTRS76` in `maps/html.py` — attributes of `plaintext`.
pub const HTML_ATTRS_76: &[(u32, &str)] = &[(0x9399, "clear")];

/// `ATTRS77` in `maps/html.py` — attributes of `pre`.
pub const HTML_ATTRS_77: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x9399, "clear"),
];

/// `ATTRS78` in `maps/html.py` — attributes of `q`.
pub const HTML_ATTRS_78: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS82` in `maps/html.py` — attributes of `s`.
pub const HTML_ATTRS_82: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS83` in `maps/html.py` — attributes of `samp`.
pub const HTML_ATTRS_83: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS84` in `maps/html.py` — attributes of `script`.
pub const HTML_ATTRS_84: &[(u32, &str)] = &[
    (0x03EA, "src"),
    (0x03ED, "for"),
    (0x03EE, "event"),
    (0x03F0, "defer"),
    (0x03F2, "type"),
    (0x978E, "onerror"),
];

/// `ATTRS85` in `maps/html.py` — attributes of `select`.
pub const HTML_ATTRS_85: &[(u32, &str)] = &[
    (0x03EB, "size"),
    (0x03EC, "multiple"),
    (0x8000, "align"),
    (0x8001, "name"),
    (0x978F, "onchange"),
];

/// `ATTRS86` in `maps/html.py` — attributes of `small`.
pub const HTML_ATTRS_86: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS87` in `maps/html.py` — attributes of `span`.
pub const HTML_ATTRS_87: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS88` in `maps/html.py` — attributes of `strike`.
pub const HTML_ATTRS_88: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS89` in `maps/html.py` — attributes of `strong`.
pub const HTML_ATTRS_89: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS90` in `maps/html.py` — attributes of `style`.
pub const HTML_ATTRS_90: &[(u32, &str)] = &[
    (0x03EB, "type"),
    (0x03EF, "media"),
    (0x8046, "title"),
    (0x978E, "onerror"),
    (0x9791, "onload"),
];

/// `ATTRS91` in `maps/html.py` — attributes of `sub`.
pub const HTML_ATTRS_91: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS92` in `maps/html.py` — attributes of `sup`.
pub const HTML_ATTRS_92: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS93` in `maps/html.py` — attributes of `table`.
pub const HTML_ATTRS_93: &[(u32, &str)] = &[
    (0x03EA, "cols"),
    (0x03EB, "border"),
    (0x03EC, "rules"),
    (0x03ED, "frame"),
    (0x03EE, "cellspacing"),
    (0x03EF, "cellpadding"),
    (0x03FA, "datapagesize"),
    (0x8006, "width"),
    (0x8007, "height"),
    (0x8046, "title"),
    (0x804A, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x938A, "background"),
    (0x93A5, "bordercolor"),
    (0x93A6, "bordercolorlight"),
    (0x93A7, "bordercolordark"),
    (0xFE0C, "bgcolor"),
];

/// `ATTRS94` in `maps/html.py` — attributes of `tbody`.
pub const HTML_ATTRS_94: &[(u32, &str)] =
    &[(0x8049, "align"), (0x93A8, "valign"), (0xFE0C, "bgcolor")];

/// `ATTRS95` in `maps/html.py` — attributes of `tc`.
pub const HTML_ATTRS_95: &[(u32, &str)] = &[(0x8049, "align"), (0x93A8, "valign")];

/// `ATTRS96` in `maps/html.py` — attributes of `td`.
pub const HTML_ATTRS_96: &[(u32, &str)] = &[
    (0x07D2, "rowspan"),
    (0x07D3, "colspan"),
    (0x8006, "width"),
    (0x8007, "height"),
    (0x8046, "title"),
    (0x8049, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x938A, "background"),
    (0x938E, "nowrap"),
    (0x93A5, "bordercolor"),
    (0x93A6, "bordercolorlight"),
    (0x93A7, "bordercolordark"),
    (0x93A8, "valign"),
    (0xFE0C, "bgcolor"),
];

/// `ATTRS97` in `maps/html.py` — attributes of `textarea`.
pub const HTML_ATTRS_97: &[(u32, &str)] = &[
    (0x1B5A, "rows"),
    (0x1B5B, "cols"),
    (0x1B5C, "wrap"),
    (0x1B5D, "readonly"),
    (0x8001, "name"),
    (0x977B, "onselect"),
    (0x978F, "onchange"),
];

/// `ATTRS98` in `maps/html.py` — attributes of `tfoot`.
pub const HTML_ATTRS_98: &[(u32, &str)] =
    &[(0x8049, "align"), (0x93A8, "valign"), (0xFE0C, "bgcolor")];

/// `ATTRS99` in `maps/html.py` — attributes of `th`.
pub const HTML_ATTRS_99: &[(u32, &str)] = &[
    (0x07D2, "rowspan"),
    (0x07D3, "colspan"),
    (0x8006, "width"),
    (0x8007, "height"),
    (0x8046, "title"),
    (0x8049, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x938A, "background"),
    (0x938E, "nowrap"),
    (0x93A5, "bordercolor"),
    (0x93A6, "bordercolorlight"),
    (0x93A7, "bordercolordark"),
    (0x93A8, "valign"),
    (0xFE0C, "bgcolor"),
];

/// `ATTRS100` in `maps/html.py` — attributes of `thead`.
pub const HTML_ATTRS_100: &[(u32, &str)] =
    &[(0x8049, "align"), (0x93A8, "valign"), (0xFE0C, "bgcolor")];

/// `ATTRS102` in `maps/html.py` — attributes of `tr`.
pub const HTML_ATTRS_102: &[(u32, &str)] = &[
    (0x8007, "height"),
    (0x8046, "title"),
    (0x8049, "align"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x93A5, "bordercolor"),
    (0x93A6, "bordercolorlight"),
    (0x93A7, "bordercolordark"),
    (0x93A8, "valign"),
    (0xFE0C, "bgcolor"),
];

/// `ATTRS103` in `maps/html.py` — attributes of `tt`.
pub const HTML_ATTRS_103: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS104` in `maps/html.py` — attributes of `u`.
pub const HTML_ATTRS_104: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `ATTRS105` in `maps/html.py` — attributes of `ul`.
pub const HTML_ATTRS_105: &[(u32, &str)] = &[
    (0x03EB, "compact"),
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
    (0x939A, "type"),
];

/// `ATTRS106` in `maps/html.py` — attributes of `var`.
pub const HTML_ATTRS_106: &[(u32, &str)] = &[
    (0x8046, "title"),
    (0x804B, "style"),
    (0x83EA, "class"),
    (0x83EB, "id"),
];

/// `TAGS_ATTRS` in `maps/html.py`, indexed by tag code.
pub const HTML_TAG_ATTRS: &[AttrTable] = &[
    &[],            // 0
    &[],            // 1
    &[],            // 2
    HTML_ATTRS_3,   // 3 a
    &[],            // 4 acronym
    HTML_ATTRS_5,   // 5 address
    HTML_ATTRS_6,   // 6 applet
    HTML_ATTRS_7,   // 7 area
    HTML_ATTRS_8,   // 8 b
    HTML_ATTRS_9,   // 9 base
    HTML_ATTRS_10,  // 10 basefont
    &[],            // 11 bdo
    HTML_ATTRS_12,  // 12 bgsound
    HTML_ATTRS_13,  // 13 big
    &[],            // 14 blink
    HTML_ATTRS_15,  // 15 blockquote
    HTML_ATTRS_16,  // 16 body
    HTML_ATTRS_17,  // 17 br
    HTML_ATTRS_18,  // 18 button
    HTML_ATTRS_19,  // 19 caption
    HTML_ATTRS_20,  // 20 center
    HTML_ATTRS_21,  // 21 cite
    HTML_ATTRS_22,  // 22 code
    HTML_ATTRS_23,  // 23 col
    HTML_ATTRS_24,  // 24 colgroup
    &[],            // 25
    &[],            // 26
    HTML_ATTRS_27,  // 27 dd
    &[],            // 28 del
    HTML_ATTRS_29,  // 29 dfn
    &[],            // 30 dir
    HTML_ATTRS_31,  // 31 div
    HTML_ATTRS_32,  // 32 dl
    HTML_ATTRS_33,  // 33 dt
    HTML_ATTRS_34,  // 34 em
    HTML_ATTRS_35,  // 35 embed
    HTML_ATTRS_36,  // 36 fieldset
    HTML_ATTRS_37,  // 37 font
    HTML_ATTRS_38,  // 38 form
    HTML_ATTRS_39,  // 39 frame
    HTML_ATTRS_40,  // 40 frameset
    &[],            // 41
    HTML_ATTRS_42,  // 42 h1
    HTML_ATTRS_43,  // 43 h2
    HTML_ATTRS_44,  // 44 h3
    HTML_ATTRS_45,  // 45 h4
    HTML_ATTRS_46,  // 46 h5
    HTML_ATTRS_47,  // 47 h6
    &[],            // 48 head
    HTML_ATTRS_49,  // 49 hr
    &[],            // 50 html
    HTML_ATTRS_51,  // 51 i
    HTML_ATTRS_52,  // 52 iframe
    HTML_ATTRS_53,  // 53 img
    HTML_ATTRS_54,  // 54 input
    &[],            // 55 ins
    HTML_ATTRS_56,  // 56 kbd
    HTML_ATTRS_57,  // 57 label
    HTML_ATTRS_58,  // 58 legend
    HTML_ATTRS_59,  // 59 li
    HTML_ATTRS_60,  // 60 link
    HTML_ATTRS_61,  // 61 tag61
    HTML_ATTRS_62,  // 62 map
    HTML_ATTRS_63,  // 63 tag63
    &[],            // 64 tag64
    HTML_ATTRS_65,  // 65 meta
    HTML_ATTRS_66,  // 66 nextid
    &[],            // 67 nobr
    &[],            // 68 noembed
    &[],            // 69 noframes
    &[],            // 70 noscript
    HTML_ATTRS_71,  // 71 object
    HTML_ATTRS_72,  // 72 ol
    HTML_ATTRS_73,  // 73 option
    HTML_ATTRS_74,  // 74 p
    HTML_ATTRS_75,  // 75 param
    HTML_ATTRS_76,  // 76 plaintext
    HTML_ATTRS_77,  // 77 pre
    HTML_ATTRS_78,  // 78 q
    &[],            // 79 rp
    &[],            // 80 rt
    &[],            // 81 ruby
    HTML_ATTRS_82,  // 82 s
    HTML_ATTRS_83,  // 83 samp
    HTML_ATTRS_84,  // 84 script
    HTML_ATTRS_85,  // 85 select
    HTML_ATTRS_86,  // 86 small
    HTML_ATTRS_87,  // 87 span
    HTML_ATTRS_88,  // 88 strike
    HTML_ATTRS_89,  // 89 strong
    HTML_ATTRS_90,  // 90 style
    HTML_ATTRS_91,  // 91 sub
    HTML_ATTRS_92,  // 92 sup
    HTML_ATTRS_93,  // 93 table
    HTML_ATTRS_94,  // 94 tbody
    HTML_ATTRS_95,  // 95 tc
    HTML_ATTRS_96,  // 96 td
    HTML_ATTRS_97,  // 97 textarea
    HTML_ATTRS_98,  // 98 tfoot
    HTML_ATTRS_99,  // 99 th
    HTML_ATTRS_100, // 100 thead
    &[],            // 101 title
    HTML_ATTRS_102, // 102 tr
    HTML_ATTRS_103, // 103 tt
    HTML_ATTRS_104, // 104 u
    HTML_ATTRS_105, // 105 ul
    HTML_ATTRS_106, // 106 var
    &[],            // 107 wbr
    &[],            // 108
];

/// `TAGS` in `maps/opf.py`.
pub const OPF_TAGS: &[Option<&str>] = &[
    None,                   // 0
    Some("package"),        // 1
    Some("dc:Title"),       // 2
    Some("dc:Creator"),     // 3
    None,                   // 4
    None,                   // 5
    None,                   // 6
    None,                   // 7
    None,                   // 8
    None,                   // 9
    None,                   // 10
    None,                   // 11
    None,                   // 12
    None,                   // 13
    None,                   // 14
    None,                   // 15
    Some("manifest"),       // 16
    Some("item"),           // 17
    Some("spine"),          // 18
    Some("itemref"),        // 19
    Some("metadata"),       // 20
    Some("dc-metadata"),    // 21
    Some("dc:Subject"),     // 22
    Some("dc:Description"), // 23
    Some("dc:Publisher"),   // 24
    Some("dc:Contributor"), // 25
    Some("dc:Date"),        // 26
    Some("dc:Type"),        // 27
    Some("dc:Format"),      // 28
    Some("dc:Identifier"),  // 29
    Some("dc:Source"),      // 30
    Some("dc:Language"),    // 31
    Some("dc:Relation"),    // 32
    Some("dc:Coverage"),    // 33
    Some("dc:Rights"),      // 34
    Some("x-metadata"),     // 35
    Some("meta"),           // 36
    Some("tours"),          // 37
    Some("tour"),           // 38
    Some("site"),           // 39
    Some("guide"),          // 40
    Some("reference"),      // 41
    None,                   // 42
];

/// `ATTRS` in `maps/opf.py`.
pub const OPF_ATTRS: &[(u32, &str)] = &[
    (0x0001, "href"),
    (0x0002, "%never-used"),
    (0x0003, "%guid"),
    (0x0004, "%minimum_level"),
    (0x0005, "%attr5"),
    (0x0006, "id"),
    (0x0007, "href"),
    (0x0008, "media-type"),
    (0x0009, "fallback"),
    (0x000A, "idref"),
    (0x000B, "xmlns:dc"),
    (0x000C, "xmlns:oebpackage"),
    (0x000D, "role"),
    (0x000E, "file-as"),
    (0x000F, "event"),
    (0x0010, "scheme"),
    (0x0011, "title"),
    (0x0012, "type"),
    (0x0013, "unique-identifier"),
    (0x0014, "name"),
    (0x0015, "content"),
    (0x0016, "xml:lang"),
];

/// `TAGS_ATTRS` in `maps/opf.py` — all empty, but the length matters
/// because `writer.py` indexes it by tag code.
pub const OPF_TAG_ATTRS: &[AttrTable] = &[EMPTY_ATTRS; 43];

/// `HTML_MAP` in `calibre.ebooks.lit.maps`.
pub const HTML_MAP: TagMap = TagMap {
    tags: HTML_TAGS,
    attrs: HTML_ATTRS,
    tag_attrs: HTML_TAG_ATTRS,
    html: true,
};

/// `OPF_MAP` in `calibre.ebooks.lit.maps`.
pub const OPF_MAP: TagMap = TagMap {
    tags: OPF_TAGS,
    attrs: OPF_ATTRS,
    tag_attrs: OPF_TAG_ATTRS,
    html: false,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_lengths_match_the_python() {
        assert_eq!(HTML_TAGS.len(), 109);
        assert_eq!(HTML_ATTRS.len(), 59);
        assert_eq!(HTML_TAG_ATTRS.len(), HTML_TAGS.len());
        assert_eq!(OPF_TAGS.len(), 43);
        assert_eq!(OPF_ATTRS.len(), 22);
        assert_eq!(OPF_TAG_ATTRS.len(), OPF_TAGS.len());
    }

    #[test]
    fn spot_checks_against_the_python_tables() {
        assert_eq!(HTML_MAP.tag(3), Some("a"));
        assert_eq!(HTML_MAP.tag(16), Some("body"));
        assert_eq!(HTML_MAP.tag(50), Some("html"));
        assert_eq!(HTML_MAP.tag(0), None);
        assert_eq!(HTML_MAP.tag(1), None);
        assert_eq!(OPF_MAP.tag(1), Some("package"));
        assert_eq!(OPF_MAP.tag(17), Some("item"));
        assert_eq!(attr_name(OPF_ATTRS, 0x0008), Some("media-type"));
        assert_eq!(attr_name(OPF_ATTRS, 0x0001), Some("href"));
        assert_eq!(attr_name(OPF_ATTRS, 0x0007), Some("href"));
        assert_eq!(attr_name(OPF_ATTRS, 0xFFFF), None);
    }

    #[test]
    fn out_of_range_lookups_are_none_not_panics() {
        assert_eq!(HTML_MAP.tag(100_000), None);
        assert!(HTML_MAP.tag_attrs(100_000).is_empty());
        assert!(OPF_MAP.tag_attrs(0).is_empty());
    }

    #[test]
    fn only_the_html_map_reports_itself_as_html() {
        assert!(HTML_MAP.is_html());
        assert!(!OPF_MAP.is_html());
    }
}
