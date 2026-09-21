//! Delivering events from the Rust side into the served web page
//! (issue #818).
//!
//! `web/` deliberately carries no `@tauri-apps/api` dependency -- it
//! is a plain browser SPA that this app navigates its webview to, and
//! it has to keep working when served as an ordinary web page (see
//! `web/src/tauri.ts`). Reimplementing Tauri's event-listener protocol
//! against `__TAURI_INTERNALS__.transformCallback` on the JS side
//! would work, but evaluating a one-line script that dispatches a DOM
//! `CustomEvent` gets the same result with ordinary `addEventListener`
//! on the receiving end and no new dependency.

use serde::Serialize;
use tauri::{Runtime, WebviewWindow};

/// Dispatches `CustomEvent(name, { detail })` on the page's `window`.
///
/// `detail` is serialized with `serde_json` rather than formatted into
/// the script by hand. That matters: some of what travels this way
/// originates from the page itself (menu action ids) or from the
/// filesystem (dropped file names), so it is data crossing into a JS
/// context, and building the literal by concatenation would be a
/// script-injection bug waiting for the first value containing a
/// quote.
pub fn dispatch<R: Runtime, T: Serialize>(window: &WebviewWindow<R>, name: &str, detail: &T) {
    let (Ok(name_literal), Ok(detail_literal)) = (serde_json::to_string(name), serde_json::to_string(detail)) else {
        eprintln!("could not serialize the {name} page event");
        return;
    };
    let js = format!("window.dispatchEvent(new CustomEvent({name_literal}, {{ detail: {detail_literal} }}))");
    if let Err(e) = window.eval(&js) {
        eprintln!("could not deliver the {name} page event: {e}");
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    /// Counts quotes that would actually terminate a JS string
    /// literal -- i.e. ones not preceded by an odd number of
    /// backslashes. A naive `contains("\"")` check cannot tell an
    /// injected quote from a correctly-escaped one, since the escaped
    /// form contains the unescaped form as a substring.
    fn unescaped_quotes(literal: &str) -> usize {
        let mut count = 0;
        let mut backslashes = 0;
        for c in literal.chars() {
            match c {
                '\\' => backslashes += 1,
                '"' => {
                    if backslashes % 2 == 0 {
                        count += 1;
                    }
                    backslashes = 0;
                }
                _ => backslashes = 0,
            }
        }
        count
    }

    /// The escaping guarantee the module doc relies on: a value
    /// carrying a quote must not be able to close the JS string
    /// literal and start running script.
    #[test]
    fn hostile_values_cannot_break_out_of_the_script() {
        let hostile = json!({ "id": r#"x") ;alert("pwned"# });
        let literal = serde_json::to_string(&hostile).unwrap();

        // Exactly the four structural quotes -- around the `id` key
        // and around its value. Every quote the hostile value
        // contributed is escaped, so none of them can terminate the
        // literal early.
        assert_eq!(unescaped_quotes(&literal), 4, "an injected quote survived: {literal}");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&literal).unwrap(), hostile, "escaping must not corrupt the value");
    }

    #[test]
    fn a_value_ending_in_a_backslash_cannot_escape_the_closing_quote() {
        // The classic off-by-one: a trailing backslash that swallows
        // the closing quote would splice the rest of the script into
        // the string.
        let hostile = json!({ "id": r#"trailing\"# });
        let literal = serde_json::to_string(&hostile).unwrap();

        assert_eq!(unescaped_quotes(&literal), 4, "a trailing backslash ate the closing quote: {literal}");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&literal).unwrap(), hostile);
    }

    #[test]
    fn the_counter_itself_notices_a_real_break_out() {
        // Guards the guard: if `unescaped_quotes` always returned the
        // structural count, the tests above would pass vacuously.
        let hand_built = r#"{"id":"x") ;alert("pwned"}"#;
        assert!(unescaped_quotes(hand_built) > 4, "an unescaped injection must be detected");
    }

    #[test]
    fn event_names_are_escaped_too() {
        assert_eq!(serde_json::to_string("oxide:menu-action").unwrap(), "\"oxide:menu-action\"");
    }
}
