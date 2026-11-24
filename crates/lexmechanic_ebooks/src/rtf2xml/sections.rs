//! Port of `old_src/src/calibre/ebooks/rtf2xml/sections.py` (`Sections`).
//!
//! Consumes [`super::process_tokens`]'s bracket-tagged intermediate
//! format (see that module's own docs for the line shapes) and inserts
//! `section`-tag markup around each RTF section (`\sect`/`\sectd`)
//! found in the body. RTF stores section breaks with the `\sect`
//! control word (emitted here as `cw<sc<section___`); each time it's
//! seen, a counter is bumped. A following `\sectd` (`cw<sc<sect-defin`)
//! starts collecting the section's descriptive attributes (columns,
//! margins) until a terminator (`\pard`/body-open/header-indent, or --
//! for older RTF producers -- stray text/font tokens with no `\pard` at
//! all) is reached, at which point a `section` open tag carrying the
//! collected attributes -- plus a `section` close tag if this isn't the
//! first section -- is written.
//!
//! The one exception is section breaks inside field blocks (e.g. an
//! index): [`super::process_tokens`]'s own docs note that fields are
//! out of scope for this crate's early-structure issue, but the
//! upstream pipeline's `fields_large.py` pass (also out of scope here,
//! and which -- despite the name -- actually runs *before* this one)
//! wraps such a block's contents in `mi<mk<sec-fd-beg` /
//! `mi<mk<sec-fd-end` marker lines, which this module still needs to
//! recognize on its input. Per the Python module docstring's
//! 2004-04-26 change note, section information inside a field block is
//! no longer surfaced as `section` tags at all -- it's left untouched
//! in the output instead, so the index ends up nested inside whatever
//! section tag was already open before the field started.

use indexmap::IndexMap;
use thiserror::Error;

/// Port of the single `raise self.__bug_handler(msg)` call site in the
/// whole module (inside `__write_section`'s final, `run_level`-gated
/// `elif`). See [`Sections::write_section`]'s doc for why this is
/// actually unreachable through [`make_sections`]'s own state machine.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SectionsError {
    #[error("missed a flag\n")]
    MissedFlag,
}

pub type Result<T> = std::result::Result<T, SectionsError>;

/// Port of `self.__mark_start`.
const MARK_START: &str = "mi<mk<sect-start\n";
/// Port of `self.__mark_end`.
const MARK_END: &str = "mi<mk<sect-end__\n";

/// Port of `self.__state`'s six string values. Rust's exhaustive
/// `match` on this enum makes the Python's `sys.stderr.write('no
/// matching state in module sections.py\n')` fallback (reached only if
/// `self.__state_dict.get(self.__state)` ever missed) structurally
/// unrepresentable -- every state has a mapped handler by construction,
/// so that diagnostic has no Rust equivalent and isn't ported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    BeforeBody,
    Body,
    BeforeFirstSec,
    Section,
    SectionDef,
    SecInField,
}

/// Port of the `Sections` instance's mutable parsing state
/// (`__initiate_values`'s fields, minus the file handles).
struct Sections {
    state: State,
    /// Port of `self.__in_field`.
    in_field: bool,
    /// Port of `self.__section_values`. `IndexMap` because key
    /// insertion order is observable: [`Sections::write_section`]
    /// iterates it to build the `section` open tag's attribute list in
    /// the order the RTF attributes were encountered, exactly like
    /// Python dict iteration order (dicts have been insertion-ordered
    /// since Python 3.7, which this module's target interpreter is).
    section_values: IndexMap<String, String>,
    /// Port of `self.__section_num`.
    section_num: u64,
    /// Port of `self.__found_first_sec`.
    found_first_sec: bool,
    /// Port of `self.__run_level`.
    run_level: u32,
}

impl Sections {
    fn new(run_level: u32) -> Self {
        Sections {
            state: State::BeforeBody,
            in_field: false,
            section_values: IndexMap::new(),
            section_num: 0,
            found_first_sec: false,
            run_level,
        }
    }

    /// Port of `__write_section`. Builds the `section` open-tag string
    /// (with a preceding close tag if a section has already been
    /// opened) from the current `section_num` and `section_values`,
    /// then either appends it to `out` (`state == 'body'`) or discards
    /// it (`state == 'sec_in_field'`, matching the module docstring's
    /// "ignore all section information in a field-block" -- see
    /// [`Sections::section_def_func`]'s doc for how that state is
    /// actually reached).
    ///
    /// The remaining Python `elif self.__run_level > 3: raise
    /// self.__bug_handler(...)` branch is preserved here (as the
    /// wildcard match arm below) even though it can never fire through
    /// [`make_sections`]'s own state machine: both call sites
    /// (`__end_sec_def_func`, `__end_sec_premature_func`, ported as
    /// [`Sections::end_sec_def_func`] / [`Sections::end_sec_premature_func`])
    /// unconditionally set `self.state` to `Body` or `SecInField`
    /// immediately before calling this method, so `state` here is
    /// always one of those two. Verified by inspection of every call
    /// site in the Python source (there are exactly two, both with
    /// this shape) and exercised directly -- bypassing that guarantee
    /// on purpose -- by the `write_section_missed_flag_*` tests below.
    fn write_section(&mut self, out: &mut String) -> Result<()> {
        let mut my_string = String::from(MARK_START);
        if self.found_first_sec {
            my_string.push_str("mi<tg<close_____<section\n");
        } else {
            self.found_first_sec = true;
        }
        my_string.push_str(&format!(
            "mi<tg<open-att__<section<num>{0}<num-in-level>{0}<type>rtf-native<level>0",
            self.section_num
        ));
        for (key, value) in &self.section_values {
            my_string.push_str(&format!("<{key}>{value}"));
        }
        my_string.push('\n');
        my_string.push_str(MARK_END);

        match self.state {
            State::Body => out.push_str(&my_string),
            State::SecInField => {
                // Port of `__handle_sec_def`. The Python appends (an
                // alias of, not a copy of -- `values_dict =
                // self.__section_values` then
                // `self.__list_of_sec_values.append(values_dict)`)
                // `self.__section_values` to
                // `self.__list_of_sec_values`, a list that no
                // reachable code ever reads back: the two callables
                // that would read it, `__found_section_in_field_func`
                // / `__found_section_def_in_field_func`, are dead --
                // their `__sec_in_field_dict` entries are commented
                // out with the note "changed this 2004-04-26" -- and
                // `__print_field_sec_attributes` (the only reader of
                // `list_of_sec_values`) is never called from anywhere.
                // So `list_of_sec_values`/`field_num` bookkeeping has
                // zero effect on any output and isn't ported; the
                // built `my_string` above is simply discarded here,
                // which is the actual mechanism (together with
                // `section_def_func` never being reached with
                // `in_field == true` in practice -- see that method's
                // doc) behind "ignore all section information in a
                // field-block".
            }
            _ => {
                if self.run_level > 3 {
                    return Err(SectionsError::MissedFlag);
                }
                // run_level <= 3: Python's elif condition is false too
                // in this case, so the whole if/elif/elif chain falls
                // through with no else -- silently does nothing further.
            }
        }
        Ok(())
    }

    /// Port of `__attribute_func`. `line` is the *original* line
    /// (without its Python-style trailing `\n`, already stripped by
    /// `str::lines()`), so `line[20:-1]` becomes `line[20..]`.
    fn attribute_func(&mut self, name: &str, line: &str) {
        let value = line.get(20..).unwrap_or("");
        self.section_values
            .insert(name.to_string(), value.to_string());
    }

    /// Port of `__before_body_func`.
    fn before_body_func(&mut self, out: &mut String, line: &str, token_info: &str) {
        if token_info == "mi<mk<body-open_" {
            self.state = State::BeforeFirstSec;
        }
        push_line(out, line);
    }

    /// Port of `__before_first_sec_func`.
    fn before_first_sec_func(&mut self, out: &mut String, line: &str, token_info: &str) {
        match token_info {
            "cw<sc<sect-defin" => {
                self.state = State::SectionDef;
                self.section_num += 1;
                self.section_values.clear();
            }
            "cw<pf<par-def___" => {
                self.state = State::Body;
                self.section_num += 1;
                out.push_str(&format!(
                    "mi<tg<open-att__<section<num>{0}<num-in-level>{0}<type>rtf-native<level>0\n",
                    self.section_num
                ));
                self.found_first_sec = true;
            }
            "tx<nu<__________" => {
                self.state = State::Body;
                self.section_num += 1;
                out.push_str(&format!(
                    "mi<tg<open-att__<section<num>{0}<num-in-level>{0}<type>rtf-native<level>0\n",
                    self.section_num
                ));
                // Note: unlike every other emitted `par-def___` line in
                // this module, this one is missing its `nu<` subtype
                // field -- `'cw<pf<par-def___<true\n'`, not
                // `'cw<pf<par-def___<nu<true\n'`. Verified against the
                // Python source (`__before_first_sec_func`'s `tx<nu`
                // branch): a genuine upstream inconsistency, preserved
                // literally rather than "fixed".
                out.push_str("cw<pf<par-def___<true\n");
                self.found_first_sec = true;
            }
            _ => {}
        }
        push_line(out, line);
    }

    /// Port of `__body_func` + `__body_dict`.
    fn body_func(&mut self, out: &mut String, line: &str, token_info: &str) {
        match token_info {
            "cw<sc<section___" => {
                // __found_section_func
                self.state = State::Section;
                push_line(out, line);
                self.section_num += 1;
            }
            "mi<mk<sec-fd-beg" => {
                // __found_sec_in_field_func. Deliberately does NOT
                // call push_line: the Python never writes this line to
                // the output object either, only stashes it as the
                // (dead -- see `write_section`'s doc) seed value of
                // `self.__sec_in_field_string`. This marker is
                // therefore dropped from the output entirely -- an
                // asymmetry with `mi<mk<sec-fd-end`, which IS written
                // through (see `sec_in_field_func` below).
                self.state = State::SecInField;
                self.in_field = true;
            }
            "cw<sc<sect-defin" => {
                // __found_section_def_bef_sec_func
                self.section_num += 1;
                self.state = State::SectionDef;
                self.section_values.clear();
                push_line(out, line);
            }
            _ => push_line(out, line),
        }
    }

    /// Port of `__section_func`.
    fn section_func(&mut self, out: &mut String, line: &str, token_info: &str) {
        if token_info == "cw<sc<sect-defin" {
            // __found_section_def_func
            self.state = State::SectionDef;
            self.section_values.clear();
        }
        push_line(out, line);
    }

    /// Port of `__section_def_func` + `__section_def_dict`, dispatching
    /// to `__attribute_func`, `__end_sec_def_func`, or
    /// `__end_sec_premature_func`.
    ///
    /// The Python conditionally drops the triggering `line` (appending
    /// it to the dead `self.__sec_in_field_string` instead of writing
    /// it) whenever `self.__in_field` is true. In practice that branch
    /// never fires: `self.__in_field` is set exactly in lockstep with
    /// `self.__state == 'sec_in_field'` (true only between
    /// `__found_sec_in_field_func` and `__end_sec_in_field_func`), and
    /// no code path transitions from `sec_in_field` into `section_def`
    /// -- `__sec_in_field_dict` only maps `mi<mk<sec-fd-end`, so any
    /// `cw<sc<sect-defin` token seen while `state == 'sec_in_field'`
    /// just falls through [`Sections::sec_in_field_func`]'s unmatched
    /// branch and is echoed verbatim, never reaching this method with
    /// `in_field == true`. This is the real mechanism behind "ignore
    /// section info in field blocks" (not the `in_field` checks
    /// below, which are dead in every reachable call). Ported
    /// faithfully anyway for structural fidelity, and exercised
    /// directly (bypassing the normally-impossible state) by the
    /// `section_def_in_field_*` tests below.
    fn section_def_func(&mut self, out: &mut String, line: &str, token_info: &str) -> Result<()> {
        match token_info {
            "cw<pf<par-def___" | "mi<mk<body-open_" | "mi<mk<header-ind" => {
                self.end_sec_def_func(out)?;
                if self.in_field {
                    // dropped -- see doc above.
                } else {
                    push_line(out, line);
                }
            }
            "cw<tb<columns___" => {
                self.attribute_func("columns", line);
                if !self.in_field {
                    push_line(out, line);
                }
            }
            "cw<pa<margin-lef" => {
                self.attribute_func("margin-left", line);
                if !self.in_field {
                    push_line(out, line);
                }
            }
            "cw<pa<margin-rig" => {
                self.attribute_func("margin-right", line);
                if !self.in_field {
                    push_line(out, line);
                }
            }
            "tx<nu<__________" | "cw<ci<font-style" | "cw<ci<font-size_" => {
                self.end_sec_premature_func(out)?;
                if !self.in_field {
                    push_line(out, line);
                }
            }
            _ => push_line(out, line),
        }
        Ok(())
    }

    /// Port of `__end_sec_def_func`.
    fn end_sec_def_func(&mut self, out: &mut String) -> Result<()> {
        self.state = if !self.in_field {
            State::Body
        } else {
            State::SecInField
        };
        self.write_section(out)
    }

    /// Port of `__end_sec_premature_func`. The three extra lines are
    /// written unconditionally -- unlike the triggering `line` itself
    /// (handled by the caller, [`Sections::section_def_func`]), these
    /// are NOT gated on `in_field`, matching the Python's
    /// `self.__write_obj.write(...)` calls sitting directly in this
    /// method rather than behind the caller's `in_field` check.
    fn end_sec_premature_func(&mut self, out: &mut String) -> Result<()> {
        self.state = if !self.in_field {
            State::Body
        } else {
            State::SecInField
        };
        self.write_section(out)?;
        out.push_str("cw<pf<par-def___<nu<true\n");
        out.push_str("ob<nu<open-brack<0000\n");
        out.push_str("cb<nu<clos-brack<0000\n");
        Ok(())
    }

    /// Port of `__sec_in_field_func` + `__sec_in_field_dict`. The dict
    /// only maps `mi<mk<sec-fd-end` (the two `cw<sc<section___` /
    /// `cw<sc<sect-defin` entries mentioned in the module's own
    /// comments are commented out, "changed this 2004-04-26") --
    /// every other token seen while in a field block, including
    /// `\sect`/`\sectd` ones, is echoed straight through unchanged.
    /// That's the actual "ignore section info in field blocks"
    /// mechanism (see [`Sections::section_def_func`]'s doc).
    fn sec_in_field_func(&mut self, out: &mut String, line: &str, token_info: &str) {
        if token_info == "mi<mk<sec-fd-end" {
            // __end_sec_in_field_func
            self.state = State::Body;
            self.in_field = false;
        }
        push_line(out, line);
    }
}

/// Appends `line` (without its terminator, as yielded by
/// `str::lines()`) plus a restored `\n`, matching the Python's
/// `self.__write_obj.write(line)` where `line` still carries the
/// trailing `\n` `readline()` left on it.
fn push_line(out: &mut String, line: &str) {
    out.push_str(line);
    out.push('\n');
}

/// Port of `Sections.__token_info`'s `line[:16]` (applied to a
/// `str::lines()` line, whose trailing `\n` -- unlike Python's
/// `readline()` result -- is already stripped).
fn token_info(line: &str) -> &str {
    if line.len() >= 16 {
        &line[..16]
    } else {
        line
    }
}

/// Port of `Sections.make_sections` (the temp-file / `Copy` / rename
/// dance around it is pipeline plumbing, not ported here -- see
/// [`super::process_tokens`]'s own doc for the same call). Takes the
/// intermediate-format text described in that module's docs and
/// returns it with `section` tags inserted.
///
/// The Python's final loop iteration -- `readline()` returning `''` at
/// EOF, which still gets dispatched through `action('')` one last time
/// before the `while line_to_read:` condition stops the loop -- is not
/// replicated: `token_info('') == ''` never matches any of the
/// per-state branches below (all fixed 16-character prefixes), so that
/// trailing call is a verified no-op (`write_obj.write('')`) in every
/// state, and iterating `content.lines()` (which never yields a final
/// empty element) already produces the same observable output.
pub fn make_sections(content: &str, run_level: u32) -> Result<String> {
    let mut sections = Sections::new(run_level);
    let mut out = String::new();
    for line in content.lines() {
        let tok = token_info(line);
        match sections.state {
            State::BeforeBody => sections.before_body_func(&mut out, line, tok),
            State::Body => sections.body_func(&mut out, line, tok),
            State::BeforeFirstSec => sections.before_first_sec_func(&mut out, line, tok),
            State::Section => sections.section_func(&mut out, line, tok),
            State::SectionDef => sections.section_def_func(&mut out, line, tok)?,
            State::SecInField => sections.sec_in_field_func(&mut out, line, tok),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_open() -> &'static str {
        "mi<mk<body-open_\n"
    }

    #[test]
    fn empty_input_produces_empty_output() {
        assert_eq!(make_sections("", 1).unwrap(), "");
    }

    #[test]
    fn before_body_lines_pass_through_and_body_open_switches_state() {
        let content = "cw<ri<rtf_______<nu<1\n".to_string() + body_open();
        let out = make_sections(&content, 1).unwrap();
        assert_eq!(out, content);
    }

    #[test]
    fn sectd_with_attributes_writes_open_tag_on_pard() {
        let content = format!(
            "{}{}{}{}{}{}",
            body_open(),
            "cw<sc<sect-defin<nu<true\n",
            "cw<tb<columns___<nu<2\n",
            "cw<pa<margin-lef<nu<1440\n",
            "cw<pa<margin-rig<nu<1440\n",
            "cw<pf<par-def___<nu<true\n",
        );
        let out = make_sections(&content, 1).unwrap();
        // The section-definition control words are all echoed through
        // as-is (only recorded into section_values on the side, not
        // removed from the stream) -- the section tag block is
        // inserted separately, right before the terminating par-def
        // line.
        assert!(out.contains("cw<sc<sect-defin<nu<true\n"));
        assert!(out.contains("cw<tb<columns___<nu<2\n"));
        assert!(out.contains("cw<pf<par-def___<nu<true\n"));
        // The section tag itself: first section, so no close tag, and
        // attributes appear in encounter order, inserted right before
        // the terminating par-def line.
        let tag_block = "mi<mk<sect-start\nmi<tg<open-att__<section<num>1<num-in-level>1<type>rtf-native<level>0<columns>2<margin-left>1440<margin-right>1440\nmi<mk<sect-end__\n";
        assert!(out.contains(&format!("{tag_block}cw<pf<par-def___<nu<true\n")));
        assert!(!out.contains("close_____<section"));
    }

    #[test]
    fn body_open_then_par_def_starts_first_section_with_no_sectd() {
        // Older RTF: no \sectd at all, just body-open then a bare pard.
        let content = format!("{}{}", body_open(), "cw<pf<par-def___<nu<true\n");
        let out = make_sections(&content, 1).unwrap();
        assert_eq!(
            out,
            format!(
                "{}mi<tg<open-att__<section<num>1<num-in-level>1<type>rtf-native<level>0\n{}",
                body_open(),
                "cw<pf<par-def___<nu<true\n"
            )
        );
    }

    #[test]
    fn body_open_then_stray_text_starts_first_section_with_synthetic_pard() {
        // Even older RTF: text appears before any pard at all.
        let content = format!("{}{}", body_open(), "tx<nu<__________<hello\n");
        let out = make_sections(&content, 1).unwrap();
        assert_eq!(
            out,
            format!(
                "{}mi<tg<open-att__<section<num>1<num-in-level>1<type>rtf-native<level>0\ncw<pf<par-def___<true\n{}",
                body_open(),
                "tx<nu<__________<hello\n"
            )
        );
        // Verified upstream quirk: this synthetic par-def line is
        // missing the `nu<` subtype field that every other emitted
        // par-def line in this module has.
    }

    #[test]
    fn second_sect_emits_close_tag_before_new_open_tag() {
        let content = format!(
            "{}{}{}{}{}",
            body_open(),
            "cw<pf<par-def___<nu<true\n", // opens section 1 (no sectd)
            "cw<sc<section___<nu<true\n", // \sect -> section 2, state=section
            "cw<sc<sect-defin<nu<true\n", // \sectd -> section_def
            "cw<pf<par-def___<nu<true\n", // terminator -> writes section 2's tags
        );
        let out = make_sections(&content, 1).unwrap();
        assert!(out.contains("mi<tg<open-att__<section<num>1"));
        assert!(out.contains("mi<tg<close_____<section\n"));
        assert!(out.contains("mi<tg<open-att__<section<num>2<num-in-level>2"));
    }

    #[test]
    fn premature_ending_inserts_synthetic_pard_and_brackets() {
        let content = format!(
            "{}{}{}",
            body_open(),
            "cw<sc<sect-defin<nu<true\n",
            "cw<ci<font-style<nu<1\n", // premature ending: text-ish token before pard
        );
        let out = make_sections(&content, 1).unwrap();
        assert!(out
            .contains("cw<pf<par-def___<nu<true\nob<nu<open-brack<0000\ncb<nu<clos-brack<0000\n"));
        assert!(out.contains("cw<ci<font-style<nu<1\n"));
    }

    #[test]
    fn field_block_begin_marker_is_dropped_but_end_marker_kept() {
        let content = format!(
            "{}{}{}{}{}",
            body_open(),
            "cw<pf<par-def___<nu<true\n", // opens section 1
            "mi<mk<sec-fd-beg\n",
            "mi<mk<sec-fd-end\n",
            "cw<sc<section___<nu<true\n", // back in body, ordinary \sect
        );
        let out = make_sections(&content, 1).unwrap();
        assert!(!out.contains("sec-fd-beg"));
        assert!(out.contains("mi<mk<sec-fd-end\n"));
    }

    #[test]
    fn section_tokens_inside_field_block_pass_through_untouched() {
        // \sect and \sectd occurring inside a field block are not
        // intercepted at all -- they're echoed verbatim, which is the
        // real mechanism behind "ignore section info in field blocks".
        let content = format!(
            "{}{}{}{}{}{}",
            body_open(),
            "cw<pf<par-def___<nu<true\n", // opens section 1
            "mi<mk<sec-fd-beg\n",
            "cw<sc<section___<nu<true\n",
            "cw<sc<sect-defin<nu<true\n",
            "mi<mk<sec-fd-end\n",
        );
        let out = make_sections(&content, 1).unwrap();
        // Only one section tag was ever opened (section 1); the \sect
        // and \sectd inside the field did not bump the counter or
        // produce a second section tag.
        assert_eq!(out.matches("mi<tg<open-att__<section").count(), 1);
        assert!(out.contains("cw<sc<section___<nu<true\n"));
        assert!(out.contains("cw<sc<sect-defin<nu<true\n"));
    }

    #[test]
    fn write_section_missed_flag_degrades_silently_at_default_run_level() {
        // State the public make_sections state machine can never
        // produce at the point write_section runs (see that method's
        // doc): exercised directly to demonstrate the run_level <= 3
        // degrade path.
        let mut sections = Sections::new(1);
        sections.state = State::BeforeBody;
        let mut out = String::new();
        assert_eq!(sections.write_section(&mut out), Ok(()));
        assert_eq!(out, "");
    }

    #[test]
    fn write_section_missed_flag_raises_above_run_level_three() {
        let mut sections = Sections::new(4);
        sections.state = State::Section;
        let mut out = String::new();
        assert_eq!(
            sections.write_section(&mut out),
            Err(SectionsError::MissedFlag)
        );
    }

    #[test]
    fn section_def_in_field_drops_attribute_lines_and_discards_section_tag() {
        // Demonstrates the (dead-in-practice, per section_def_func's
        // doc) in_field-aware branches directly: attribute lines are
        // dropped from output, and the constructed section tag is
        // discarded (write_section's SecInField arm) rather than
        // written, instead of raising.
        let mut sections = Sections::new(1);
        sections.state = State::SectionDef;
        sections.in_field = true;
        let mut out = String::new();
        sections
            .section_def_func(&mut out, "cw<tb<columns___<nu<2", "cw<tb<columns___")
            .unwrap();
        assert_eq!(out, "", "attribute line must be dropped while in_field");
        assert_eq!(
            sections.section_values.get("columns"),
            Some(&"2".to_string())
        );

        sections
            .section_def_func(&mut out, "cw<pf<par-def___<nu<true", "cw<pf<par-def___")
            .unwrap();
        assert_eq!(
            out, "",
            "terminator line and the section tag it would have written must both be dropped"
        );
        assert_eq!(sections.state, State::SecInField);
    }
}
