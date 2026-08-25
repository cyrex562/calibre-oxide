//! Symbol-font glyph remapping ([`is_symbol_font`]/[`map_symbol_text`]/
//! [`SYMBOL_MAPS`]) and the `Fonts` class -- reading a document's
//! `fontTable.xml`-declared families ([`Fonts::call`]/[`Family`]),
//! resolving a run's CSS `font-family` string ([`Fonts::family_for`]),
//! and extracting/writing whichever embedded fonts actually got used
//! ([`Fonts::embed_fonts`]/`write`).
//!
//! Full port of `old_src/src/calibre/ebooks/docx/fonts.py`. An earlier
//! version of this module ported only the symbol-map half, believing
//! the rest was blocked on `calibre.utils.fonts.scanner.font_scanner`
//! (a *system*-installed-font resolver with no Rust counterpart) --
//! tracing the Python source precisely found that's wrong: the scanner
//! is used in exactly one place, [`has_system_fonts`] (deciding
//! whether `Family` should prefer a declared `w:altName` over the
//! font's own primary name, when the primary isn't installed on the
//! system) -- everything else (`family_for`, `embed_fonts`, `write`,
//! `is_truetype_font`, `panose_to_css_generic_family`) has zero system
//! dependency. [`has_system_fonts`] is stubbed to always return
//! `false` -- see its own doc comment for why that's a real, already-
//! existing Python code path, not a hypothetical.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::path::Path;
use std::sync::LazyLock;

use calibre_utils::filenames::ascii_filename;
use roxmltree::Node;

use super::block_styles::binary_property;
use super::container::Docx;
use super::names::DocxNamespace;

/// One symbol font's 256-codepoint glyph table (`0xF000..=0xF0FF`),
/// in Unicode-codepoint order (`SYMBOL_MAPS[font][0]` is `U+F000`).
///
/// Port of the Python `SYMBOL_MAPS`.
pub static SYMBOL_MAPS: LazyLock<HashMap<&'static str, [&'static str; 256]>> =
    LazyLock::new(|| {
        let mut m = HashMap::new();
        m.insert(
            "Wingdings",
            [
                " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ",
                " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ",
                " ", "🖉", "✂", "✁", "👓", "🕭", "🕮", "🕯", "🕿", "✆", "🖂", "🖃", "📪", "📫", "📬",
                "📭", "🗀", "🗁", "🗎", "🗏", "🗐", "🗄", "⏳", "🖮", "🖰", "🖲", "🖳", "🖴", "🖫", "🖬", "✇",
                "✍", "🖎", "✌", "🖏", "👍", "👎", "☜", "☞", "☜", "🖗", "🖐", "☺", "😐", "☹", "💣", "🕱",
                "🏳", "🏱", "✈", "☼", "🌢", "❄", "🕆", "✞", "🕈", "✠", "✡", "☪", "☯", "🕉", "☸", "♈",
                "♉", "♊", "♋", "♌", "♍", "♎", "♏", "♐", "♑", "♒", "♓", "🙰", "🙵", "⚫",
                "🔾", "◼", "🞏", "🞐", "❑", "❒", "🞟", "⧫", "◆", "❖", "🞙", "⌧", "⮹", "⌘", "🏵", "🏶",
                "🙶", "🙷", " ", "🄋", "➀", "➁", "➂", "➃", "➄", "➅", "➆", "➇", "➈", "➉", "🄌", "➊",
                "➋", "➌", "➍", "➎", "➏", "➐", "➑", "➒", "➓", "🙢", "🙠", "🙡", "🙣", "🙦", "🙤", "🙥",
                "🙧", "∙", "•", "⬝", "⭘", "🞆", "🞈", "🞊", "🞋", "🔿", "▪", "🞎", "🟀", "🟁", "★", "🟋",
                "🟏", "🟓", "🟑", "⯐", "⌖", "⯎", "⯏", "⯑", "✪", "✰", "🕐", "🕑", "🕒", "🕓", "🕔",
                "🕕", "🕖", "🕗", "🕘", "🕙", "🕚", "🕛", "⮰", "⮱", "⮲", "⮳", "⮴", "⮵", "⮶", "⮷",
                "🙪", "🙫", "🙕", "🙔", "🙗", "🙖", "🙐", "🙑", "🙒", "🙓", "⌫", "⌦", "⮘", "⮚", "⮙", "⮛",
                "⮈", "⮊", "⮉", "⮋", "🡨", "🡪", "🡩", "🡫", "🡬", "🡭", "🡯", "🡮", "🡸", "🡺", "🡹", "🡻",
                "🡼", "🡽", "🡿", "🡾", "⇦", "⇨", "⇧", "⇩", "⬄", "⇳", "⬁", "⬀", "⬃", "⬂", "🢬", "🢭",
                "🗶", "✓", "🗷", "🗹", " ",
            ],
        );
        m.insert(
            "Wingdings 2",
            [
                " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ",
                " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ",
                " ", "🖊", "🖋", "🖌", "🖍", "✄", "✀", "🕾", "🕽", "🗅", "🗆", "🗇", "🗈", "🗉", "🗊", "🗋",
                "🗌", "🗍", "📋", "🗑", "🗔", "🖵", "🖶", "🖷", "🖸", "🖭", "🖯", "🖱", "🖒", "🖓", "🖘", "🖙",
                "🖚", "🖛", "👈", "👉", "🖜", "🖝", "🖞", "🖟", "🖠", "🖡", "👆", "👇", "🖢", "🖣", "🖑", "🗴",
                "🗸", "🗵", "☑", "⮽", "☒", "⮾", "⮿", "🛇", "⦸", "🙱", "🙴", "🙲", "🙳", "‽", "🙹", "🙺",
                "🙻", "🙦", "🙤", "🙥", "🙧", "🙚", "🙘", "🙙", "🙛", "⓪", "①", "②", "③", "④", "⑤", "⑥",
                "⑦", "⑧", "⑨", "⑩", "⓿", "❶", "❷", "❸", "❹", "❺", "❻", "❼", "❽", "❾", "❿", " ",
                "☉", "🌕", "☽", "☾", "⸿", "✝", "🕇", "🕜", "🕝", "🕞", "🕟", "🕠", "🕡", "🕢", "🕣",
                "🕤", "🕥", "🕦", "🕧", "🙨", "🙩", "⋅", "🞄", "⦁", "●", "●", "🞅", "🞇", "🞉", "⊙", "⦿",
                "🞌", "🞍", "◾", "■", "□", "🞑", "🞒", "🞓", "🞔", "▣", "🞕", "🞖", "🞗", "🞘", "⬩", "⬥",
                "◇", "🞚", "◈", "🞛", "🞜", "🞝", "🞞", "⬪", "⬧", "◊", "🞠", "◖", "◗", "⯊", "⯋", "⯀",
                "⯁", "⬟", "⯂", "⬣", "⬢", "⯃", "⯄", "🞡", "🞢", "🞣", "🞤", "🞥", "🞦", "🞧", "🞨", "🞩",
                "🞪", "🞫", "🞬", "🞭", "🞮", "🞯", "🞰", "🞱", "🞲", "🞳", "🞴", "🞵", "🞶", "🞷", "🞸", "🞹",
                "🞺", "🞻", "🞼", "🞽", "🞾", "🞿", "🟀", "🟂", "🟄", "🟆", "🟉", "🟊", "✶", "🟌", "🟎", "🟐",
                "🟒", "✹", "🟃", "🟇", "✯", "🟍", "🟔", "⯌", "⯍", "※", "⁂", " ", " ", " ", " ", " ",
                " ",
            ],
        );
        m.insert(
            "Wingdings 3",
            [
                " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ",
                " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ",
                " ", "⭠", "⭢", "⭡", "⭣", "⭤", "⭥", "⭧", "⭦", "⭰", "⭲", "⭱", "⭳", "⭶", "⭸", "⭻",
                "⭽", "⭤", "⭥", "⭪", "⭬", "⭫", "⭭", "⭍", "⮠", "⮡", "⮢", "⮣", "⮤", "⮥", "⮦", "⮧",
                "⮐", "⮑", "⮒", "⮓", "⮀", "⮃", "⭾", "⭿", "⮄", "⮆", "⮅", "⮇", "⮏", "⮍", "⮎", "⮌",
                "⭮", "⭯", "⎋", "⌤", "⌃", "⌥", "␣", "⍽", "⇪", "⮸", "🢠", "🢡", "🢢", "🢣", "🢤", "🢥",
                "🢦", "🢧", "🢨", "🢩", "🢪", "🢫", "🡐", "🡒", "🡑", "🡓", "🡔", "🡕", "🡗", "🡖", "🡘", "🡙",
                "▲", "▼", "△", "▽", "◀", "▶", "◁", "▷", "◣", "◢", "◤", "◥", "🞀", "🞂", "🞁", " ",
                "🞃", "⯅", "⯆", "⯇", "⯈", "⮜", "⮞", "⮝", "⮟", "🠐", "🠒", "🠑", "🠓", "🠔", "🠖", "🠕",
                "🠗", "🠘", "🠚", "🠙", "🠛", "🠜", "🠞", "🠝", "🠟", "🠀", "🠂", "🠁", "🠃", "🠄", "🠆", "🠅",
                "🠇", "🠈", "🠊", "🠉", "🠋", "🠠", "🠢", "🠤", "🠦", "🠨", "🠪", "🠬", "🢜", "🢝", "🢞", "🢟",
                "🠮", "🠰", "🠲", "🠴", "🠶", "🠸", "🠺", "🠹", "🠻", "🢘", "🢚", "🢙", "🢛", "🠼", "🠾", "🠽",
                "🠿", "🡀", "🡂", "🡁", "🡃", "🡄", "🡆", "🡅", "🡇", "⮨", "⮩", "⮪", "⮫", "⮬", "⮭", "⮮",
                "⮯", "🡠", "🡢", "🡡", "🡣", "🡤", "🡥", "🡧", "🡦", "🡰", "🡲", "🡱", "🡳", "🡴", "🡵", "🡷",
                "🡶", "🢀", "🢂", "🢁", "🢃", "🢄", "🢅", "🢇", "🢆", "🢐", "🢒", "🢑", "🢓", "🢔", "🢕", "🢗",
                "🢖", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ",
            ],
        );
        m.insert(
            "Webdings",
            [
                " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ",
                " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ",
                " ", "🕷", "🕸", "🕲", "🕶", "🏆", "🎖", "🖇", "🗨", "🗩", "🗰", "🗱", "🌶", "🎗", "🙾", "🙼",
                "🗕", "🗖", "🗗", "⏴", "⏵", "⏶", "⏷", "⏪", "⏩", "⏮", "⏭", "⏸", "⏹", "⏺", "🗚", "🗳",
                "🛠", "🏗", "🏘", "🏙", "🏚", "🏜", "🏭", "🏛", "🏠", "🏖", "🏝", "🛣", "🔍", "🏔", "👁", "👂",
                "🏞", "🏕", "🛤", "🏟", "🛳", "🕬", "🕫", "🕨", "🔈", "🎔", "🎕", "🗬", "🙽", "🗭", "🗪", "🗫",
                "⮔", "✔", "🚲", "⬜", "🛡", "📦", "🛱", "⬛", "🚑", "🛈", "🛩", "🛰", "🟈", "🕴", "⬤",
                "🛥", "🚔", "🗘", "🗙", "❓", "🛲", "🚇", "🚍", "⛳", "⦸", "⊖", "🚭", "🗮", "⏐", "🗯",
                "🗲", " ", "🚹", "🚺", "🛉", "🛊", "🚼", "👽", "🏋", "⛷", "🏂", "🏌", "🏊", "🏄", "🏍",
                "🏎", "🚘", "🗠", "🛢", "📠", "🏷", "📣", "👪", "🗡", "🗢", "🗣", "✯", "🖄", "🖅", "🖃", "🖆",
                "🖹", "🖺", "🖻", "🕵", "🕰", "🖽", "🖾", "📋", "🗒", "🗓", "🕮", "📚", "🗞", "🗟", "🗃", "🗂",
                "🖼", "🎭", "🎜", "🎘", "🎙", "🎧", "💿", "🎞", "📷", "🎟", "🎬", "📽", "📹", "📾", "📻",
                "🎚", "🎛", "📺", "💻", "🖥", "🖦", "🖧", "🍹", "🎮", "🎮", "🕻", "🕼", "🖁", "🖀", "🖨",
                "🖩", "🖿", "🖪", "🗜", "🔒", "🔓", "🗝", "📥", "📤", "🕳", "🌣", "🌤", "🌥", "🌦", "☁", "🌨",
                "🌧", "🌩", "🌪", "🌬", "🌫", "🌜", "🌡", "🛋", "🛏", "🍽", "🍸", "🛎", "🛍", "Ⓟ", "♿", "🛆",
                "🖈", "🎓", "🗤", "🗥", "🗦", "🗧", "🛪", "🐿", "🐦", "🐟", "🐕", "🐈", "🙬", "🙮", "🙭",
                "🙯", "🗺", "🌍", "🌏", "🌎", "🕊",
            ],
        );
        m.insert(
            "Symbol",
            [
                " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ",
                " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ",
                " ", "!", "∀", "#", "∃", "%", "&", "∍", "(", ")", "*", "+", ",", "−", ".", "/",
                "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", ":", ";", "<", "=", ">", "?",
                "≅", "Α", "Β", "Χ", "Δ", "Ε", "Φ", "Γ", "Η", "Ι", "ϑ", "Λ", "Μ", "Ν", "Ξ", "Ο",
                "Π", "Θ", "Ρ", "Σ", "Τ", "Υ", "ς", "Ω", "Ξ", "Ψ", "Ζ", "[", "∴", "]", "⊥", "_",
                "", "α", "β", "χ", "δ", "ε", "φ", "γ", "η", "ι", "ϕ", "λ", "μ", "ν", "ξ", "ο",
                "π", "θ", "ρ", "σ", "τ", "υ", "ϖ", "ω", "ξ", "ψ", "ζ", "{", "|", "}", "~", " ",
                " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ",
                " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ", " ",
                "€", "ϒ", "′", "≤", "⁄", "∞", "ƒ", "♣", "♥", "♦", "♠", "↔", "←", "↑", "→", "↓",
                "°", "±", "″", "≥", "×", "∝", "∂", "•", "÷", "≠", "≡", "≈", "…", "⏐", "⎯", "↲",
                "ℵ", "ℑ", "ℜ", "℘", "⊗", "⊕", "∅", "∩", "∪", "⊃", "⊇", "⊄", "⊂", "⊆", "∈", "∉",
                "∠", "∂", "®", "©", "™", "∏", "√", "⋅", "¬", "∦", "∧", "⇔", "⇐", "⇑", "⇒", "⇓",
                "◊", "〈", "®", "©", "™", "∑", "⎛", "⎜", "⎝", "⎡", "⎢", "⎣", "⎧", "⎨", "⎩", "⎪",
                " ", "〉", "∫", "⌠", "⎮", "⌡", "⎞", "⎟", "⎠", "⎤", "⎥", "⎦", "⎪", "⎫", "⎬", " ",
            ],
        );
        m
    });

/// Port of the Python `is_symbol_font`.
pub fn is_symbol_font(family: &str) -> bool {
    SYMBOL_MAPS.contains_key(family_key(family).as_str())
}

/// `SYMBOL_MAPS` is keyed by the font's exact-case name; Python's own
/// `is_symbol_font` lowercases first via a separate `SYMBOL_FONT_NAMES`
/// set built from the same keys -- reproduced here as a lowercase
/// lookup pass rather than a second table, same effect.
fn family_key(family: &str) -> String {
    SYMBOL_MAPS
        .keys()
        .find(|k| k.eq_ignore_ascii_case(family))
        .copied()
        .unwrap_or(family)
        .to_string()
}

/// Remaps `text` through `font`'s private-use-area glyph table.
/// Codepoints outside `U+F000..=U+F0FF` pass through unchanged.
///
/// Port of the Python `map_symbol_text`/`do_map`.
pub fn map_symbol_text(text: &str, font: &str) -> String {
    let Some(m) = SYMBOL_MAPS.get(family_key(font).as_str()) else {
        return text.to_string();
    };
    text.chars()
        .map(|c| {
            let p = c as u32;
            if (0xf000..0xf100).contains(&p) {
                m[(p - 0xf000) as usize].to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// This crate has no system font scanner (`calibre.utils.fonts.scanner`
/// has no Rust counterpart) -- always returns `false`. This is a real,
/// already-existing Python code path (`except NoFonts: return False`,
/// for a system with zero fonts matching the requested family), not a
/// hypothetical: it means every [`Family`] here keeps its own primary
/// declared name rather than ever switching to a `w:altName`, leaving
/// the reading device's own CSS font-family fallback
/// ([`Family::css_generic_family`]) to handle a font that turns out
/// not to be installed there.
///
/// Port of the Python `has_system_fonts`.
pub fn has_system_fonts(_name: &str) -> bool {
    false
}

/// `(bold, italic)` -> the `w:embed*`/variant-map key Word itself uses.
///
/// Port of the Python `get_variant`.
pub fn get_variant(bold: bool, italic: bool) -> &'static str {
    match (bold, italic) {
        (false, false) => "Regular",
        (false, true) => "Italic",
        (true, false) => "Bold",
        (true, true) => "BoldItalic",
    }
}

/// PANOSE classification (ECMA-376's `w:panose1`, ten bytes) -> a CSS
/// generic font family, used when a font declares no `w:family`
/// generic class of its own.
///
/// Port of the Python `panose_to_css_generic_family`
/// (`calibre.utils.fonts.utils`).
pub fn panose_to_css_generic_family(panose: &[u8]) -> String {
    let proportion = panose.get(3).copied().unwrap_or(0);
    if proportion == 9 {
        return "monospace".to_string();
    }
    let family_type = panose.first().copied().unwrap_or(0);
    if family_type == 3 {
        return "cursive".to_string();
    }
    if family_type == 4 {
        return "fantasy".to_string();
    }
    let serif_style = panose.get(1).copied().unwrap_or(0);
    if matches!(serif_style, 11 | 12 | 13) {
        return "sans-serif".to_string();
    }
    "serif".to_string()
}

/// Parses a run of hex-digit pairs (`w:panose1`'s/an embedded font
/// key's `@w:val`), two characters -> one byte, tolerating a trailing
/// single digit the way Python's `v[i:i+2]` slicing does. Returns
/// `None` on any unparseable byte, matching Python's own
/// `except (TypeError, ValueError, IndexError): pass` (the caller
/// keeps whatever default it already had).
fn parse_hex_bytes(v: &str) -> Option<Vec<u8>> {
    let chars: Vec<char> = v.chars().collect();
    let mut out = Vec::with_capacity(chars.len().div_ceil(2));
    let mut i = 0;
    while i < chars.len() {
        let end = (i + 2).min(chars.len());
        let s: String = chars[i..end].iter().collect();
        out.push(u8::from_str_radix(&s, 16).ok()?);
        i += 2;
    }
    Some(out)
}

/// One embedded font file's relationship id (resolved to its real zip
/// name), obfuscation key, and subsetted flag.
///
/// Port of the Python `Embed` namedtuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Embed {
    pub name: String,
    pub key: Option<String>,
    pub subsetted: bool,
}

/// One `fontTable.xml` `w:font` entry: its declared name(s), whichever
/// `Regular`/`Bold`/`Italic`/`BoldItalic` variants are embedded, and
/// its CSS generic-family fallback.
///
/// Port of the Python `Family`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Family {
    pub name: String,
    pub family_name: String,
    pub alt_names: Vec<String>,
    pub embedded: HashMap<String, Embed>,
    pub generic_family: String,
    pub is_ttf: bool,
    pub panose1: Option<Vec<u8>>,
    pub panose_name: Option<String>,
    pub css_generic_family: String,
}

impl Family {
    /// Port of `Family.__init__`.
    pub fn from_elem(
        elem: Node,
        embed_relationships: &HashMap<String, String>,
        ns: &DocxNamespace,
    ) -> Self {
        let name = ns.get(elem, "w:name").unwrap_or_default().to_string();
        let mut family_name = name.clone();

        let alt_names: Vec<String> = ns
            .children(elem, &["w:altName"])
            .into_iter()
            .filter_map(|x| ns.get(x, "w:val"))
            .map(str::to_string)
            .collect();
        if !alt_names.is_empty() && !has_system_fonts(&name) {
            if let Some(x) = alt_names.iter().find(|x| has_system_fonts(x)) {
                family_name = x.clone();
            }
        }

        let mut embedded = HashMap::new();
        for variant in ["Regular", "Bold", "Italic", "BoldItalic"] {
            let tag = format!("w:embed{variant}");
            for y in ns.children(elem, &[tag.as_str()]) {
                let Some(rid) = ns.get(y, "r:id") else {
                    continue;
                };
                let Some(target) = embed_relationships.get(rid) else {
                    continue;
                };
                let key = ns.get(y, "w:fontKey").map(str::to_string);
                let subsetted = matches!(ns.get(y, "w:subsetted"), Some("1" | "true" | "on"));
                embedded.insert(
                    variant.to_string(),
                    Embed {
                        name: target.clone(),
                        key,
                        subsetted,
                    },
                );
            }
        }

        let mut generic_family = "auto".to_string();
        for x in ns.children(elem, &["w:family"]) {
            generic_family = ns.get_or(x, "w:val", "auto").to_string();
        }

        let ntt = binary_property(elem, "notTrueType", ns);
        let is_ttf = !ntt.unwrap_or(false);

        let mut panose1 = None;
        let mut panose_name = None;
        for x in ns.children(elem, &["w:panose1"]) {
            if let Some(v) = ns.get(x, "w:val") {
                if let Some(bytes) = parse_hex_bytes(v) {
                    panose_name = Some(panose_to_css_generic_family(&bytes));
                    panose1 = Some(bytes);
                }
            }
        }

        let css_generic_family = match generic_family.as_str() {
            "roman" => Some("serif"),
            "swiss" => Some("sans-serif"),
            "modern" => Some("monospace"),
            "decorative" => Some("fantasy"),
            "script" => Some("cursive"),
            _ => None,
        }
        .map(str::to_string)
        .or_else(|| panose_name.clone())
        .unwrap_or_else(|| "serif".to_string());

        Family {
            name,
            family_name,
            alt_names,
            embedded,
            generic_family,
            is_ttf,
            panose1,
            panose_name,
            css_generic_family,
        }
    }
}

/// Reads a document's declared font families ([`Fonts::call`]),
/// resolves a run's CSS `font-family` string against them
/// ([`Fonts::family_for`], tracking which `(name, variant)` pairs
/// actually got used), and extracts/writes those, and only those,
/// embedded font files as real `@font-face` CSS
/// ([`Fonts::embed_fonts`]).
///
/// Port of the Python `Fonts` class. Python keeps `self.fonts`/
/// `self.used` as instance state written by `__call__`/`family_for`
/// and read by `embed_fonts`/`write`; this port's `family_for` takes
/// `&mut self` for the same reason (recording `used` as it resolves
/// each run), matching every other `&mut self`-cache method in this
/// package (`Styles::resolve_run`, etc.) rather than introducing a
/// separately-threaded tracking parameter.
#[derive(Debug, Clone, Default)]
pub struct Fonts {
    fonts: HashMap<String, Family>,
    used: HashSet<(String, String)>,
}

impl Fonts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads every `w:font[@w:name]` under `root` (a `fontTable.xml`
    /// document's root `w:fonts` element).
    ///
    /// Port of `Fonts.__call__`.
    pub fn call(
        &mut self,
        root: Node,
        embed_relationships: &HashMap<String, String>,
        ns: &DocxNamespace,
    ) {
        for elem in ns.descendants(root, &["w:font"]) {
            if let Some(name) = ns.get(elem, "w:name") {
                self.fonts.insert(
                    name.to_string(),
                    Family::from_elem(elem, embed_relationships, ns),
                );
            }
        }
    }

    /// A declared family name a `w:font` entry exists for, and only
    /// such a name -- used by tests/callers that need to look up what
    /// [`Fonts::call`] actually read without going through
    /// `family_for`'s `used`-tracking side effect.
    pub fn get(&self, name: &str) -> Option<&Family> {
        self.fonts.get(name)
    }

    /// Resolves `name`/`bold`/`italic` to a CSS `font-family` value,
    /// recording `(name, variant)` as used so [`Fonts::embed_fonts`]
    /// later knows which embedded fonts are actually referenced.
    ///
    /// Port of `Fonts.family_for`.
    pub fn family_for(&mut self, name: &str, bold: bool, italic: bool) -> String {
        let Some(f) = self.fonts.get(name) else {
            return "serif".to_string();
        };
        let variant = get_variant(bold, italic);
        self.used.insert((name.to_string(), variant.to_string()));
        let resolved = if f.embedded.contains_key(variant) {
            f.name.clone()
        } else {
            f.family_name.clone()
        };
        if is_symbol_font(&resolved) {
            return resolved;
        }
        format!(
            "\"{}\", {}",
            resolved.replace('"', ""),
            f.css_generic_family
        )
    }

    /// Extracts and writes every embedded font [`Fonts::family_for`]
    /// actually used, returning the `@font-face` CSS block(s)
    /// referencing them (empty if none were used, or none of the used
    /// fonts are embedded).
    ///
    /// Port of `Fonts.embed_fonts`.
    pub fn embed_fonts<R: Read + Seek>(&self, dest_dir: &Path, docx: &mut Docx<R>) -> String {
        let mut defs = Vec::new();
        let fonts_dir = dest_dir.join("fonts");
        for (name, variant) in &self.used {
            let Some(f) = self.fonts.get(name) else {
                continue;
            };
            if !f.embedded.contains_key(variant.as_str()) {
                continue;
            }
            if !fonts_dir.exists() {
                let _ = std::fs::create_dir(&fonts_dir);
            }
            let Some(fname) = self.write(name, &fonts_dir, docx, variant) else {
                continue;
            };
            let weight = if variant.contains("Bold") {
                "bold"
            } else {
                "normal"
            };
            let style = if variant.contains("Italic") {
                "italic"
            } else {
                "normal"
            };
            let d = [
                ("font-family", format!("\"{}\"", name.replace('"', ""))),
                ("src", format!("url(\"fonts/{fname}\")")),
                ("font-weight", weight.to_string()),
                ("font-style", style.to_string()),
            ];
            let body = d
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join(";\n\t");
            defs.push(format!("@font-face {{\n\t{body}\n}}\n"));
        }
        defs.join("\n")
    }

    /// Extracts one embedded font's raw bytes from the docx zip,
    /// reverses its obfuscation key (Word XORs the font's first 32
    /// bytes when `w:fontKey` is present -- an embedding-protection
    /// scheme, not encryption), and writes it to `dest_dir` under a
    /// sanitized filename. Returns `None` when the font can't be read
    /// out of the package at all.
    ///
    /// Python's own `is_truetype_font(prefix)` gate here is dead code:
    /// the real `is_truetype_font` (`calibre.utils.fonts.utils`)
    /// returns a 2-tuple, always truthy regardless of its own bool
    /// element, so `if not is_truetype_font(prefix): return None`
    /// never actually fires -- every prefix proceeds to have its
    /// extension guessed and gets written regardless of whether it's
    /// really a valid sfnt header. Reproduced as-is, by simply not
    /// gating on it, rather than porting a real bool check that would
    /// reject fonts Python itself always accepts.
    ///
    /// Port of `Fonts.write`.
    fn write<R: Read + Seek>(
        &self,
        name: &str,
        dest_dir: &Path,
        docx: &mut Docx<R>,
        variant: &str,
    ) -> Option<String> {
        let f = self.fonts.get(name)?;
        let ef = f.embedded.get(variant)?;
        let raw = docx.read(&ef.name).ok()?;
        let split = 32.min(raw.len());
        let mut prefix = raw[..split].to_vec();
        if let Some(key) = &ef.key {
            let hex: String = key.chars().filter(|c| c.is_ascii_hexdigit()).collect();
            if let Some(mut key_bytes) = parse_hex_bytes(&hex) {
                key_bytes.reverse();
                if !key_bytes.is_empty() {
                    for (i, b) in prefix.iter_mut().enumerate() {
                        *b ^= key_bytes[i % key_bytes.len()];
                    }
                }
            }
        }
        let ext = if prefix.starts_with(b"OTTO") {
            "otf"
        } else {
            "ttf"
        };
        let fname = ascii_filename(&format!("{name} - {variant}.{ext}"))
            .replace(' ', "_")
            .replace('&', "_");
        let mut full = prefix;
        full.extend_from_slice(&raw[split..]);
        std::fs::write(dest_dir.join(&fname), &full).ok()?;
        Some(fname)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_symbol_font_names_case_insensitively() {
        assert!(is_symbol_font("Wingdings"));
        assert!(is_symbol_font("wingdings"));
        assert!(is_symbol_font("SYMBOL"));
        assert!(is_symbol_font("Webdings"));
        assert!(is_symbol_font("Wingdings 2"));
        assert!(!is_symbol_font("Arial"));
        assert!(!is_symbol_font(""));
    }

    #[test]
    fn every_symbol_map_has_all_256_private_use_codepoints() {
        for (name, table) in SYMBOL_MAPS.iter() {
            assert_eq!(table.len(), 256, "{name} should have 256 entries");
        }
        assert_eq!(SYMBOL_MAPS.len(), 5);
    }

    /// Spot checks against the live Python `SYMBOL_MAPS`, extracted via
    /// `ast.literal_eval` rather than transcribed by hand -- see the
    /// project's "large data-table port technique".
    #[test]
    fn spot_checks_match_the_python_symbol_maps() {
        for (font, codepoint, expected) in [
            ("Symbol", 0xf041u32, "Α"),
            ("Symbol", 0xf061, "α"),
            ("Symbol", 0xf0e5, "∑"),
            ("Wingdings", 0xf0fc, "✓"),
            ("Webdings", 0xf057, "🕨"),
        ] {
            let text = char::from_u32(codepoint).unwrap().to_string();
            assert_eq!(
                map_symbol_text(&text, font),
                expected,
                "{font} codepoint {codepoint:#x}"
            );
        }
    }

    #[test]
    fn codepoints_outside_the_private_use_range_pass_through() {
        assert_eq!(
            map_symbol_text("hello, world!", "Wingdings"),
            "hello, world!"
        );
        assert_eq!(
            map_symbol_text("\u{f0ff}", "Wingdings"),
            " ",
            "last entry in range"
        );
    }

    #[test]
    fn an_unknown_font_leaves_text_untouched() {
        assert_eq!(map_symbol_text("\u{f041}", "NotARealFont"), "\u{f041}");
    }

    #[test]
    fn is_symbol_font_matches_the_map_symbol_text_lookup() {
        // Every key `map_symbol_text` can resolve should also report
        // true from `is_symbol_font`, and vice versa.
        for name in SYMBOL_MAPS.keys() {
            assert!(is_symbol_font(name));
        }
    }
}

#[cfg(test)]
mod fonts_tests {
    use super::*;
    use roxmltree::Document;
    use std::collections::HashMap;

    const FONTS_OPEN: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

    fn parse_fonts(body: &str) -> (Document<'static>, DocxNamespace) {
        let xml: &'static str =
            Box::leak(format!("<w:fonts {FONTS_OPEN}>{body}</w:fonts>").into_boxed_str());
        (
            Document::parse(xml).expect("valid XML"),
            DocxNamespace::default(),
        )
    }

    #[test]
    fn panose_to_css_generic_family_covers_every_branch() {
        assert_eq!(panose_to_css_generic_family(&[0, 0, 0, 9]), "monospace");
        assert_eq!(panose_to_css_generic_family(&[3, 0, 0, 0]), "cursive");
        assert_eq!(panose_to_css_generic_family(&[4, 0, 0, 0]), "fantasy");
        assert_eq!(panose_to_css_generic_family(&[0, 11, 0, 0]), "sans-serif");
        assert_eq!(panose_to_css_generic_family(&[0, 12, 0, 0]), "sans-serif");
        assert_eq!(panose_to_css_generic_family(&[0, 0, 0, 0]), "serif");
    }

    #[test]
    fn has_system_fonts_always_reports_false_no_scanner_available() {
        assert!(!has_system_fonts("Arial"));
        assert!(!has_system_fonts("Some Font Nobody Has"));
    }

    #[test]
    fn get_variant_maps_bold_italic_combinations() {
        assert_eq!(get_variant(false, false), "Regular");
        assert_eq!(get_variant(false, true), "Italic");
        assert_eq!(get_variant(true, false), "Bold");
        assert_eq!(get_variant(true, true), "BoldItalic");
    }

    #[test]
    fn family_reads_name_generic_family_and_panose() {
        let (doc, ns) = parse_fonts(
            r#"<w:font w:name="MyFont">
                <w:family w:val="swiss"/>
                <w:panose1 w:val="02000000000000000000"/>
               </w:font>"#,
        );
        let elem = ns.descendants(doc.root_element(), &["w:font"])[0];
        let f = Family::from_elem(elem, &HashMap::new(), &ns);
        assert_eq!(f.name, "MyFont");
        assert_eq!(f.family_name, "MyFont");
        assert_eq!(f.generic_family, "swiss");
        assert_eq!(f.css_generic_family, "sans-serif");
        assert!(f.is_ttf);
        assert!(f.embedded.is_empty());
    }

    #[test]
    fn family_with_no_generic_family_falls_back_to_panose_then_serif() {
        let (doc, ns) = parse_fonts(r#"<w:font w:name="Plain"/>"#);
        let elem = ns.descendants(doc.root_element(), &["w:font"])[0];
        let f = Family::from_elem(elem, &HashMap::new(), &ns);
        assert_eq!(f.generic_family, "auto");
        assert_eq!(f.css_generic_family, "serif");
    }

    #[test]
    fn family_alt_name_is_never_preferred_with_no_system_scanner() {
        // Python would switch to an altName only if the primary name
        // has no system-installed match but an altName does --
        // has_system_fonts always returning false means that branch
        // never fires here, so family_name always stays the primary
        // declared name.
        let (doc, ns) = parse_fonts(
            r#"<w:font w:name="Some Obscure Font">
                <w:altName w:val="Arial"/>
               </w:font>"#,
        );
        let elem = ns.descendants(doc.root_element(), &["w:font"])[0];
        let f = Family::from_elem(elem, &HashMap::new(), &ns);
        assert_eq!(f.alt_names, vec!["Arial".to_string()]);
        assert_eq!(f.family_name, "Some Obscure Font");
    }

    #[test]
    fn family_not_true_type_flag_is_reproduced() {
        let (doc, ns) =
            parse_fonts(r#"<w:font w:name="Bitmap Font"><w:notTrueType w:val="true"/></w:font>"#);
        let elem = ns.descendants(doc.root_element(), &["w:font"])[0];
        let f = Family::from_elem(elem, &HashMap::new(), &ns);
        assert!(!f.is_ttf);
    }

    #[test]
    fn family_records_embedded_variants_resolved_against_relationships() {
        let (doc, ns) = parse_fonts(
            r#"<w:font w:name="Embedded Font">
                <w:embedRegular r:id="rId1" w:fontKey="{01234567-89AB-CDEF-0123-456789ABCDEF}"/>
                <w:embedBold r:id="rId2" w:subsetted="1"/>
               </w:font>"#,
        );
        let elem = ns.descendants(doc.root_element(), &["w:font"])[0];
        let mut rels = HashMap::new();
        rels.insert("rId1".to_string(), "fonts/font1.fntdata".to_string());
        rels.insert("rId2".to_string(), "fonts/font2.fntdata".to_string());
        let f = Family::from_elem(elem, &rels, &ns);
        assert_eq!(f.embedded.len(), 2);
        assert_eq!(f.embedded["Regular"].name, "fonts/font1.fntdata");
        assert!(!f.embedded["Regular"].subsetted);
        assert!(f.embedded["Bold"].subsetted);
        assert_eq!(f.embedded["Bold"].key, None);
    }

    #[test]
    fn family_embed_with_unresolvable_relationship_id_is_dropped() {
        let (doc, ns) =
            parse_fonts(r#"<w:font w:name="Ghost"><w:embedRegular r:id="rIdMissing"/></w:font>"#);
        let elem = ns.descendants(doc.root_element(), &["w:font"])[0];
        let f = Family::from_elem(elem, &HashMap::new(), &ns);
        assert!(f.embedded.is_empty());
    }

    #[test]
    fn fonts_call_reads_every_font_element_keyed_by_name() {
        let (doc, ns) = parse_fonts(
            r#"<w:font w:name="Font A"/><w:font w:name="Font B"><w:family w:val="modern"/></w:font>"#,
        );
        let mut fonts = Fonts::new();
        fonts.call(doc.root_element(), &HashMap::new(), &ns);
        assert!(fonts.get("Font A").is_some());
        assert_eq!(fonts.get("Font B").unwrap().css_generic_family, "monospace");
        assert!(fonts.get("Font C").is_none());
    }

    #[test]
    fn family_for_an_unknown_font_falls_back_to_serif() {
        let mut fonts = Fonts::new();
        assert_eq!(fonts.family_for("Nope", false, false), "serif");
    }

    #[test]
    fn family_for_a_known_non_embedded_font_quotes_the_name_with_its_generic_family() {
        let (doc, ns) =
            parse_fonts(r#"<w:font w:name="Georgia"><w:family w:val="roman"/></w:font>"#);
        let mut fonts = Fonts::new();
        fonts.call(doc.root_element(), &HashMap::new(), &ns);
        assert_eq!(
            fonts.family_for("Georgia", false, false),
            "\"Georgia\", serif"
        );
    }

    #[test]
    fn family_for_a_symbol_font_returns_the_bare_name_unquoted() {
        let (doc, ns) = parse_fonts(r#"<w:font w:name="Wingdings"/>"#);
        let mut fonts = Fonts::new();
        fonts.call(doc.root_element(), &HashMap::new(), &ns);
        assert_eq!(fonts.family_for("Wingdings", false, false), "Wingdings");
    }

    #[test]
    fn family_for_marks_the_resolved_name_and_variant_used() {
        let (doc, ns) = parse_fonts(r#"<w:font w:name="Georgia"/>"#);
        let mut fonts = Fonts::new();
        fonts.call(doc.root_element(), &HashMap::new(), &ns);
        fonts.family_for("Georgia", true, false);
        assert!(fonts
            .used
            .contains(&("Georgia".to_string(), "Bold".to_string())));
    }

    fn docx_with_embedded_font() -> (
        super::super::container::Docx<std::io::Cursor<Vec<u8>>>,
        Vec<u8>,
    ) {
        use std::io::Write;
        // A minimal but real sfnt header (`OTTO`, an OpenType/CFF
        // signature) so `write`'s extension guess exercises the
        // ".otf" branch, not just the ".ttf" default.
        let mut font_bytes = b"OTTO".to_vec();
        font_bytes.extend_from_slice(&[0u8; 60]);
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let options = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("[Content_Types].xml", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#,
            )
            .unwrap();
            zip.start_file("_rels/.rels", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#,
            )
            .unwrap();
            zip.start_file("word/fonts/font1.fntdata", options).unwrap();
            zip.write_all(&font_bytes).unwrap();
            zip.finish().unwrap();
        }
        (
            super::super::container::Docx::new(std::io::Cursor::new(buf))
                .expect("minimal docx package opens"),
            font_bytes,
        )
    }

    #[test]
    fn embed_fonts_writes_the_used_embedded_font_and_returns_a_font_face_block() {
        let (doc, ns) =
            parse_fonts(r#"<w:font w:name="Embedded Font"><w:embedRegular r:id="rId1"/></w:font>"#);
        let mut rels = HashMap::new();
        rels.insert("rId1".to_string(), "word/fonts/font1.fntdata".to_string());
        let mut fonts = Fonts::new();
        fonts.call(doc.root_element(), &rels, &ns);
        fonts.family_for("Embedded Font", false, false);

        let (mut docx, font_bytes) = docx_with_embedded_font();
        let tmp =
            std::env::temp_dir().join(format!("calibre_oxide_fonts_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let css = fonts.embed_fonts(&tmp, &mut docx);

        assert!(css.contains("@font-face"));
        assert!(css.contains(r#"font-family: "Embedded Font""#));
        assert!(css.contains("font-weight: normal"));
        assert!(css.contains("font-style: normal"));
        assert!(css.contains(r#"src: url("fonts/Embedded_Font_-_Regular.otf")"#));

        let written = std::fs::read(tmp.join("fonts").join("Embedded_Font_-_Regular.otf")).unwrap();
        assert_eq!(written, font_bytes);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn embed_fonts_is_empty_when_the_used_variant_is_not_embedded() {
        let (doc, ns) = parse_fonts(r#"<w:font w:name="Not Embedded"/>"#);
        let mut fonts = Fonts::new();
        fonts.call(doc.root_element(), &HashMap::new(), &ns);
        fonts.family_for("Not Embedded", false, false);

        let (mut docx, _) = docx_with_embedded_font();
        let tmp = std::env::temp_dir().join(format!(
            "calibre_oxide_fonts_test_empty_{}",
            std::process::id()
        ));
        let css = fonts.embed_fonts(&tmp, &mut docx);
        assert_eq!(css, "");
    }

    #[test]
    fn embed_fonts_bold_italic_variant_sets_weight_and_style() {
        let (doc, ns) = parse_fonts(
            r#"<w:font w:name="Embedded Font"><w:embedBoldItalic r:id="rId1"/></w:font>"#,
        );
        let mut rels = HashMap::new();
        rels.insert("rId1".to_string(), "word/fonts/font1.fntdata".to_string());
        let mut fonts = Fonts::new();
        fonts.call(doc.root_element(), &rels, &ns);
        fonts.family_for("Embedded Font", true, true);

        let (mut docx, _) = docx_with_embedded_font();
        let tmp = std::env::temp_dir().join(format!(
            "calibre_oxide_fonts_test_bi_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let css = fonts.embed_fonts(&tmp, &mut docx);
        assert!(css.contains("font-weight: bold"));
        assert!(css.contains("font-style: italic"));
        std::fs::remove_dir_all(&tmp).ok();
    }
}
