//! Symbol-font glyph remapping: Word writes some dingbat/symbol fonts
//! (Wingdings, Symbol, ...) using the font's own private-use-area
//! codepoints rather than the actual Unicode character, since the
//! *rendering* is meant to come from the font's own glyph table, not
//! Unicode semantics. Browsers don't have that font's private-use
//! mapping, so calibre remaps codepoints U+F000..U+F0FF back to the
//! real Unicode character each such font actually draws.
//!
//! Partial port of `old_src/src/calibre/ebooks/docx/fonts.py` --
//! [`is_symbol_font`]/[`map_symbol_text`]/[`SYMBOL_MAPS`] only, which are
//! pure data lookups with no system dependency. The `Fonts` class itself
//! (embedded-font extraction, `family_for`'s system-installed-font
//! matching) needs `calibre.utils.fonts.scanner`, which has no Rust
//! counterpart -- see issue #130.

use std::collections::HashMap;
use std::sync::LazyLock;

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
