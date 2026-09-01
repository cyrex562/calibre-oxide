//! Port of `old_src/src/calibre/web/site_parsers/nytimes.py`:
//! extracts article HTML from a NYTimes page's embedded
//! `window.__preloadedData` JSON blob (or the "live" liveblog JSON
//! shape), for the "web to ebook" news-recipe pipeline.
//!
//! # Scope
//!
//! Real: every JSON->HTML transformation function
//! ([`json_to_html`]/[`article_parse`]/[`parse_types`]/etc.) and
//! [`extract_html`]'s `<script>`-tag extraction (a plain substring
//! scan, since the only thing needed from the HTML is one script
//! tag's raw text -- narrower than pulling in a full HTML parser for
//! this one job, matching `calibre_ebooks::scraper`'s own precedent of
//! not reaching for heavier machinery than a job actually needs).
//!
//! Narrowed/not ported: `download_url_from_wayback`/`download_url`
//! (a fetch through a specific `calibre-ebook.com` Wayback-Machine
//! proxy used to work around NYTimes's anti-bot measures) -- a
//! network-integration detail orthogonal to the JSON parsing this
//! module's real value is in, not implemented here.
//!
//! [`iso_date`] formats in UTC rather than preserving the original
//! date's own offset (`parse_iso8601(x, as_utc=False)` upstream) -- a
//! display-string simplification, not a parsing gap.

use calibre_utils::date::parse_date;
use regex::Regex;
use serde_json::Value;

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Port of `parse_image`.
fn parse_image(i: &Value) -> String {
    let mut out = String::new();
    let crop = i.get("crops").filter(|v| !v.is_null()).or_else(|| i.get("spanImageCrops"));
    if let Some(crop) = crop.and_then(Value::as_array).and_then(|a| a.first()) {
        if let Some(url) = crop.get("renditions").and_then(Value::as_array).and_then(|a| a.first()).and_then(|r| r.get("url")).and_then(Value::as_str) {
            out.push_str(&format!("<div><img src=\"{url}\" title=\"{}\">", s(i, "altText")));
        }
    }
    if let Some(caption) = i.get("caption") {
        out.push_str(&format!("<div class=\"cap\">{}", parse_types(caption)));
        if has(i, "credit") {
            out.push_str(&format!("<span class=\"cred\"> {}</span>", s(i, "credit")));
        }
        out.push_str("</div>");
    } else if let Some(legacy) = i.get("legacyHtmlCaption").and_then(Value::as_str) {
        if !legacy.trim().is_empty() {
            out.push_str(&format!("<div class=\"cap\">{legacy}</div>"));
        }
    }
    out.push_str("</div>");
    out
}

fn has(v: &Value, key: &str) -> bool {
    v.get(key).map(|x| !x.is_null()).unwrap_or(false)
}

/// Port of `parse_img_grid`.
fn parse_img_grid(g: &Value) -> String {
    let mut out = String::new();
    if let Some(media) = g.get("gridMedia").and_then(Value::as_array) {
        for grd in media {
            out.push_str(&parse_image(grd));
        }
    }
    if has(g, "caption") {
        out.push_str(&format!("<div class=\"cap\">{}", s(g, "caption")));
        if has(g, "credit") {
            out.push_str(&format!("<span class=\"cred\"> {}</span>", s(g, "credit")));
        }
        out.push_str("</div>");
    }
    out
}

/// Port of `parse_vid`.
fn parse_vid(v: &Value) -> String {
    let mut out = String::new();
    if !has(v, "promotionalMedia") {
        return out;
    }
    let headline = v.get("headline").and_then(|h| h.get("default")).and_then(Value::as_str).unwrap_or("");
    let rendition = v.get("renditions").and_then(Value::as_array).filter(|a| !a.is_empty());
    if let Some(r) = rendition.and_then(|a| a.first()) {
        out.push_str(&format!("<div><b><a href=\"{}\">Video</a>: {headline}</b></div>", s(r, "url")));
    } else {
        out.push_str(&format!("<div><b>{headline}</b></div>"));
    }
    out.push_str(&parse_types(&v["promotionalMedia"]));
    if has(v, "promotionalSummary") {
        out.push_str(&format!("<div class=\"cap\">{}</div>", s(v, "promotionalSummary")));
    }
    out
}

/// Port of `get_data_wrapper`.
fn get_data_wrapper(html: &str) -> String {
    let re = Regex::new(r"datawrapper\.dwcdn\.net/(.{5})").unwrap();
    re.captures(html).map(|c| c[1].to_string()).unwrap_or_default()
}

/// Port of `parse_emb`.
fn parse_emb(e: &Value) -> String {
    let mut out = String::new();
    let dw = get_data_wrapper(s(e, "html"));
    if !dw.is_empty() {
        out.push_str(&format!("<div><img src=\"https://datawrapper.dwcdn.net/{dw}/full.png\"></div>"));
    } else if has(e, "promotionalMedia") {
        if has(e, "headline") {
            out.push_str(&format!("<div><b>{}</b></div>", s(&e["headline"], "default")));
        }
        out.push_str(&parse_types(&e["promotionalMedia"]));
        if has(e, "note") {
            out.push_str(&format!("<div class=\"cap\">{}</div>", s(e, "note")));
        }
    }
    out
}

/// Port of `parse_byline`.
fn parse_byline(byl: &Value) -> String {
    let mut out = String::new();
    if let Some(bylines) = byl.get("bylines").and_then(Value::as_array) {
        for b in bylines {
            out.push_str(&format!("<div><b>{}</b></div>", s(b, "renderedRepresentation")));
        }
    }
    out.push_str("<div><i>");
    if let Some(roles) = byl.get("role").and_then(Value::as_array) {
        for rl in roles {
            let text = parse_cnt(rl);
            if !text.trim().is_empty() {
                out.push_str(&text);
            }
        }
    }
    out.push_str("</i></div>");
    out
}

/// Port of `iso_date`. See the module doc for the UTC-only narrowing.
fn iso_date(x: &str) -> String {
    parse_date(x, true).map(|dt| dt.format("%b %d, %Y at %I:%M %p").to_string()).unwrap_or_default()
}

/// Port of `parse_header`.
fn parse_header(h: &Value) -> String {
    let mut out = String::new();
    if has(h, "label") {
        out.push_str(&format!("<div class=\"lbl\">{}</div>", parse_types(&h["label"])));
    }
    if has(h, "headline") {
        out.push_str(&parse_types(&h["headline"]));
    }
    if has(h, "summary") {
        out.push_str(&format!("<p><i>{}</i></p>", parse_types(&h["summary"])));
    }
    if has(h, "ledeMedia") {
        out.push_str(&parse_types(&h["ledeMedia"]));
    }
    if has(h, "byline") {
        out.push_str(&parse_types(&h["byline"]));
    }
    if has(h, "timestampBlock") {
        out.push_str(&parse_types(&h["timestampBlock"]));
    }
    out
}

/// Port of `parse_fmt_type`.
fn parse_fmt_type(fm: &Value) -> String {
    let mut out = String::new();
    let formats: Vec<&Value> = fm.get("formats").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
    for f in &formats {
        match f.get("__typename").and_then(Value::as_str) {
            Some("BoldFormat") => out.push_str("<strong>"),
            Some("ItalicFormat") => out.push_str("<em>"),
            Some("LinkFormat") => out.push_str(&format!("<a href=\"{}\">", s(f, "url"))),
            _ => {}
        }
    }
    out.push_str(s(fm, "text"));
    for f in formats.iter().rev() {
        match f.get("__typename").and_then(Value::as_str) {
            Some("BoldFormat") => out.push_str("</strong>"),
            Some("ItalicFormat") => out.push_str("</em>"),
            Some("LinkFormat") => out.push_str("</a>"),
            _ => {}
        }
    }
    out
}

/// Port of `parse_cnt`.
fn parse_cnt(cnt: &Value) -> String {
    let mut out = String::new();
    let Some(obj) = cnt.as_object() else { return out };
    for (k, v) in obj {
        if let Some(arr) = v.as_array() {
            if k == "formats" {
                out.push_str(&parse_fmt_type(cnt));
            } else {
                for item in arr {
                    out.push_str(&parse_types(item));
                }
            }
        } else if v.is_object() {
            out.push_str(&parse_types(v));
        }
    }
    if !obj.contains_key("formats") && !obj.contains_key("content") {
        if let Some(text) = obj.get("text").and_then(Value::as_str) {
            out.push_str(text);
        }
    }
    out
}

/// Port of `parse_types`.
fn parse_types(x: &Value) -> String {
    let typename = s(x, "__typename");
    let align = x
        .get("textAlign")
        .and_then(Value::as_str)
        .map(|a| format!(" style=\"text-align: {};\"", a.to_lowercase()))
        .unwrap_or_default();

    if typename.contains("Header") {
        return parse_header(x);
    }
    if typename.starts_with("Heading") {
        let level = typename.chars().find(|c| c.is_ascii_digit()).unwrap_or('1');
        let htag = format!("h{level}");
        return format!("<{htag}{align}>{}</{htag}>", parse_cnt(x));
    }
    match typename {
        "ParagraphBlock" => format!("<p>{}</p>", parse_cnt(x)),
        "DetailBlock" | "TextRunKV" => format!("<p style=\"font-size: small;\">{}</p>", parse_cnt(x)),
        "BylineBlock" => format!("<div class=\"byl\"><br/>{}</div>", parse_byline(x)),
        "LabelBlock" => format!("<div class=\"sc\">{}</div>", parse_cnt(x)),
        "BlockquoteBlock" => format!("<blockquote>{}</blockquote>", parse_cnt(x)),
        "TimestampBlock" => format!("<div class=\"time\">{}</div>", iso_date(s(x, "timestamp"))),
        "LineBreakInline" => "<br/>".to_string(),
        "RuleBlock" => "<hr/>".to_string(),
        "Image" => parse_image(x),
        "GridBlock" => parse_img_grid(x),
        "Video" => parse_vid(x),
        "EmbeddedInteractive" => parse_emb(x),
        "ListBlock" => format!("\n<ul>{}</ul>", parse_cnt(x)),
        "ListItemBlock" => format!("\n<li>{}</li>", parse_cnt(x)),
        "" | "RelatedLinksBlock" | "EmailSignupBlock" | "Dropzone" | "AudioBlock" => String::new(),
        _ => parse_cnt(x),
    }
}

/// Port of `article_parse`.
fn article_parse(data: &[Value]) -> String {
    let mut out = String::from("<html><body>");
    for d in data {
        out.push_str(&parse_types(d));
    }
    out.push_str("</body></html>");
    out
}

/// Port of `clean_js_json`.
fn clean_js_json(text: &str) -> String {
    let undefined_re = Regex::new(r"\bundefined\b").unwrap();
    let text = undefined_re.replace_all(text, "null");
    let check_gate_re = Regex::new(r#"(?s)\{"checkGate":.*"#).unwrap();
    check_gate_re.replace(&text, "null}}").into_owned()
}

/// Port of `json_to_html`. Returns `None` for the "interactive
/// article" and CAPTCHA cases upstream raises on; callers should
/// treat that the same way (skip/report, not crash).
pub fn json_to_html(raw: &str) -> Result<String, String> {
    let cleaned = clean_js_json(raw);
    let data: Value = serde_json::from_str(&cleaned).map_err(|e| format!("invalid JSON: {e}"))?;

    let article_content = data.get("initialData").and_then(|d| d.get("data")).and_then(|d| d.get("article"));
    if let Some(article) = article_content {
        let content = article.get("sprinkledBody").and_then(|b| b.get("content")).and_then(Value::as_array).cloned().unwrap_or_default();
        return Ok(article_parse(&content));
    }
    if let Some(initial_state) = data.get("initialState") {
        return live_json_to_html(initial_state);
    }
    Err("unrecognized NYTimes JSON shape".to_string())
}

/// Port of `add_live_item`.
fn add_live_item(item: &Value, item_type: &str, lines: &mut Vec<String>) -> Result<(), String> {
    match item_type {
        "text" => lines.push(format!("<p>{}</p>", s(item, "value"))),
        "list" => lines.push(format!("<li>{}</li>", s(item, "value"))),
        "bulletedList" => {
            lines.push("<ul>".to_string());
            if let Some(arr) = item.get("value").and_then(Value::as_array) {
                for x in arr {
                    lines.push(format!("<li>{}</li>", x.as_str().unwrap_or_default()));
                }
            }
            lines.push("</ul>".to_string());
        }
        "items" => {
            if let Some(arr) = item.get("value").and_then(Value::as_array) {
                for x in arr {
                    lines.push(format!("<h5>{}</h5>", s(x, "subtitle")));
                    add_live_item(&serde_json::json!({"value": x.get("text").cloned().unwrap_or_default()}), "text", lines)?;
                }
            }
        }
        "section" => {
            if let Some(arr) = item.get("value").and_then(Value::as_array) {
                for sub in arr {
                    let sub_type = s(sub, "type").to_string();
                    add_live_item(sub, &sub_type, lines)?;
                }
            }
        }
        "" => {
            let b = item;
            if has(b, "title") {
                lines.push(format!("<h3>{}</h3>", s(b, "title")));
            }
            if has(b, "imageUrl") {
                lines.push(format!("<div><img src=\"{}\"/></div>", s(b, "imageUrl")));
            }
            if has(b, "leadIn") {
                lines.push(format!("<p>{}</p>", s(b, "leadIn")));
            }
            if let Some(items) = b.get("items") {
                add_live_item(&serde_json::json!({"value": items}), "items", lines)?;
                return Ok(());
            }
            if let Some(bl) = b.get("bulletedList") {
                add_live_item(&serde_json::json!({"value": bl}), "bulletedList", lines)?;
                return Ok(());
            }
            if let Some(sections) = b.get("sections").and_then(Value::as_array) {
                for section in sections {
                    add_live_item(&serde_json::json!({"value": section.get("section").cloned().unwrap_or_default()}), "section", lines)?;
                }
                return Ok(());
            }
            return Err(format!("Unknown item: {b}"));
        }
        other => return Err(format!("Unknown item type: {other}")),
    }
    Ok(())
}

/// Port of `live_json_to_html`.
fn live_json_to_html(data: &Value) -> Result<String, String> {
    let root_query = data.get("ROOT_QUERY").and_then(Value::as_object).ok_or("No ROOT_QUERY found")?;
    let root_ref = root_query.values().find(|v| v.is_object() && v.get("id").is_some()).ok_or("No id found in ROOT_QUERY")?;
    let root_id = s(root_ref, "id");
    let root = data.get(root_id).ok_or("root id not found in data")?;

    let storyline_id = root.get("storylines").and_then(Value::as_array).and_then(|a| a.first()).and_then(|s0| s0.get("id")).and_then(Value::as_str).ok_or("no storylines")?;
    let storyline_wrap = data.get(storyline_id).ok_or("storyline wrapper not found")?;
    let storyline_id2 = storyline_wrap.get("storyline").and_then(|s0| s0.get("id")).and_then(Value::as_str).ok_or("no nested storyline")?;
    let storyline = data.get(storyline_id2).ok_or("storyline not found")?;

    let title = s(storyline, "displayName");
    let mut lines = vec![format!("<h1>{}</h1>", crate::oeb::parse_utils::escape_xml(title))];

    let blob_str = s(storyline, "experimentalJsonBlob");
    let blob: Value = serde_json::from_str(blob_str).map_err(|e| format!("invalid experimentalJsonBlob: {e}"))?;
    if let Some(entries) = blob.get("data").and_then(Value::as_array).and_then(|a| a.first()).and_then(|d| d.get("data")).and_then(Value::as_array) {
        for b_wrap in entries {
            let b = b_wrap.get("data").cloned().unwrap_or_default();
            if let Some(arr) = b.as_array() {
                for x in arr {
                    let t = s(x, "type").to_string();
                    add_live_item(x, &t, &mut lines)?;
                }
            } else {
                add_live_item(&b, "", &mut lines)?;
            }
        }
    }
    Ok(format!("<html><body>{}</body></html>", lines.join("\n")))
}

/// Port of `extract_html`. `raw_html` is the full page source;
/// `url` decides the interactive-article short-circuit.
pub fn extract_html(raw_html: &str, url: &str) -> Result<String, String> {
    if url.contains("/interactive/") {
        return Ok(
            "<html><body><p><em>This is an interactive article, which is supposed to be read in a browser.</p></em></body></html>"
                .to_string(),
        );
    }

    let script = find_preloaded_data_script(raw_html)
        .ok_or_else(|| {
            if raw_html.contains("https://ct.captcha-delivery.com/c.js") {
                "NYTimes returned a CAPTCHA page from captcha-delivery.com".to_string()
            } else {
                "NYTimes returned HTML without preloaded data".to_string()
            }
        })?;

    let brace = script.find('{').ok_or("no JSON object found in preloaded-data script")?;
    let semi = script.rfind(';').unwrap_or(script.len());
    let raw = script[brace..semi.max(brace)].trim().trim_end_matches(';');
    json_to_html(raw)
}

/// Scans `html` for a `<script>...</script>` block whose text
/// contains `window.__preloadedData`, returning its inner text.
/// Narrower than a full HTML parser -- see the module doc.
fn find_preloaded_data_script(html: &str) -> Option<&str> {
    let marker = "window.__preloadedData";
    let mut search_from = 0;
    while let Some(rel_start) = html[search_from..].find("<script") {
        let tag_start = search_from + rel_start;
        let content_start = html[tag_start..].find('>')? + tag_start + 1;
        let content_end = html[content_start..].find("</script>")? + content_start;
        let content = &html[content_start..content_end];
        if content.contains(marker) {
            return Some(content);
        }
        search_from = content_end + "</script>".len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_a_paragraph_block() {
        let block = json!({"__typename": "ParagraphBlock", "text": "Hello world"});
        assert_eq!(parse_types(&block), "<p>Hello world</p>");
    }

    #[test]
    fn parses_bold_and_italic_formats() {
        let block = json!({
            "__typename": "ParagraphBlock",
            "text": [{"__typename": "TextInline", "text": "bold text", "formats": [{"__typename": "BoldFormat"}]}]
        });
        let html = parse_types(&block);
        assert!(html.contains("<strong>bold text</strong>"), "{html}");
    }

    #[test]
    fn parses_a_heading_level() {
        let block = json!({"__typename": "Heading2Block", "text": "Section"});
        assert_eq!(parse_types(&block), "<h2>Section</h2>");
    }

    #[test]
    fn json_to_html_extracts_sprinkled_body() {
        let raw = json!({
            "initialData": {"data": {"article": {"sprinkledBody": {"content": [
                {"__typename": "ParagraphBlock", "text": "First paragraph"}
            ]}}}}
        })
        .to_string();
        let html = json_to_html(&raw).unwrap();
        assert!(html.contains("<p>First paragraph</p>"), "{html}");
    }

    #[test]
    fn extract_html_short_circuits_interactive_articles() {
        let html = extract_html("<html></html>", "https://nytimes.com/interactive/2023/foo").unwrap();
        assert!(html.contains("interactive article"));
    }

    #[test]
    fn extract_html_reports_a_captcha_page() {
        let raw = r#"<html><script src="https://ct.captcha-delivery.com/c.js"></script></html>"#;
        let err = extract_html(raw, "https://nytimes.com/2023/foo").unwrap_err();
        assert!(err.contains("CAPTCHA"), "{err}");
    }

    #[test]
    fn extract_html_finds_and_parses_the_preloaded_data_script() {
        let payload = json!({
            "initialData": {"data": {"article": {"sprinkledBody": {"content": [
                {"__typename": "ParagraphBlock", "text": "From the page"}
            ]}}}}
        });
        let raw = format!(
            r#"<html><body><script>window.__preloadedData = {payload};</script></body></html>"#
        );
        let html = extract_html(&raw, "https://nytimes.com/2023/foo").unwrap();
        assert!(html.contains("From the page"), "{html}");
    }

    #[test]
    fn get_data_wrapper_extracts_the_embed_id() {
        assert_eq!(get_data_wrapper("https://datawrapper.dwcdn.net/abcde/1/"), "abcde");
        assert_eq!(get_data_wrapper("no embed here"), "");
    }
}
