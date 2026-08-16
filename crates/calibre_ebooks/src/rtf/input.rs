//! Port of `old_src/src/calibre/ebooks/rtf/input.py`'s `InlineClass`.
//!
//! `InlineClass` is an `lxml.etree.XSLTExtension`: a callback hook
//! invoked *during* an XSLT transform, where the transform itself
//! belongs to `calibre.ebooks.rtf2xml.*` (the RTF-to-XHTML pipeline
//! that reads the token stream `crate::rtf::preprocess` now produces
//! for real and drives it through a chain of XSLT stylesheets). That
//! pipeline is a large, separate subsystem, genuinely out of scope for
//! this issue (`#50`, `rtf/{input,preprocess,rtfml}.py` only) -- this
//! crate has no XSLT engine and is not getting one here.
//!
//! What `execute()` actually *computes*, though, is trivial, pure, and
//! has nothing to do with XSLT: given a handful of boolean formatting
//! flags plus optional font-size/color strings, it produces a
//! space-separated class-name string, assigning each distinct
//! font-size/color a stable, increasing index the first time it is
//! seen. [`InlineClassAssigner`] ports that computation directly, as a
//! plain struct with no XSLT dependency at all.
//!
//! This is real, working logic with no current caller in this crate
//! (nothing here invokes an XSLT transform) -- the same situation
//! several earlier issues this session shipped real logic ahead of its
//! eventual caller (e.g. `oeb::polish::cascade`/`oeb::polish::split`
//! before `oeb::transforms::*` existed to call them). It is not a
//! `todo!()`: the moment this crate's own `rtf2xml`-equivalent exists
//! and needs "assign a stable per-document class name to this
//! formatting-flags combination," this is that function.

/// The boolean formatting flags `InlineClass.FMTS` iterates plus the
/// separately-handled `underlined` flag. Port of what `execute` reads
/// off `input_node.get(x, None) == 'true'` for each of
/// `('italics', 'bold', 'strike-through', 'small-caps')`, plus
/// `input_node.get('underlined', 'false') != 'false'`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InlineFormatting {
    pub italics: bool,
    pub bold: bool,
    pub strike_through: bool,
    pub small_caps: bool,
    pub underlined: bool,
}

/// The per-call inputs to [`InlineClassAssigner::execute`]. Stands in
/// for the XSLT `input_node`'s attributes -- this crate has no XSLT
/// node type, so the same information is passed as a plain struct
/// instead of an `input_node: &Node`-shaped parameter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlineClassInput {
    pub formatting: InlineFormatting,
    /// Port of `input_node.get('font-size', False)`: `None` means the
    /// attribute was absent (Python's falsy `False` sentinel), `Some`
    /// (even `Some("")`) means it was present. An empty string is
    /// still falsy in Python, so it is treated the same as absent
    /// below -- see [`InlineClassAssigner::execute`].
    pub font_size: Option<String>,
    /// Port of `input_node.get('font-color', False)`. Same
    /// absent-vs-empty handling as `font_size`.
    pub font_color: Option<String>,
}

/// Port of `InlineClass`, minus the `etree.XSLTExtension`
/// machinery (`__init__`'s `log`, and the `execute(context, self_node,
/// input_node, output_parent)` XSLT callback shape) -- see the module
/// docs for why. `font_sizes`/`colors` are `InlineClass.font_sizes`/
/// `.colors`: growable, ordered, deduplicated lists that persist
/// across calls on the same instance, assigning each distinct value
/// the index it was first seen at.
#[derive(Debug, Clone, Default)]
pub struct InlineClassAssigner {
    pub font_sizes: Vec<String>,
    pub colors: Vec<String>,
}

impl InlineClassAssigner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Port of `InlineClass.execute`. Returns the space-separated
    /// class string Python writes to `output_parent.text`.
    pub fn execute(&mut self, input: &InlineClassInput) -> String {
        let mut classes = vec!["none".to_string()];

        if input.formatting.italics {
            classes.push("italics".to_string());
        }
        if input.formatting.bold {
            classes.push("bold".to_string());
        }
        if input.formatting.strike_through {
            classes.push("strike-through".to_string());
        }
        if input.formatting.small_caps {
            classes.push("small-caps".to_string());
        }
        // underlined is special: Python's `!= 'false'` default-string
        // comparison collapses to "the flag is set" once modeled as a
        // real bool.
        if input.formatting.underlined {
            classes.push("underlined".to_string());
        }

        // Port of `fs = input_node.get('font-size', False); if fs:`.
        // `Some("")` is falsy in Python (an empty string), so it is
        // treated the same as `None` here -- neither assigns or
        // appends an `fsN` class.
        if let Some(fs) = input.font_size.as_deref().filter(|s| !s.is_empty()) {
            if !self.font_sizes.iter().any(|x| x == fs) {
                self.font_sizes.push(fs.to_string());
            }
            let idx = self.font_sizes.iter().position(|x| x == fs).unwrap();
            classes.push(format!("fs{idx}"));
        }

        if let Some(fc) = input.font_color.as_deref().filter(|s| !s.is_empty()) {
            if !self.colors.iter().any(|x| x == fc) {
                self.colors.push(fc.to_string());
            }
            let idx = self.colors.iter().position(|x| x == fc).unwrap();
            classes.push(format!("col{idx}"));
        }

        classes.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_flags_set_yields_just_none() {
        let mut a = InlineClassAssigner::new();
        let out = a.execute(&InlineClassInput::default());
        assert_eq!(out, "none");
    }

    #[test]
    fn each_boolean_flag_appends_its_own_class_name() {
        let mut a = InlineClassAssigner::new();
        let out = a.execute(&InlineClassInput {
            formatting: InlineFormatting {
                italics: true,
                bold: true,
                strike_through: true,
                small_caps: true,
                underlined: true,
            },
            ..Default::default()
        });
        assert_eq!(
            out,
            "none italics bold strike-through small-caps underlined"
        );
    }

    #[test]
    fn first_distinct_font_size_gets_index_zero() {
        let mut a = InlineClassAssigner::new();
        let out = a.execute(&InlineClassInput {
            font_size: Some("12pt".to_string()),
            ..Default::default()
        });
        assert_eq!(out, "none fs0");
        assert_eq!(a.font_sizes, vec!["12pt".to_string()]);
    }

    #[test]
    fn repeated_identical_font_size_reuses_the_same_index() {
        let mut a = InlineClassAssigner::new();
        a.execute(&InlineClassInput {
            font_size: Some("12pt".to_string()),
            ..Default::default()
        });
        let out = a.execute(&InlineClassInput {
            font_size: Some("12pt".to_string()),
            ..Default::default()
        });
        assert_eq!(out, "none fs0");
        // Not appended a second time.
        assert_eq!(a.font_sizes, vec!["12pt".to_string()]);
    }

    #[test]
    fn a_new_distinct_font_size_gets_the_next_index() {
        let mut a = InlineClassAssigner::new();
        a.execute(&InlineClassInput {
            font_size: Some("12pt".to_string()),
            ..Default::default()
        });
        let out = a.execute(&InlineClassInput {
            font_size: Some("14pt".to_string()),
            ..Default::default()
        });
        assert_eq!(out, "none fs1");
        assert_eq!(a.font_sizes, vec!["12pt".to_string(), "14pt".to_string()]);
    }

    #[test]
    fn colors_are_tracked_independently_of_font_sizes() {
        let mut a = InlineClassAssigner::new();
        a.execute(&InlineClassInput {
            font_size: Some("12pt".to_string()),
            ..Default::default()
        });
        let out = a.execute(&InlineClassInput {
            font_color: Some("#ff0000".to_string()),
            ..Default::default()
        });
        assert_eq!(out, "none col0");
        assert_eq!(a.colors, vec!["#ff0000".to_string()]);
        assert_eq!(a.font_sizes, vec!["12pt".to_string()]);
    }

    #[test]
    fn repeated_identical_color_reuses_the_same_index() {
        let mut a = InlineClassAssigner::new();
        a.execute(&InlineClassInput {
            font_color: Some("#ff0000".to_string()),
            ..Default::default()
        });
        a.execute(&InlineClassInput {
            font_color: Some("#00ff00".to_string()),
            ..Default::default()
        });
        let out = a.execute(&InlineClassInput {
            font_color: Some("#ff0000".to_string()),
            ..Default::default()
        });
        assert_eq!(out, "none col0");
        assert_eq!(a.colors, vec!["#ff0000".to_string(), "#00ff00".to_string()]);
    }

    #[test]
    fn empty_string_font_size_is_treated_as_absent() {
        // Port of Python falsy-string handling: `if fs:` is False for
        // `fs == ''`.
        let mut a = InlineClassAssigner::new();
        let out = a.execute(&InlineClassInput {
            font_size: Some(String::new()),
            ..Default::default()
        });
        assert_eq!(out, "none");
        assert!(a.font_sizes.is_empty());
    }

    #[test]
    fn combined_flags_and_font_size_and_color_in_declared_order() {
        let mut a = InlineClassAssigner::new();
        let out = a.execute(&InlineClassInput {
            formatting: InlineFormatting {
                bold: true,
                ..Default::default()
            },
            font_size: Some("10pt".to_string()),
            font_color: Some("#000000".to_string()),
        });
        assert_eq!(out, "none bold fs0 col0");
    }
}
