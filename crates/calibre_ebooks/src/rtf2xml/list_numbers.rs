//! Port of `old_src/src/calibre/ebooks/rtf2xml/list_numbers.py`
//! (`ListNumbers`).
//!
//! RTF puts a list item's number/bullet text (`\pntext`/`\listtext`,
//! resolved to `cw<ls<list-text_`) as a sibling of the paragraph it
//! introduces, not inside it. This pass moves that "list text" chunk
//! to just after the following paragraph's opening bracket/text,
//! wrapped in `list-text` tags, and tags whether the list looks
//! ordered (e.g. `"1."`, `"(a)"`) or unordered (a Wingdings/Symbol
//! bullet byte, or anything that doesn't parse as a bare number).
//!
//! Checkpoint `list_number_info`; runs after [`super::header`]'s
//! separate pass and before [`super::preamble_div`].

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Default,
    AfterOb,
    ListText,
    AfterListText,
}

fn token_info(line: &str) -> &str {
    if line.len() >= 16 {
        &line[..16]
    } else {
        line
    }
}

fn first_two(line: &str) -> &str {
    if line.len() >= 2 {
        &line[..2]
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

/// Port of `__determine_list_type`: `'\'B7'` (a bullet-character hex
/// byte -- Symbol font code point 0xB7) marks the list unordered
/// outright; otherwise the collected plain text, with `.`/`(`/`)`
/// stripped, is checked for being all-digits.
fn determine_list_type(chunk: &str) -> &'static str {
    let mut text = String::new();
    for line in chunk.lines() {
        if line.len() >= 5 && &line[..5] == "tx<hx" {
            // Port of `line[17:]` (no trailing `\n` here to drop).
            if line.len() > 17 && &line[17..] == "'B7" {
                return "unordered";
            }
        } else if line.len() >= 5 && &line[..5] == "tx<nu" && line.len() > 17 {
            text.push_str(&line[17..]);
        }
    }
    let text: String = text
        .chars()
        .filter(|&c| c != '.' && c != '(' && c != ')')
        .collect();
    if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) {
        "ordered"
    } else {
        "unordered"
    }
}

/// Port of `ListNumbers.fix_list_numbers`, operating directly on
/// intermediate-format content (see [`super::process_tokens`]'s module
/// docs) rather than reopening a file.
pub fn fix_list_numbers(content: &str) -> String {
    let mut stage = Stage::Default;
    let mut out = String::new();
    let mut list_chunk = String::new();
    let mut previous_line = String::new();
    let mut list_text_ob = String::new();
    let mut ob_count = String::new();
    let mut cb_count = String::new();

    for line in content.lines() {
        let info = token_info(line);
        if info == "ob<nu<open-brack" {
            ob_count = last_four(line);
        }
        if info == "cb<nu<clos-brack" {
            cb_count = last_four(line);
        }

        match stage {
            Stage::Default => {
                // Port of `__default_func`.
                if info == "ob<nu<open-brack" {
                    stage = Stage::AfterOb;
                    previous_line = line.to_string();
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            Stage::AfterOb => {
                // Port of `__after_ob_func`.
                if info == "cw<ls<list-text_" {
                    stage = Stage::ListText;
                    list_chunk.push_str(&previous_line);
                    list_chunk.push('\n');
                    list_chunk.push_str(line);
                    list_chunk.push('\n');
                    list_text_ob = ob_count.clone();
                    cb_count = "0".to_string();
                } else {
                    out.push_str(&previous_line);
                    out.push('\n');
                    out.push_str(line);
                    out.push('\n');
                    stage = Stage::Default;
                }
            }
            Stage::ListText => {
                // Port of `__list_text_func`.
                if list_text_ob == cb_count {
                    stage = Stage::AfterListText;
                    let list_type = determine_list_type(&list_chunk);
                    out.push_str(&format!("mi<mk<list-type_<{list_type}\n"));
                }
                if info != "cw<pf<par-def___" {
                    list_chunk.push_str(line);
                    list_chunk.push('\n');
                }
            }
            Stage::AfterListText => {
                // Port of `__after_list_text_func`.
                let first_two = first_two(line);
                if first_two == "ob" || first_two == "tx" {
                    stage = Stage::Default;
                    out.push_str(
                        "mi<mk<lst-txbeg_\n\
                         mi<mk<para-beg__\n\
                         mi<mk<lst-tx-beg\n\
                         mi<tg<open-att__<list-text\n",
                    );
                    out.push_str(&list_chunk);
                    out.push_str("mi<tg<close_____<list-text\nmi<mk<lst-tx-end\n");
                    list_chunk.clear();
                }
                out.push_str(line);
                out.push('\n');
            }
        }
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
    fn plain_content_passes_through_unchanged() {
        let content = lines(&["tx<nu<__________<hello"]);
        assert_eq!(fix_list_numbers(&content), content);
    }

    #[test]
    fn ordered_list_text_is_moved_into_following_paragraph() {
        let content = lines(&[
            "ob<nu<open-brack<0001",
            "cw<ls<list-text_<nu<true",
            "tx<nu<__________<1.",
            "cb<nu<clos-brack<0001",
            "tx<nu<__________<paragraph text",
        ]);
        let out = fix_list_numbers(&content);
        assert!(out.contains("mi<mk<list-type_<ordered"));
        assert!(out.contains("mi<tg<open-att__<list-text"));
        assert!(out.contains("mi<tg<close_____<list-text"));
        assert!(out.contains("mi<mk<lst-tx-end"));
        // the list-text markup appears before the paragraph text it
        // was moved into.
        let markup_pos = out.find("mi<mk<lst-txbeg_").unwrap();
        let text_pos = out.find("paragraph text").unwrap();
        assert!(markup_pos < text_pos);
    }

    #[test]
    fn bullet_hex_byte_marks_list_unordered() {
        let content = lines(&[
            "ob<nu<open-brack<0001",
            "cw<ls<list-text_<nu<true",
            "tx<hx<__________<'B7",
            "cb<nu<clos-brack<0001",
            "tx<nu<__________<paragraph text",
        ]);
        let out = fix_list_numbers(&content);
        assert!(out.contains("mi<mk<list-type_<unordered"));
    }

    #[test]
    fn non_numeric_text_marks_list_unordered() {
        let content = lines(&[
            "ob<nu<open-brack<0001",
            "cw<ls<list-text_<nu<true",
            "tx<nu<__________<*",
            "cb<nu<clos-brack<0001",
            "tx<nu<__________<paragraph text",
        ]);
        let out = fix_list_numbers(&content);
        assert!(out.contains("mi<mk<list-type_<unordered"));
    }

    #[test]
    fn parenthesized_number_is_still_ordered() {
        let content = lines(&[
            "ob<nu<open-brack<0001",
            "cw<ls<list-text_<nu<true",
            "tx<nu<__________<(3)",
            "cb<nu<clos-brack<0001",
            "tx<nu<__________<paragraph text",
        ]);
        let out = fix_list_numbers(&content);
        assert!(out.contains("mi<mk<list-type_<ordered"));
    }

    #[test]
    fn open_bracket_not_followed_by_list_text_is_untouched() {
        let content = lines(&[
            "ob<nu<open-brack<0001",
            "cw<ci<bold______<nu<true",
            "cb<nu<clos-brack<0001",
        ]);
        assert_eq!(fix_list_numbers(&content), content);
    }

    #[test]
    fn par_def_token_inside_list_text_is_excluded_from_chunk() {
        // cw<pf<par-def___ lines are dropped from the collected chunk
        // (but still counted for bracket tracking via the outer loop).
        let content = lines(&[
            "ob<nu<open-brack<0001",
            "cw<ls<list-text_<nu<true",
            "cw<pf<par-def___<nu<true",
            "tx<nu<__________<1.",
            "cb<nu<clos-brack<0001",
            "tx<nu<__________<paragraph text",
        ]);
        let out = fix_list_numbers(&content);
        assert!(!out.contains("par-def___"));
        assert!(out.contains("1."));
    }
}
