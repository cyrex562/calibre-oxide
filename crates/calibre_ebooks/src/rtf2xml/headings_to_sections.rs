//! Port of `old_src/src/calibre/ebooks/rtf2xml/headings_to_sections.py`
//! (`HeadingsToSections`).
//!
//! Wraps paragraphs styled `heading 1`..`heading 9` in nested
//! `<section>` tags, numbering each one (`num`, a dotted path like
//! `1.2`; `num-in-level`, a per-depth sibling counter; `level`, the
//! current nesting depth) as it goes. A new heading at level *N*
//! closes every currently-open section at level *N* or deeper (so a
//! `heading 2` after a `heading 3` closes the `heading 3` section but
//! leaves the enclosing `heading 1`/`heading 2` sections open), while
//! table and list groups are tracked (`in_table`/`in_list` states)
//! purely to suppress heading detection inside them -- a
//! `style-name` marker inside a table cell or list item never starts
//! a section.
//!
//! # Confirmed-dead copy-paste debris, omitted
//!
//! `__close_lists` and `self.__id_regex` are both leftover copies from
//! [`super::make_lists`] (`__close_lists` is even textually identical
//! to that module's own method) that were never wired into this
//! class at all: `__close_lists` is never called anywhere (confirmed
//! by grep), and would crash immediately if it ever were --
//! `self.__all_lists`/`self.__left_indent` are never initialized in
//! this class, and `self.__write_end_item`/`self.__write_end_list`
//! don't exist on it. `self.__id_regex` is compiled in
//! `__initiate_values` and never referenced again anywhere. Neither
//! has any Rust counterpart.
//!
//! Operates directly on intermediate-format content (see
//! [`super::process_tokens`]'s module docs) rather than reopening
//! files.

const HEADINGS: [&str; 9] = [
    "heading 1",
    "heading 2",
    "heading 3",
    "heading 4",
    "heading 5",
    "heading 6",
    "heading 7",
    "heading 8",
    "heading 9",
];

const END_LIST: [&str; 3] = ["mi<mk<body-close", "mi<mk<sect-close", "mi<mk<sect-start"];

fn token_info(line: &str) -> &str {
    if line.len() >= 16 { &line[..16] } else { line }
}

fn payload(line: &str) -> &str {
    if line.len() >= 17 { &line[17..] } else { "" }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Default,
    InTable,
    InList,
    AfterBody,
}

struct HeadingsToSectionsBuilder {
    state: State,
    all_sections: Vec<u32>,
    list_depth: i64,
    section_num: Vec<u32>,
}

impl HeadingsToSectionsBuilder {
    fn new() -> Self {
        Self { state: State::default(), all_sections: Vec::new(), list_depth: 0, section_num: vec![0] }
    }

    /// Port of `__close_sections`. Same "assumes a contiguous prefix
    /// once reversed" shape as [`super::make_lists`]'s `__close_lists`
    /// -- see that module's own doc.
    fn close_sections(&mut self, out: &mut String, current_level: u32) {
        self.all_sections.reverse();
        let mut num_levels_closed = 0;
        for &level in &self.all_sections {
            if current_level <= level {
                self.write_end_section(out);
                num_levels_closed += 1;
            }
        }
        self.all_sections.drain(0..num_levels_closed);
        self.all_sections.reverse();
    }

    fn write_end_section(&self, out: &mut String) {
        out.push_str("mi<mk<sect-close\nmi<tg<close_____<section\n");
    }

    /// Port of `__write_start_section` (the `current_level` parameter
    /// is unused in the Python too -- dropped here).
    fn write_start_section(&mut self, out: &mut String, name: &str) {
        let mut section_num_str: String = self.section_num.iter().map(|n| format!("{n}.")).collect();
        section_num_str.pop();
        let num_in_level_idx = self.all_sections.len();
        let num_in_level = self.section_num[num_in_level_idx];
        let level = self.all_sections.len();
        out.push_str("mi<mk<sect-start\n");
        out.push_str(&format!(
            "mi<tg<open-att__<section<num>{section_num_str}<num-in-level>{num_in_level}<level>{level}<type>{name}\n"
        ));
    }

    /// Port of `__handle_heading`.
    fn handle_heading(&mut self, out: &mut String, name: &str) {
        let num = HEADINGS.iter().position(|h| *h == name).expect("caller already checked HEADINGS.contains") as u32
            + 1;
        self.close_sections(out, num);
        self.all_sections.push(num);
        let level_depth = self.all_sections.len() + 1;
        self.section_num.truncate(level_depth);
        if self.section_num.len() < level_depth {
            self.section_num.push(1);
        } else {
            *self.section_num.last_mut().expect("level_depth >= 1") += 1;
        }
        self.write_start_section(out, name);
    }

    /// Port of `__default_func`.
    fn default(&mut self, out: &mut String, line: &str, tok: &str) {
        if tok == "mi<mk<sect-start" {
            self.section_num[0] += 1;
            self.section_num.truncate(1);
        }
        if tok == "mi<mk<tabl-start" {
            self.state = State::InTable;
        } else if tok == "mi<mk<list_start" {
            self.state = State::InList;
            self.list_depth += 1;
        } else if END_LIST.contains(&tok) {
            self.close_sections(out, 0);
        } else if tok == "mi<mk<style-name" {
            let name = payload(line);
            if HEADINGS.contains(&name) {
                self.handle_heading(out, name);
            }
        }
        if tok == "mi<mk<body-close" {
            self.state = State::AfterBody;
        }
        out.push_str(line);
        out.push('\n');
    }

    /// Port of `__in_table_func`.
    fn in_table(&mut self, out: &mut String, line: &str, tok: &str) {
        if tok == "mi<mk<table-end_" {
            self.state = State::Default;
        }
        out.push_str(line);
        out.push('\n');
    }

    /// Port of `__in_list_func`.
    fn in_list(&mut self, out: &mut String, line: &str, tok: &str) {
        if tok == "mi<mk<list_close" {
            self.list_depth -= 1;
        } else if tok == "mi<mk<list_start" {
            self.list_depth += 1;
        }
        if self.list_depth == 0 {
            self.state = State::Default;
        }
        out.push_str(line);
        out.push('\n');
    }
}

/// Port of `HeadingsToSections.make_sections`, operating directly on
/// intermediate-format content (see this module's own doc) rather
/// than reopening a file.
pub fn make_sections(content: &str) -> String {
    let mut b = HeadingsToSectionsBuilder::new();
    let mut out = String::new();

    for line in content.lines() {
        let tok = token_info(line);
        match b.state {
            State::Default => b.default(&mut out, line, tok),
            State::InTable => b.in_table(&mut out, line, tok),
            State::InList => b.in_list(&mut out, line, tok),
            State::AfterBody => {
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

    fn style(name: &str) -> String {
        format!("mi<mk<style-name<{name}\n")
    }

    #[test]
    fn plain_content_with_no_headings_passes_through_unchanged() {
        let content = "tx<nu<__________<hello\nmi<mk<body-close\n";
        assert_eq!(make_sections(content), content);
    }

    #[test]
    fn a_single_heading_one_opens_a_top_level_section() {
        // `section_num` starts as Python's `[0]` -- a placeholder only
        // `mi<mk<sect-start` (a real RTF `\sect` boundary, not a
        // heading) ever increments -- so every heading-derived `num`
        // carries a leading `0.` prefix in a document with no real
        // section breaks.
        let out = make_sections(&style("heading 1"));
        assert!(out.contains("mi<mk<sect-start\n"), "{out}");
        assert!(
            out.contains("mi<tg<open-att__<section<num>0.1<num-in-level>1<level>1<type>heading 1\n"),
            "{out}"
        );
    }

    #[test]
    fn a_sibling_heading_one_increments_the_top_level_counter() {
        let mut content = style("heading 1");
        content.push_str(&style("heading 1"));
        let out = make_sections(&content);
        assert!(out.contains("<num>0.1<num-in-level>1<level>1"), "{out}");
        assert!(out.contains("<num>0.2<num-in-level>2<level>1"), "{out}");
        // The first section closes before the second opens.
        assert_eq!(out.matches("mi<mk<sect-close\n").count(), 1, "{out}");
    }

    #[test]
    fn a_nested_heading_two_opens_a_deeper_section_without_closing_the_parent() {
        let mut content = style("heading 1");
        content.push_str(&style("heading 2"));
        let out = make_sections(&content);
        assert!(out.contains("<num>0.1<num-in-level>1<level>1<type>heading 1"), "{out}");
        assert!(out.contains("<num>0.1.1<num-in-level>1<level>2<type>heading 2"), "{out}");
        assert_eq!(out.matches("mi<mk<sect-close\n").count(), 0, "{out}");
    }

    #[test]
    fn a_higher_level_heading_closes_the_deeper_open_section() {
        let mut content = style("heading 1");
        content.push_str(&style("heading 2"));
        content.push_str(&style("heading 1"));
        let out = make_sections(&content);
        // heading 2's section closes; heading 1's own section (the
        // first one) also closes since a new heading-1 always closes
        // everything at level >= 1.
        assert_eq!(out.matches("mi<mk<sect-close\n").count(), 2, "{out}");
        assert!(out.contains("<num>0.2<num-in-level>2<level>1<type>heading 1"), "{out}");
    }

    #[test]
    fn a_style_name_inside_a_table_is_not_treated_as_a_heading() {
        let mut content = "mi<mk<tabl-start\n".to_string();
        content.push_str(&style("heading 1"));
        content.push_str("mi<mk<table-end_\n");
        let out = make_sections(&content);
        assert!(!out.contains("mi<mk<sect-start\n"), "{out}");
    }

    #[test]
    fn a_style_name_inside_a_possibly_nested_list_is_not_treated_as_a_heading() {
        let mut content = "mi<mk<list_start\nmi<mk<list_start\n".to_string();
        content.push_str(&style("heading 1"));
        content.push_str("mi<mk<list_close\nmi<mk<list_close\n");
        content.push_str(&style("heading 1"));
        let out = make_sections(&content);
        // Only the heading AFTER both nested lists closed starts a section.
        assert_eq!(out.matches("mi<mk<sect-start\n").count(), 1, "{out}");
    }

    #[test]
    fn a_body_close_token_transitions_to_after_body_and_still_writes_through() {
        let mut content = style("heading 1");
        content.push_str("mi<mk<body-close\ntx<nu<__________<trailer\n");
        let out = make_sections(&content);
        assert!(out.contains("tx<nu<__________<trailer\n"), "{out}");
    }
}
