//! Port of `oeb/polish/kepubify.py`'s CSS processing (issue #652, one
//! slice of the #543 epic -- see [`super`]'s module docs; the intricate
//! sentence-span-wrapping core is #651, HTML markup/cover handling is
//! #653, the `kepubify_container`/`unkepubify_container` orchestrator is
//! #654).
//!
//! Kobo's own EPUB renderer chokes on `@page` rules and on the
//! `widows`/`orphans` properties, so kepubifying a book hides both; a
//! later un-kepubify pass needs to restore them byte-for-byte. Real
//! upstream does this with `css_parser`'s object model, which supports
//! full-fidelity CSS **comment** nodes as first-class, reinsertable
//! members of a stylesheet's rule list: a removed `@page` rule gets
//! reserialized as `/* {CSS_COMMENT_COOKIE}: <original text> */` and
//! spliced back in at the same index, safely inert to any real CSS
//! engine (which ignores comments) and cheaply reversible later.
//!
//! [`crate::css::model`] (issue #164/#590) has no such comment-as-rule
//! concept -- `cssparser`, the tokenizer it's built on, discards raw
//! `/* */` comments during tokenization rather than surfacing them as
//! AST nodes, and [`crate::css::model::Rule`] has no `Comment` variant.
//! This is a real, disclosed narrowing: instead of a comment, a hidden
//! `@page` rule is reserialized (whitespace-flattened, so it survives as
//! a single CSS string token) into a marker unknown-at-rule's string
//! prelude -- `@calibre-removed-css-for-kobo-page "<escaped text>";` --
//! which [`crate::css::parser`] already parses generically via
//! [`Rule::Unknown`] (confirmed: any unrecognized `@word ...;` round-
//! trips through `UnknownAtRule` losslessly, the same mechanism
//! `@supports`/`@keyframes` rely on elsewhere in this crate). Any real
//! CSS engine ignores an at-rule it doesn't recognize exactly as it
//! would ignore a comment, so the observable behavior (Kobo's renderer
//! never sees the hidden `@page` rule) is identical; only the concrete
//! on-disk encoding differs from real calibre's own kepub output.

use crate::css::model::{Rule, Stylesheet, UnknownAtRule};

/// Port of `kepubify.py`'s `CSS_COMMENT_COOKIE`.
pub const CSS_COMMENT_COOKIE: &str = "calibre-removed-css-for-kobo";

fn hidden_page_at_keyword() -> String {
    format!("{CSS_COMMENT_COOKIE}-page")
}

fn hidden_property_name(prop: &str) -> String {
    format!("-{CSS_COMMENT_COOKIE}-{prop}")
}

/// Port of `nest_css_comments()`: escapes `*/` inside text that will be
/// embedded in a real CSS comment elsewhere in this crate, so it can't
/// be closed early. Kept even though this module's own hidden-rule
/// encoding uses a quoted string, not a comment (see the module docs),
/// since #653 embeds user-supplied CSS text inside a real `<style>`
/// comment for a different reason and can reuse this helper verbatim.
pub fn nest_css_comments(text: &str) -> String {
    text.replace("*/", "*\u{200c}/")
}

/// Port of `kepubify.py`'s `Options` NamedTuple. Only
/// [`process_stylesheet`] is implemented in this module (#652); the
/// other fields exist here so #653/#654 can share one `Options` type
/// rather than each inventing their own subset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    pub extra_css: String,
    pub hyphenation_css: String,
    pub remove_widows_and_orphans: bool,
    pub remove_at_page_rules: bool,
    /// Affects HTML markup generation (#653), not [`process_stylesheet`]
    /// -- real upstream's own `process_stylesheet` never reads this
    /// field either, confirmed directly against the Python source.
    pub prefer_justification: bool,
    pub for_removal: bool,
}

impl Options {
    /// Port of the `needs_stylesheet_processing` property.
    pub fn needs_stylesheet_processing(&self) -> bool {
        self.remove_at_page_rules || self.remove_widows_and_orphans || self.for_removal
    }
}

/// Port of `check_if_css_needs_modification()`. Real upstream's own type
/// hint says `-> tuple[bool, bool]`, but the function body actually
/// returns a 3-tuple `(sheet, remove_widows_and_orphans,
/// remove_at_page_rules)` -- a genuine stale/wrong annotation in the
/// Python source (confirmed by reading the body, not just the
/// signature). `make_options` (the only real caller) immediately
/// discards the parsed `sheet`, so this port returns just the two
/// booleans callers actually use rather than reproducing the misleading
/// annotation's shape or the unused first element.
pub fn check_if_css_needs_modification(extra_css: &str) -> (bool, bool) {
    let mut remove_widows_and_orphans = false;
    let mut remove_at_page_rules = false;
    if !extra_css.is_empty() {
        let sheet = Stylesheet::parse(extra_css);
        for rule in &sheet.rules {
            match rule {
                Rule::Page(_) => remove_at_page_rules = true,
                Rule::Style(s) => {
                    if !s.style.get_property_value("widows").is_empty()
                        || !s.style.get_property_value("orphans").is_empty()
                    {
                        remove_widows_and_orphans = true;
                    }
                }
                _ => {}
            }
            if remove_widows_and_orphans && remove_at_page_rules {
                break;
            }
        }
    }
    (remove_widows_and_orphans, remove_at_page_rules)
}

/// Arguments to [`make_options`], port of `make_options()`'s keyword
/// parameters. `remove_widows_and_orphans`/`remove_at_page_rules` are
/// `Option<bool>` to mirror Python's `bool | None = None` defaults.
#[derive(Debug, Clone)]
pub struct MakeOptionsArgs {
    pub extra_css: String,
    pub affect_hyphenation: bool,
    pub disable_hyphenation: bool,
    pub hyphenation_min_chars: u32,
    pub hyphenation_min_chars_before: u32,
    pub hyphenation_min_chars_after: u32,
    pub hyphenation_limit_lines: u32,
    pub prefer_justification: bool,
    pub remove_widows_and_orphans: Option<bool>,
    pub remove_at_page_rules: Option<bool>,
}

impl Default for MakeOptionsArgs {
    fn default() -> Self {
        Self {
            extra_css: String::new(),
            affect_hyphenation: false,
            disable_hyphenation: false,
            hyphenation_min_chars: 6,
            hyphenation_min_chars_before: 3,
            hyphenation_min_chars_after: 3,
            hyphenation_limit_lines: 2,
            prefer_justification: false,
            remove_widows_and_orphans: None,
            remove_at_page_rules: None,
        }
    }
}

/// Port of `make_options()`.
pub fn make_options(args: MakeOptionsArgs) -> Options {
    // Real upstream quirk, preserved exactly: if EITHER flag is `None`,
    // BOTH get overwritten by `check_if_css_needs_modification`'s
    // result -- even one the caller explicitly passed in gets
    // discarded, not just the missing one.
    let (remove_widows_and_orphans, remove_at_page_rules) =
        match (args.remove_widows_and_orphans, args.remove_at_page_rules) {
            (Some(w), Some(p)) => (w, p),
            _ => check_if_css_needs_modification(&args.extra_css),
        };

    let hyphenation_css = if args.affect_hyphenation {
        if args.disable_hyphenation {
            "\n* {\n  -webkit-hyphens: none !important;\n  hyphens: none !important;\n}\n".to_string()
        } else if args.hyphenation_min_chars > 0 {
            format!(
                "\n* {{\n    \
                -webkit-hyphens: auto;\n    \
                -webkit-hyphenate-limit-after: {after};\n    \
                -webkit-hyphenate-limit-before: {before};\n    \
                -webkit-hyphenate-limit-chars: {chars} {before} {after};\n    \
                -webkit-hyphenate-limit-lines: {lines};\n\n    \
                hyphens: auto;\n    \
                hyphenate-limit-chars: {chars} {before} {after};\n    \
                hyphenate-limit-lines: {lines};\n    \
                hyphenate-limit-last: page;\n}}\n\n\
                h1, h2, h3, h4, h5, h6, td {{\n    \
                -webkit-hyphens: none !important;\n    \
                hyphens: none !important;\n}}\n",
                chars = args.hyphenation_min_chars,
                before = args.hyphenation_min_chars_before,
                after = args.hyphenation_min_chars_after,
                lines = args.hyphenation_limit_lines,
            )
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    Options {
        extra_css: args.extra_css,
        hyphenation_css,
        remove_widows_and_orphans,
        remove_at_page_rules,
        prefer_justification: args.prefer_justification,
        for_removal: false,
    }
}

fn escape_hidden_payload(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn unescape_hidden_payload(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Hides a `@page` rule behind the marker unknown-at-rule described in
/// this module's docs. `rule` must be [`Rule::Page`].
fn hide_page_rule(rule: &Rule) -> Rule {
    let text = rule.to_css_text().replace('\n', " ");
    let escaped = escape_hidden_payload(&text);
    Rule::Unknown(UnknownAtRule {
        at_keyword: hidden_page_at_keyword(),
        prelude: format!("\"{escaped}\""),
        block: None,
    })
}

/// Reverses [`hide_page_rule`]: given the marker unknown-at-rule, parses
/// its hidden payload back into a real `@page` rule. Returns `None` if
/// `u`'s prelude isn't a well-formed hidden payload (defensive; every
/// real caller of this module only ever restores rules it hid itself).
fn restore_page_rule(u: &UnknownAtRule) -> Option<Rule> {
    let raw = u.prelude.trim();
    let inner = raw.strip_prefix('"')?.strip_suffix('"')?;
    let text = unescape_hidden_payload(inner);
    Stylesheet::parse(&text)
        .rules
        .into_iter()
        .find(|r| matches!(r, Rule::Page(_)))
}

/// Port of `process_stylesheet()`: the real, reversible CSS transform
/// kepubify/unkepubify apply to every stylesheet in a book. See the
/// module docs for the one disclosed encoding difference (marker
/// unknown-at-rule instead of a CSS comment).
pub fn process_stylesheet(css: &str, opts: &Options) -> String {
    let has_comment_cookie = css.contains(CSS_COMMENT_COOKIE);
    if opts.for_removal && !has_comment_cookie {
        return css.to_string();
    }

    let mut sheet = Stylesheet::parse(css);
    let mut changed = false;
    let hidden_kw = hidden_page_at_keyword();

    if has_comment_cookie {
        for rule in sheet.rules.iter_mut() {
            if let Rule::Style(s) = rule {
                for q in ["widows", "orphans"] {
                    let hidden_name = hidden_property_name(q);
                    if let Some(d) = s
                        .style
                        .properties
                        .iter_mut()
                        .find(|d| d.name.eq_ignore_ascii_case(&hidden_name))
                    {
                        d.name = q.to_string();
                        changed = true;
                    }
                }
            }
        }
        for rule in sheet.rules.iter_mut() {
            let is_hidden_page = matches!(rule, Rule::Unknown(u) if u.at_keyword.eq_ignore_ascii_case(&hidden_kw));
            if is_hidden_page {
                if let Rule::Unknown(u) = rule {
                    if let Some(restored) = restore_page_rule(u) {
                        *rule = restored;
                        changed = true;
                    }
                }
            }
        }
    }

    if opts.for_removal {
        return if changed { sheet.to_css_text() } else { css.to_string() };
    }

    if opts.remove_widows_and_orphans {
        for rule in sheet.rules.iter_mut() {
            if let Rule::Style(s) = rule {
                for q in ["widows", "orphans"] {
                    if let Some(d) = s
                        .style
                        .properties
                        .iter_mut()
                        .find(|d| d.name.eq_ignore_ascii_case(q))
                    {
                        changed = true;
                        d.name = hidden_property_name(q);
                    }
                }
            }
        }
    }

    if opts.remove_at_page_rules {
        for rule in sheet.rules.iter_mut() {
            if matches!(rule, Rule::Page(_)) {
                changed = true;
                *rule = hide_page_rule(rule);
            }
        }
    }

    if changed {
        sheet.to_css_text()
    } else {
        css.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(remove_widows_and_orphans: bool, remove_at_page_rules: bool, for_removal: bool) -> Options {
        Options {
            remove_widows_and_orphans,
            remove_at_page_rules,
            for_removal,
            ..Default::default()
        }
    }

    #[test]
    fn nest_css_comments_escapes_close() {
        assert_eq!(nest_css_comments("a*/b"), "a*\u{200c}/b");
        assert_eq!(nest_css_comments("plain"), "plain");
    }

    #[test]
    fn leaves_css_untouched_when_no_flags_set() {
        let css = "p { color: red; }";
        assert_eq!(process_stylesheet(css, &opts(false, false, false)), css);
    }

    #[test]
    fn removal_short_circuits_without_cookie() {
        let css = "p { color: red; }";
        // for_removal with no cookie present must return the input
        // byte-for-byte (real upstream's own "avoid expensive parse"
        // fast path) even though `Stylesheet::parse` would reformat it.
        assert_eq!(process_stylesheet(css, &opts(false, false, true)), css);
    }

    fn has_page_rule(css: &str) -> bool {
        Stylesheet::parse(css).rules.iter().any(|r| matches!(r, Rule::Page(_)))
    }

    #[test]
    fn hides_and_restores_widows_and_orphans() {
        let css = "p { widows: 2; orphans: 3; color: red; }";
        let hidden = process_stylesheet(css, &opts(true, false, false));
        let hidden_sheet = Stylesheet::parse(&hidden);
        let style = hidden_sheet.style_rules().next().expect("style rule");
        assert!(style.style.get_property("widows").is_none());
        assert!(style.style.get_property("orphans").is_none());
        assert_eq!(
            style
                .style
                .get_property_value(&format!("-{CSS_COMMENT_COOKIE}-widows")),
            "2"
        );
        assert_eq!(
            style
                .style
                .get_property_value(&format!("-{CSS_COMMENT_COOKIE}-orphans")),
            "3"
        );
        assert!(hidden.contains(CSS_COMMENT_COOKIE));

        let restored = process_stylesheet(&hidden, &opts(false, false, true));
        let restored_sheet = Stylesheet::parse(&restored);
        let style = restored_sheet.style_rules().next().expect("style rule");
        assert_eq!(style.style.get_property_value("widows"), "2");
        assert_eq!(style.style.get_property_value("orphans"), "3");
        assert!(!restored.contains(CSS_COMMENT_COOKIE));
    }

    #[test]
    fn hides_and_restores_page_rules() {
        let css = "@page { margin: 1in; }\np { color: blue; }";
        let hidden = process_stylesheet(css, &opts(false, true, false));
        assert!(!has_page_rule(&hidden));
        assert!(hidden.contains(CSS_COMMENT_COOKIE));
        assert!(hidden.contains("color: blue"));

        let restored = process_stylesheet(&hidden, &opts(false, false, true));
        assert!(has_page_rule(&restored));
        assert!(restored.contains("margin: 1in"));
        assert!(!restored.contains(CSS_COMMENT_COOKIE));
    }

    #[test]
    fn hides_page_rule_with_selector_and_margin_box() {
        let css = "@page :first { margin-top: 2in; @top-left { content: \"hi\"; } }";
        let hidden = process_stylesheet(css, &opts(false, true, false));
        assert!(!has_page_rule(&hidden));

        let restored = process_stylesheet(&hidden, &opts(false, false, true));
        let sheet = Stylesheet::parse(&restored);
        let page = sheet
            .rules
            .iter()
            .find_map(Rule::as_page)
            .expect("restored @page rule");
        assert_eq!(page.selector.pseudo_class.as_deref(), Some("first"));
        assert_eq!(page.declarations.get_property_value("margin-top"), "2in");
        assert_eq!(page.margin_rules.len(), 1);
        assert_eq!(page.margin_rules[0].at_keyword, "@top-left");
    }

    #[test]
    fn removal_leaves_unrelated_css_alone_when_nothing_hidden() {
        // Cookie substring present (e.g. inside an unrelated string
        // value) but nothing this module itself hid -- must not crash
        // and must not fabricate a bogus restoration.
        let css = format!("p::before {{ content: \"{CSS_COMMENT_COOKIE}\"; }}");
        let restored = process_stylesheet(&css, &opts(false, false, true));
        assert!(restored.contains(CSS_COMMENT_COOKIE));
    }

    #[test]
    fn check_if_css_needs_modification_detects_both_flags() {
        assert_eq!(check_if_css_needs_modification(""), (false, false));
        assert_eq!(
            check_if_css_needs_modification("p { widows: 2; }"),
            (true, false)
        );
        assert_eq!(
            check_if_css_needs_modification("@page { margin: 1in; }"),
            (false, true)
        );
        assert_eq!(
            check_if_css_needs_modification("@page { margin: 1in; } p { orphans: 3; }"),
            (true, true)
        );
    }

    #[test]
    fn make_options_defaults_run_needs_modification_check() {
        let o = make_options(MakeOptionsArgs {
            extra_css: "@page { margin: 1in; }".to_string(),
            ..Default::default()
        });
        assert!(o.remove_at_page_rules);
        assert!(!o.remove_widows_and_orphans);
        assert!(o.hyphenation_css.is_empty());
        assert!(!o.for_removal);
    }

    #[test]
    fn make_options_explicit_flags_both_required() {
        // Real upstream quirk: providing only ONE of the two flags
        // still triggers a full recompute of BOTH from extra_css.
        let o = make_options(MakeOptionsArgs {
            extra_css: "@page { margin: 1in; }".to_string(),
            remove_widows_and_orphans: Some(true),
            remove_at_page_rules: None,
            ..Default::default()
        });
        assert!(o.remove_at_page_rules);
        assert!(!o.remove_widows_and_orphans);

        let o2 = make_options(MakeOptionsArgs {
            extra_css: "@page { margin: 1in; }".to_string(),
            remove_widows_and_orphans: Some(true),
            remove_at_page_rules: Some(false),
            ..Default::default()
        });
        assert!(o2.remove_widows_and_orphans);
        assert!(!o2.remove_at_page_rules);
    }

    #[test]
    fn make_options_hyphenation_disabled() {
        let o = make_options(MakeOptionsArgs {
            affect_hyphenation: true,
            disable_hyphenation: true,
            ..Default::default()
        });
        assert!(o.hyphenation_css.contains("hyphens: none !important"));
    }

    #[test]
    fn make_options_hyphenation_enabled_with_limits() {
        let o = make_options(MakeOptionsArgs {
            affect_hyphenation: true,
            hyphenation_min_chars: 7,
            hyphenation_min_chars_before: 2,
            hyphenation_min_chars_after: 4,
            hyphenation_limit_lines: 3,
            ..Default::default()
        });
        assert!(o.hyphenation_css.contains("hyphenate-limit-chars: 7 2 4"));
        assert!(o.hyphenation_css.contains("hyphenate-limit-lines: 3"));
        assert!(o.hyphenation_css.contains("-webkit-hyphens: auto"));
    }

    #[test]
    fn make_options_hyphenation_zero_min_chars_yields_empty_css() {
        let o = make_options(MakeOptionsArgs {
            affect_hyphenation: true,
            hyphenation_min_chars: 0,
            ..Default::default()
        });
        assert!(o.hyphenation_css.is_empty());
    }

    #[test]
    fn options_needs_stylesheet_processing() {
        assert!(!Options::default().needs_stylesheet_processing());
        assert!(opts(true, false, false).needs_stylesheet_processing());
        assert!(opts(false, true, false).needs_stylesheet_processing());
        assert!(opts(false, false, true).needs_stylesheet_processing());
    }
}
