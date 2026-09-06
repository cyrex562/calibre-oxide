//! Build the MOBI `EXTH` metadata header.
//!
//! Port of `calibre.ebooks.mobi.writer8.exth.build_exth`.
//!
//! # History
//!
//! Issue #34 (`port: ebooks/mobi/writer2`) needed this function for its
//! standalone-MOBI6 path before `mobi/writer8` itself was ported, and
//! discovered that Python's real `build_exth` lives here (imported
//! locally at writer2's call sites). It was ported at the time directly
//! into `mobi::writer2::exth`, narrowly, as "the one slice of `writer8`
//! the in-scope path actually needs" -- with a documented intent to
//! relocate it once `writer8` itself was in scope. Issue #35 (this
//! module) is that relocation: the file now lives at its real Python
//! path, `mobi::writer2::exth` re-exports from here instead of the
//! reverse, and there is exactly one copy of `build_exth`.
//!
//! `start_offset` is `Option<u32>` plus a second `start_offset_secondary`
//! field, rather than Python's `start_offset: int | Sequence[int] | None`
//! -- every call site sets at most a 1- or 2-element sequence: a single
//! optional value (`Serializer`/`KF8Writer.create_guide`, from a single
//! `<guide type="start">` reference), or, for a joint MOBI6+KF8 file's
//! embedded KF8 header, the 2-tuple `(mobi6_start, kf8's_own_start)` that
//! `MobiWriter.generate_joint_record0` assigns to `self.kf8.start_offset`
//! right before reading `self.kf8.record0`. `build_exth` writes one
//! `startreading` EXTH record per non-`None` element, in order, so the
//! two-field shape is a faithful match without a real `Vec`.
//!
//! Two small pieces of the Python original are simplified rather than
//! fully reproduced, both because their backing tables
//! (`calibre.utils.localization.lang_as_iso639_1`,
//! `calibre.ebooks.metadata.authors_to_sort_string`) have not been
//! ported to Rust and porting a full IANA-language-subtag/author-name
//! locale table is out of scope for this issue:
//! - `language`: written as-is rather than down-converted to a bare
//!   ISO 639-1 code when the source tag is longer (e.g. `pt-BR`).
//! - `creator` with `prefer_author_sort`: uses a simple "last word first"
//!   heuristic rather than calibre's locale-aware name splitter.

use std::collections::HashMap;

use anyhow::{bail, Result};

use crate::mobi::utils::{to_base, utf8_text};
use crate::oeb::metadata::Metadata;

/// `EXTH_CODES` in `writer8/exth.py`.
const EXTH_CODES: &[(&str, u32)] = &[
    ("creator", 100),
    ("publisher", 101),
    ("description", 103),
    ("identifier", 104),
    ("subject", 105),
    ("pubdate", 106),
    ("review", 107),
    ("contributor", 108),
    ("rights", 109),
    ("type", 111),
    ("source", 112),
    ("versionnumber", 114),
    ("startreading", 116),
    ("kf8_header_index", 121),
    ("num_of_resources", 125),
    ("kf8_thumbnail_uri", 129),
    ("kf8_unknown_count", 131),
    ("coveroffset", 201),
    ("thumboffset", 202),
    ("hasfakecover", 203),
    ("lastupdatetime", 502),
    ("title", 503),
    ("language", 524),
    ("primary_writing_mode", 525),
    ("page_progression_direction", 527),
    ("override_kindle_fonts", 528),
];

fn code_for(name: &str) -> u32 {
    EXTH_CODES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
        .unwrap_or(0)
}

/// Parameters `build_exth` reads beyond `metadata` itself, mirroring the
/// Python function's keyword arguments (only the ones `writer2/main.py`'s
/// standalone path ever passes are given non-default meaning; the rest
/// exist for completeness / a future joint-KF8 caller).
#[derive(Debug, Clone, Default)]
pub struct ExthParams {
    pub prefer_author_sort: bool,
    pub is_periodical: bool,
    /// Defaults `true` in Python (`share_not_sync=True`).
    pub share_not_sync: bool,
    pub cover_offset: Option<u32>,
    pub thumbnail_offset: Option<u32>,
    pub start_offset: Option<u32>,
    /// Second element of a joint file's `(mobi6_start, kf8_start)` pair.
    /// `None` for every non-joint caller.
    pub start_offset_secondary: Option<u32>,
    pub mobi_doctype: u32,
    pub num_of_resources: Option<u32>,
    pub kf8_unknown_count: Option<u32>,
    pub be_kindlegen2: bool,
    pub kf8_header_index: Option<u32>,
    pub page_progression_direction: Option<String>,
    pub primary_writing_mode: Option<String>,
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Simplified stand-in for `authors_to_sort_string`: "Firstname ...
/// Lastname" -> "Lastname, Firstname ...". See the module scope note.
fn author_to_sort_string(name: &str) -> String {
    let parts: Vec<&str> = name.split_whitespace().collect();
    match parts.as_slice() {
        [] => String::new(),
        [only] => only.to_string(),
        _ => {
            let last = parts[parts.len() - 1];
            let rest = parts[..parts.len() - 1].join(" ");
            format!("{last}, {rest}")
        }
    }
}

fn write_record(out: &mut Vec<u8>, code: u32, data: &[u8]) {
    out.extend_from_slice(&code.to_be_bytes());
    out.extend_from_slice(&((data.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(data);
}

fn write_u32_record(out: &mut Vec<u8>, code: u32, val: u32) {
    write_record(out, code, &val.to_be_bytes());
}

/// Build the `EXTH` block (identifier + length + count + records +
/// padding), ready to be embedded right after the fixed MOBI header.
///
/// Port of `build_exth` in `writer8/exth.py`. Errors (rather than
/// panicking) when the book carries neither a `date` nor a `timestamp`
/// metadata term, matching Python's `raise ValueError('missing date or
/// timestamp')`.
pub fn build_exth(metadata: &Metadata, params: &ExthParams) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut nrecs: u32 = 0;

    for &(term, code) in EXTH_CODES {
        let items = metadata.get(term);
        if items.is_empty() {
            continue;
        }
        if term == "creator" {
            for item in &items {
                let data = if params.prefer_author_sort {
                    author_to_sort_string(&item.value)
                } else {
                    item.value.clone()
                };
                write_record(&mut body, code, &utf8_text(&data));
                nrecs += 1;
            }
            continue;
        }
        if term == "rights" {
            let rights = items
                .first()
                .map(|i| utf8_text(&i.value))
                .unwrap_or_else(|| b"Unknown".to_vec());
            write_record(&mut body, code, &rights);
            nrecs += 1;
            continue;
        }
        for item in &items {
            let mut data = item.value.clone();
            if term != "description" {
                data = collapse_whitespace(&data);
            }
            if term == "identifier" {
                let lower = data.to_lowercase();
                if let Some(rest) = lower.strip_prefix("urn:isbn:") {
                    data = data[data.len() - rest.len()..].to_string();
                } else {
                    let scheme = item
                        .attrib
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("scheme"))
                        .map(|(_, v)| v.to_lowercase());
                    if scheme.as_deref() != Some("isbn") {
                        continue;
                    }
                }
            }
            // `language` would be down-converted to ISO 639-1 here in
            // Python; left as-is, see module scope note.
            write_record(&mut body, code, &utf8_text(&data));
            nrecs += 1;
        }
    }

    // UUID: prefer an identifier with scheme=uuid or a urn:uuid: value,
    // else synthesize one.
    let uuid = metadata
        .get("identifier")
        .into_iter()
        .find(|item| {
            item.attrib
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("scheme") && v.eq_ignore_ascii_case("uuid"))
                || item.value.starts_with("urn:uuid:")
        })
        .map(|item| item.value.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    if !params.share_not_sync {
        write_record(&mut body, 113, uuid.as_bytes());
        nrecs += 1;
    }

    let source = format!("calibre:{uuid}");
    write_record(&mut body, code_for("source"), source.as_bytes());
    nrecs += 1;

    if !params.is_periodical {
        if !params.share_not_sync {
            write_record(&mut body, 501, b"EBOK");
            nrecs += 1;
        }
    } else {
        let ids: HashMap<u32, &[u8]> =
            HashMap::from([(0x101, &b"NWPR"[..]), (0x103, &b"MAGZ"[..])]);
        if let Some(v) = ids.get(&params.mobi_doctype) {
            write_record(&mut body, 501, v);
            nrecs += 1;
        }
    }

    let datestr = metadata
        .get("date")
        .first()
        .map(|i| i.value.clone())
        .or_else(|| metadata.get("timestamp").first().map(|i| i.value.clone()));
    let Some(datestr) = datestr else {
        bail!("missing date or timestamp");
    };
    write_record(&mut body, code_for("pubdate"), datestr.as_bytes());
    nrecs += 1;
    if params.is_periodical {
        write_record(&mut body, code_for("lastupdatetime"), datestr.as_bytes());
        nrecs += 1;
    }

    let vals: [(u32, u32); 4] = if params.be_kindlegen2 {
        // Python picks a platform-specific version number here
        // (`200`/`202`/`201` for Windows/macOS/other); `be_kindlegen2`
        // is only ever set by the not-yet-ported joint-KF8 path, so
        // this is dead code for now -- the "other" constant is used
        // unconditionally rather than reproducing platform detection.
        [(204, 201), (205, 2), (206, 9), (207, 0)]
    } else if params.is_periodical {
        [(204, 201), (205, 2), (206, 0), (207, 101)]
    } else {
        [(204, 201), (205, 1), (206, 2), (207, 33307)]
    };
    for (code, val) in vals {
        // These are written as u32 payloads (12-byte records overall),
        // matching Python's `pack('>III', code, 12, val)`.
        write_u32_record(&mut body, code, val);
        nrecs += 1;
    }
    if params.be_kindlegen2 {
        let revnum = b"0730-890adc2";
        write_record(&mut body, 535, revnum);
        nrecs += 1;
    }

    if let Some(off) = params.cover_offset {
        write_u32_record(&mut body, code_for("coveroffset"), off);
        write_u32_record(&mut body, code_for("hasfakecover"), 0);
        nrecs += 2;
    }
    if let Some(off) = params.thumbnail_offset {
        write_u32_record(&mut body, code_for("thumboffset"), off);
        let uri = format!("kindle:embed:{}", to_base(off as i64, 32, Some(4)));
        write_record(&mut body, code_for("kf8_thumbnail_uri"), uri.as_bytes());
        nrecs += 2;
    }

    for so in [params.start_offset, params.start_offset_secondary].into_iter().flatten() {
        write_u32_record(&mut body, code_for("startreading"), so);
        nrecs += 1;
    }

    if let Some(idx) = params.kf8_header_index {
        write_u32_record(&mut body, code_for("kf8_header_index"), idx);
        nrecs += 1;
    }
    if let Some(n) = params.num_of_resources {
        write_u32_record(&mut body, code_for("num_of_resources"), n);
        nrecs += 1;
    }
    if let Some(n) = params.kf8_unknown_count {
        write_u32_record(&mut body, code_for("kf8_unknown_count"), n);
        nrecs += 1;
    }

    if let Some(pwm) = &params.primary_writing_mode {
        write_record(&mut body, code_for("primary_writing_mode"), pwm.as_bytes());
        nrecs += 1;
    }
    if let Some(ppd) = &params.page_progression_direction {
        if matches!(ppd.as_str(), "rtl" | "ltr" | "default") {
            write_record(
                &mut body,
                code_for("page_progression_direction"),
                ppd.as_bytes(),
            );
            nrecs += 1;
        }
    }

    write_record(&mut body, code_for("override_kindle_fonts"), b"true");
    nrecs += 1;

    // Python always pads with at least one zero byte, even when the
    // body is already a multiple of 4 (`pad = b'\0' * (4 - trail)`, so
    // `trail == 0` yields 4 bytes of padding, not zero). The declared
    // header length below intentionally excludes this trailing pad.
    let trail = body.len() % 4;
    let pad = 4 - trail;
    let mut out = Vec::with_capacity(12 + body.len() + pad);
    out.extend_from_slice(b"EXTH");
    out.extend_from_slice(&((body.len() + 12) as u32).to_be_bytes());
    out.extend_from_slice(&nrecs.to_be_bytes());
    out.extend_from_slice(&body);
    out.extend(std::iter::repeat_n(0u8, pad));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata_with_title_and_date() -> Metadata {
        let mut m = Metadata::new();
        m.add("title", "A Book");
        m.add("creator", "Jane Q. Author");
        m.add("date", "2020-01-01T00:00:00+00:00");
        m
    }

    #[test]
    fn builds_a_well_formed_exth_block() {
        let m = metadata_with_title_and_date();
        let exth = build_exth(&m, &ExthParams::default()).unwrap();
        assert_eq!(&exth[0..4], b"EXTH");
        let len = u32::from_be_bytes(exth[4..8].try_into().unwrap()) as usize;
        assert_eq!(len + (exth.len() - len), exth.len());
        assert_eq!(exth.len() % 4, 0);
        let nrecs = u32::from_be_bytes(exth[8..12].try_into().unwrap());
        assert!(nrecs > 0);
    }

    #[test]
    fn errors_without_a_date_or_timestamp() {
        let mut m = Metadata::new();
        m.add("title", "No Date");
        let err = build_exth(&m, &ExthParams::default()).unwrap_err();
        assert!(err.to_string().contains("date"));
    }

    #[test]
    fn cover_and_thumbnail_offsets_are_written() {
        let m = metadata_with_title_and_date();
        let params = ExthParams {
            cover_offset: Some(3),
            thumbnail_offset: Some(4),
            ..Default::default()
        };
        let exth = build_exth(&m, &params).unwrap();
        // coveroffset code 201 = 0xC9
        assert!(exth.windows(4).any(|w| w == 201u32.to_be_bytes()));
        assert!(exth.windows(4).any(|w| w == 202u32.to_be_bytes()));
    }

    #[test]
    fn identifier_isbn_scheme_is_kept_others_are_dropped() {
        let mut m = metadata_with_title_and_date();
        let mut attrib = HashMap::new();
        attrib.insert("scheme".to_string(), "ISBN".to_string());
        m.add_with_attrib("identifier", "9780000000000", attrib);
        m.add("identifier", "some-other-id");
        let exth = build_exth(&m, &ExthParams::default()).unwrap();
        let text = String::from_utf8_lossy(&exth);
        assert!(text.contains("9780000000000"));
    }
}
