//! Port of `old_src/src/calibre/translations/msgfmt.py` -- compiles a
//! textual gettext `.po` translation catalog into the binary GNU `.mo`
//! format (the same format `python3 msgfmt.py` upstream produces;
//! verified byte-for-byte against a real run of that exact script,
//! since it has no calibre-specific dependencies -- see this module's
//! own tests).
//!
//! # Scope
//!
//! Real: comment/fuzzy-marker handling, `msgctxt`/`msgid`/
//! `msgid_plural`/`msgstr`/`msgstr[N]` sections, the `NON_UNIQUE`/
//! `uniqify` disambiguation upstream's own `add()` does, and the
//! exact binary `.mo` layout (`generate()`) -- header, sorted key/
//! value offset tables, then the null-terminated key and value string
//! pools, all little-endian (matching `struct.pack('Iiiiiii', ...)`'s
//! native byte order on every real platform this crate targets).
//!
//! Narrowed: upstream re-detects the source encoding mid-parse from
//! the catalog's own `Content-Type: ...; charset=X` header entry
//! (starting from a `latin-1` fallback so nothing fails to decode
//! before that's found). This port always treats the `.po` source as
//! UTF-8 -- every real `.po` file in this project (and virtually
//! every `.po` file produced by modern gettext tooling) already is.
//! String-literal escape decoding ([`unescape_po_string`]) covers the
//! escapes gettext catalogs actually use (`\\`, `\"`, `\n`, `\t`,
//! `\r`), not `ast.literal_eval`'s full Python-literal grammar.

use std::collections::{HashMap, HashSet};

/// Port of `STATS`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub translated: u32,
    pub untranslated: u32,
    pub uniqified: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum MsgfmtError {
    #[error("msgid_plural not preceded by msgid on line {0}")]
    PluralWithoutId(usize),
    #[error("plural without msgid_plural on line {0}")]
    IndexedWithoutPlural(usize),
    #[error("indexed msgstr required for plural on line {0}")]
    PluralRequiresIndexed(usize),
    #[error("syntax error on line {0}: {1:?}")]
    Syntax(usize, String),
}

/// Decodes one quoted `.po` string literal (e.g. `"Hello\n"` including
/// the surrounding quotes) the way gettext catalogs use escapes --
/// see this module's own doc for what's covered.
fn unescape_po_string(quoted: &str) -> String {
    let inner = quoted.trim().trim_start_matches('"').trim_end_matches('"');
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

struct Compiler {
    messages: HashMap<Vec<u8>, Vec<u8>>,
    non_unique: HashSet<Vec<u8>>,
    make_unique: bool,
    stats: Stats,
}

#[derive(PartialEq)]
enum Section {
    None,
    Ctxt,
    Id,
    Str,
}

impl Compiler {
    fn new(make_unique: bool) -> Self {
        Compiler { messages: HashMap::new(), non_unique: HashSet::new(), make_unique, stats: Stats::default() }
    }

    /// Port of `add`.
    fn add(&mut self, ctxt: Option<Vec<u8>>, msgid: Vec<u8>, mut msgstr: Vec<u8>, fuzzy: bool) {
        if (!fuzzy || msgid.is_empty()) && !msgstr.is_empty() {
            if !msgid.is_empty() {
                self.stats.translated += 1;
            }
            match ctxt {
                None => {
                    if self.non_unique.contains(&msgstr) {
                        self.stats.uniqified += 1;
                        if self.make_unique {
                            msgstr.extend_from_slice(b" (");
                            msgstr.extend_from_slice(&msgid);
                            msgstr.push(b')');
                        }
                    } else {
                        self.non_unique.insert(msgstr.clone());
                    }
                    self.messages.insert(msgid, msgstr);
                }
                Some(ctxt) => {
                    let mut key = ctxt;
                    key.push(0x04);
                    key.extend_from_slice(&msgid);
                    self.messages.insert(key, msgstr);
                }
            }
        } else if !msgid.is_empty() {
            self.stats.untranslated += 1;
        }
    }

    /// Port of `generate` -- the binary `.mo` layout.
    fn generate(&self) -> Vec<u8> {
        let mut keys: Vec<&Vec<u8>> = self.messages.keys().collect();
        keys.sort();

        let mut ids = Vec::new();
        let mut strs = Vec::new();
        let mut offsets = Vec::with_capacity(keys.len());
        for id in &keys {
            let value = &self.messages[*id];
            offsets.push((ids.len(), id.len(), strs.len(), value.len()));
            ids.extend_from_slice(id);
            ids.push(0);
            strs.extend_from_slice(value);
            strs.push(0);
        }

        let keystart = 7 * 4 + 16 * keys.len();
        let valuestart = keystart + ids.len();
        let mut table = Vec::with_capacity(keys.len() * 4);
        for (o1, l1, _, _) in &offsets {
            table.push(*l1 as i32);
            table.push((*o1 + keystart) as i32);
        }
        for (_, _, o2, l2) in &offsets {
            table.push(*l2 as i32);
            table.push((*o2 + valuestart) as i32);
        }

        let mut out = Vec::with_capacity(28 + table.len() * 4 + ids.len() + strs.len());
        out.extend_from_slice(&0x950412de_u32.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&(keys.len() as i32).to_le_bytes());
        out.extend_from_slice(&(7 * 4_i32).to_le_bytes());
        out.extend_from_slice(&((7 * 4 + keys.len() * 8) as i32).to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        for v in &table {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&ids);
        out.extend_from_slice(&strs);
        out
    }
}

/// Port of `make`/`make_with_stats`: compiles `po_source` (the text
/// of a `.po` file) into the binary `.mo` format, returning the bytes
/// and the same translated/untranslated/uniqified counts upstream's
/// `--statistics` flag reports. `make_unique` matches upstream's
/// `uniqify` mode (disambiguate colliding translations by appending
/// ` (msgid)`).
pub fn compile_po(po_source: &str, make_unique: bool) -> Result<(Vec<u8>, Stats), MsgfmtError> {
    let mut compiler = Compiler::new(make_unique);

    let mut section = Section::None;
    let mut fuzzy = false;
    let mut msgctxt: Option<Vec<u8>> = None;
    let mut msgid: Vec<u8> = Vec::new();
    let mut msgstr: Vec<u8> = Vec::new();
    let mut is_plural = false;

    for (lno, raw_line) in po_source.lines().enumerate() {
        let lno = lno + 1;
        let l = raw_line;
        if l.is_empty() {
            continue;
        }

        // If we get a comment line after a msgstr, this is a new entry.
        if l.starts_with('#') && section == Section::Str {
            compiler.add(msgctxt.take(), std::mem::take(&mut msgid), std::mem::take(&mut msgstr), fuzzy);
            section = Section::None;
            fuzzy = false;
        }
        if l.starts_with("#,") && l.contains("fuzzy") {
            fuzzy = true;
        }
        if l.starts_with('#') {
            continue;
        }

        let mut rest = l;
        if let Some(r) = l.strip_prefix("msgctxt") {
            if section == Section::Str {
                compiler.add(msgctxt.take(), std::mem::take(&mut msgid), std::mem::take(&mut msgstr), fuzzy);
            }
            section = Section::Ctxt;
            rest = r;
            msgctxt = Some(Vec::new());
        } else if let Some(r) = l.strip_prefix("msgid").filter(|_| !l.starts_with("msgid_plural")) {
            if section == Section::Str {
                compiler.add(msgctxt.take(), std::mem::take(&mut msgid), std::mem::take(&mut msgstr), fuzzy);
            }
            section = Section::Id;
            rest = r;
            msgid.clear();
            msgstr.clear();
            is_plural = false;
        } else if let Some(r) = l.strip_prefix("msgid_plural") {
            if section != Section::Id {
                return Err(MsgfmtError::PluralWithoutId(lno));
            }
            rest = r;
            msgid.push(0);
            is_plural = true;
        } else if let Some(r) = l.strip_prefix("msgstr") {
            section = Section::Str;
            if let Some(after_bracket) = r.strip_prefix('[') {
                if !is_plural {
                    return Err(MsgfmtError::IndexedWithoutPlural(lno));
                }
                rest = after_bracket.split_once(']').map(|(_, r)| r).unwrap_or("");
                if !msgstr.is_empty() {
                    msgstr.push(0);
                }
            } else {
                if is_plural {
                    return Err(MsgfmtError::PluralRequiresIndexed(lno));
                }
                rest = r;
            }
        }

        let rest = rest.trim();
        if rest.is_empty() {
            continue;
        }
        let decoded = unescape_po_string(rest);
        let bytes = decoded.into_bytes();
        match section {
            Section::Ctxt => msgctxt.get_or_insert_with(Vec::new).extend_from_slice(&bytes),
            Section::Id => msgid.extend_from_slice(&bytes),
            Section::Str => msgstr.extend_from_slice(&bytes),
            Section::None => return Err(MsgfmtError::Syntax(lno, rest.to_string())),
        }
    }

    if section == Section::Str {
        compiler.add(msgctxt.take(), msgid, msgstr, fuzzy);
    }

    let stats = compiler.stats;
    Ok((compiler.generate(), stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_a_simple_catalog_matching_the_real_upstream_script() {
        // Cross-validated byte-for-byte against a real run of
        // `python3 old_src/src/calibre/translations/msgfmt.py` on
        // this exact source (that script has no calibre-specific
        // imports -- stdlib only -- so it runs standalone).
        let po = "msgid \"\"\nmsgstr \"\"\n\"Content-Type: text/plain; charset=UTF-8\\n\"\n\nmsgid \"Hello\"\nmsgstr \"Bonjour\"\n\nmsgid \"Goodbye\"\nmsgstr \"Au revoir\"\n";
        let (mo, stats) = compile_po(po, false).unwrap();
        let expected: &[u8] = &[
            0xde, 0x12, 0x04, 0x95, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x1c, 0x00, 0x00, 0x00, 0x34, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x4c, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x4d, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x55, 0x00, 0x00, 0x00, 0x28, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x09, 0x00,
            0x00, 0x00, 0x84, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x8e, 0x00, 0x00, 0x00, 0x00, b'G', b'o', b'o', b'd', b'b', b'y', b'e', 0x00, b'H', b'e', b'l', b'l', b'o', 0x00, b'C', b'o',
            b'n', b't', b'e', b'n', b't', b'-', b'T', b'y', b'p', b'e', b':', b' ', b't', b'e', b'x', b't', b'/', b'p', b'l', b'a', b'i', b'n', b';', b' ', b'c', b'h', b'a', b'r', b's', b'e', b't',
            b'=', b'U', b'T', b'F', b'-', b'8', b'\n', 0x00, b'A', b'u', b' ', b'r', b'e', b'v', b'o', b'i', b'r', 0x00, b'B', b'o', b'n', b'j', b'o', b'u', b'r', 0x00,
        ];
        assert_eq!(mo, expected, "output should be byte-for-byte identical to real msgfmt.py's own output");
        assert_eq!(stats, Stats { translated: 2, untranslated: 0, uniqified: 0 });
    }

    #[test]
    fn skips_fuzzy_entries() {
        let po = "#, fuzzy\nmsgid \"Maybe\"\nmsgstr \"Peut-etre\"\n";
        let (_mo, stats) = compile_po(po, false).unwrap();
        assert_eq!(stats, Stats { translated: 0, untranslated: 1, uniqified: 0 });
    }

    #[test]
    fn uniqifies_colliding_translations_when_asked() {
        let po = "msgid \"A\"\nmsgstr \"Same\"\n\nmsgid \"B\"\nmsgstr \"Same\"\n";
        let (_mo, stats) = compile_po(po, true).unwrap();
        assert_eq!(stats.uniqified, 1);
    }

    #[test]
    fn msgid_plural_without_msgid_is_an_error() {
        let po = "msgid_plural \"x\"\n";
        assert!(matches!(compile_po(po, false), Err(MsgfmtError::PluralWithoutId(_))));
    }

    #[test]
    fn handles_msgctxt() {
        let po = "msgctxt \"menu\"\nmsgid \"Open\"\nmsgstr \"Ouvrir\"\n";
        let (_mo, stats) = compile_po(po, false).unwrap();
        assert_eq!(stats.translated, 1);
    }

    #[test]
    fn decodes_common_escapes() {
        assert_eq!(unescape_po_string("\"a\\nb\\tc\\\\d\\\"e\""), "a\nb\tc\\d\"e");
    }
}
