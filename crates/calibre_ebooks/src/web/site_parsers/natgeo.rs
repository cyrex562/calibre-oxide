//! Port of `old_src/src/calibre/web/site_parsers/natgeo.py`: extracts
//! article HTML from a National Geographic page's embedded
//! `window['__natgeo__']` JSON blob, for the "web to ebook" news-
//! recipe pipeline.
//!
//! # Scope
//!
//! Real: every JSON->HTML transformation function
//! ([`extract_json`]/[`article_parse`]/[`parse_article`]/etc.) --
//! pure logic over `serde_json::Value`, no network or GUI dependency.
//!
//! Narrowed: [`iso_date`] formats in UTC rather than preserving the
//! original date's own offset (`parse_iso8601(x, as_utc=False)`
//! upstream) -- a display-string simplification, not a parsing gap.

use calibre_utils::date::parse_date;
use serde_json::Value;

fn escape(s: &str) -> String {
    crate::oeb::parse_utils::escape_xml(s)
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn has(v: &Value, key: &str) -> bool {
    v.get(key).is_some()
}

/// Port of `extract_json`.
pub fn extract_json(raw: &str) -> Option<Value> {
    let s_idx = raw.find("window['__natgeo__']")?;
    let rest = &raw[s_idx..];
    let end = rest.find("</script>").unwrap_or(rest.len());
    let script = &rest[..end];
    let brace = script.find('{')?;
    let json_text = script[brace..].trim_end().trim_end_matches(';');
    let parsed: Value = serde_json::from_str(json_text).ok()?;
    let content = parsed.get("page")?.get("content")?;
    if content.get("prismarticle").is_some() {
        return Some(content["prismarticle"].clone());
    }
    if content.get("article").is_some() {
        return Some(content["article"].clone());
    }
    None
}

/// Port of `parse_contributors`.
fn parse_contributors(grp: &Value) -> String {
    let mut out = String::new();
    let Some(items) = grp.as_array() else { return out };
    for item in items {
        out.push_str("<div class=\"auth\">");
        out.push_str(&escape(s(item, "title")));
        out.push(' ');
        if let Some(contributors) = item.get("contributors").and_then(Value::as_array) {
            for c in contributors {
                out.push_str(&escape(s(c, "displayName")));
            }
        }
        out.push_str("</div>");
    }
    out
}

/// Port of `parse_lead_image`.
fn parse_lead_image(media: &Value) -> String {
    let mut out = String::new();
    let Some(image) = media.get("image") else { return out };
    out.push_str("<p>");
    if has(image, "dsc") {
        out.push_str(&format!(
            "<div><img src=\"{}\" alt=\"{}\"></div>",
            escape(s(image, "src")),
            escape(s(image, "dsc"))
        ));
    } else {
        out.push_str(&format!("<div><img src=\"{}\"></div>", escape(s(image, "src"))));
    }
    if has(media, "caption") && has(media, "credit") {
        out.push_str(&format!(
            "<div class=\"cap\">{}<span class=\"cred\"> {}</span></div>",
            s(media, "caption"),
            s(media, "credit")
        ));
    } else if has(media, "caption") {
        out.push_str(&format!("<div class=\"cap\">{}</div>", s(media, "caption")));
    }
    out.push_str("</p>");
    out
}

/// Port of `parse_inline`.
fn parse_inline(inl: &Value) -> String {
    let mut out = String::new();
    let content = inl.get("content");
    let name = content.and_then(|c| c.get("name")).and_then(Value::as_str).unwrap_or("");

    if name == "Image" {
        if let Some(props) = content.and_then(|c| c.get("props")) {
            out.push_str("<p>");
            if let Some(image) = props.get("image") {
                out.push_str(&format!("<div class=\"img\"><img src=\"{}\"></div>", s(image, "src")));
            }
            if let Some(caption) = props.get("caption") {
                out.push_str(&format!(
                    "<div class=\"cap\">{}<span class=\"cred\"> {}</span></div>",
                    s(caption, "text"),
                    s(caption, "credit")
                ));
            }
            out.push_str("</p>");
        }
    }
    if name == "ImageGroup" {
        if let Some(images) = content.and_then(|c| c.get("props")).and_then(|p| p.get("images")).and_then(Value::as_array) {
            for imgs in images {
                out.push_str("<p>");
                if let Some(src) = imgs.get("src").and_then(Value::as_str) {
                    out.push_str(&format!("<div class=\"img\"><img src=\"{src}\"></div>"));
                }
                if let Some(caption) = imgs.get("caption") {
                    out.push_str(&format!(
                        "<div class=\"cap\">{}<span class=\"cred\"> {}</span></div>",
                        s(caption, "text"),
                        s(caption, "credit")
                    ));
                }
                out.push_str("</p>");
            }
        }
    }
    out
}

/// Port of `parse_cont`.
fn parse_cont(content: &Value) -> String {
    let mut out = String::new();
    let Some(items) = content.get("content").and_then(Value::as_array) else { return out };
    for cont in items {
        if cont.is_object() {
            out.push_str(&parse_body(cont));
        } else if let Some(txt) = cont.as_str() {
            out.push_str(txt);
        }
    }
    out
}

/// Port of `parse_body`.
fn parse_body(x: &Value) -> String {
    let mut out = String::new();
    if let Some(obj) = x.as_object() {
        if let Some(tag) = obj.get("type").and_then(Value::as_str) {
            if tag == "inline" {
                out.push_str(&parse_inline(x));
            } else if x.get("attrs").and_then(|a| a.get("href")).is_some() {
                out.push_str(&format!("<{tag} href=\"{}\">", s(&x["attrs"], "href")));
                out.push_str(&parse_cont(x));
                out.push_str(&format!("</{tag}>"));
            } else {
                out.push_str(&format!("<{tag}>"));
                out.push_str(&parse_cont(x));
                out.push_str(&format!("</{tag}>"));
            }
        }
    } else if let Some(arr) = x.as_array() {
        for y in arr {
            if y.is_object() {
                out.push_str(&parse_body(y));
            }
        }
    }
    out
}

/// Port of `parse_bdy`.
fn parse_bdy(item: &Value) -> String {
    let mut out = String::new();
    let Some(c) = item.get("cntnt") else { return out };
    if item.get("type").and_then(Value::as_str) == Some("inline") {
        match c.get("cmsType").and_then(Value::as_str) {
            Some("listicle") => {
                if has(c, "title") {
                    out.push_str(&format!("<h3>{}</h3>", escape(s(c, "title"))));
                }
                out.push_str(s(c, "text"));
            }
            Some("image") => out.push_str(&parse_lead_image(c)),
            Some("imagegroup") => {
                if let Some(images) = c.get("images").and_then(Value::as_array) {
                    for imgs in images {
                        out.push_str(&parse_lead_image(imgs));
                    }
                }
            }
            Some("pullquote") => {
                if has(c, "quote") {
                    out.push_str(&format!("<blockquote>{}</blockquote>", s(c, "quote")));
                }
            }
            Some("editorsNote") => {
                if has(c, "note") {
                    out.push_str(&format!("<blockquote>{}</blockquote>", s(c, "note")));
                }
            }
            _ => {}
        }
    } else {
        let markup = s(c, "mrkup");
        if markup.trim_start().starts_with('<') {
            out.push_str(markup);
        } else {
            let tag = s(item, "type");
            out.push_str(&format!("<{tag}>{markup}</{tag}>"));
        }
    }
    out
}

/// Port of `parse_article`.
fn parse_article(edg: &Value) -> String {
    let mut out = String::new();
    let Some(sc) = edg.get("schma") else { return out };
    out.push_str(&format!("<div class=\"sub\">{}</div>", escape(s(edg, "sctn"))));
    out.push_str(&format!("<h1>{}</h1>", escape(s(sc, "sclTtl"))));
    if !s(sc, "sclDsc").is_empty() {
        out.push_str(&format!("<div class=\"byline\">{}</div>", escape(s(sc, "sclDsc"))));
    }
    out.push_str("<p>");
    if let Some(grp) = edg.get("cntrbGrp") {
        out.push_str(&parse_contributors(grp));
    }
    if let Some(md_dt) = edg.get("mdDt").and_then(Value::as_str) {
        if let Some(dt) = parse_date(md_dt, true) {
            out.push_str(&format!("<div class=\"time\">Published: {}</div>", escape(&dt.format("%B %d, %Y").to_string())));
        }
    }
    if has(edg, "readTime") {
        out.push_str(&format!("<div class=\"time\">{}</div>", escape(s(edg, "readTime"))));
    }
    out.push_str("</p>");

    if edg.get("ldMda").and_then(|m| m.get("cmsType")).and_then(Value::as_str) == Some("image") {
        out.push_str(&parse_lead_image(&edg["ldMda"]));
    }

    if let Some(prism) = edg.get("prismData") {
        if let Some(mains) = prism.get("mainComponents").and_then(Value::as_array) {
            for main in mains {
                if main.get("name").and_then(Value::as_str) == Some("Body") {
                    if let Some(body) = main.get("props").and_then(|p| p.get("body")).and_then(Value::as_array) {
                        for item in body {
                            if item.is_object() {
                                if item.get("type").and_then(Value::as_str) == Some("inline") {
                                    out.push_str(&parse_inline(item));
                                }
                            } else if let Some(list) = item.as_array() {
                                for line in list {
                                    out.push_str(&parse_body(line));
                                }
                            }
                        }
                    }
                }
            }
        }
    } else if let Some(bdy) = edg.get("bdy").and_then(Value::as_array) {
        for item in bdy {
            out.push_str(&parse_bdy(item));
        }
    }
    out
}

/// Port of `article_parse`.
fn article_parse(data: &Value) -> String {
    let mut out = String::from("<html><body>");
    if let Some(frms) = data.get("frms").and_then(Value::as_array) {
        for frm in frms {
            if frm.is_null() {
                continue;
            }
            if let Some(mods) = frm.get("mods").and_then(Value::as_array) {
                for m in mods {
                    if let Some(edgs) = m.get("edgs").and_then(Value::as_array) {
                        for edg in edgs {
                            if edg.get("cmsType").and_then(Value::as_str) == Some("ImmersiveLeadTile") {
                                if let Some(img) = edg.get("cmsImage") {
                                    if img.get("image").is_some() {
                                        out.push_str(&parse_lead_image(img));
                                    }
                                }
                            }
                            if edg.get("cmsType").and_then(Value::as_str) == Some("ArticleBodyTile") {
                                out.push_str(&parse_article(edg));
                            }
                        }
                    }
                }
            }
        }
    }
    out.push_str("</body></html>");
    out
}

/// Port of `extract_html`.
pub fn extract_html(raw_html: &str) -> Option<String> {
    let data = extract_json(raw_html)?;
    Some(article_parse(&data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_the_embedded_json_and_finds_the_article_key() {
        let payload = json!({"page": {"content": {"article": {"schma": {"sclTtl": "Hello"}, "sctn": "News"}}}});
        let raw = format!("<script>window['__natgeo__'] = {payload};</script>");
        let extracted = extract_json(&raw).unwrap();
        assert_eq!(extracted["schma"]["sclTtl"], "Hello");
    }

    #[test]
    fn prefers_prismarticle_over_article_when_both_present() {
        let payload = json!({"page": {"content": {
            "article": {"marker": "old"},
            "prismarticle": {"marker": "new"},
        }}});
        let raw = format!("<script>window['__natgeo__'] = {payload};</script>");
        let extracted = extract_json(&raw).unwrap();
        assert_eq!(extracted["marker"], "new");
    }

    #[test]
    fn renders_a_full_article_with_title_byline_and_body() {
        let payload = json!({"page": {"content": {"article": {"frms": [{"mods": [{"edgs": [{
            "cmsType": "ArticleBodyTile",
            "sctn": "Animals",
            "schma": {"sclTtl": "Big Cats", "sclDsc": "A story about cats"},
            "cntrbGrp": [{"title": "By", "contributors": [{"displayName": "Jane Doe"}]}],
            "mdDt": "2023-05-01T12:00:00Z",
            "bdy": [{"type": "p", "cntnt": {"mrkup": "Hello world"}}],
        }]}]}]}}}});
        let raw = format!("<script>window['__natgeo__'] = {payload};</script>");
        let html = extract_html(&raw).unwrap();
        assert!(html.contains("<h1>Big Cats</h1>"), "{html}");
        assert!(html.contains("A story about cats"), "{html}");
        assert!(html.contains("Jane Doe"), "{html}");
        assert!(html.contains("Published: May 01, 2023"), "{html}");
        assert!(html.contains("<p>Hello world</p>"), "{html}");
    }

    #[test]
    fn returns_none_when_the_marker_script_is_absent() {
        assert!(extract_json("<html><body>no marker here</body></html>").is_none());
    }
}
