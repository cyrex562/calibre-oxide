use lazy_static::lazy_static;
use regex::{Captures, Regex};

lazy_static! {
    static ref POSFID_PATTERN: Regex = Regex::new(r"(?i)(<a.*?href=.*?>)").unwrap();
    // Adjusted pattern to handle ' or " quotes and capture groups correctly
    static ref POSFID_INDEX_PATTERN: Regex = Regex::new(r#"['"]kindle:pos:fid:([0-9A-Va-v]+):off:([0-9A-Va-v]+).*?['"]"#).unwrap();

    static ref FIND_TAG_WITH_AID_PATTERN: Regex = Regex::new(r"(?i)(<[^>]*\s[ac]id\s*=[^>]*>)").unwrap();
    static ref WITHIN_TAG_AID_POSITION_PATTERN: Regex = Regex::new(r#"\s[ac]id\s*=['"]([^'"]*)['"]"#).unwrap();

    static ref FIND_TAG_WITH_AMZNPAGEBREAK_PATTERN: Regex = Regex::new(r"(?i)(<[^>]*\sdata-AmznPageBreak=[^>]*>)").unwrap();
    static ref WITHIN_TAG_AMZNPAGEBREAK_POSITION_PATTERN: Regex = Regex::new(r#"\sdata-AmznPageBreak=['"]([^'"]*)['"]"#).unwrap();

    static ref IMG_PATTERN: Regex = Regex::new(r"(?i)(<[img\s|image\s|svg:image\s][^>]*>)").unwrap();
    // Allow for varying quote styles in attributes
    static ref IMG_INDEX_PATTERN: Regex = Regex::new(r#"['"]kindle:embed:([0-9A-Va-v]+)[^'"]*['"]"#).unwrap();

    static ref TAG_PATTERN: Regex = Regex::new(r"(<[^>]*>)").unwrap();
    static ref FLOW_PATTERN: Regex = Regex::new(r#"['"]kindle:flow:([0-9A-Va-v]+)\?mime=([^'"]+)['"]"#).unwrap();

    static ref URL_PATTERN: Regex = Regex::new(r"(?i)(url\(.*?\))").unwrap();
    static ref URL_IMG_INDEX_PATTERN: Regex = Regex::new(r"(?i)kindle:embed:([0-9A-Va-v]+)\?mime=image/[^\)]*").unwrap();
    static ref FONT_INDEX_PATTERN: Regex = Regex::new(r"(?i)kindle:embed:([0-9A-Va-v]+)").unwrap();
    static ref URL_CSS_INDEX_PATTERN: Regex = Regex::new(r"(?i)kindle:flow:([0-9A-Va-v]+)\?mime=text/css[^\)]*").unwrap();

    static ref STYLE_PATTERN: Regex = Regex::new(r"(?i)(<[a-zA-Z0-9]+\s[^>]*style\s*=\s*[^>]*>)").unwrap();
    static ref SVG_TAG_PATTERN: Regex = Regex::new(r"(?i)(<(?:svg)[^>]*>)").unwrap();
}

pub struct FlowInfo {
    pub dir: String,
    pub fname: String,
    pub format: String,
}

pub trait MobiReaderTrait {
    fn get_id_tag_by_pos_fid(&self, pos: u32, off: u32) -> Option<(String, String)>;
    fn get_flow_info(&self, num: usize) -> Option<&FlowInfo>;
    fn get_flow(&self, num: usize) -> Option<&String>;
    fn get_header_codec(&self) -> &str;
    fn get_aid_anchor_suffix(&self) -> Option<&str>;
    fn is_aid_linked(&self, aid: &str) -> bool;
}

pub fn update_internal_links<R: MobiReaderTrait>(
    parts: &mut [String],
    reader: &R,
    log: &impl Fn(&str),
) {
    for part in parts.iter_mut() {
        *part = POSFID_PATTERN
            .replace_all(part, |caps: &Captures| {
                let tag = &caps[0];
                POSFID_INDEX_PATTERN
                    .replace(tag, |inner_caps: &Captures| {
                        let posfid_str = &inner_caps[1];
                        let offset_str = &inner_caps[2];

                        let posfid = u32::from_str_radix(posfid_str, 32).unwrap_or(0);
                        let offset = u32::from_str_radix(offset_str, 32).unwrap_or(0);

                        match reader.get_id_tag_by_pos_fid(posfid, offset) {
                            Some((filename, idtag)) => {
                                let suffix = if !idtag.is_empty() {
                                    format!("#{}", idtag)
                                } else {
                                    String::new()
                                };
                                let fname_part =
                                    filename.split('/').last().unwrap_or("").to_string();
                                let replacement =
                                    format!("{}{}", fname_part, suffix).replace("\"", "&quot;");
                                format!("\"{}\"", replacement)
                            }
                            None => {
                                log("Invalid link, points to nowhere, ignoring");
                                "\"#\"".to_string()
                            }
                        }
                    })
                    .to_string()
            })
            .to_string();
    }
}

pub fn remove_kindlegen_markup<R: MobiReaderTrait>(parts: &mut [String], reader: &R) {
    for part in parts.iter_mut() {
        // remove aid/cid
        *part = FIND_TAG_WITH_AID_PATTERN
            .replace_all(part, |caps: &Captures| {
                let tag = &caps[0];
                WITHIN_TAG_AID_POSITION_PATTERN
                    .replace(tag, |inner_caps: &Captures| {
                        let aid = &inner_caps[1];
                        if reader.is_aid_linked(aid) {
                            if let Some(suffix) = reader.get_aid_anchor_suffix() {
                                format!(" id=\"{}-{}\"", aid, suffix)
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        }
                    })
                    .to_string()
            })
            .to_string();

        // remove AmznPageBreak
        *part = FIND_TAG_WITH_AMZNPAGEBREAK_PATTERN
            .replace_all(part, |caps: &Captures| {
                let tag = &caps[0];
                WITHIN_TAG_AMZNPAGEBREAK_POSITION_PATTERN
                    .replace(tag, |inner_caps: &Captures| {
                        let val = &inner_caps[1];
                        format!(" style=\"page-break-after:{}\"", val)
                    })
                    .to_string()
            })
            .to_string();
    }
}

pub fn update_flow_links<R: MobiReaderTrait>(
    flows: &mut [Option<String>],
    resource_map: &[Option<String>],
    reader: &R,
    log: &impl Fn(&str),
) {
    for flow_opt in flows.iter_mut() {
        if let Some(flow) = flow_opt {
            let mut new_flow = flow.clone();

            // Images
            new_flow = IMG_PATTERN
                .replace_all(&new_flow, |caps: &Captures| {
                    let tag = &caps[0];
                    IMG_INDEX_PATTERN
                        .replace(tag, |inner_caps: &Captures| {
                            let num = u32::from_str_radix(&inner_caps[1], 32).unwrap_or(0);
                            if num > 0 && (num as usize) <= resource_map.len() {
                                if let Some(href) = &resource_map[num as usize - 1] {
                                    return format!("\"../{}\"", href);
                                }
                            }
                            log(&format!("Referenced image {} not recognized", num));
                            inner_caps[0].to_string()
                        })
                        .to_string()
                })
                .to_string();

            // CSS URLs
            new_flow = URL_PATTERN
                .replace_all(&new_flow, |caps: &Captures| {
                    let tag = &caps[0];

                    // Images in URL
                    let tag_img_replaced = URL_IMG_INDEX_PATTERN
                        .replace(tag, |inner_caps: &Captures| {
                            let num = u32::from_str_radix(&inner_caps[1], 32).unwrap_or(0);
                            if num > 0 && (num as usize) <= resource_map.len() {
                                if let Some(href) = &resource_map[num as usize - 1] {
                                    return format!("\"../{}\"", href);
                                }
                            }
                            inner_caps[0].to_string()
                        })
                        .to_string();

                    // Fonts in URL
                    let tag_font_replaced = FONT_INDEX_PATTERN
                        .replace(&tag_img_replaced, |inner_caps: &Captures| {
                            let num = u32::from_str_radix(&inner_caps[1], 32).unwrap_or(0);
                            if num > 0 && (num as usize) <= resource_map.len() {
                                if let Some(href) = &resource_map[num as usize - 1] {
                                    let replacement = if href.ends_with(".failed") {
                                        format!("\"failed-{}\"", href)
                                    } else {
                                        format!("\"../{}\"", href)
                                    };
                                    return replacement;
                                }
                            }
                            log(&format!("Referenced font {} not recognized", num));
                            inner_caps[0].to_string()
                        })
                        .to_string();

                    // CSS in URL
                    let tag_css_replaced = URL_CSS_INDEX_PATTERN
                        .replace(&tag_font_replaced, |inner_caps: &Captures| {
                            let num = u32::from_str_radix(&inner_caps[1], 32).unwrap_or(0);
                            if let Some(fi) = reader.get_flow_info(num as usize) {
                                return format!("\"../{}/{}\"", fi.dir, fi.fname);
                            }
                            inner_caps[0].to_string()
                        })
                        .to_string();

                    tag_css_replaced
                })
                .to_string();

            // Flow refs not inside URL
            new_flow = TAG_PATTERN
                .replace_all(&new_flow, |caps: &Captures| {
                    let tag = &caps[0];
                    FLOW_PATTERN
                        .replace(tag, |inner_caps: &Captures| {
                            let num = u32::from_str_radix(&inner_caps[1], 32).unwrap_or(0);
                            if let Some(fi) = reader.get_flow_info(num as usize) {
                                if fi.format == "inline" {
                                    if let Some(inline_flow) = reader.get_flow(num as usize) {
                                        return inline_flow.clone();
                                    }
                                } else {
                                    return format!("\"../{}/{}\"", fi.dir, fi.fname);
                                }
                            }
                            log("Ignoring invalid flow reference");
                            "".to_string()
                        })
                        .to_string()
                })
                .to_string();

            *flow = new_flow;
        }
    }
}

pub fn insert_flows_into_markup<R: MobiReaderTrait>(
    parts: &mut [String],
    flows: &[Option<String>],
    reader: &R,
    log: &impl Fn(&str),
) {
    for part in parts.iter_mut() {
        *part = TAG_PATTERN
            .replace_all(part, |caps: &Captures| {
                let tag = &caps[0];
                FLOW_PATTERN
                    .replace(tag, |inner_caps: &Captures| {
                        let num = u32::from_str_radix(&inner_caps[1], 32).unwrap_or(0);
                        if let Some(fi) = reader.get_flow_info(num as usize) {
                            if fi.format == "inline" {
                                if let Some(Some(f)) = flows.get(num as usize) {
                                    return f.clone();
                                }
                            } else {
                                return format!("\"../{}/{}\"", fi.dir, fi.fname);
                            }
                        }
                        log(&format!(
                            "Ignoring invalid flow reference: {}",
                            &inner_caps[0]
                        ));
                        "".to_string()
                    })
                    .to_string()
            })
            .to_string();
    }
}

pub fn insert_images_into_markup(
    parts: &mut [String],
    resource_map: &[Option<String>],
    log: &impl Fn(&str),
) {
    for part in parts.iter_mut() {
        // img tags
        let mut new_part = IMG_PATTERN
            .replace_all(part, |caps: &Captures| {
                let tag = &caps[0];
                IMG_INDEX_PATTERN
                    .replace(tag, |inner_caps: &Captures| {
                        let num = u32::from_str_radix(&inner_caps[1], 32).unwrap_or(0);
                        if num > 0 && (num as usize) <= resource_map.len() {
                            if let Some(href) = &resource_map[num as usize - 1] {
                                return format!("\"../{}\"", href);
                            }
                        }
                        log(&format!("Referenced image {} not recognized", num));
                        inner_caps[0].to_string()
                    })
                    .to_string()
            })
            .to_string();

        // style attributes
        new_part = STYLE_PATTERN
            .replace_all(&new_part, |caps: &Captures| {
                let tag = &caps[0];
                if tag.contains("kindle:embed") {
                    IMG_INDEX_PATTERN
                        .replace(tag, |inner_caps: &Captures| {
                            let num = u32::from_str_radix(&inner_caps[1], 32).unwrap_or(0);
                            let full_match = &inner_caps[0];
                            let osep = &full_match[0..1];
                            let csep = &full_match[full_match.len() - 1..];

                            if num > 0 && (num as usize) <= resource_map.len() {
                                if let Some(href) = &resource_map[num as usize - 1] {
                                    return format!("{}{}{}", osep, format!("../{}", href), csep);
                                }
                            }
                            log(&format!("Referenced image {} not recognized in style", num));
                            inner_caps[0].to_string()
                        })
                        .to_string()
                } else {
                    tag.to_string()
                }
            })
            .to_string();

        *part = new_part;
    }
}

pub fn upshift_markup(parts: &mut [String]) {
    for part in parts.iter_mut() {
        *part = SVG_TAG_PATTERN
            .replace_all(part, |caps: &Captures| {
                let mut tag = caps[0].to_string();
                if tag.to_lowercase().starts_with("<svg") {
                    tag = tag.replace("preserveaspectratio", "preserveAspectRatio");
                    tag = tag.replace("viewbox", "viewBox");
                }
                tag
            })
            .to_string();
    }
}

pub fn expand_mobi8_markup<R: MobiReaderTrait>(
    parts: &mut Vec<String>,
    flows: &mut Vec<Option<String>>,
    reader: &mut R,
    resource_map: &[Option<String>],
    log: &impl Fn(&str),
) -> Vec<String> {
    update_internal_links(parts, reader, log);
    remove_kindlegen_markup(parts, reader);
    update_flow_links(flows, resource_map, reader, log);
    insert_flows_into_markup(parts, flows, reader, log);
    insert_images_into_markup(parts, resource_map, log);
    upshift_markup(parts);

    // Returning spine logic (mocked for now as we don't write files here)
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct MockReader {
        pos_fid_map: HashMap<(u32, u32), (String, String)>,
        linked_aids: HashMap<String, bool>,
        flows: Vec<Option<String>>,
        flow_info: Vec<FlowInfo>,
    }

    impl MobiReaderTrait for MockReader {
        fn get_id_tag_by_pos_fid(&self, pos: u32, off: u32) -> Option<(String, String)> {
            self.pos_fid_map.get(&(pos, off)).cloned()
        }
        fn get_flow_info(&self, num: usize) -> Option<&FlowInfo> {
            self.flow_info.get(num)
        }
        fn get_flow(&self, num: usize) -> Option<&String> {
            self.flows.get(num).and_then(|x| x.as_ref())
        }
        fn get_header_codec(&self) -> &str {
            "utf-8"
        }
        fn get_aid_anchor_suffix(&self) -> Option<&str> {
            Some("suffix")
        }
        fn is_aid_linked(&self, aid: &str) -> bool {
            *self.linked_aids.get(aid).unwrap_or(&false)
        }
    }

    #[test]
    fn test_upshift_markup() {
        let mut parts = vec![String::from(
            "<svg width=\"100\" viewBox=\"0 0 100 100\" preserveaspectratio=\"none\">",
        )];
        upshift_markup(&mut parts);
        assert!(parts[0].contains("preserveAspectRatio"));
        assert!(parts[0].contains("viewBox"));
    }

    #[test]
    fn test_update_internal_links() {
        let mut map = HashMap::new();
        map.insert((1, 10), ("file.html".to_string(), "anchor".to_string()));
        let reader = MockReader {
            pos_fid_map: map,
            linked_aids: HashMap::new(),
            flows: vec![],
            flow_info: vec![],
        };

        let mut parts = vec![String::from(
            r#"<a href='kindle:pos:fid:0001:off:000A'>Link</a>"#,
        )];
        update_internal_links(&mut parts, &reader, &|_| {});
        // 0001 base32 is 1
        // 000A base32 is 10
        assert_eq!(parts[0], r#"<a href="file.html#anchor">Link</a>"#);
    }
}
