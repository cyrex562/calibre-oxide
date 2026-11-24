//! Port of `old_src/src/calibre/ebooks/rtf2xml/default_encoding.py`
//! (`DefaultEncoding`).
//!
//! Determines the codepage/platform/default-font-number an RTF
//! document implies, either by scanning [`super::process_tokens`]'s
//! intermediate-format output for the relevant `cw<ri<...>` control-word
//! lines it already parsed out of the header (`check_raw = false`, the
//! normal pipeline mode), or -- for callers that haven't run the
//! tokenizer yet -- by regex-scanning raw RTF text directly
//! (`check_raw = true`, used by the Python's own `__main__` CLI entry
//! point).

use std::collections::HashMap;

use lazy_static::lazy_static;
use regex::Regex;

/// Port of `DefaultEncoding.ENCODINGS`. Maps a handful of Python/system
/// codec names (as used elsewhere in calibre for the input document's
/// detected encoding) to the RTF `\ansicpgNNNN` codepage number that
/// best represents it.
///
/// Note: not all of these codepages are actually supported by the rest
/// of the rtf2xml pipeline -- ported as a literal data table regardless,
/// matching the Python's own comment to that effect.
pub fn encodings_table() -> &'static HashMap<&'static str, &'static str> {
    lazy_static! {
        static ref ENCODINGS: HashMap<&'static str, &'static str> = {
            let mut m = HashMap::new();
            // Special cases
            m.insert("cp1252", "1252");
            m.insert("utf-8", "1252");
            m.insert("ascii", "1252");
            // Normal cases
            m.insert("big5", "950");
            m.insert("cp1250", "1250");
            m.insert("cp1251", "1251");
            m.insert("cp1253", "1253");
            m.insert("cp1254", "1254");
            m.insert("cp1255", "1255");
            m.insert("cp1256", "1256");
            m.insert("shift_jis", "932");
            m.insert("gb2312", "936");
            // Not in RTF 1.9.1 codepage specification
            m.insert("hz", "52936");
            m.insert("iso8859_5", "28595");
            m.insert("iso2022_jp", "50222");
            m.insert("iso2022_kr", "50225");
            m.insert("euc_jp", "51932");
            m.insert("euc_kr", "51949");
            m.insert("gb18030", "54936");
            m
        };
    }
    &ENCODINGS
}

/// Port of the `self.__platform` values (`'Windows'`, `'Macintosh'`,
/// `'IBMPC'`, `'OS/2'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Macintosh,
    IbmPc,
    Os2,
}

impl Platform {
    fn as_str(self) -> &'static str {
        match self {
            Platform::Windows => "Windows",
            Platform::Macintosh => "Macintosh",
            Platform::IbmPc => "IBMPC",
            Platform::Os2 => "OS/2",
        }
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Port of `DefaultEncoding`. Constructed with the *content* to scan
/// (either already-tokenized intermediate-format text, or raw RTF text
/// when `check_raw` is set) rather than a file path, so tests and
/// callers that already have the content in memory don't need a real
/// file on disk -- matches this crate's convention for the rest of the
/// self-contained rtf2xml passes.
pub struct DefaultEncoding {
    content: String,
    check_raw: bool,
    platform: Platform,
    code_page: String,
    default_num: String,
    fetched: bool,
}

impl DefaultEncoding {
    /// Port of `DefaultEncoding.__init__`. `default_encoding` is looked
    /// up in [`encodings_table`], falling back to `"1252"` exactly as
    /// `ENCODINGS.get(default_encoding, '1252')` does.
    pub fn new(content: impl Into<String>, default_encoding: &str, check_raw: bool) -> Self {
        let code_page = encodings_table()
            .get(default_encoding)
            .copied()
            .unwrap_or("1252")
            .to_string();
        DefaultEncoding {
            content: content.into(),
            check_raw,
            platform: Platform::Windows,
            code_page,
            default_num: "not-defined".to_string(),
            fetched: false,
        }
    }

    fn ensure_fetched(&mut self) {
        if self.fetched {
            return;
        }
        self.compute_encoding();
        self.fetched = true;
    }

    /// Port of `DefaultEncoding.find_default_encoding`: returns
    /// `(platform, "ansicpgNNNN", default_num)`.
    pub fn find_default_encoding(&mut self) -> (Platform, String, String) {
        self.ensure_fetched();
        (
            self.platform,
            format!("ansicpg{}", self.code_page),
            self.default_num.clone(),
        )
    }

    /// Port of `DefaultEncoding.get_codepage`.
    pub fn get_codepage(&mut self) -> String {
        self.ensure_fetched();
        self.code_page.clone()
    }

    /// Port of `DefaultEncoding.get_platform`.
    pub fn get_platform(&mut self) -> Platform {
        self.ensure_fetched();
        self.platform
    }

    /// Port of `DefaultEncoding._encoding`.
    fn compute_encoding(&mut self) {
        if !self.check_raw {
            let mut cp_found = false;
            for line in self.content.lines() {
                let token_info: &str = if line.len() >= 16 { &line[..16] } else { line };
                if token_info == "mi<mk<rtfhed-end" {
                    break;
                }
                if token_info == "cw<ri<macintosh_" {
                    self.platform = Platform::Macintosh;
                } else if token_info == "cw<ri<pc________" {
                    self.platform = Platform::IbmPc;
                } else if token_info == "cw<ri<pca_______" {
                    self.platform = Platform::Os2;
                }
                if token_info == "cw<ri<ansi-codpg" && line.len() > 20 {
                    let value = &line[20..];
                    if value.parse::<i64>().unwrap_or(0) != 0 {
                        self.code_page = value.to_string();
                    }
                }
                if token_info == "cw<ri<deflt-font" && line.len() > 20 {
                    self.default_num = line[20..].to_string();
                    cp_found = true;
                }
            }
            if self.platform != Platform::Windows && !cp_found {
                self.code_page = match self.platform {
                    Platform::Macintosh => "10000".to_string(),
                    Platform::IbmPc => "437".to_string(),
                    Platform::Os2 => "850".to_string(),
                    Platform::Windows => unreachable!(),
                };
            }
        } else {
            lazy_static! {
                static ref FENC: Regex = Regex::new(r"\\(mac|pc|ansi|pca)[\\ \{\}\t\n]+").unwrap();
                static ref FENCCP: Regex = Regex::new(r"\\ansicpg(\d+)[\\ \{\}\t\n]+").unwrap();
            }
            let mut enc: Option<String> = None;
            let mut cp_found = false;
            for line in self.content.lines() {
                if let Some(caps) = FENC.captures(line) {
                    enc = Some(caps[1].to_string());
                }
                if let Some(caps) = FENCCP.captures(line) {
                    let cp = &caps[1];
                    if cp.parse::<i64>().unwrap_or(0) == 0 {
                        self.code_page = cp.to_string();
                    }
                    cp_found = true;
                    break;
                }
            }
            if self.platform != Platform::Windows && !cp_found {
                match enc.as_deref() {
                    Some("mac") => self.code_page = "10000".to_string(),
                    Some("pc") => self.code_page = "437".to_string(),
                    Some("pca") => self.code_page = "850".to_string(),
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_windows_1252_when_nothing_found() {
        let mut enc = DefaultEncoding::new("", "ascii", false);
        assert_eq!(enc.get_platform(), Platform::Windows);
        assert_eq!(enc.get_codepage(), "1252");
        let (platform, cp, num) = enc.find_default_encoding();
        assert_eq!(platform, Platform::Windows);
        assert_eq!(cp, "ansicpg1252");
        assert_eq!(num, "not-defined");
    }

    #[test]
    fn detects_macintosh_platform_and_falls_back_codepage() {
        let content = "cw<ri<macintosh_<nu<true\n";
        let mut enc = DefaultEncoding::new(content, "ascii", false);
        assert_eq!(enc.get_platform(), Platform::Macintosh);
        assert_eq!(enc.get_codepage(), "10000");
    }

    #[test]
    fn detects_ibmpc_and_os2_platforms() {
        let mut pc = DefaultEncoding::new("cw<ri<pc________<nu<true\n", "ascii", false);
        assert_eq!(pc.get_platform(), Platform::IbmPc);
        assert_eq!(pc.get_codepage(), "437");

        let mut os2 = DefaultEncoding::new("cw<ri<pca_______<nu<true\n", "ascii", false);
        assert_eq!(os2.get_platform(), Platform::Os2);
        assert_eq!(os2.get_codepage(), "850");
    }

    #[test]
    fn explicit_ansi_codepage_control_word_wins() {
        let content = "cw<ri<ansi-codpg<nu<1257\n";
        let mut enc = DefaultEncoding::new(content, "ascii", false);
        assert_eq!(enc.get_codepage(), "1257");
    }

    #[test]
    fn zero_ansi_codepage_is_ignored() {
        let content = "cw<ri<ansi-codpg<nu<0\n";
        let mut enc = DefaultEncoding::new(content, "ascii", false);
        assert_eq!(enc.get_codepage(), "1252");
    }

    #[test]
    fn deflt_font_sets_default_num_and_suppresses_platform_fallback() {
        // Macintosh platform, but a deflt-font line was found, so the
        // Mac->10000 fallback codepage substitution does not fire.
        let content = "cw<ri<macintosh_<nu<true\ncw<ri<deflt-font<nu<3\n";
        let mut enc = DefaultEncoding::new(content, "cp1250", false);
        let (platform, _cp, num) = enc.find_default_encoding();
        assert_eq!(platform, Platform::Macintosh);
        assert_eq!(num, "3");
        assert_eq!(enc.get_codepage(), "1250");
    }

    #[test]
    fn scan_stops_at_rtfhed_end_marker() {
        let content =
            "cw<ri<macintosh_<nu<true\nmi<mk<rtfhed-end<nu<true\ncw<ri<pc________<nu<true\n";
        let mut enc = DefaultEncoding::new(content, "ascii", false);
        // The IBMPC line comes after the header-end marker, so it must
        // not be seen.
        assert_eq!(enc.get_platform(), Platform::Macintosh);
    }

    #[test]
    fn encodings_table_maps_known_names() {
        let table = encodings_table();
        assert_eq!(table.get("big5"), Some(&"950"));
        assert_eq!(table.get("gb18030"), Some(&"54936"));
        assert_eq!(table.get("cp1252"), Some(&"1252"));
    }

    // ---- check_raw mode ----

    #[test]
    fn raw_mode_reads_ansicpg_directly_from_rtf_source() {
        let content = r"{\rtf1\ansi\ansicpg1251\deff0";
        let mut enc = DefaultEncoding::new(content, "ascii", true);
        // Non-zero codepage found in the regex group means it is
        // reported as-is (not substituted), matching the Python's `if
        // not int(cp): self.__code_page = cp` which only overwrites on
        // a *zero* value -- verified against the source: a genuinely
        // "found" nonzero codepage is otherwise left at whatever
        // ENCODINGS-derived default was set in `__init__`.
        assert_eq!(enc.get_codepage(), "1252");
    }

    #[test]
    fn raw_mode_platform_fallback_is_verified_dead_code() {
        // Verified against a live run of the Python: `_encoding`'s
        // `check_raw` branch never assigns `self.__platform` (only the
        // non-raw branch does), so `self.__platform` is always still
        // `'Windows'` (the `__init__` default) by the time the
        // `if self.__platform != 'Windows' and not cpfound:` fallback
        // guard runs -- making the `enc == 'mac'/'pc'/'pca'` fallback
        // unreachable in `check_raw` mode. A `\mac` marker with no
        // `\ansicpgNNNN` therefore leaves the codepage at its
        // `ENCODINGS`-derived default rather than falling back to
        // `'10000'`, unlike the equivalent non-raw-mode scenario in
        // `detects_macintosh_platform_and_falls_back_codepage` above.
        let content = r"{\rtf1\mac\deff0";
        let mut enc = DefaultEncoding::new(content, "ascii", true);
        assert_eq!(enc.get_platform(), Platform::Windows);
        assert_eq!(enc.get_codepage(), "1252");
    }
}
