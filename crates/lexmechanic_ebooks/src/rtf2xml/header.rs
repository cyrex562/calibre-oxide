//! Port of `old_src/src/calibre/ebooks/rtf2xml/header.py` (`Header`).
//!
//! The same separate/rejoin shape as [`super::footnote`], for RTF
//! header/footer groups (`\headerf`/`\headerl`/`\headerr`/`\footerf`/
//! `\footerl`/`\footerr`/`\header`/`\footer` -- see
//! [`super::process_tokens`]'s `hf` category) instead of `\footnote`
//! groups:
//!
//! - [`separate_headers`] (checkpoint `separate_headers_info`, run as
//!   this issue's 6th pass): extracts every header/footer group's
//!   contents to the end of the content, leaving an
//!   `mi<mk<header-ind<NNNN` placeholder in the main stream.
//! - [`join_headers`] (called much later in the real pipeline, out of
//!   scope for its caller but ported here since it's the same Python
//!   file): splices each header/footer's content back in at its
//!   placeholder.
//!
//! `found_a_header` threads between the two the same way
//! [`super::footnote`]'s `found_a_footnote` does.

/// Result of [`separate_headers`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeparateHeadersOutput {
    pub content: String,
    /// Port of `self.__found_a_header`.
    pub found_a_header: bool,
}

fn token_info(line: &str) -> &str {
    if line.len() >= 16 {
        &line[..16]
    } else {
        line
    }
}

fn last_four(line: &str) -> String {
    if line.len() >= 4 {
        line[line.len() - 4..].to_string()
    } else {
        line.to_string()
    }
}

/// Port of `__head_dict`: maps a `hf`-category label (the last 10
/// characters of `cw<hf<{label}`) to the `type` attribute value written
/// on the extracted `header-or-footer` element.
fn head_dict(label: &str) -> Option<&'static str> {
    Some(match label {
        "head-left_" => "header-left",
        "head-right" => "header-right",
        "foot-left_" => "footer-left",
        "foot-right" => "footer-right",
        "head-first" => "header-first",
        "foot-first" => "footer-first",
        "header____" => "header",
        "footer____" => "footer",
        _ => return None,
    })
}

/// Port of `Header.separate_headers`, operating directly on
/// intermediate-format content (see [`super::process_tokens`]'s module
/// docs) rather than reopening files.
pub fn separate_headers(content: &str) -> SeparateHeadersOutput {
    let mut body = String::new();
    let mut head = String::new();
    let mut in_header = false;
    let mut header_count: u32 = 0;
    let mut header_bracket_count = String::new();
    let mut ob_count = String::new();
    let mut cb_count = String::new();
    let mut found_a_header = false;

    for line in content.lines() {
        let info = token_info(line);
        if info == "ob<nu<open-brack" {
            ob_count = last_four(line);
        }
        if info == "cb<nu<clos-brack" {
            cb_count = last_four(line);
        }

        if in_header {
            // Port of `__in_header_func`.
            if cb_count == header_bracket_count {
                in_header = false;
                body.push_str(line);
                body.push('\n');
                head.push_str(
                    "mi<mk<head___clo\n\
                     mi<tg<close_____<header-or-footer\n\
                     mi<mk<header-clo\n",
                );
            } else {
                head.push_str(line);
                head.push('\n');
            }
        } else {
            // Port of `__default_sep`.
            if info.len() >= 5 && &info[3..5] == "hf" {
                // Port of `__found_header`.
                found_a_header = true;
                in_header = true;
                header_count += 1;
                cb_count = "0".to_string();
                header_bracket_count = ob_count.clone();
                body.push_str(&format!("mi<mk<header-ind<{header_count:04}\n"));
                head.push_str(&format!("mi<mk<header-ope<{header_count:04}\n"));
                // Port of `info = line[6:16]`.
                let sub_label = if line.len() >= 16 { &line[6..16] } else { "" };
                match head_dict(sub_label) {
                    Some(kind) => {
                        head.push_str(&format!("mi<tg<open-att__<header-or-footer<type>{kind}\n"));
                    }
                    None => {
                        // Port of the Python's `sys.stderr.write(...)`
                        // diagnostic in the `else` branch.
                        eprintln!(
                            "module is header\nmethod is __found_header\nno dict entry\nline is {line}"
                        );
                        head.push_str("mi<tg<open-att__<header-or-footer<type>none\n");
                    }
                }
            }
            body.push_str(line);
            body.push('\n');
        }
    }

    body.push_str("mi<mk<header-beg\n");
    body.push_str(&head);
    body.push_str("mi<mk<header-end\n");

    SeparateHeadersOutput {
        content: body,
        found_a_header,
    }
}

/// Port of `Header.join_headers`. `found_a_header` is
/// [`SeparateHeadersOutput::found_a_header`] from the matching
/// [`separate_headers`] call -- if `false`, this is a no-op (matching
/// the Python's early `return`).
pub fn join_headers(content: &str, found_a_header: bool) -> String {
    if !found_a_header {
        return content.to_string();
    }

    // Port of `__get_headers`.
    let mut body_no_head = String::new();
    let mut head_payload = String::new();
    let mut in_head = false;
    for line in content.lines() {
        let info = token_info(line);
        if in_head {
            if info == "mi<mk<header-end" {
                in_head = false;
            } else {
                head_payload.push_str(line);
                head_payload.push('\n');
            }
        } else if info == "mi<mk<header-beg" {
            in_head = true;
        } else {
            body_no_head.push_str(line);
            body_no_head.push('\n');
        }
    }

    // Port of `__get_head_from_temp`.
    let get_head = |num: &str| -> Option<String> {
        let look_for = format!("mi<mk<header-ope<{num}");
        let mut found = false;
        let mut collected = String::new();
        for line in head_payload.lines() {
            if found {
                if line == "mi<mk<header-clo" {
                    return Some(collected);
                }
                collected.push_str(line);
                collected.push('\n');
            } else if line == look_for {
                found = true;
            }
        }
        None
    };

    // Port of `__join_from_temp`.
    let mut out = String::new();
    for line in body_no_head.lines() {
        if token_info(line) == "mi<mk<header-ind" {
            let num = if line.len() > 17 { &line[17..] } else { "" };
            if let Some(header_text) = get_head(num) {
                out.push_str(&header_text);
                continue;
            }
            // See `join_footnotes`'s equivalent note: unreachable for
            // any well-formed output of `separate_headers`.
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> String {
        v.join("\n") + "\n"
    }

    #[test]
    fn no_headers_still_appends_empty_trailer() {
        let content = "tx<nu<__________<hello\n";
        let out = separate_headers(content);
        assert!(!out.found_a_header);
        assert!(out.content.starts_with("tx<nu<__________<hello\n"));
        assert!(out.content.contains("mi<mk<header-beg\nmi<mk<header-end\n"));
    }

    #[test]
    fn header_group_is_extracted_with_type_attribute() {
        let content = lines(&[
            "ob<nu<open-brack<0001",
            "cw<hf<header____<nu<true",
            "tx<nu<__________<page 1",
            "cb<nu<clos-brack<0001",
        ]);
        let out = separate_headers(&content);
        assert!(out.found_a_header);
        assert!(out.content.contains("mi<mk<header-ind<0001"));
        assert!(out.content.contains("mi<mk<header-ope<0001"));
        assert!(out
            .content
            .contains("mi<tg<open-att__<header-or-footer<type>header"));
        assert!(out.content.contains("page 1"));
        assert!(out.content.contains("mi<tg<close_____<header-or-footer"));
    }

    #[test]
    fn footer_left_variant_maps_to_footer_left_type() {
        let content = lines(&[
            "ob<nu<open-brack<0001",
            "cw<hf<foot-left_<nu<true",
            "cb<nu<clos-brack<0001",
        ]);
        let out = separate_headers(&content);
        assert!(out
            .content
            .contains("mi<tg<open-att__<header-or-footer<type>footer-left"));
    }

    #[test]
    fn join_headers_is_a_no_op_when_none_were_found() {
        let content = "tx<nu<__________<unchanged\n";
        assert_eq!(join_headers(content, false), content);
    }

    #[test]
    fn round_trip_separate_then_join_restores_original_shape() {
        let content = lines(&[
            "tx<nu<__________<before",
            "ob<nu<open-brack<0001",
            "cw<hf<header____<nu<true",
            "tx<nu<__________<header text",
            "cb<nu<clos-brack<0001",
            "tx<nu<__________<after",
        ]);
        let sep = separate_headers(&content);
        assert!(sep.found_a_header);
        let joined = join_headers(&sep.content, sep.found_a_header);
        assert!(joined.contains("header text"));
        assert!(!joined.contains("header-ind"));
        assert!(!joined.contains("header-beg"));
        assert!(!joined.contains("header-end"));
        assert!(joined.contains("before"));
        assert!(joined.contains("after"));
    }

    #[test]
    fn multiple_headers_are_joined_back_at_their_own_markers() {
        let content = lines(&[
            "ob<nu<open-brack<0001",
            "cw<hf<header____<nu<true",
            "tx<nu<__________<first",
            "cb<nu<clos-brack<0001",
            "tx<nu<__________<middle",
            "ob<nu<open-brack<0002",
            "cw<hf<footer____<nu<true",
            "tx<nu<__________<second",
            "cb<nu<clos-brack<0002",
        ]);
        let sep = separate_headers(&content);
        let joined = join_headers(&sep.content, sep.found_a_header);
        let first_pos = joined.find("first").unwrap();
        let middle_pos = joined.find("middle").unwrap();
        let second_pos = joined.find("second").unwrap();
        assert!(first_pos < middle_pos);
        assert!(middle_pos < second_pos);
    }
}
