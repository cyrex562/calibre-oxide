//! Port of `old_src/src/calibre/ebooks/rtf2xml/info.py` (`Info`).
//!
//! Converts the RTF `\info` document-info group's already-resolved
//! control words (`cw<di<title_____`, `cw<di<create-tim`, etc. -- see
//! [`super::process_tokens`]'s `di` category) into `mi<tg<...` tag
//! markup: some fields (title, author, ...) collect their following
//! text run and become `<title>...</title>`-shaped open/text/close tag
//! triples; others (creation time, revision time, ...) collect a run
//! of sub-tokens (year/month/day/...) into one `empty-att_` element's
//! attributes; a few single-value fields (word count, page count, ...)
//! become a single `empty-att_` element directly from their own value.
//!
//! # Relationship to `delete_info`
//!
//! Despite the thematic overlap (both this module and
//! [`super::delete_info`] touch document-info-shaped content), the real
//! `ParseRtf.py` pipeline does *not* run them back to back: checkpoint
//! `delete_data_info` (this issue's very first pass) is ~11 passes
//! before checkpoint `styles_info`, which is immediately before *this*
//! module's own checkpoint. `delete_info` is a generic pass over MS
//! Word's optional-destination-group convention (`{\*\keyword ...}`)
//! that happens to allow `cw<di<company___` through as one of its
//! several allow-listed keywords; it has no other special-cased
//! knowledge of the `\info` group specifically, and this module has no
//! dependency on it. They are two separate, independently-scheduled
//! passes that both happen to touch overlapping token categories, not
//! a tightly coupled pair -- verified directly against `ParseRtf.py`'s
//! actual call order (see this crate's `rtf2xml` module doc for the
//! full 18-pass sequence).

use thiserror::Error;

/// Port of the `run_level > 3` gated `raise self.__bug_handler(msg)` in
/// `__collect_tokens_func`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InfoError {
    /// Port of `f'No dictionary match for {att}\n'`.
    #[error("No dictionary match for {0}\n")]
    NoDictionaryMatch(String),
}

pub type Result<T> = std::result::Result<T, InfoError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    BeforeInfoTable,
    InInfoTable,
    CollectText,
    CollectTokens,
    AfterInfoTable,
}

/// Which of the three field shapes a `cw<di<...>` info-table token maps
/// to -- port of `__info_table_dict`'s per-entry function reference.
#[derive(Debug, Clone, Copy)]
enum FieldKind {
    /// `__found_tag_with_text_func`: collect a following text run into
    /// an open/text/close tag triple.
    Text,
    /// `__found_tag_with_tokens_func`: collect a following run of
    /// `cw<di<...>` sub-tokens into one `empty-att_` element's
    /// attributes.
    Tokens,
    /// `__single_field_func`: this one token's own value becomes a
    /// single `empty-att_` element directly.
    Single,
}

/// Port of `__info_table_dict`: `(FieldKind, output tag name)` per
/// recognized `cw<di<...>` token.
fn info_table_dict(label: &str) -> Option<(FieldKind, &'static str)> {
    Some(match label {
        "title_____" => (FieldKind::Text, "title"),
        "author____" => (FieldKind::Text, "author"),
        "operator__" => (FieldKind::Text, "operator"),
        "manager___" => (FieldKind::Text, "manager"),
        "company___" => (FieldKind::Text, "company"),
        "keywords__" => (FieldKind::Text, "keywords"),
        "category__" => (FieldKind::Text, "category"),
        "doc-notes_" => (FieldKind::Text, "doc-notes"),
        "subject___" => (FieldKind::Text, "subject"),
        "linkbase__" => (FieldKind::Text, "hyperlink-base"),

        "create-tim" => (FieldKind::Tokens, "creation-time"),
        "revis-time" => (FieldKind::Tokens, "revision-time"),
        "print-time" => (FieldKind::Tokens, "printing-time"),
        "backuptime" => (FieldKind::Tokens, "backup-time"),

        "num-of-wor" => (FieldKind::Single, "number-of-words"),
        "num-of-chr" => (FieldKind::Single, "number-of-characters"),
        "numofchrws" => (FieldKind::Single, "number-of-characters-without-space"),
        "num-of-pag" => (FieldKind::Single, "number-of-pages"),
        "version___" => (FieldKind::Single, "version"),
        "edit-time_" => (FieldKind::Single, "editing-time"),
        "intern-ver" => (FieldKind::Single, "internal-version-number"),
        "internalID" => (FieldKind::Single, "internal-id-number"),
        _ => return None,
    })
}

/// Port of `__token_dict`: sub-token labels valid inside a
/// [`FieldKind::Tokens`] field's collection run.
fn token_dict(label: &str) -> Option<&'static str> {
    Some(match label {
        "year______" => "year",
        "month_____" => "month",
        "day_______" => "day",
        "minute____" => "minute",
        "second____" => "second",
        "revis-time" => "revision-time",
        "create-tim" => "creation-time",
        "edit-time_" => "editing-time",
        "print-time" => "printing-time",
        "backuptime" => "backup-time",
        "num-of-wor" => "number-of-words",
        "num-of-chr" => "number-of-characters",
        "numofchrws" => "number-of-characters-without-space",
        "num-of-pag" => "number-of-pages",
        "version___" => "version",
        "intern-ver" => "internal-version-number",
        "internalID" => "internal-id-number",
        _ => return None,
    })
}

fn token_info(line: &str) -> &str {
    if line.len() >= 16 {
        &line[..16]
    } else {
        line
    }
}

/// Port of `line[20:-1]` (Python line still carries its trailing `\n`)
/// applied to a `str::lines()` line (already stripped): the value field
/// after the fourth `<`-delimiter.
fn value_field(line: &str) -> &str {
    if line.len() > 20 {
        &line[20..]
    } else {
        ""
    }
}

/// Port of `Info.fix_info`, operating directly on intermediate-format
/// content (see [`super::process_tokens`]'s module docs) rather than
/// reopening a file.
pub fn fix_info(content: &str) -> Result<String> {
    fix_info_with_run_level(content, 1)
}

/// [`fix_info`] with an explicit `run_level`, matching the Python
/// constructor's `run_level` parameter (default `1`).
pub fn fix_info_with_run_level(content: &str, run_level: u32) -> Result<String> {
    let mut stage = Stage::BeforeInfoTable;
    let mut out = String::new();
    let mut tag = String::new();
    let mut text_string = String::new();
    // Port of `self.rmspace = re.compile(r'\s+')`, used only to test
    // for "is this string non-empty once whitespace is removed" -- no
    // regex crate needed for that check.
    let is_all_whitespace = |s: &str| s.chars().all(char::is_whitespace);

    for line in content.lines() {
        let info = token_info(line);
        match stage {
            Stage::BeforeInfoTable => {
                if info == "mi<mk<doc-in-beg" {
                    stage = Stage::InInfoTable;
                }
                out.push_str(line);
                out.push('\n');
            }
            Stage::InInfoTable => {
                if info == "mi<mk<doc-in-end" {
                    // Preserved upstream quirk: `__in_info_table_func`
                    // only flips the state to `'after_info_table'`
                    // here -- it never calls `self.__write_obj.write`
                    // for this branch, so the `mi<mk<doc-in-end`
                    // marker line itself is silently dropped from the
                    // output, unlike `mi<mk<doc-in-beg` (written
                    // unconditionally by `__before_info_table_func`).
                    // Verified directly against the Python source
                    // (`info.py` lines ~136-137) and demonstrated in
                    // `doc_in_end_marker_itself_is_dropped` below.
                    stage = Stage::AfterInfoTable;
                } else if line.len() >= 16 {
                    let label = &line[6..16];
                    if let Some((kind, tag_name)) = info_table_dict(label) {
                        match kind {
                            FieldKind::Text => {
                                tag = tag_name.to_string();
                                stage = Stage::CollectText;
                            }
                            FieldKind::Tokens => {
                                text_string = format!("mi<tg<empty-att_<{tag_name}");
                                stage = Stage::CollectTokens;
                            }
                            FieldKind::Single => {
                                let value = value_field(line);
                                out.push_str(&format!(
                                    "mi<tg<empty-att_<{tag_name}<{tag_name}>{value}\n"
                                ));
                            }
                        }
                    } else {
                        out.push_str(line);
                        out.push('\n');
                    }
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            Stage::CollectText => {
                if info == "mi<mk<docinf-end" {
                    stage = Stage::InInfoTable;
                    if !text_string.is_empty() && !is_all_whitespace(&text_string) {
                        out.push_str(&format!(
                            "mi<tg<open______<{tag}\ntx<nu<__________<{text_string}\nmi<tg<close_____<{tag}\n"
                        ));
                    }
                    text_string.clear();
                } else if line.len() >= 2 && &line[..2] == "tx" {
                    // Port of `line[17:-1]`: field value after the
                    // third `<`, trailing `\n` already stripped here.
                    if line.len() > 17 {
                        text_string.push_str(&line[17..]);
                    }
                }
            }
            Stage::CollectTokens => {
                if info == "mi<mk<docinf-end" {
                    stage = Stage::InInfoTable;
                    out.push_str(&text_string);
                    out.push('\n');
                    text_string.clear();
                } else if line.len() >= 16 {
                    let att = &line[6..16];
                    let value = value_field(line);
                    match token_dict(att) {
                        Some(changed) => {
                            text_string.push_str(&format!("<{changed}>{value}"));
                        }
                        None => {
                            if run_level > 3 {
                                return Err(InfoError::NoDictionaryMatch(att.to_string()));
                            }
                        }
                    }
                }
            }
            Stage::AfterInfoTable => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> String {
        v.join("\n") + "\n"
    }

    #[test]
    fn lines_outside_info_table_pass_through() {
        let content = "tx<nu<__________<hello\n";
        assert_eq!(fix_info(content).unwrap(), content);
    }

    #[test]
    fn text_field_collects_and_wraps_in_tags() {
        let content = lines(&[
            "mi<mk<doc-in-beg",
            "cw<di<title_____<nu<true",
            "tx<nu<__________<My Book",
            "mi<mk<docinf-end",
            "mi<mk<doc-in-end",
        ]);
        let out = fix_info(&content).unwrap();
        assert_eq!(
            out,
            lines(&[
                "mi<mk<doc-in-beg",
                "mi<tg<open______<title",
                "tx<nu<__________<My Book",
                "mi<tg<close_____<title",
            ])
        );
    }

    #[test]
    fn empty_text_field_is_dropped() {
        let content = lines(&[
            "mi<mk<doc-in-beg",
            "cw<di<author____<nu<true",
            "mi<mk<docinf-end",
            "mi<mk<doc-in-end",
        ]);
        let out = fix_info(&content).unwrap();
        assert_eq!(out, lines(&["mi<mk<doc-in-beg"]));
    }

    #[test]
    fn whitespace_only_text_field_is_dropped() {
        let content = lines(&[
            "mi<mk<doc-in-beg",
            "cw<di<author____<nu<true",
            "tx<nu<__________<   ",
            "mi<mk<docinf-end",
            "mi<mk<doc-in-end",
        ]);
        let out = fix_info(&content).unwrap();
        assert_eq!(out, lines(&["mi<mk<doc-in-beg"]));
    }

    #[test]
    fn tokens_field_collects_sub_tokens_into_one_empty_att_element() {
        let content = lines(&[
            "mi<mk<doc-in-beg",
            "cw<di<create-tim<nu<true",
            "cw<di<year______<nu<2003",
            "cw<di<month_____<nu<4",
            "cw<di<day_______<nu<1",
            "mi<mk<docinf-end",
            "mi<mk<doc-in-end",
        ]);
        let out = fix_info(&content).unwrap();
        assert_eq!(
            out,
            lines(&[
                "mi<mk<doc-in-beg",
                "mi<tg<empty-att_<creation-time<year>2003<month>4<day>1",
            ])
        );
    }

    #[test]
    fn single_field_becomes_one_empty_att_element_directly() {
        let content = lines(&[
            "mi<mk<doc-in-beg",
            "cw<di<num-of-wor<nu<250",
            "mi<mk<doc-in-end",
        ]);
        let out = fix_info(&content).unwrap();
        assert_eq!(
            out,
            lines(&[
                "mi<mk<doc-in-beg",
                "mi<tg<empty-att_<number-of-words<number-of-words>250",
            ])
        );
    }

    #[test]
    fn unrecognized_di_token_inside_info_table_passes_through() {
        let content = lines(&[
            "mi<mk<doc-in-beg",
            "cw<di<vern______<nu<1",
            "mi<mk<doc-in-end",
        ]);
        let out = fix_info(&content).unwrap();
        assert_eq!(out, lines(&["mi<mk<doc-in-beg", "cw<di<vern______<nu<1"]));
    }

    #[test]
    fn unknown_sub_token_in_tokens_run_degrades_silently_below_run_level_four() {
        let content = lines(&[
            "mi<mk<doc-in-beg",
            "cw<di<create-tim<nu<true",
            "cw<di<vern______<nu<1",
            "mi<mk<docinf-end",
            "mi<mk<doc-in-end",
        ]);
        let out = fix_info_with_run_level(&content, 1).unwrap();
        assert_eq!(
            out,
            lines(&["mi<mk<doc-in-beg", "mi<tg<empty-att_<creation-time"])
        );
    }

    #[test]
    fn unknown_sub_token_in_tokens_run_raises_above_run_level_three() {
        let content = lines(&[
            "mi<mk<doc-in-beg",
            "cw<di<create-tim<nu<true",
            "cw<di<vern______<nu<1",
            "mi<mk<docinf-end",
            "mi<mk<doc-in-end",
        ]);
        let err = fix_info_with_run_level(&content, 4).unwrap_err();
        assert_eq!(err, InfoError::NoDictionaryMatch("vern______".to_string()));
    }

    #[test]
    fn text_after_info_table_passes_through_unchanged() {
        let content = lines(&[
            "mi<mk<doc-in-beg",
            "mi<mk<doc-in-end",
            "tx<nu<__________<body text",
        ]);
        assert_eq!(
            fix_info(&content).unwrap(),
            lines(&["mi<mk<doc-in-beg", "tx<nu<__________<body text"])
        );
    }

    /// Preserved upstream quirk: the `mi<mk<doc-in-end` marker line
    /// that closes the info table is itself dropped from the output,
    /// unlike its `mi<mk<doc-in-beg` counterpart. See the comment on
    /// [`fix_info_with_run_level`]'s `Stage::InInfoTable` handling.
    #[test]
    fn doc_in_end_marker_itself_is_dropped() {
        let content = lines(&["mi<mk<doc-in-beg", "mi<mk<doc-in-end"]);
        let out = fix_info(&content).unwrap();
        assert_eq!(out, lines(&["mi<mk<doc-in-beg"]));
        assert!(!out.contains("doc-in-end"));
    }
}
