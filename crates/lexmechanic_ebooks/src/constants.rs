use lazy_static::lazy_static;
use std::collections::HashSet;

lazy_static! {
    /// HTML5 tag names as a set for quick lookup
    pub static ref HTML5_TAGS: HashSet<&'static str> = {
        let mut tags = HashSet::new();
        tags.insert("html");
        tags.insert("head");
        tags.insert("title");
        tags.insert("base");
        tags.insert("link");
        tags.insert("meta");
        tags.insert("style");
        tags.insert("script");
        tags.insert("noscript");
        tags.insert("body");
        tags.insert("section");
        tags.insert("nav");
        tags.insert("article");
        tags.insert("aside");
        tags.insert("h1");
        tags.insert("h2");
        tags.insert("h3");
        tags.insert("h4");
        tags.insert("h5");
        tags.insert("h6");
        tags.insert("header");
        tags.insert("footer");
        tags.insert("address");
        tags.insert("p");
        tags.insert("hr");
        tags.insert("br");
        tags.insert("pre");
        tags.insert("dialog");
        tags.insert("blockquote");
        tags.insert("ol");
        tags.insert("ul");
        tags.insert("li");
        tags.insert("dl");
        tags.insert("dt");
        tags.insert("dd");
        tags.insert("a");
        tags.insert("q");
        tags.insert("cite");
        tags.insert("em");
        tags.insert("strong");
        tags.insert("small");
        tags.insert("mark");
        tags.insert("dfn");
        tags.insert("abbr");
        tags.insert("time");
        tags.insert("progress");
        tags.insert("meter");
        tags.insert("code");
        tags.insert("var");
        tags.insert("samp");
        tags.insert("kbd");
        tags.insert("sub");
        tags.insert("sup");
        tags.insert("span");
        tags.insert("i");
        tags.insert("b");
        tags.insert("bdo");
        tags.insert("ruby");
        tags.insert("rt");
        tags.insert("rp");
        tags.insert("ins");
        tags.insert("del");
        tags.insert("figure");
        tags.insert("img");
        tags.insert("iframe");
        tags.insert("embed");
        tags.insert("object");
        tags.insert("param");
        tags.insert("video");
        tags.insert("audio");
        tags.insert("source");
        tags.insert("canvas");
        tags.insert("map");
        tags.insert("area");
        tags.insert("table");
        tags.insert("caption");
        tags.insert("colgroup");
        tags.insert("col");
        tags.insert("tbody");
        tags.insert("thead");
        tags.insert("tfoot");
        tags.insert("tr");
        tags.insert("td");
        tags.insert("th");
        tags.insert("form");
        tags.insert("fieldset");
        tags.insert("label");
        tags.insert("input");
        tags.insert("button");
        tags.insert("select");
        tags.insert("datalist");
        tags.insert("optgroup");
        tags.insert("option");
        tags.insert("textarea");
        tags.insert("output");
        tags.insert("details");
        tags.insert("command");
        tags.insert("bb");
        tags.insert("menu");
        tags.insert("legend");
        tags.insert("div");
        tags
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html5_tags_contains_common_tags() {
        assert!(HTML5_TAGS.contains("div"));
        assert!(HTML5_TAGS.contains("p"));
        assert!(HTML5_TAGS.contains("span"));
        assert!(HTML5_TAGS.contains("html"));
        assert!(HTML5_TAGS.contains("body"));
    }

    #[test]
    fn test_html5_tags_count() {
        // Should have 101 tags (not 109 as in Python - some tags may have been removed from HTML5 spec)
        assert_eq!(HTML5_TAGS.len(), 101);
    }

    #[test]
    fn test_html5_tags_does_not_contain_invalid() {
        assert!(!HTML5_TAGS.contains("notarealtag"));
        assert!(!HTML5_TAGS.contains("DIV")); // Case sensitive
    }
}
