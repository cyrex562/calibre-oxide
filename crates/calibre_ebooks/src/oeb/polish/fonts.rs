//! Port of `old_src/src/calibre/ebooks/oeb/polish/fonts.py`.
//!
//! Every function in this file except [`unquote`] needs real CSS
//! parsing: reading/mutating `font-family` declarations inside parsed
//! `css_parser` stylesheets, `<style>` tags, and `style=""` attributes,
//! plus `tinycss.fonts3`'s `font`/`font-family` shorthand
//! parsing/serialization. This crate has no CSS parser (issues
//! #34/#35/#36, `oeb::polish::utils::parse_css`'s docs) -- see this
//! module's functions for the specific `todo!()` per blocked capability.
//! [`unquote`] needs none of that (it's a plain string-quote strip) and
//! is ported for real.

use anyhow::Result;

use super::container::Container;

/// Port of `unquote`: strips one layer of matching leading/trailing
/// quote characters, if present.
pub fn unquote(x: &str) -> &str {
    let bytes = x.as_bytes();
    if x.len() > 1 && bytes[0] == bytes[x.len() - 1] && matches!(bytes[0], b'"' | b'\'') {
        &x[1..x.len() - 1]
    } else {
        x
    }
}

/// Port of `font_family_data_from_declaration`. Needs a real CSS
/// declaration parser (`style.getProperty(...)`) plus
/// `tinycss.fonts3.parse_font`/`parse_font_family`.
pub fn font_family_data_from_declaration(
    _style_css_text: &str,
    _families: &mut std::collections::HashMap<String, bool>,
) {
    todo!(
        "placeholder: needs a real CSS declaration parser plus \
         tinycss.fonts3's font/font-family shorthand parsing -- see \
         oeb::polish::utils::parse_css's docs (same gap as issues #34/#35/#36)"
    )
}

/// Port of `font_family_data_from_sheet`. Needs `CSSStyleSheet.cssRules`
/// from a real CSS parse.
pub fn font_family_data_from_sheet(
    _sheet_css_text: &str,
    _families: &mut std::collections::HashMap<String, bool>,
) {
    todo!("placeholder: needs a real CSS parser -- see font_family_data_from_declaration's docs")
}

/// Port of `font_family_data`: every font-family name referenced
/// anywhere in the book (in stylesheets, `<style>` tags, and `style=""`
/// attributes), and whether it's declared via `@font-face` (embedded).
pub fn font_family_data(
    _container: &mut Container,
) -> Result<std::collections::HashMap<String, bool>> {
    todo!(
        "placeholder: needs a real CSS parser to walk stylesheets/<style> \
         tags/style attributes -- see font_family_data_from_declaration's docs"
    )
}

/// Port of `change_font_in_declaration`. Needs a real CSS declaration
/// parser/serializer.
pub fn change_font_in_declaration(
    _style_css_text: &str,
    _old_name: &str,
    _new_name: Option<&str>,
) -> Result<(String, bool)> {
    todo!(
        "placeholder: needs a real CSS declaration parser/serializer plus \
         tinycss.fonts3's font/font-family (de)serialization -- see \
         oeb::polish::utils::parse_css's docs"
    )
}

/// Port of `remove_embedded_font`. Needs a real `CSSRule`/`src`
/// property from a parsed `@font-face` rule.
pub fn remove_embedded_font(_container: &mut Container, _sheet_name: &str) -> Result<()> {
    todo!("placeholder: needs a real CSS parser -- see change_font_in_declaration's docs")
}

/// Port of `change_font_in_sheet`. Needs `CSSStyleSheet.cssRules`.
pub fn change_font_in_sheet(
    _container: &mut Container,
    _sheet_name: &str,
    _old_name: &str,
    _new_name: Option<&str>,
) -> Result<bool> {
    todo!("placeholder: needs a real CSS parser -- see change_font_in_declaration's docs")
}

/// Port of `change_font`: renames (or removes) a font-family everywhere
/// it's referenced in the book, removing the embedded font file if the
/// old name pointed at one.
pub fn change_font(
    _container: &mut Container,
    _old_name: &str,
    _new_name: Option<&str>,
) -> Result<bool> {
    todo!(
        "placeholder: needs a real CSS parser to find and rewrite every \
         font-family reference (stylesheets, <style> tags, style \
         attributes) -- see change_font_in_declaration's docs"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquote_strips_matching_quotes() {
        assert_eq!(unquote("\"My Font\""), "My Font");
        assert_eq!(unquote("'My Font'"), "My Font");
        assert_eq!(unquote("My Font"), "My Font");
        assert_eq!(unquote("\"unterminated"), "\"unterminated");
        assert_eq!(unquote(""), "");
        assert_eq!(unquote("\""), "\"");
    }
}
