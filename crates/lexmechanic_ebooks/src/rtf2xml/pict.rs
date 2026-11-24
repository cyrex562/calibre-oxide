//! Port of `old_src/src/calibre/ebooks/rtf2xml/pict.py` (`Pict`).
//!
//! # The image question
//!
//! `ParseRtf.py` passes this pass `orig_file`/`out_file` in addition to
//! the usual `in_file`/`copy`/`run_level`, which raised the question of
//! whether this pass does real image decoding (in which case this
//! crate's `image` dependency, already used elsewhere in this session's
//! ports, would be the right tool). It does not: `Pict.process_pict`
//! never decodes pixel data at all. A `\pict` group's contents are RTF
//! tokens like any other (already lexed by [`super::tokenize`] and
//! [`super::process_tokens`] into the same `cw`/`ob`/`cb`/`tx`
//! intermediate format everything else in this pipeline uses -- RTF's
//! `\pict` destination stores image bytes as literal hex-digit *text*
//! tokens, not binary). This pass just re-serializes those tokens back
//! into a minimal, syntactically valid standalone `.rtf` sub-document
//! (`{\rtf1 ...}` with a trivial font/color table and `\pard`) written
//! to a sibling file, using [`super::process_tokens`]'s reverse mapping
//! (`ob`->`{`, `cb`->`}`, `tx<nu<...`->the literal text) -- `orig_file`
//! only supplies the base filename used to derive that sibling
//! directory's name, and `out_file` supplies the directory to put it
//! next to. `orig_file`/`out_file` are therefore ported here as plain
//! path/name parameters, not file handles to read image bytes from.
//!
//! The main token stream gets three replacement marker lines in place
//! of each `\pict` group's contents: `mi<mk<pict-start`, an
//! `mi<tg<empty-att_<pict<num>NNN` element carrying the picture's
//! sequence number, and `mi<mk<pict-end__`. A later, out-of-scope pass
//! is expected to re-read the extracted `.rtf` sub-document (e.g. to
//! actually rasterize/convert it) using that sequence number to find
//! the right one; this pass's own job ends at extraction.

/// Result of [`process_pict`]: the transformed main-stream content, the
/// extracted picture sub-document's RTF text (`None` if the input had
/// no `\pict` groups at all -- port of `self.__already_found_pict`
/// gating whether `self.__pict_file`/`self.__write_pic_obj` are ever
/// created), and the count of pictures found (port of
/// `self.__pict_count`, also used by the real pipeline to decide
/// whether to remove the now-empty picture directory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictOutput {
    pub content: String,
    pub pict_document: Option<String>,
    pub pict_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Default,
    InPict,
}

fn token_info(line: &str) -> &str {
    if line.len() >= 16 {
        &line[..16]
    } else {
        line
    }
}

fn last_four(line: &str) -> u32 {
    if line.len() >= 4 {
        line[line.len() - 4..].parse().unwrap_or(0)
    } else {
        0
    }
}

/// Port of `__pict_dict`'s three action functions
/// (`__open_br_func`/`__close_br_func`/`__text_func`), applied to a
/// line already known to be inside a `\pict` group, re-serializing the
/// intermediate-format token back to literal RTF syntax for the
/// extracted sub-document.
fn pict_line_to_rtf(line: &str, info: &str) -> Option<String> {
    match info {
        "ob<nu<open-brack" => Some("{\n".to_string()),
        "cb<nu<clos-brack" => Some("}\n".to_string()),
        "tx<nu<__________" => {
            // Port of `line[17:]` (Python line still carries its
            // trailing `\n`; ours doesn't).
            if line.len() > 17 {
                Some(format!("{}\n", &line[17..]))
            } else {
                Some("\n".to_string())
            }
        }
        _ => None,
    }
}

/// Port of `Pict.process_pict`, operating directly on intermediate-
/// format content (see [`super::process_tokens`]'s module docs) rather
/// than reopening files. Directory creation/cleanup
/// (`__make_dir`) is pipeline plumbing around the filesystem and is not
/// ported here -- callers that need an on-disk sibling `.rtf` file are
/// expected to write `pict_document` themselves using their own
/// storage discipline (see `docs/FAULT_TOLERANCE.md`).
pub fn process_pict(content: &str) -> PictOutput {
    let mut stage = Stage::Default;
    let mut out = String::new();
    let mut ob_count: u32 = 0;
    let mut cb_count: u32 = 0;
    let mut pict_count: u32 = 0;
    let mut pict_br_count: u32 = 0;
    let mut already_found_pict = false;
    let mut pict_doc = String::new();

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
                // Port of `__default`.
                if info == "cw<gr<picture___" {
                    pict_count += 1;
                    out.push_str("mi<mk<pict-start\n");
                    out.push_str(&format!("mi<tg<empty-att_<pict<num>{pict_count:03}\n"));
                    out.push_str("mi<mk<pict-end__\n");
                    if !already_found_pict {
                        already_found_pict = true;
                        // Port of `__print_rtf_header`.
                        pict_doc.push_str("{\\rtf1 \n{\\fonttbl\\f0\\null;} \n");
                        pict_doc.push_str("{\\colortbl\\red255\\green255\\blue255;} \n\\pard \n");
                    }
                    stage = Stage::InPict;
                    pict_br_count = ob_count;
                    cb_count = 0;
                    pict_doc.push_str("{\\pict\n");
                    // the `cw<gr<picture___` line itself is dropped
                    // (replaced by the three marker lines above).
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            Stage::InPict => {
                // Port of `__in_pict_func`.
                if cb_count == pict_br_count {
                    stage = Stage::Default;
                    pict_doc.push_str("}\n");
                    out.push_str(line);
                    out.push('\n');
                } else if let Some(rtf) = pict_line_to_rtf(line, info) {
                    pict_doc.push_str(&rtf);
                    // matches `__in_pict_func` returning `False`: the
                    // main stream does not also get this line.
                } else {
                    // no action in `__pict_dict` for this token ->
                    // Python's `action = None` -> `if action:` is
                    // false -> nothing written to the pict file either
                    // -- the token is dropped from both streams.
                }
            }
        }
    }

    PictOutput {
        content: out,
        pict_document: if already_found_pict {
            Some(pict_doc)
        } else {
            None
        },
        pict_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ob(n: u32) -> String {
        format!("ob<nu<open-brack<{n:04}")
    }
    fn cb(n: u32) -> String {
        format!("cb<nu<clos-brack<{n:04}")
    }
    fn lines(v: &[&str]) -> String {
        v.join("\n") + "\n"
    }

    #[test]
    fn no_pict_group_leaves_content_unchanged_and_no_document() {
        let content = lines(&[&ob(1), "tx<nu<__________<hello", &cb(1)]);
        let out = process_pict(&content);
        assert_eq!(out.content, content);
        assert_eq!(out.pict_document, None);
        assert_eq!(out.pict_count, 0);
    }

    #[test]
    fn pict_group_is_extracted_and_replaced_with_markers() {
        // { \pict { hex-encoded-data } }
        let content = lines(&[
            &ob(1),
            "cw<gr<picture___<nu<true",
            &ob(2),
            "tx<nu<__________<89504e470d0a",
            &cb(2),
            &cb(1),
        ]);
        let out = process_pict(&content);
        assert_eq!(out.pict_count, 1);
        assert_eq!(
            out.content,
            lines(&[
                &ob(1),
                "mi<mk<pict-start",
                "mi<tg<empty-att_<pict<num>001",
                "mi<mk<pict-end__",
                &cb(1),
            ])
        );
        let doc = out.pict_document.unwrap();
        assert!(doc.starts_with("{\\rtf1 \n{\\fonttbl\\f0\\null;} \n"));
        assert!(doc.contains("{\\pict\n"));
        assert!(doc.contains("{\n"));
        assert!(doc.contains("89504e470d0a\n"));
        assert!(doc.contains("}\n"));
    }

    #[test]
    fn multiple_pict_groups_share_one_document_with_incrementing_numbers() {
        let content = lines(&[
            &ob(1),
            "cw<gr<picture___<nu<true",
            "tx<nu<__________<aaaa",
            &cb(1),
            &ob(2),
            "cw<gr<picture___<nu<true",
            "tx<nu<__________<bbbb",
            &cb(2),
        ]);
        let out = process_pict(&content);
        assert_eq!(out.pict_count, 2);
        assert!(out.content.contains("mi<tg<empty-att_<pict<num>001"));
        assert!(out.content.contains("mi<tg<empty-att_<pict<num>002"));
        let doc = out.pict_document.unwrap();
        // only one RTF header is emitted despite two picture groups.
        assert_eq!(doc.matches("\\fonttbl").count(), 1);
        assert_eq!(doc.matches("{\\pict\n").count(), 2);
    }

    #[test]
    fn tokens_with_no_pict_dict_action_are_dropped_from_both_streams() {
        // a control-word line inside \pict that isn't ob/cb/plain-text
        // (e.g. a resolved cw<... line) has no entry in __pict_dict
        // and is silently dropped from both the main stream and the
        // extracted document.
        let content = lines(&[
            &ob(1),
            "cw<gr<picture___<nu<true",
            "cw<ci<bold______<nu<true",
            "tx<nu<__________<abcd",
            &cb(1),
        ]);
        let out = process_pict(&content);
        let doc = out.pict_document.unwrap();
        assert!(!doc.contains("bold"));
        assert!(doc.contains("abcd\n"));
    }
}
