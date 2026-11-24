//! Port of `old_src/src/calibre/ebooks/rtf2xml/body_styles.py`
//! (`BodyStyles`).
//!
//! Inserts [`super::paragraph_def`]'s collected body-style strings
//! (`ParagraphDefOutput::body_style_strings`, threaded from
//! `ParagraphDef.make_paragraph_def`'s return value as `list_of_styles`
//! in the real `ParseRtf.py`) into the content immediately before the
//! `mi<tg<close_____<style-table` marker, wrapped in a
//! `styles-in-body` open/close tag pair -- this is the real, in-scope
//! cross-file dependency this issue calls out: [`insert_info`] takes
//! [`super::paragraph_def::ParagraphDefOutput::body_style_strings`]
//! directly as its second parameter, exactly as `ParseRtf.py` threads
//! `list_of_styles` from one call into the other.
//!
//! Checkpoint `body_styles_info`; the very last of this issue's 18
//! passes.

use thiserror::Error;

/// Port of the `run_level > 3` gated
/// `raise self.__bug_handler(msg)` in `insert_info`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BodyStylesError {
    /// Port of `'Not enough data for each table\n'`.
    #[error("Not enough data for each table\n")]
    NotEnoughData,
}

pub type Result<T> = std::result::Result<T, BodyStylesError>;

/// Port of `BodyStyles.insert_info`, operating directly on
/// intermediate-format content (see [`super::process_tokens`]'s module
/// docs) rather than reopening a file. `list_of_styles` is
/// [`super::paragraph_def::ParagraphDefOutput::body_style_strings`]
/// from the matching [`super::paragraph_def::make_paragraph_def`] call
/// -- each entry already ends with its own `\n`, matching
/// `''.join(list_of_styles)`'s expectation in the Python.
pub fn insert_info(content: &str, list_of_styles: &[String], run_level: u32) -> Result<String> {
    let mut out = String::new();
    for line in content.lines() {
        if line == "mi<tg<close_____<style-table" {
            if !list_of_styles.is_empty() {
                out.push_str("mi<tg<open______<styles-in-body\n");
                for s in list_of_styles {
                    out.push_str(s);
                }
                out.push_str("mi<tg<close_____<styles-in-body\n");
            } else if run_level > 3 {
                return Err(BodyStylesError::NotEnoughData);
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rtf2xml::paragraph_def::make_paragraph_def;

    fn lines(v: &[&str]) -> String {
        v.join("\n") + "\n"
    }

    #[test]
    fn no_styles_leaves_content_unchanged_below_run_level_four() {
        let content = lines(&["mi<tg<close_____<style-table", "tx<nu<__________<after"]);
        let out = insert_info(&content, &[], 1).unwrap();
        assert_eq!(out, content);
    }

    #[test]
    fn no_styles_raises_above_run_level_three() {
        let content = lines(&["mi<tg<close_____<style-table"]);
        let err = insert_info(&content, &[], 4).unwrap_err();
        assert_eq!(err, BodyStylesError::NotEnoughData);
    }

    #[test]
    fn styles_are_inserted_immediately_before_the_close_marker() {
        let content = lines(&["mi<tg<close_____<style-table", "tx<nu<__________<after"]);
        let styles = vec![
            "mi<tg<empty-att_<paragraph-style-in-body<name>Normal<style-number>s0001\n".to_string(),
        ];
        let out = insert_info(&content, &styles, 1).unwrap();
        assert_eq!(
            out,
            lines(&[
                "mi<tg<open______<styles-in-body",
                "mi<tg<empty-att_<paragraph-style-in-body<name>Normal<style-number>s0001",
                "mi<tg<close_____<styles-in-body",
                "mi<tg<close_____<style-table",
                "tx<nu<__________<after",
            ])
        );
    }

    #[test]
    fn no_close_marker_present_leaves_content_untouched() {
        let content = lines(&["tx<nu<__________<no marker here"]);
        let out = insert_info(&content, &[], 4).unwrap();
        assert_eq!(out, content);
    }

    /// The real, in-scope dependency this issue calls for: wire
    /// [`super::paragraph_def::make_paragraph_def`]'s
    /// `body_style_strings` output directly into [`insert_info`], the
    /// same way `ParseRtf.py` threads `list_of_styles` from one call
    /// into the other.
    #[test]
    fn body_style_strings_from_paragraph_def_thread_into_insert_info() {
        let paragraph_content = lines(&[
            "cw<pf<par-def___<nu<true",
            "cw<pf<align_____<nu<left",
            "mi<mk<para-start",
            "tx<nu<__________<hello",
            "mi<mk<para-end__",
            "mi<mk<body-close",
        ]);
        let para_out = make_paragraph_def(&paragraph_content, "Times", 1).unwrap();
        assert_eq!(para_out.body_style_strings.len(), 1);

        let style_table_content = lines(&["mi<tg<close_____<style-table", "mi<mk<body-open_"]);
        let out = insert_info(&style_table_content, &para_out.body_style_strings, 1).unwrap();
        assert!(out.contains("mi<tg<open______<styles-in-body"));
        assert!(out.contains("paragraph-style-in-body"));
        assert!(out.contains("<name>Normal"));
        assert!(out.contains("<style-number>s0001"));
        assert!(out.contains("mi<tg<close_____<styles-in-body"));
        // the original style-table close marker and everything after
        // it survive too.
        assert!(out.contains("mi<tg<close_____<style-table"));
        assert!(out.contains("mi<mk<body-open_"));
    }
}
