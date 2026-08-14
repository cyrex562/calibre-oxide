//! Cross-validation of the CFI parser against calibre's.
//!
//! calibre's `parse.py` needs the third-party `regex` module, which is
//! not installable here, so the vectors come from a stdlib-only
//! transcription of it. That transcription is only trusted because it
//! first reproduces every vector in calibre's own
//! `epub/cfi/tests.py` — the 25 parse cases and 4 sort cases — exactly;
//! it refuses to emit anything otherwise.
//!
//! The corpus is 2826 inputs: calibre's own vectors, a grammar-driven
//! generator covering steps, redirects, all four offset forms and text
//! assertions with parameters, and the same again after random damage
//! (truncation, deletion, junk insertion) so the error paths are
//! exercised rather than only the happy ones.

#[path = "data/cfi_vectors.rs"]
mod vectors;

use calibre_ebooks::epub::cfi::parse::{parse_path, Path, Step};

/// Serialize a parse result the way the reference does, so the two can
/// be compared as strings.
fn serialize(path: &Option<Path>, leftover: &str) -> String {
    format!("{}|'{}'", ser_path(path.as_ref()), leftover)
}

fn ser_path(path: Option<&Path>) -> String {
    let Some(path) = path else {
        return "NONE".to_string();
    };
    let steps: Vec<String> = path.steps.iter().map(ser_step).collect();
    let mut ans = format!("[{}]", steps.join(" "));
    if let Some(redirect) = &path.redirect {
        ans.push('!');
        ans.push_str(&ser_path(Some(redirect)));
    }
    ans
}

fn ser_step(step: &Step) -> String {
    let mut parts = vec![format!("num={}", step.num)];
    if let Some(id) = &step.id {
        parts.push(format!("id='{id}'"));
    }
    if let Some(offset) = step.text_offset {
        parts.push(format!("text={offset}"));
    }
    if let Some(t) = step.temporal_offset {
        parts.push(format!("temporal={t:?}"));
    }
    if let Some((x, y)) = step.spatial_offset {
        parts.push(format!("spatial=({x:?},{y:?})"));
    }
    if let Some(ta) = &step.text_assertion {
        let mut ta_parts = Vec::new();
        if let Some(before) = &ta.before {
            ta_parts.push(format!("before='{before}'"));
        }
        if let Some(after) = &ta.after {
            ta_parts.push(format!("after='{after}'"));
        }
        for (name, values) in &ta.params {
            let vals: Vec<String> = values.iter().map(|v| format!("'{v}'")).collect();
            ta_parts.push(format!("param['{name}']=({})", vals.join(",")));
        }
        parts.push(format!("ta{{{}}}", ta_parts.join(" ")));
    }
    format!("{{{}}}", parts.join(" "))
}

#[test]
fn parse_path_matches_calibre_on_every_vector() {
    let mut mismatches = Vec::new();
    for (input, expected) in vectors::VECTORS {
        let (path, leftover) = parse_path(input);
        let got = serialize(&path, leftover);
        if got != *expected {
            mismatches.push(format!(
                "{input:?}\n     rust: {got}\n  calibre: {expected}"
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} vectors differ:\n{}",
        mismatches.len(),
        vectors::VECTORS.len(),
        mismatches
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_corpus_reaches_every_branch_of_the_grammar() {
    // A corpus that never produced a redirect or a parameter would pass
    // vacuously.
    let outputs: Vec<&str> = vectors::VECTORS.iter().map(|(_, e)| *e).collect();
    assert!(vectors::VECTORS.len() > 2000, "corpus is too small");
    for (needle, what) in [
        ("id=", "an id assertion"),
        ("!", "a redirect"),
        ("text=", "a text offset"),
        ("temporal=", "a temporal offset"),
        ("spatial=", "a spatial offset"),
        ("before=", "a text assertion"),
        ("after=", "a trailing text assertion"),
        ("param[", "an assertion parameter"),
        ("NONE", "an unparseable input"),
    ] {
        assert!(
            outputs.iter().any(|o| o.contains(needle)),
            "no vector produced {what}"
        );
    }
    // And plenty of the vectors must leave something unconsumed, or the
    // leftover half of the comparison proves nothing.
    let with_leftover = outputs.iter().filter(|o| !o.ends_with("|''")).count();
    assert!(
        with_leftover > 200,
        "only {with_leftover} vectors had leftovers"
    );
}
