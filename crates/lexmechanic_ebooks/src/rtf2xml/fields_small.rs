//! Port of `old_src/src/calibre/ebooks/rtf2xml/fields_small.py`
//! (`FieldsSmall`).
//!
//! Wraps bookmark, index-entry, and toc-entry field markers found in
//! the body with `mi<mk<inline-fld` field-instruction tags. Doesn't
//! handle index/toc *tables* -- those are out of scope here (see the
//! `table*.py` follow-up files).
//!
//! `field_strings.FieldStrings` is imported and instantiated in the
//! Python (`self.__string_obj`) but its `process_string` method is
//! never actually called -- the real call site is commented out
//! (`# change here`), replaced by this file's own `parse_bookmark` /
//! `parse_index` / `parse_toc` family. `field_strings.py` is therefore
//! NOT a real runtime dependency of this port.
//!
//! Operates directly on intermediate-format content (see
//! [`super::process_tokens`]'s module docs) rather than reopening
//! files -- the temp-file / [`super::copy`] / rename dance around the
//! real pass is pipeline plumbing, not ported here.
//!
//! One small, deliberate deviation from a literal port: Python
//! accumulates a toc/index entry's lines by string-concatenating each
//! raw `\n`-terminated line, then re-splitting on `\n` inside the
//! `__parse_index_func` / `__parse_toc_func` family to get individual
//! lines back out. This port skips that needless join-then-resplit
//! round trip and accumulates a `Vec<String>` of lines directly --
//! same lines, same order, same content; only the intermediate
//! representation differs. The one behavioral wrinkle this drops is a
//! harmless trailing empty-string element Python's own
//! `"a\nb\n".split('\n')` produces (`["a", "b", ""]`), which never
//! matches any token check there either -- it's a no-op except for
//! very occasionally appending one extra blank line into an attribute
//! value nobody reads structurally.

fn token_info(line: &str) -> &str {
    if line.len() >= 16 { &line[..16] } else { line }
}

fn last_four(line: &str) -> String {
    if line.len() >= 4 { line[line.len() - 4..].to_string() } else { line.to_string() }
}

fn tx_payload(line: &str) -> &str {
    if line.len() >= 17 { &line[17..] } else { "" }
}

const MARKER: &str = "mi<mk<inline-fld\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    BeforeBody,
    Body,
    Bookmark,
    TocIndex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BookmarkTag {
    Start,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TocIndexTag {
    Toc,
    Index,
}

/// Port of `__parse_bookmark_func`.
fn parse_bookmark(text: &str, type_: &str) -> String {
    format!("mi<tg<empty-att_<field<type>{type_}<number>{text}<update>none\n")
}

/// Port of `__index_see_func`: pulls `\v ... \v0`-style "see"
/// cross-reference text out of `lines` into its own string, dropping
/// every line from the first `cw<in<index-see_` marker up to (and
/// including) its matching close bracket except any `tx` payload
/// found inside.
fn index_see(lines: &[String]) -> (Vec<String>, String) {
    let mut in_see = false;
    let mut bracket_count: i64 = 0;
    let mut end_bracket_count: i64 = i64::MAX;
    let mut see_string = String::new();
    let mut changed = Vec::new();
    for line in lines {
        let tok = token_info(line);
        if tok == "ob<nu<open-brack" {
            bracket_count += 1;
        }
        if tok == "cb<nu<clos-brack" {
            bracket_count -= 1;
        }
        if in_see {
            if bracket_count == end_bracket_count && tok == "cb<nu<clos-brack" {
                in_see = false;
            } else if tok == "tx<nu<__________" {
                see_string.push_str(tx_payload(line));
            }
        } else {
            if tok == "cw<in<index-see_" {
                end_bracket_count = bracket_count - 1;
                in_see = true;
            }
            changed.push(line.clone());
        }
    }
    (changed, see_string)
}

/// Port of `__index_bookmark_func`: pulls the text inside a
/// `cw<an<place_____` bookmark group out into its own string. Unlike
/// [`index_see`], ordinary lines inside the group (and the group's own
/// closing bracket) are kept in `lines` rather than dropped.
fn index_bookmark(lines: &[String]) -> (Vec<String>, String) {
    let mut in_bookmark = false;
    let mut bracket_count: i64 = 0;
    let mut end_bracket_count: i64 = i64::MAX;
    let mut bookmark_string = String::new();
    let mut kept = Vec::new();
    for line in lines {
        let tok = token_info(line);
        if tok == "ob<nu<open-brack" {
            bracket_count += 1;
        }
        if tok == "cb<nu<clos-brack" {
            bracket_count -= 1;
        }
        if in_bookmark {
            if bracket_count == end_bracket_count && tok == "cb<nu<clos-brack" {
                in_bookmark = false;
                kept.push(line.clone());
            } else if tok == "tx<nu<__________" {
                bookmark_string.push_str(tx_payload(line));
            } else {
                kept.push(line.clone());
            }
        } else {
            if tok == "cw<an<place_____" {
                end_bracket_count = bracket_count - 1;
                in_bookmark = true;
            }
            kept.push(line.clone());
        }
    }
    (kept, bookmark_string)
}

/// Port of `__index__format_func`.
fn index_format(lines: &[String]) -> (bool, bool) {
    let mut italics = false;
    let mut bold = false;
    for line in lines {
        let tok = token_info(line);
        if tok == "cw<in<index-bold" {
            bold = true;
        }
        if tok == "cw<in<index-ital" {
            italics = true;
        }
    }
    (italics, bold)
}

/// Port of `__parse_index_func`. Runs [`index_see`] then
/// [`index_bookmark`] in sequence (each stage's output feeds the
/// next), matching the Python's own chained reassignment of
/// `my_string`.
fn parse_index(lines: &[String]) -> String {
    let (lines, see_string) = index_see(lines);
    let (lines, bookmark_string) = index_bookmark(&lines);
    let (italics, bold) = index_format(&lines);

    let mut out = String::from("mi<tg<empty-att_<field<type>index-entry<update>static");
    if !see_string.is_empty() {
        out.push_str(&format!("<additional-text>{see_string}"));
    }
    if !bookmark_string.is_empty() {
        out.push_str(&format!("<bookmark>{bookmark_string}"));
    }
    if italics {
        out.push_str("<italics>true");
    }
    if bold {
        out.push_str("<bold>true");
    }

    let mut found_sub = false;
    let mut main_entry = String::new();
    let mut sub_entry = String::new();
    for line in &lines {
        let tok = token_info(line);
        if tok == "cw<ml<colon_____" {
            found_sub = true;
        } else if line.len() >= 2 && &line[..2] == "tx" {
            if found_sub {
                sub_entry.push_str(tx_payload(line));
            } else {
                main_entry.push_str(tx_payload(line));
            }
        }
    }
    out.push_str(&format!("<main-entry>{main_entry}"));
    if found_sub {
        out.push_str(&format!("<sub-entry>{sub_entry}"));
    }
    out.push('\n');
    out
}

/// Port of `__parse_bookmark_for_toc`: same shape as
/// [`index_bookmark`], but tracks a `cw<an<book-mk-st`/`book-mk-en`
/// pair into two separate accumulators instead of one.
fn parse_bookmark_for_toc(lines: &[String]) -> (Vec<String>, String, String) {
    let mut in_bookmark = false;
    let mut bracket_count: i64 = 0;
    let mut end_bracket_count: i64 = i64::MAX;
    let mut book_start = String::new();
    let mut book_end = String::new();
    let mut book_type: Option<BookmarkTag> = None;
    let mut kept = Vec::new();
    for line in lines {
        let tok = token_info(line);
        if tok == "ob<nu<open-brack" {
            bracket_count += 1;
        }
        if tok == "cb<nu<clos-brack" {
            bracket_count -= 1;
        }
        if in_bookmark {
            if bracket_count == end_bracket_count && tok == "cb<nu<clos-brack" {
                in_bookmark = false;
                kept.push(line.clone());
            } else if tok == "tx<nu<__________" {
                match book_type {
                    Some(BookmarkTag::Start) => book_start.push_str(tx_payload(line)),
                    Some(BookmarkTag::End) => book_end.push_str(tx_payload(line)),
                    None => {}
                }
            } else {
                kept.push(line.clone());
            }
        } else {
            if tok == "cw<an<book-mk-st" || tok == "cw<an<book-mk-en" {
                book_type = Some(if tok == "cw<an<book-mk-st" {
                    BookmarkTag::Start
                } else {
                    BookmarkTag::End
                });
                end_bracket_count = bracket_count - 1;
                in_bookmark = true;
            }
            kept.push(line.clone());
        }
    }
    (kept, book_start, book_end)
}

/// Port of `__parse_toc_func`.
fn parse_toc(lines: &[String]) -> String {
    let (lines, book_start, book_end) = parse_bookmark_for_toc(lines);

    let mut out = String::from("mi<tg<empty-att_<field<type>toc-entry<update>static");
    if !book_start.is_empty() {
        out.push_str(&format!("<bookmark-start>{book_start}"));
    }
    if !book_end.is_empty() {
        out.push_str(&format!("<bookmark-end>{book_end}"));
    }

    let mut main_entry = String::new();
    let mut toc_level = String::new();
    let mut toc_suppress = false;
    for line in &lines {
        let tok = token_info(line);
        if line.len() >= 2 && &line[..2] == "tx" {
            main_entry.push_str(tx_payload(line));
        }
        if tok == "cw<tc<toc-level_" {
            toc_level = if line.len() >= 20 { line[20..].to_string() } else { String::new() };
        }
        if tok == "cw<tc<toc-sup-nu" {
            toc_suppress = true;
        }
    }
    if !toc_level.is_empty() {
        out.push_str(&format!("<toc-level>{toc_level}"));
    }
    if toc_suppress {
        out.push_str("<toc-suppress-number>true");
    }
    out.push_str(&format!("<main-entry>{main_entry}"));
    out.push('\n');
    out
}

/// Port of `FieldsSmall.fix_fields`, operating directly on
/// intermediate-format content (see this module's own docs) rather
/// than reopening a file.
pub fn fix_fields(content: &str) -> String {
    let mut state = State::BeforeBody;
    let mut out = String::new();

    let mut ob_count = String::new();
    let mut cb_count = String::new();
    let mut beg_bracket_count = String::new();

    let mut bookmark_tag: Option<BookmarkTag> = None;
    let mut bookmark_text = String::new();

    let mut toc_index_tag: Option<TocIndexTag> = None;
    let mut toc_index_lines: Vec<String> = Vec::new();

    for line in content.lines() {
        let tok = token_info(line);
        if tok == "ob<nu<open-brack" {
            ob_count = last_four(line);
        }
        if tok == "cb<nu<clos-brack" {
            cb_count = last_four(line);
        }

        match state {
            State::BeforeBody => {
                if tok == "mi<mk<body-open_" {
                    state = State::Body;
                }
                out.push_str(line);
                out.push('\n');
            }
            State::Body => match tok {
                "cw<an<book-mk-st" => {
                    beg_bracket_count = ob_count.clone();
                    cb_count.clear();
                    bookmark_tag = Some(BookmarkTag::Start);
                    state = State::Bookmark;
                }
                "cw<an<book-mk-en" => {
                    beg_bracket_count = ob_count.clone();
                    cb_count.clear();
                    bookmark_tag = Some(BookmarkTag::End);
                    state = State::Bookmark;
                }
                "cw<an<toc_______" => {
                    beg_bracket_count = ob_count.clone();
                    cb_count.clear();
                    toc_index_tag = Some(TocIndexTag::Toc);
                    state = State::TocIndex;
                }
                "cw<an<index-mark" => {
                    beg_bracket_count = ob_count.clone();
                    cb_count.clear();
                    toc_index_tag = Some(TocIndexTag::Index);
                    state = State::TocIndex;
                }
                _ => {
                    out.push_str(line);
                    out.push('\n');
                }
            },
            State::Bookmark => {
                if beg_bracket_count == cb_count {
                    state = State::Body;
                    let tag = match bookmark_tag.take() {
                        Some(BookmarkTag::Start) => "start",
                        Some(BookmarkTag::End) => "end",
                        None => "",
                    };
                    let type_str = format!("bookmark-{tag}");
                    let my_string = parse_bookmark(&bookmark_text, &type_str);
                    out.push_str(MARKER);
                    out.push_str(&my_string);
                    bookmark_text.clear();
                    out.push_str(line);
                    out.push('\n');
                } else if line.len() >= 2 && &line[..2] == "tx" {
                    bookmark_text.push_str(tx_payload(line));
                }
            }
            State::TocIndex => {
                if beg_bracket_count == cb_count {
                    state = State::Body;
                    let my_string = match toc_index_tag.take() {
                        Some(TocIndexTag::Index) => parse_index(&toc_index_lines),
                        Some(TocIndexTag::Toc) => parse_toc(&toc_index_lines),
                        None => String::new(),
                    };
                    out.push_str(MARKER);
                    out.push_str(&my_string);
                    toc_index_lines.clear();
                    out.push_str(line);
                    out.push('\n');
                } else {
                    toc_index_lines.push(line.to_string());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap_body(inner: &str) -> String {
        format!("mi<mk<body-open_\n{inner}")
    }

    #[test]
    fn plain_body_lines_pass_through_unchanged() {
        let content = wrap_body("tx<nu<__________<hello world\n");
        assert_eq!(fix_fields(&content), content);
    }

    #[test]
    fn content_before_the_body_is_passed_through_unchanged() {
        let content = "ig<nu<__________<preamble\nmi<mk<body-open_\n";
        assert_eq!(fix_fields(content), content);
    }

    #[test]
    fn wraps_a_bookmark_start_with_a_field_marker() {
        let content = wrap_body(
            "ob<nu<open-brack<0001\n\
             cw<an<book-mk-st<nu<true\n\
             tx<nu<__________<my-bookmark\n\
             cb<nu<clos-brack<0001\n",
        );
        let out = fix_fields(&content);
        assert!(out.contains(MARKER), "{out}");
        assert!(
            out.contains(
                "mi<tg<empty-att_<field<type>bookmark-start<number>my-bookmark<update>none\n"
            ),
            "{out}"
        );
        // The book-mk-st line itself is consumed, not written through.
        assert!(!out.contains("cw<an<book-mk-st"), "{out}");
        // The closing bracket line IS written through.
        assert!(out.contains("cb<nu<clos-brack<0001\n"), "{out}");
    }

    #[test]
    fn wraps_a_bookmark_end() {
        let content = wrap_body(
            "ob<nu<open-brack<0002\n\
             cw<an<book-mk-en<nu<true\n\
             tx<nu<__________<my-bookmark\n\
             cb<nu<clos-brack<0002\n",
        );
        let out = fix_fields(&content);
        assert!(
            out.contains(
                "mi<tg<empty-att_<field<type>bookmark-end<number>my-bookmark<update>none\n"
            ),
            "{out}"
        );
    }

    #[test]
    fn index_entry_collects_see_bookmark_and_format_and_splits_on_colon() {
        let content = wrap_body(
            "ob<nu<open-brack<0001\n\
             cw<an<index-mark<nu<true\n\
             cw<in<index-bold<nu<true\n\
             tx<nu<__________<main\n\
             cw<ml<colon_____<nu<true\n\
             tx<nu<__________<sub\n\
             cb<nu<clos-brack<0001\n",
        );
        let out = fix_fields(&content);
        assert!(out.contains("<bold>true"), "{out}");
        assert!(out.contains("<main-entry>main"), "{out}");
        assert!(out.contains("<sub-entry>sub"), "{out}");
    }

    #[test]
    fn toc_entry_collects_bookmarks_level_and_suppress() {
        let content = wrap_body(
            "ob<nu<open-brack<0001\n\
             cw<an<toc_______<nu<true\n\
             ob<nu<open-brack<0002\n\
             cw<an<book-mk-st<nu<true\n\
             tx<nu<__________<start-mark\n\
             cb<nu<clos-brack<0002\n\
             cw<tc<toc-level_<nu<2\n\
             cw<tc<toc-sup-nu<nu<true\n\
             tx<nu<__________<Chapter One\n\
             cb<nu<clos-brack<0001\n",
        );
        let out = fix_fields(&content);
        assert!(out.contains("<bookmark-start>start-mark"), "{out}");
        assert!(out.contains("<toc-level>2"), "{out}");
        assert!(out.contains("<toc-suppress-number>true"), "{out}");
        assert!(out.contains("<main-entry>Chapter One"), "{out}");
    }
}
