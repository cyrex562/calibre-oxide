use crate::metadata::zip_edit::placeholders;
use crate::metadata::MetaInformation;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::bytes::Regex;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

lazy_static! {
    static ref TITLE_PAT: Regex =
        Regex::new(r"(?si)\{\\info.*?\{\\title(\s*(?:[^\\}]|\\.)*)\}").unwrap();
    static ref AUTHOR_PAT: Regex =
        Regex::new(r"(?si)\{\\info.*?\{\\author(\s*(?:[^\\}]|\\.)*)\}").unwrap();
    static ref COMMENT_PAT: Regex =
        Regex::new(r"(?si)\{\\info.*?\{\\subject(\s*(?:[^\\}]|\\.)*)\}").unwrap();
    static ref TAGS_PAT: Regex =
        Regex::new(r"(?si)\{\\info.*?\{\\category(\s*(?:[^\\}]|\\.)*)\}").unwrap();
    static ref PUBLISHER_PAT: Regex =
        Regex::new(r"(?si)\{\\info.*?\{\\manager(\s*(?:[^\\}]|\\.)*)\}").unwrap();
    static ref CODEPAGE_PAT: Regex = Regex::new(r"\\ansicpg(\d+)").unwrap();
    static ref MATCH_HEX: Regex = Regex::new(r"\\'([0-9a-fA-F]{2})").unwrap();
}

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    stream.seek(SeekFrom::Start(0))?;
    let mut header = [0u8; 5];
    stream.read_exact(&mut header)?;
    if &header != b"{\\rtf" {
        // Not RTF
        return Ok(MetaInformation::default());
    }

    // Read initial chunk to find metadata
    // RTF headers usually in first few KB
    stream.seek(SeekFrom::Start(0))?;
    let mut buffer = Vec::with_capacity(8192);
    stream.take(8192).read_to_end(&mut buffer)?;

    let mut mi = MetaInformation::default();
    mi.title = "Unknown".to_string();

    if let Some(cap) = TITLE_PAT.captures(&buffer) {
        let title_raw = &cap[1];
        let title = decode_rtf_string(title_raw);
        if !title.trim().is_empty() {
            mi.title = title.trim().to_string();
        }
    }

    if let Some(cap) = AUTHOR_PAT.captures(&buffer) {
        let auth_raw = &cap[1];
        let auth_str = decode_rtf_string(auth_raw);
        mi.authors = auth_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }

    if mi.authors.is_empty() {
        mi.authors.push("Unknown".to_string());
    }

    Ok(mi)
}

/// Writes `mi` into an RTF's `{\info}` group, in place (#834).
///
/// Port of `metadata/rtf.py`'s `set_metadata`. RTF has no XML to edit, so
/// each field is a `{\name value}` group inside `{\info}`: replaced where
/// it exists, appended where it does not, and the whole `{\info}` group
/// created if the document has none.
///
/// Field mapping is upstream's: title, `subject` for comments, `author`,
/// `category` for tags, `manager` for publisher.
pub fn set_metadata(path: &Path, mi: &MetaInformation) -> Result<()> {
    let original = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    // RTF is ASCII with escapes, so byte and char positions coincide and
    // lossy conversion cannot lose anything a valid file contains.
    let text = String::from_utf8_lossy(&original).into_owned();

    let mut fields: Vec<(&str, String)> = Vec::new();
    if let Some(title) = placeholders::real_title(&mi.title) {
        fields.push(("title", title.to_string()));
    }
    if let Some(authors) = placeholders::real_authors(&mi.authors) {
        // Comma-joined, which is how the reader splits it back -- unlike
        // the EPUB/PDF writers, whose readers split on `&`.
        fields.push(("author", authors.join(", ")));
    }
    if let Some(comments) = mi.comments.as_ref().filter(|c| !c.trim().is_empty()) {
        fields.push(("subject", comments.to_string()));
    }
    if !mi.tags.is_empty() {
        fields.push(("category", mi.tags.join(", ")));
    }
    if let Some(publisher) = mi.publisher.as_ref().filter(|p| !p.trim().is_empty()) {
        fields.push(("manager", publisher.to_string()));
    }
    if fields.is_empty() {
        return Ok(());
    }

    let updated = rewrite_info(&text, &fields).context("rewriting the RTF info group")?;

    let staging = tempfile::Builder::new().prefix("set-metadata").suffix(".rtf").tempfile_in(path.parent().unwrap_or(Path::new(".")))?;
    std::fs::write(staging.path(), updated.as_bytes())?;
    staging.persist(path).map_err(|e| anyhow::anyhow!("replacing {}: {e}", path.display()))?;
    Ok(())
}

/// The byte range of the `{\info ...}` group's braces, if present.
///
/// Found by brace counting rather than a regex, because the group nests:
/// `{\info{\title X}}` cannot be matched by anything that stops at the
/// first `}`. Escaped braces are skipped so a `\}` inside a value does not
/// end the group early.
fn info_group_range(text: &str) -> Option<(usize, usize)> {
    let start = text.find("{\\info")?;
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                // Skip the escaped character, whatever it is.
                i += 2;
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((start, i + 1));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Replaces or appends each field inside the `{\info}` group.
fn rewrite_info(text: &str, fields: &[(&str, String)]) -> Result<String> {
    match info_group_range(text) {
        Some((start, end)) => {
            let mut group = text[start..end].to_string();
            for (name, value) in fields {
                group = replace_or_append(&group, name, value);
            }
            Ok(format!("{}{}{}", &text[..start], group, &text[end..]))
        }
        None => {
            // No info group at all. It goes immediately after the RTF
            // header, which is where every producer puts it and where
            // readers look first.
            let mut group = String::from("{\\info");
            for (name, value) in fields {
                group.push_str(&format!("{{\\{name} {}}}", encode_rtf_string(value)));
            }
            group.push('}');

            // Inserted immediately after the `\rtf1` control word.
            // Upstream hardcodes byte 6 for this (`src[:6] + md + src[6:]`),
            // which is exactly `{\rtf1`; this finds the word's end instead,
            // so `{\rtf` or a different version number works too. A group
            // placed any earlier would split the header and make the file
            // unopenable -- which is what a first attempt here did.
            let insert_at = match text.find("{\\rtf") {
                Some(at) => {
                    let mut i = at + "{\\rtf".len();
                    let bytes = text.as_bytes();
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    i
                }
                // Not an RTF header at all. Refusing beats writing a group
                // into something that is not RTF.
                None => anyhow::bail!("this does not look like an RTF file: no {{\\rtf header"),
            };
            Ok(format!("{}{}{}", &text[..insert_at], group, &text[insert_at..]))
        }
    }
}

/// Replaces `{\name ...}` inside `group`, appending it if absent.
fn replace_or_append(group: &str, name: &str, value: &str) -> String {
    let encoded = encode_rtf_string(value);
    let needle = format!("{{\\{name}");

    if let Some(at) = find_field(group, &needle) {
        // Scan to this field's own closing brace, honouring escapes.
        let bytes = group.as_bytes();
        let mut depth = 0usize;
        let mut i = at;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => {
                    i += 2;
                    continue;
                }
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return format!("{}{{\\{name} {encoded}}}{}", &group[..at], &group[i + 1..]);
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    // Absent: append just before the group's final `}`.
    match group.rfind('}') {
        Some(close) => format!("{}{{\\{name} {encoded}}}{}", &group[..close], &group[close..]),
        None => format!("{group}{{\\{name} {encoded}}}"),
    }
}

/// Finds `{\name` where the control word ends there, so looking for
/// `{\title` does not match `{\titlepg`.
fn find_field(group: &str, needle: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(offset) = group[from..].find(needle) {
        let at = from + offset;
        let after = group[at + needle.len()..].chars().next();
        match after {
            // A control word ends at a space, a brace, or a backslash.
            Some(c) if c.is_ascii_alphanumeric() => from = at + needle.len(),
            _ => return Some(at),
        }
    }
    None
}

/// Decodes an RTF-escaped metadata value.
///
/// Handles, in order: `\\uNNNN?` unicode escapes, `\\'XX` hex escapes, and
/// the literal escapes `\\\\`, `\\{`, `\\}`. Everything else is treated as
/// Latin-1, which is a 1:1 byte-to-char mapping.
///
/// `\\uNNNN?` used to be skipped with a "complex parsing" note, which meant
/// **any** RTF with a non-ASCII character in its title -- anything Word
/// wrote with an accent in it -- came back with the raw escape text in
/// place of the character.
fn decode_rtf_string(raw: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'\\' && i + 1 < raw.len() {
            // `\uNNNN?`: a signed 16-bit code point, followed by a
            // replacement character for readers that cannot handle it.
            // The `?` is conventional but the spec allows any single
            // character there, so whatever follows the digits is dropped.
            if raw[i + 1] == b'u' {
                let mut j = i + 2;
                let negative = j < raw.len() && raw[j] == b'-';
                if negative {
                    j += 1;
                }
                let digits_start = j;
                while j < raw.len() && raw[j].is_ascii_digit() {
                    j += 1;
                }
                if j > digits_start {
                    if let Ok(value) = std::str::from_utf8(&raw[digits_start..j]).unwrap_or("").parse::<i32>() {
                        // Negative values are how RTF spells code points
                        // above 32767 in a signed field.
                        let code = if negative { 65536 - value } else { value };
                        if let Some(c) = u32::try_from(code).ok().and_then(char::from_u32) {
                            out.push(c);
                        }
                    }
                    // Skip the one-character fallback that follows.
                    if j < raw.len() {
                        j += 1;
                    }
                    i = j;
                    continue;
                }
            }
            // `\'XX` hex escape.
            if raw[i + 1] == b'\'' && i + 3 < raw.len() {
                if let Ok(byte) = u8::from_str_radix(std::str::from_utf8(&raw[i + 2..i + 4]).unwrap_or("00"), 16) {
                    out.push(byte as char);
                    i += 4;
                    continue;
                }
            }
            // A literal backslash, brace, or other escaped character.
            if matches!(raw[i + 1], b'\\' | b'{' | b'}') {
                out.push(raw[i + 1] as char);
                i += 2;
                continue;
            }
        }
        out.push(raw[i] as char);
        i += 1;
    }
    out
}

/// Encodes a metadata value for an RTF `{\\info}` field.
///
/// The inverse of [`decode_rtf_string`]. Non-ASCII becomes `\\uNNNN?`, as
/// upstream's own `encode` does.
///
/// **Also escapes `\\`, `{` and `}`, which upstream's `encode` does not.**
/// Those three are RTF's own control characters: a book whose title
/// contains a brace would otherwise close the `{\\info}` group early and
/// corrupt the file. Writing a title is not worth risking the document
/// for.
fn encode_rtf_string(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            c if c.is_ascii() => out.push(c),
            c => {
                // The trailing `?` is the fallback character a reader that
                // does not understand `\u` shows instead.
                out.push_str(&format!("\\u{}?", c as u32));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_rtf_metadata() {
        // Use raw byte string to ensure backslashes are literal
        let rtf_content = br#"{\rtf1\ansi{\info{\title My Title}{\author Me, Myself}}}"#;
        let mut stream = Cursor::new(rtf_content);
        let mi = get_metadata(&mut stream).unwrap();
        assert_eq!(mi.title, "My Title", "Title mismatch. Got: '{}'", mi.title);
        assert_eq!(mi.authors, vec!["Me", "Myself"]);
    }

    #[test]
    fn test_rtf_escapes() {
        // \'41 = A
        let rtf_content = br#"{\rtf1\ansi{\info{\title \'41 Title}}}"#;
        let mut stream = Cursor::new(rtf_content);
        let mi = get_metadata(&mut stream).unwrap();
        assert_eq!(
            mi.title, "A Title",
            "Title mismatch with escapes. Got: '{}'",
            mi.title
        );
    }

    #[test]
    fn test_no_header() {
        let content = br"Not RTF";
        let mut stream = Cursor::new(content);
        let mi = get_metadata(&mut stream).unwrap();
        assert_eq!(mi.title, "Unknown");
    }
}

#[cfg(test)]
mod set_metadata_tests {
    use super::*;

    fn write_rtf(dir: &tempfile::TempDir, body: &str) -> std::path::PathBuf {
        let path = dir.path().join("book.rtf");
        std::fs::write(&path, body).unwrap();
        path
    }

    fn with_info(dir: &tempfile::TempDir) -> std::path::PathBuf {
        write_rtf(dir, r#"{\rtf1\ansi{\info{\title Old Title}{\author Old Author}{\keywords kept}}\titlepg Body text}"#)
    }

    fn read_back(path: &Path) -> MetaInformation {
        get_metadata(std::fs::File::open(path).unwrap()).unwrap()
    }

    #[test]
    fn title_and_authors_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = with_info(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        mi.authors = vec!["Ann Author".to_string(), "Bob Writer".to_string()];
        set_metadata(&path, &mi).unwrap();

        let got = read_back(&path);
        assert_eq!(got.title, "New Title");
        assert_eq!(got.authors, vec!["Ann Author".to_string(), "Bob Writer".to_string()]);
    }

    /// A field the document did not have is appended rather than dropped.
    #[test]
    fn a_missing_field_is_appended() {
        let dir = tempfile::tempdir().unwrap();
        let path = with_info(&dir);

        let mut mi = MetaInformation::default();
        mi.tags = vec!["sf".to_string()];
        set_metadata(&path, &mi).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r"{\category sf}"), "category should have been added:\n{text}");
    }

    /// `{\title` must not match `{\titlepg`, a different control word that
    /// happens to start the same way.
    #[test]
    fn a_control_word_with_a_shared_prefix_is_not_clobbered() {
        let dir = tempfile::tempdir().unwrap();
        let path = with_info(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        set_metadata(&path, &mi).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r"\titlepg"), "\\titlepg was destroyed:\n{text}");
        assert!(text.contains("Body text"), "the document body was lost:\n{text}");
    }

    /// Fields inside `{\info}` that this does not set must survive.
    #[test]
    fn other_info_fields_survive() {
        let dir = tempfile::tempdir().unwrap();
        let path = with_info(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        set_metadata(&path, &mi).unwrap();

        assert!(std::fs::read_to_string(&path).unwrap().contains(r"{\keywords kept}"));
    }

    /// A document with no `{\info}` group gets one, and stays openable --
    /// the group must land after the `{\rtf1` header, not before it.
    #[test]
    fn a_document_without_an_info_group_gains_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rtf(&dir, r#"{\rtf1\ansi Body text}"#);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        set_metadata(&path, &mi).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(r"{\rtf1"), "the header must stay first:\n{text}");
        assert!(text.contains("Body text"), "the body was lost:\n{text}");
        assert_eq!(read_back(&path).title, "New Title");
    }

    /// A brace in a title would close the `{\info}` group early and corrupt
    /// the file. Upstream's own `encode` does not escape these.
    #[test]
    fn braces_and_backslashes_in_a_value_are_escaped() {
        let dir = tempfile::tempdir().unwrap();
        let path = with_info(&dir);

        let mut mi = MetaInformation::default();
        mi.title = r"Braces {and} a\backslash".to_string();
        set_metadata(&path, &mi).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r"\{and\}"), "braces were not escaped:\n{text}");
        // The document still parses as a whole, and the title comes back.
        assert_eq!(read_back(&path).title, r"Braces {and} a\backslash");
    }

    /// Non-ASCII goes out as `\uNNNN?` and must come back. The reader used
    /// to skip that escape entirely, so any accented title was unreadable.
    #[test]
    fn non_ascii_round_trips_through_a_unicode_escape() {
        let dir = tempfile::tempdir().unwrap();
        let path = with_info(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "Les Misérables — Café".to_string();
        set_metadata(&path, &mi).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r"\u233?"), "é should be a unicode escape:\n{text}");
        assert_eq!(read_back(&path).title, "Les Misérables — Café");
    }

    #[test]
    fn placeholder_metadata_does_not_overwrite_real_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = with_info(&dir);
        let before = std::fs::read(&path).unwrap();

        set_metadata(&path, &MetaInformation::default()).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), before, "a no-op write should not touch the file");
        assert_eq!(read_back(&path).title, "Old Title");
    }

    #[test]
    fn repeated_edits_do_not_accumulate_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = with_info(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "First".to_string();
        set_metadata(&path, &mi).unwrap();
        mi.title = "Second".to_string();
        set_metadata(&path, &mi).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches(r"{\title").count(), 1, "titles accumulated:\n{text}");
        assert_eq!(read_back(&path).title, "Second");
    }
}
