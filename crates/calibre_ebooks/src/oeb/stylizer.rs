use crate::oeb::normalize_css::DEFAULTS;
use roxmltree::Node;
use std::collections::HashMap;

pub struct Stylizer {
    pub dpi: f32,
    pub font_base: f32, // in pts
}

impl Stylizer {
    pub fn new(dpi: f32, font_base: f32) -> Self {
        Self { dpi, font_base }
    }

    pub fn style<'a, 'input>(&self, node: &Node<'a, 'input>) -> Style<'a, 'input, '_> {
        Style {
            node: *node,
            stylizer: self,
        }
    }
}

pub struct Style<'a, 'input, 'b> {
    node: Node<'a, 'input>,
    stylizer: &'b Stylizer,
}

impl<'a, 'input, 'b> Style<'a, 'input, 'b> {
    pub fn get(&self, property: &str) -> String {
        // 1. Check inline style
        if let Some(val) = self.get_inline_style(property) {
            return val;
        }

        // 2. Check inheritance
        // List of inherited properties (simplified)
        let inherited = [
            "color",
            "font-family",
            "font-size",
            "font-style",
            "font-weight",
            "text-align",
            "line-height",
        ];

        if inherited.contains(&property) {
            if let Some(parent) = self.node.parent() {
                if parent.is_element() {
                    return self.stylizer.style(&parent).get(property);
                }
            }
        }

        // 3. Return default
        DEFAULTS.get(property).cloned().unwrap_or("").to_string()
    }

    fn get_inline_style(&self, property: &str) -> Option<String> {
        if let Some(style_attr) = self.node.attribute("style") {
            // Simple CSS parser: split by ; then :
            for decl in style_attr.split(';') {
                let parts: Vec<&str> = decl.splitn(2, ':').collect();
                if parts.len() == 2 {
                    let prop = parts[0].trim();
                    let val = parts[1].trim();
                    if prop == property {
                        return Some(val.to_string());
                    }
                    // Handle edge expansion? E.g. simple cases if needed
                }
            }
        }
        None
    }

    pub fn font_size(&self) -> f32 {
        let val = self.get("font-size");
        // Naive conversion
        if val.ends_with("pt") {
            val.trim_end_matches("pt")
                .parse()
                .unwrap_or(self.stylizer.font_base)
        } else if val.ends_with("px") {
            let px = val.trim_end_matches("px").parse().unwrap_or(0.0);
            (px * 72.0) / self.stylizer.dpi
        } else if val == "medium" || val == "initial" {
            self.stylizer.font_base
        } else if val.ends_with("em") {
            let em = val.trim_end_matches("em").parse().unwrap_or(1.0);
            // Get parent font size
            if let Some(parent) = self.node.parent() {
                if parent.is_element() {
                    return self.stylizer.style(&parent).font_size() * em;
                }
            }
            self.stylizer.font_base * em
        } else {
            // Default fallthrough
            self.stylizer.font_base
        }
    }

    pub fn color(&self) -> String {
        self.get("color")
    }
}
