use regex::bytes::Regex;
use std::str;

fn main() {
    // Regex from rtf.rs
    let pat = Regex::new(r"(?si)\{\\info.*?\{\\title(\s*(?:[^\\}]|\\.)*)\}").unwrap();

    // Test case from test_rtf_escapes
    // br"{\\rtf1\\ansi{\\info{\\title \\'41 Title}}}"
    // Note: in main.rs we use br"..." directly.
    let text = br"{\rtf1\ansi{\info{\title \'41 Title}}}";
    // Wait, in source code this looks like:
    // '{\rtf1\ansi{\info{\title \'41 Title}}}'
    // where \ is backslash.

    println!("Text: {:?}", str::from_utf8(text).unwrap());

    if let Some(cap) = pat.captures(text) {
        let captured = &cap[1];
        println!("Captured bytes: {:?}", captured);
        println!("Captured string: {:?}", str::from_utf8(captured).unwrap());

        // Decode logic check
        let decoded = decode_rtf(captured);
        println!("Decoded: '{}'", decoded);
    } else {
        println!("No match");
    }
}

fn decode_rtf(raw: &[u8]) -> String {
    let mut decoded_bytes = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'\\' && i + 3 < raw.len() && raw[i + 1] == b'\'' {
            // Hex escape \'XX
            let hex = &raw[i + 2..i + 4];
            println!(
                "Found hex escape at {}: {:?}",
                i,
                str::from_utf8(hex).unwrap()
            );
            if let Ok(byte) = u8::from_str_radix(str::from_utf8(hex).unwrap_or("00"), 16) {
                decoded_bytes.push(byte);
                i += 4;
                continue;
            }
        }
        decoded_bytes.push(raw[i]);
        i += 1;
    }

    decoded_bytes.iter().map(|&b| b as char).collect()
}
