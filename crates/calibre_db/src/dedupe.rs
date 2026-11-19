//! Finding the same book recorded differently (tranche 8).
//!
//! Byte-identical content (BLAKE3) and an exact title/author match were all
//! the library could find. Neither sees `The Hobbit`, `Hobbit, The` and
//! `The Hobbit (z-lib.org)` by `J.R.R. Tolkien`, `Tolkien, J. R. R.` and
//! `J. R. R. Tolkien` as one book. This module does, and it says *why* it
//! thinks so, so the user can decide.
//!
//! # Evidence, strongest first
//!
//! | [`Strength`] | Reason | Confidence | Evidence |
//! | --- | --- | --- | --- |
//! | `Content` | `content` | 1.00 | the same file (hash) |
//! | `Identifier` | `identifier` | 0.95-0.97 | an equal, *valid* ISBN (10 and 13 compared as 13), or an equal `asin`/`amazon*`, `goodreads`, `google`, `doi` |
//! | `Exact` | `title_author_exact` | 0.90 | equal normalised title and an equal normalised author |
//! | `Fuzzy` | `title_author_fuzzy` | 0.55-0.80 | see below |
//! | `PdfResave` / `PdfOcr` / `PdfText` | `pdf_resave` / `pdf_ocr_variant` / `pdf_text_match` | 0.85 / 0.80 / 0.60 | the same PDF's pages and text; see [`crate::pdf_similarity`] |
//!
//! The three PDF tiers rank **between `Exact` and `Fuzzy`** (`Fuzzy < PdfText <
//! PdfOcr < PdfResave < Exact`): they are found from page content, not metadata,
//! and stronger than a guess from a title but weaker than a title and author
//! that agree. `?min=pdf` is the lowest of the three.
//!
//! Overlapping evidence merges into one group (union-find) and the group
//! reports its strongest reason, plus every edge that joined it. Asking for a
//! minimum strength drops weaker edges *before* merging, so a fuzzy edge can
//! never bridge two groups the caller did not want fuzzy matches in.
//!
//! # Normalisation
//!
//! Both sides are normalised the same way, so a rule can only ever be wrong
//! symmetrically.
//!
//! **Titles** are Unicode NFKD with combining marks removed (and `ß ø ł æ œ đ
//! þ` folded by hand, since they do not decompose), lowercased, with
//! punctuation collapsed to single spaces and apostrophes dropped
//! (`Ender's` = `Enders`; `&` = `and`). Then:
//!
//! * bracketed *noise* is removed: a domain (`(z-lib.org)`), a format
//!   (`[epub]`, `(PDF)`), `[Retail]`, `(OCR)`, `(libgen)` ... and a trailing
//!   ` - site.org`. A bracket that says anything else (`(Discworld #3)`,
//!   `(Vol. 2)`) is **kept**;
//! * one leading article is removed, and the `title_sort` form `Hobbit, The`
//!   is put back first. The articles are calibre's
//!   `per_language_title_sort_articles` defaults for English, Spanish, French,
//!   Italian, German, Dutch and Portuguese, less the ones that are also common
//!   first words in another language (`I`, `O`, `As`, `De`, `Den`, `Des`);
//! * the result is the **exact** key.
//!
//! The **core** key is the exact key with the things that vary between
//! editions of one book also removed: brackets that carry no number (`(Penguin
//! Classics)`, `(Illustrated)`), a bare bracketed year, and an `Nth/revised/...
//! edition` phrase. The **short** key is the core key without its subtitle (the
//! text after `:` or ` - `).
//!
//! **Volumes are never normalised away.** `Vol. 1`/`Vol. 2`, `Part I`/`Part
//! II`, `Book One`/`Book Two`, `Dune: Part Two` all keep their numbers in the
//! exact and core keys, a subtitle containing a number or a volume word is
//! never dropped, and a small-edit-distance match needs the two titles'
//! *numeric signatures* (digit runs, Roman numerals, number words) to be equal.
//!
//! **Authors** are split on `&`, `;`, `|` and `and`; `Last, First` is turned
//! round; honorifics (`Dr.`, `PhD`) are dropped; `Jr./Sr./III` are kept apart
//! from the surname; dots and hyphens become spaces, so `J.R.R.`, `J. R. R.`
//! and `J R R` are the same three initials; an upper-case run of 2-4 letters
//! before the surname (`JRR Tolkien`) is read as initials. A book's authors are
//! compared as sets: **any** overlap is a match.
//!
//! # What is not evidence
//!
//! * An author of `Unknown`, `Unknown Author`, `Anonymous`, `Various`, `n/a`,
//!   calibre's translated `_('Unknown')` (a list of common translations), or
//!   nothing. A book with no *specific* author can only be grouped by
//!   content or identifier, never by title. (This is [`is_unknown_author`],
//!   which `grouping.rs` now uses too.)
//! * A junk author or title from [`crate::junk_metadata`] (`Admin`, `Owner`,
//!   `... print job`, a bare `foo.tif` filename, ...): never matching evidence.
//! * A boilerplate title (`Untitled`, `Document1`, `Microsoft Word - x`, only
//!   digits ...), by the same list `grouping.rs` uses.
//! * The same title with a different author. The fuzzy tier needs an author
//!   that is the same person ([`NormAuthor::compatible`]: same surname and
//!   given names that agree, an initial agreeing with the name it abbreviates)
//!   and `John Smith` is not `Jane Smith`.
//! * An identifier shared by more than [`MAX_IDENTIFIER_BUCKET`] books, which
//!   is a bulk-edit accident, not a book.
//!
//! # Fuzzy matching
//!
//! Candidates are blocked by author surname, and only books in one block are
//! compared, so the cost is the sum of squares of block sizes rather than of
//! the library. Within a block, books match when
//!
//! 1. their **core** keys are equal (edition and bracket noise differ): 0.80
//!    with an equal author, 0.70 with an initials-compatible one; or
//! 2. one's **short** key equals the other's **core** key (a subtitle present
//!    on one side only): 0.60-0.65. Two books that *both* have subtitles never
//!    match this way, so `Discworld: Mort` is not `Discworld: Eric`; or
//! 3. their core keys are at least 12 characters and a small edit apart
//!    (Levenshtein <= 2 and <= len/8), with equal numeric signatures: 0.60-0.65,
//!    **unless the titles differ by a distinguishing token** (a number or
//!    version, Roman numeral, single letter, volume/part word, or one of a tiny
//!    list of opposites such as `male`/`female`): `Names - Female` is not
//!    `Names - Male`, `Templates A to H` is not `P to Z`, `1.00` is not `1.01`.
//!    Underscores and runs of separators are already separators in [`plain`],
//!    so `Elven_Names_-_Male` and `Elven Names - Male` are one title.
//!    Blocks with more than [`MAX_EDIT_BLOCK`] distinct titles skip this rule.
//!
//! # Deviations from calibre
//!
//! calibre's `fuzzy_title` (`db/utils.py`) lowercases and strips brackets and
//! some punctuation; `find_identical_books` then intersects *all* authors'
//! books and compares those fuzzy titles. Differences here: diacritics and
//! articles are folded, noise brackets are recognised rather than every
//! bracket stripped (so `(Vol. 2)` survives), authors are matched by any
//! overlap rather than all, Jaro-Winkler is **not** used (it scores two books
//! of one series `Harry Potter and the ...` above 0.9), and a no-author book is
//! never matched on title. The legacy [`crate::utils::find_identical_books`]
//! stays as a faithful port for `legacy.rs`.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

use crate::cache::Cache;

/// An identifier shared by more books than this is treated as a bad bulk
/// edit rather than evidence.
pub const MAX_IDENTIFIER_BUCKET: usize = 50;

/// Blocks with more distinct core titles than this skip the edit-distance
/// rule (still matched on equal keys).
pub const MAX_EDIT_BLOCK: usize = 3000;

/// How strong a piece of evidence is. Ordered: `Fuzzy < Exact < Identifier
/// < Content`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Strength {
    Fuzzy = 1,
    /// The same words on different pages (`pdf_text_match`).
    PdfText = 2,
    /// The same pages, one with a text layer and one without
    /// (`pdf_ocr_variant`).
    PdfOcr = 3,
    /// The same pages and text, differently written (`pdf_resave`).
    PdfResave = 4,
    Exact = 5,
    Identifier = 6,
    Content = 7,
}

impl Strength {
    /// The `reason` string the routes report.
    pub fn reason(self) -> &'static str {
        match self {
            Strength::Content => "content",
            Strength::Identifier => "identifier",
            Strength::Exact => "title_author_exact",
            Strength::Fuzzy => "title_author_fuzzy",
            Strength::PdfResave => "pdf_resave",
            Strength::PdfOcr => "pdf_ocr_variant",
            Strength::PdfText => "pdf_text_match",
        }
    }

    /// Whether this is one of the three page-content tiers.
    pub fn is_pdf(self) -> bool {
        matches!(self, Strength::PdfText | Strength::PdfOcr | Strength::PdfResave)
    }

    /// Parses the `?min=` query value.
    pub fn parse_min(s: &str) -> Option<Strength> {
        match s.trim().to_ascii_lowercase().as_str() {
            "content" => Some(Strength::Content),
            "identifier" => Some(Strength::Identifier),
            "exact" | "title_author_exact" => Some(Strength::Exact),
            "fuzzy" | "title_author_fuzzy" => Some(Strength::Fuzzy),
            // The page-content level: every PDF reason, and everything above.
            "pdf" | "pdf_text_match" => Some(Strength::PdfText),
            "pdf_ocr_variant" => Some(Strength::PdfOcr),
            "pdf_resave" => Some(Strength::PdfResave),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------
// Text folding
// ---------------------------------------------------------------------

/// NFKD, combining marks removed, a few non-decomposing letters folded,
/// lowercased, typographic quotes and dashes made ASCII. Punctuation is
/// otherwise left alone.
pub(crate) fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.nfkd() {
        if is_combining_mark(c) {
            continue;
        }
        match c {
            'ß' => out.push_str("ss"),
            'ø' | 'Ø' => out.push('o'),
            'ł' | 'Ł' => out.push('l'),
            'æ' | 'Æ' => out.push_str("ae"),
            'œ' | 'Œ' => out.push_str("oe"),
            'đ' | 'Đ' | 'ð' | 'Ð' => out.push('d'),
            'þ' | 'Þ' => out.push_str("th"),
            'ı' => out.push('i'),
            '\u{2019}' | '\u{2018}' | '`' | '\u{00B4}' => out.push('\''),
            '\u{2013}' | '\u{2014}' | '\u{2212}' => out.push('-'),
            _ => {
                for l in c.to_lowercase() {
                    out.push(l);
                }
            }
        }
    }
    out
}

/// Alphanumerics only, one space between words. Apostrophes vanish (no
/// space), `&` is `and`, everything else is a separator.
pub(crate) fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        } else if c == '\'' {
            // dropped: "Ender's" -> "enders"
        } else if c == '&' {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str("and");
            pending_space = true;
        } else {
            pending_space = true;
        }
    }
    out
}

// ---------------------------------------------------------------------
// Titles
// ---------------------------------------------------------------------

const ARTICLES: &str = "the|an|a|der|die|das|ein|eine|le|la|les|un|une|el|los|las|il|gli|het|een|um|uma|uno|una";

/// Words that in a bracket mark the file, not the book.
const NOISE_WORDS: &[&str] = &[
    "retail", "epub", "mobi", "azw", "azw3", "kfx", "pdf", "djvu", "djv", "fb2", "ebook", "ebooks", "ocr", "scan", "scanned", "zlib", "libgen", "zlibrary", "calibre", "repack", "kindle", "html", "txt", "rtf", "cbz", "cbr", "converted", "conv", "proofed", "retail", "preview", "sample",
];

/// Words that say "this is volume N of something".
const VOLUME_WORDS: &[&str] = &["vol", "volume", "volumes", "book", "bk", "part", "pt", "tome", "tomo", "band", "no", "number", "issue", "episode", "season", "chapter", "act", "livre", "libro", "teil"];

const NUMBER_WORDS: &[&str] = &[
    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth", "tenth", "zero", "i", "ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x",
];

lazy_static! {
    static ref BRACKET: Regex = Regex::new(r"[\(\[\{]([^\)\]\}]*)[\)\]\}]").unwrap();
    static ref DOMAIN: Regex = Regex::new(r"(?:www\.)?[a-z0-9][a-z0-9\-]*\.(?:org|com|net|ru|info|me|li|is|io|cc|to|xyz|club|biz|site)\b").unwrap();
    static ref TRAILING_SITE: Regex = Regex::new(r"[\s_\-|]+(?:www\.)?[a-z0-9][a-z0-9\-]*\.(?:org|com|net|ru|info|me|li|is|io|cc|to|xyz|club|biz|site)\s*$").unwrap();
    static ref TRAILING_ARTICLE: Regex = Regex::new(&format!(r"^(?P<m>[^:]+?),\s*(?P<a>{ARTICLES})\s*(?P<rest>(?::|\s-\s).*)?$")).unwrap();
    static ref LEADING_ARTICLE: Regex = Regex::new(&format!(r"^(?:(?:{ARTICLES})\s+|l'\s*)")).unwrap();
    static ref EDITION: Regex = Regex::new(r"\b(?:\d+(?:st|nd|rd|th)|first|second|third|fourth|fifth|revised|new|updated|expanded|illustrated|annotated|special|anniversary|definitive|deluxe|international|unabridged|abridged|collectors?|centennial|reissue)(?:\s+and\s+\w+)?\s+(?:edition|ed)\b\.?").unwrap();
    static ref ROMAN: Regex = Regex::new(r"^(?:x{0,3})(?:ix|iv|v?i{0,3})$").unwrap();
}

fn is_noise(inner_folded: &str) -> bool {
    if DOMAIN.is_match(inner_folded) {
        return true;
    }
    let p = plain(inner_folded);
    if p.contains("zlib") || p.contains("z lib") || p.contains("libgen") || p.contains("annas archive") {
        return true;
    }
    p.split(' ').any(|t| NOISE_WORDS.contains(&t))
}

fn is_year_only(inner_folded: &str) -> bool {
    let p = plain(inner_folded);
    p.len() == 4 && p.parse::<u32>().map(|y| (1000..=2100).contains(&y)).unwrap_or(false)
}

/// A token that distinguishes volumes and parts: digits, Roman numerals and
/// number words.
fn is_number_token(t: &str) -> bool {
    t.chars().any(|c| c.is_ascii_digit()) || (!t.is_empty() && ROMAN.is_match(t)) || NUMBER_WORDS.contains(&t)
}

fn has_volume_marker(plain_text: &str) -> bool {
    plain_text.split(' ').any(|t| is_number_token(t) || VOLUME_WORDS.contains(&t))
}

/// The numeric signature compared before any edit-distance match.
fn numeric_signature(core_plain: &str) -> String {
    core_plain.split(' ').filter(|t| is_number_token(t)).collect::<Vec<_>>().join(" ")
}

fn strip_brackets(text: &str, keep: impl Fn(&str) -> bool) -> String {
    BRACKET.replace_all(text, |c: &regex::Captures| if keep(&c[1]) { format!(" {} ", &c[1]) } else { " ".to_string() }).into_owned()
}

/// Articles: flip `Hobbit, The` and drop one leading article.
fn strip_articles(text: &str) -> String {
    let t = text.trim();
    let flipped = match TRAILING_ARTICLE.captures(t) {
        Some(c) => format!("{} {}{}", &c["a"], c["m"].trim(), c.name("rest").map(|m| m.as_str()).unwrap_or("")),
        None => t.to_string(),
    };
    match LEADING_ARTICLE.find(&flipped) {
        Some(m) if flipped[m.end()..].chars().any(|c| c.is_alphanumeric()) => flipped[m.end()..].to_string(),
        _ => flipped,
    }
}

/// A title's comparison keys. See the module doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleKeys {
    pub exact: String,
    pub core: String,
    /// `Some` only when dropping a subtitle changed `core`.
    pub short: Option<String>,
}

impl TitleKeys {
    fn signature(&self) -> String {
        numeric_signature(&self.core)
    }
}

/// The keys for `title`, or `None` when the title carries no information
/// (empty, `Untitled`, only digits ...).
pub fn title_keys(title: &str) -> Option<TitleKeys> {
    if crate::junk_metadata::is_junk_title(title) {
        return None;
    }
    // Underscores are word separators in filenames (`Elven_Names_-_Male`);
    // made spaces first so article stripping and the other rules see words.
    let folded = fold(title.trim()).replace('_', " ");
    let base = TRAILING_SITE.replace(&folded, "").into_owned();

    let exact_text = strip_articles(&strip_brackets(&base, |inner| !is_noise(inner)));
    let exact = plain(&exact_text);
    if exact.is_empty() || crate::grouping::is_boilerplate(&exact) {
        return None;
    }

    let core_text = strip_articles(&strip_brackets(&base, |inner| !is_noise(inner) && !is_year_only(inner) && has_volume_marker(&plain(inner))));
    let core_text = EDITION.replace_all(&core_text, " ").into_owned();
    let core = plain(&core_text);
    let core = if core.is_empty() { exact.clone() } else { core };

    // Subtitle: text after the first ':' or ' - ', dropped only when it
    // names no volume. The split is on the punctuated text.
    let split_at = [core_text.find(':'), core_text.find(" - ")].into_iter().flatten().min();
    let short = split_at.and_then(|i| {
        let (head, tail) = core_text.split_at(i);
        let head = plain(head);
        if head.is_empty() || has_volume_marker(&plain(tail)) {
            return None;
        }
        (head != core).then_some(head)
    });
    Some(TitleKeys { exact, core, short })
}

// ---------------------------------------------------------------------
// Authors
// ---------------------------------------------------------------------

const UNKNOWN_AUTHORS: &[&str] = &[
    "", "unknown", "unknown author", "unknown authors", "anonymous", "anonymus", "various", "various authors", "n a", "na", "none", "null", "author", "authors", "no author", "unspecified",
    // `_('Unknown')` in the languages calibre ships a translation for.
    "unbekannt", "unbekannter autor", "inconnu", "auteur inconnu", "desconocido", "autor desconocido", "sconosciuto", "onbekend", "desconhecido", "nieznany", "okand", "ukjent", "ukendt", "tuntematon", "necunoscut", "neznamy", "ismeretlen", "bilinmeyen", "άγνωστος", "αγνωστος", "неизвестный", "неизвестен", "невідомий", "不明", "未知", "알 수 없음", "알수없음", "לא ידוע", "غير معروف",
];

/// Whether `name` says nothing about who wrote the book. The one rule
/// shared by every duplicate check: such an author is **not evidence**.
pub fn is_unknown_author(name: &str) -> bool {
    let p = plain(&fold(name.trim()));
    UNKNOWN_AUTHORS.contains(&p.as_str()) || crate::junk_metadata::is_junk_author(name)
}

lazy_static! {
    static ref AUTHOR_SPLIT: Regex = Regex::new(r"(?i)\s*(?:&|;|\|)\s*|\s+and\s+").unwrap();
}

const HONORIFICS: &[&str] = &["dr", "prof", "mr", "mrs", "ms", "phd", "md", "sir", "dame", "rev"];
const SUFFIXES: &[&str] = &["jr", "sr", "ii", "iii", "iv"];

/// One author, reduced to what can be compared.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NormAuthor {
    pub given: Vec<String>,
    pub surname: String,
    pub suffix: String,
}

impl NormAuthor {
    /// The exact-match key: `j r r tolkien`.
    pub fn key(&self) -> String {
        let mut parts: Vec<&str> = self.given.iter().map(String::as_str).collect();
        parts.push(&self.surname);
        if !self.suffix.is_empty() {
            parts.push(&self.suffix);
        }
        parts.join(" ")
    }

    /// The blocking key for fuzzy matching.
    fn block(&self) -> String {
        if self.suffix.is_empty() { self.surname.clone() } else { format!("{} {}", self.surname, self.suffix) }
    }

    /// The same person, allowing an initial to stand for the name it
    /// starts. A bare surname is compatible only with a bare surname.
    pub fn compatible(&self, other: &NormAuthor) -> bool {
        if self.surname != other.surname || self.suffix != other.suffix {
            return false;
        }
        if self.given.is_empty() != other.given.is_empty() {
            return false;
        }
        self.given.iter().zip(&other.given).all(|(a, b)| a == b || (a.chars().count() == 1 && b.starts_with(a.as_str())) || (b.chars().count() == 1 && a.starts_with(b.as_str())))
    }
}

fn parse_person(raw: &str) -> Option<NormAuthor> {
    let raw = raw.trim();
    if raw.is_empty() || is_unknown_author(raw) {
        return None;
    }
    // "Last, First" -> "First Last" (exactly one comma; more is ambiguous).
    let reordered = match raw.matches(',').count() {
        1 => {
            let (last, first) = raw.split_once(',').unwrap();
            if first.trim().is_empty() || last.trim().is_empty() {
                raw.replace(',', " ")
            } else {
                format!("{} {}", first.trim(), last.trim())
            }
        }
        _ => raw.replace(',', " "),
    };
    let raw_tokens: Vec<String> = reordered.split(|c: char| c.is_whitespace() || c == '.' || c == '-' || c == '_').filter(|t| !t.is_empty()).map(str::to_string).collect();
    let n = raw_tokens.len();
    let mut tokens: Vec<String> = Vec::new();
    for (i, t) in raw_tokens.iter().enumerate() {
        let letters = t.chars().filter(|c| c.is_alphabetic()).count();
        // `JRR Tolkien`: an upper-case run before the surname is initials.
        let initials_run = i + 1 < n && (2..=4).contains(&letters) && t.chars().all(|c| c.is_uppercase());
        if initials_run {
            tokens.extend(t.chars().map(|c| plain(&fold(&c.to_string()))));
        } else {
            tokens.push(plain(&fold(t)));
        }
    }
    tokens.retain(|t| !t.is_empty() && !HONORIFICS.contains(&t.as_str()));
    // `Smith, John Jr.` turns into `John Jr Smith`: the suffix is lifted
    // out wherever it landed.
    let mut suffix = String::new();
    if let Some(pos) = tokens.iter().rposition(|t| SUFFIXES.contains(&t.as_str())) {
        if pos >= 1 {
            suffix = tokens.remove(pos);
        }
    }
    let surname = tokens.pop()?;
    Some(NormAuthor { given: tokens, surname, suffix })
}

/// The specific authors of a book, normalised and de-duplicated. Empty when
/// the book has none worth comparing.
pub fn parse_authors(authors: &[String]) -> Vec<NormAuthor> {
    let mut out: Vec<NormAuthor> = Vec::new();
    for a in authors {
        for piece in AUTHOR_SPLIT.split(a) {
            if let Some(p) = parse_person(piece) {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// The exact (title, author) keys the add-time check, the import policy and
/// the scan's `title_author_exact` tier all share. Empty for a book with no
/// usable title or no specific author. One key per author, since any
/// overlap matches.
pub fn exact_keys(title: &str, authors: &[String]) -> Vec<(String, String)> {
    let Some(t) = title_keys(title) else { return Vec::new() };
    parse_authors(authors).iter().map(|a| (t.exact.clone(), a.key())).collect()
}

// ---------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------

fn isbn13_check(first12: &[u32]) -> u32 {
    let sum: u32 = first12.iter().enumerate().map(|(i, d)| if i % 2 == 0 { *d } else { d * 3 }).sum();
    (10 - sum % 10) % 10
}

/// One ISBN, validated, as ISBN-13. Hyphens and spaces are ignored. `None`
/// for anything whose checksum fails, the wrong length, `0000000000`-style
/// filler, or a 13-digit number outside the 978/979 prefixes.
pub fn normalize_isbn(raw: &str) -> Option<String> {
    let cleaned: String = raw.chars().filter(|c| !c.is_whitespace() && *c != '-').collect::<String>().to_ascii_uppercase();
    let chars: Vec<char> = cleaned.chars().collect();
    let digits: Vec<u32> = match chars.len() {
        10 => {
            let mut d = Vec::with_capacity(10);
            for (i, c) in chars.iter().enumerate() {
                match c {
                    '0'..='9' => d.push(c.to_digit(10)?),
                    'X' if i == 9 => d.push(10),
                    _ => return None,
                }
            }
            let sum: u32 = d.iter().enumerate().map(|(i, v)| (10 - i as u32) * v).sum();
            if sum % 11 != 0 {
                return None;
            }
            let mut v13: Vec<u32> = vec![9, 7, 8];
            v13.extend_from_slice(&d[..9]);
            let check = isbn13_check(&v13);
            v13.push(check);
            v13
        }
        13 => {
            let d: Vec<u32> = chars.iter().map(|c| c.to_digit(10)).collect::<Option<_>>()?;
            if !(d[..3] == [9, 7, 8] || d[..3] == [9, 7, 9]) || isbn13_check(&d[..12]) != d[12] {
                return None;
            }
            d
        }
        _ => return None,
    };
    // Filler: the body (after the 978/979 prefix, before the check) is one
    // repeated digit.
    let body = &digits[3..12];
    if body.iter().all(|d| *d == body[0]) {
        return None;
    }
    Some(digits.iter().map(|d| char::from_digit(*d, 10).unwrap()).collect())
}

/// The comparable keys for one `identifiers` row, with a confidence.
/// Unrecognised types, and values that do not look like their type, give
/// nothing.
fn identifier_keys(kind: &str, value: &str) -> Vec<(String, f32)> {
    let kind = kind.trim().to_ascii_lowercase();
    let value = value.trim();
    match kind.as_str() {
        "isbn" => value.split([',', ';', '/']).filter_map(normalize_isbn).map(|i| (format!("isbn:{i}"), 0.97)).collect(),
        "asin" | "mobi-asin" | "amazon" => asin_key(value).into_iter().collect(),
        k if k.starts_with("amazon_") => asin_key(value).into_iter().collect(),
        "goodreads" => {
            if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) && value.trim_start_matches('0') != "" {
                vec![(format!("goodreads:{}", value.trim_start_matches('0')), 0.95)]
            } else {
                Vec::new()
            }
        }
        "google" => {
            if value.len() >= 8 && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                vec![(format!("google:{value}"), 0.95)]
            } else {
                Vec::new()
            }
        }
        "doi" => {
            let v = value.to_ascii_lowercase();
            if v.starts_with("10.") && v.contains('/') {
                vec![(format!("doi:{v}"), 0.95)]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

fn asin_key(value: &str) -> Option<(String, f32)> {
    let v = value.trim().to_ascii_uppercase();
    (v.len() == 10 && v.chars().all(|c| c.is_ascii_alphanumeric()) && v.chars().any(|c| c.is_ascii_alphabetic() || c != '0')).then(|| (format!("asin:{v}"), 0.95))
}

// ---------------------------------------------------------------------
// Edit distance
// ---------------------------------------------------------------------

/// Levenshtein distance when it is at most `k`, else `None`. Banded, so a
/// comparison costs O(k * len) rather than O(len^2).
/// Pairs of words that are opposites: titles differing by one of these are
/// different books (`Names - Female` / `Names - Male`). Deliberately tiny;
/// everything else is caught by the data-driven rule in
/// [`titles_differ_by_designator`].
const ANTONYMS: &[(&str, &str)] = &[("male", "female"), ("men", "women"), ("man", "woman"), ("boys", "girls"), ("boy", "girl"), ("his", "her"), ("good", "evil"), ("before", "after")];

/// Whether two core titles (already normalised to space-separated words)
/// differ by a *distinguishing* token, which vetoes an edit-distance match.
///
/// A word present in one title and absent in the other is distinguishing
/// when it is a number or version (`2`, `00`, `01`, `3rd`), a Roman numeral,
/// a number word, a single letter other than the article `a` (`A to H` /
/// `P to Z`), a volume/part/chapter designator, or one half of an
/// [`ANTONYMS`] pair. Typos and spelling variants (`colour`/`color`) are
/// none of these, so they still match.
fn titles_differ_by_designator(a: &str, b: &str) -> bool {
    let a_words: Vec<&str> = a.split(' ').filter(|w| !w.is_empty()).collect();
    let b_words: Vec<&str> = b.split(' ').filter(|w| !w.is_empty()).collect();
    let a_only: Vec<&str> = a_words.iter().filter(|w| !b_words.contains(w)).copied().collect();
    let b_only: Vec<&str> = b_words.iter().filter(|w| !a_words.contains(w)).copied().collect();
    let distinguishing = |w: &&str| is_number_token(w) || VOLUME_WORDS.contains(w) || (w.chars().count() == 1 && *w != "a");
    if a_only.iter().chain(&b_only).any(distinguishing) {
        return true;
    }
    ANTONYMS.iter().any(|(x, y)| (a_only.contains(x) && b_only.contains(y)) || (a_only.contains(y) && b_only.contains(x)))
}

fn levenshtein_within(a: &[char], b: &[char], k: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > k {
        return None;
    }
    let (n, m) = (a.len(), b.len());
    let inf = k + 1;
    let mut prev: Vec<usize> = (0..=m).map(|j| if j <= k { j } else { inf }).collect();
    for i in 1..=n {
        let mut cur = vec![inf; m + 1];
        if i <= k {
            cur[0] = i;
        }
        let lo = i.saturating_sub(k).max(1);
        let hi = (i + k).min(m);
        let mut row_min = cur[0];
        for j in lo..=hi {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let v = (prev[j - 1] + cost).min(prev[j] + 1).min(cur[j - 1] + 1).min(inf);
            cur[j] = v;
            row_min = row_min.min(v);
        }
        if row_min > k {
            return None;
        }
        prev = cur;
    }
    (prev[m] <= k).then_some(prev[m])
}

// ---------------------------------------------------------------------
// Grouping
// ---------------------------------------------------------------------

/// What the matcher needs to know about one book.
#[derive(Debug, Clone, Default)]
pub struct BookRecord {
    pub id: i32,
    pub title: String,
    pub authors: Vec<String>,
    /// `(type, value)` rows of the `identifiers` table.
    pub identifiers: Vec<(String, String)>,
    /// The file names (without extension) of the book's formats. Used only to
    /// tell a title that came from a template (see [`template_leftover_titles`]).
    pub stems: Vec<String>,
}

/// One reason two books were joined.
#[derive(Debug, Clone, PartialEq)]
pub struct Evidence {
    pub a: i32,
    pub b: i32,
    pub strength: Strength,
    pub confidence: f32,
    pub detail: String,
}

/// A set of books believed to be the same book.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub book_ids: Vec<i32>,
    /// The strongest evidence in the group.
    pub strength: Strength,
    /// The highest confidence among its evidence.
    pub confidence: f32,
    pub evidence: Vec<Evidence>,
}

/// At most this many evidence entries are kept per group.
pub const MAX_EVIDENCE: usize = 100;

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind { parent: (0..n).collect() }
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[rb.max(ra)] = ra.min(rb);
        }
    }
}

struct Prepared {
    keys: Option<TitleKeys>,
    sig: String,
    authors: Vec<NormAuthor>,
}

/// Edges from a bucket of books that all share one key: a star on the
/// first member, enough to explain the connectivity.
fn star(ids: &[usize], records: &[BookRecord], strength: Strength, confidence: f32, detail: &str, edges: &mut Vec<Evidence>) {
    if let Some((&anchor, rest)) = ids.split_first() {
        for &other in rest {
            edges.push(Evidence { a: records[anchor].id, b: records[other].id, strength, confidence, detail: detail.to_string() });
        }
    }
}

/// Finds duplicate groups.
///
/// `content_groups` are sets of book ids already known to hold identical
/// files. Only evidence at least as strong as `min` is used.
pub fn find_groups(records: &[BookRecord], content_groups: &[Vec<i32>], min: Strength) -> Vec<Group> {
    find_groups_with(records, content_groups, &[], min)
}

/// [`find_groups`] with extra edges from page-content matching
/// ([`crate::pdf_similarity`]), which the caller computes because it needs
/// the sidecar. Edges naming a book not in `records` are ignored.
pub fn find_groups_with(records: &[BookRecord], content_groups: &[Vec<i32>], pdf_edges: &[crate::pdf_similarity::PdfEdge], min: Strength) -> Vec<Group> {
    find_groups_vetoed(records, content_groups, pdf_edges, &[], min)
}

/// [`find_groups_with`], with the PDFs' fingerprints so that **content can
/// veto a title match**: two books joined only by their titles, whose PDFs are
/// both fingerprinted, are not joined when the PDF comparison does not match
/// them and the content plainly differs (see [`content_contradicts`]). Applied
/// to each title-tier pair *before* grouping, so a chain of title matches
/// breaks at the books that differ instead of gluing 15 volumes of a periodical
/// into one group.
pub fn find_groups_vetoed(records: &[BookRecord], content_groups: &[Vec<i32>], pdf_edges: &[crate::pdf_similarity::PdfEdge], fingerprints: &[crate::pdf_similarity::LibraryFingerprint], min: Strength) -> Vec<Group> {
    let index: HashMap<i32, usize> = records.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
    let mut edges: Vec<Evidence> = Vec::new();

    // Book -> fingerprint row, and which fingerprint pairs the PDF comparison matched.
    let fp_of: HashMap<usize, usize> = fingerprints.iter().enumerate().flat_map(|(f, l)| l.books.iter().filter_map(|b| index.get(b).map(|&i| (i, f))).collect::<Vec<_>>()).collect();
    let matched: HashSet<(usize, usize)> = pdf_edges
        .iter()
        .filter_map(|e| Some((*fp_of.get(index.get(&e.a)?)?, *fp_of.get(index.get(&e.b)?)?)))
        .map(|(a, b)| (a.min(b), a.max(b)))
        .collect();
    let veto = |i: usize, j: usize| -> bool {
        let (Some(&fa), Some(&fb)) = (fp_of.get(&i), fp_of.get(&j)) else { return false };
        fa != fb && !matched.contains(&(fa.min(fb), fa.max(fb))) && content_contradicts(&fingerprints[fa].fingerprint, &fingerprints[fb].fingerprint)
    };

    // Content.
    for g in content_groups {
        let ids: Vec<usize> = g.iter().filter_map(|id| index.get(id).copied()).collect();
        star(&ids, records, Strength::Content, 1.0, "identical file contents", &mut edges);
    }

    // Identifiers.
    if min <= Strength::Identifier {
        let mut buckets: HashMap<String, (f32, Vec<usize>)> = HashMap::new();
        for (i, r) in records.iter().enumerate() {
            let mut seen: HashSet<String> = HashSet::new();
            for (kind, value) in &r.identifiers {
                for (key, conf) in identifier_keys(kind, value) {
                    if seen.insert(key.clone()) {
                        buckets.entry(key).or_insert((conf, Vec::new())).1.push(i);
                    }
                }
            }
        }
        let mut keys: Vec<&String> = buckets.keys().collect();
        keys.sort();
        for key in keys {
            let (conf, ids) = &buckets[key];
            if ids.len() > 1 && ids.len() <= MAX_IDENTIFIER_BUCKET {
                star(ids, records, Strength::Identifier, *conf, &format!("same {key}"), &mut edges);
            }
        }
    }

    // Title and author.
    if min <= Strength::Exact {
        let template_titles = template_leftover_titles(records, content_groups, &index);
        let prepared: Vec<Prepared> = records
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let keys = if template_titles.contains(&i) { None } else { title_keys(&r.title) };
                let sig = keys.as_ref().map(TitleKeys::signature).unwrap_or_default();
                Prepared { keys, sig, authors: parse_authors(&r.authors) }
            })
            .collect();

        let mut exact: HashMap<(String, String), Vec<usize>> = HashMap::new();
        for (i, p) in prepared.iter().enumerate() {
            if let Some(k) = &p.keys {
                for a in &p.authors {
                    let bucket = exact.entry((k.exact.clone(), a.key())).or_default();
                    if bucket.last() != Some(&i) {
                        bucket.push(i);
                    }
                }
            }
        }
        let mut exact_keys: Vec<&(String, String)> = exact.keys().collect();
        exact_keys.sort();
        let mut exact_pairs: HashSet<(usize, usize)> = HashSet::new();
        for key in exact_keys {
            let ids = &exact[key];
            if ids.len() > 1 {
                // Split the bucket into sets whose members' content does not
                // contradict each other (a template title shared by unrelated PDFs).
                let mut sets: Vec<Vec<usize>> = Vec::new();
                for &i in ids {
                    match sets.iter_mut().find(|set| set.iter().all(|&j| !veto(i, j))) {
                        Some(set) => set.push(i),
                        None => sets.push(vec![i]),
                    }
                }
                for set in sets.iter().filter(|set| set.len() > 1) {
                    star(set, records, Strength::Exact, 0.90, &format!("same title \"{}\" and author \"{}\"", key.0, key.1), &mut edges);
                    for w in set.iter() {
                        for v in set.iter() {
                            if w < v {
                                exact_pairs.insert((*w, *v));
                            }
                        }
                    }
                }
            }
        }

        if min <= Strength::Fuzzy {
            fuzzy_edges(records, &prepared, &exact_pairs, &veto, &mut edges);
        }
    }

    // Page content.
    let mut pdf_evidence: Vec<Evidence> = Vec::new();
    for e in pdf_edges {
        if !(index.contains_key(&e.a) && index.contains_key(&e.b)) {
            continue;
        }
        let strength = match e.found.kind {
            crate::pdf_similarity::PdfKind::Resave => Strength::PdfResave,
            crate::pdf_similarity::PdfKind::OcrVariant => Strength::PdfOcr,
            crate::pdf_similarity::PdfKind::TextMatch => Strength::PdfText,
        };
        if strength >= min {
            pdf_evidence.push(Evidence { a: e.a, b: e.b, strength, confidence: e.found.confidence, detail: e.found.detail.clone() });
        }
    }
    edges.extend(complete_linkage(pdf_evidence, content_groups, &index, records.len()));

    // Merge.
    edges.retain(|e| e.strength >= min);
    let mut uf = UnionFind::new(records.len());
    for e in &edges {
        uf.union(index[&e.a], index[&e.b]);
    }
    let mut by_root: HashMap<usize, Vec<Evidence>> = HashMap::new();
    for e in edges {
        by_root.entry(uf.find(index[&e.a])).or_default().push(e);
    }
    let mut groups: Vec<Group> = by_root
        .into_values()
        .map(|mut evidence| {
            evidence.sort_by(|x, y| y.strength.cmp(&x.strength).then(y.confidence.total_cmp(&x.confidence)).then((x.a, x.b).cmp(&(y.a, y.b))));
            let mut ids: Vec<i32> = evidence.iter().flat_map(|e| [e.a, e.b]).collect();
            ids.sort_unstable();
            ids.dedup();
            let strength = evidence.iter().map(|e| e.strength).max().unwrap();
            let confidence = evidence.iter().map(|e| e.confidence).fold(0.0, f32::max);
            evidence.truncate(MAX_EVIDENCE);
            Group { book_ids: ids, strength, confidence, evidence }
        })
        .collect();
    groups.sort_by(|a, b| b.strength.cmp(&a.strength).then(a.book_ids[0].cmp(&b.book_ids[0])));
    groups
}

/// Clusters up to this many may chain (`A~B`, `B~C`) without the third edge.
const CHAIN_ALLOWANCE: usize = 3;

/// Keeps a page-content edge only if its two ends end up in one **clique**.
///
/// Union-find over every pair makes one group of any chain of pairs, and a
/// run of weak "same template" pairs once chained 49 unrelated books together.
/// Page-content evidence therefore joins books by *complete linkage*: edges
/// are taken strongest first, and two clusters merge only when every cross
/// pair has an edge. Books holding identical files are one node (they are
/// already joined by the content tier, and only one of them carries the
/// fingerprint edge). Small clusters (up to [`CHAIN_ALLOWANCE`] nodes in all)
/// may chain, since a scan, its OCR and an optimized copy often have two of the
/// three edges. Returns the edges whose ends share a cluster.
fn complete_linkage(mut pdf: Vec<Evidence>, content_groups: &[Vec<i32>], index: &HashMap<i32, usize>, n: usize) -> Vec<Evidence> {
    if pdf.len() < 3 {
        // One or two edges cannot make a chain longer than the allowance.
        return pdf;
    }
    let mut same_file = UnionFind::new(n);
    for g in content_groups {
        let ids: Vec<usize> = g.iter().filter_map(|id| index.get(id).copied()).collect();
        for w in ids.windows(2) {
            same_file.union(w[0], w[1]);
        }
    }
    pdf.sort_by(|x, y| y.strength.cmp(&x.strength).then(y.confidence.total_cmp(&x.confidence)).then((x.a, x.b).cmp(&(y.a, y.b))));
    let node = |uf: &mut UnionFind, id: i32| uf.find(index[&id]);
    let pair = |a: usize, b: usize| (a.min(b), a.max(b));

    let mut linked: HashSet<(usize, usize)> = HashSet::new();
    let mut ends: Vec<(usize, usize)> = Vec::with_capacity(pdf.len());
    for e in &pdf {
        let (a, b) = (node(&mut same_file, e.a), node(&mut same_file, e.b));
        linked.insert(pair(a, b));
        ends.push((a, b));
    }

    let mut cluster_of: HashMap<usize, usize> = HashMap::new();
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for &(a, b) in &ends {
        for x in [a, b] {
            cluster_of.entry(x).or_insert(x);
            members.entry(x).or_insert_with(|| vec![x]);
        }
    }
    for &(a, b) in &ends {
        let (ca, cb) = (cluster_of[&a], cluster_of[&b]);
        if ca == cb {
            continue;
        }
        let small = members[&ca].len() + members[&cb].len() <= CHAIN_ALLOWANCE;
        let complete = small || members[&ca].iter().all(|x| members[&cb].iter().all(|y| linked.contains(&pair(*x, *y))));
        if complete {
            let moved = members.remove(&cb).unwrap();
            for m in &moved {
                cluster_of.insert(*m, ca);
            }
            members.get_mut(&ca).unwrap().extend(moved);
        }
    }
    pdf.into_iter().zip(ends).filter(|(_, (a, b))| cluster_of[a] == cluster_of[b]).map(|(e, _)| e).collect()
}

/// Whether two *different, unmatched* PDFs plainly contain different things:
/// page counts more than 10% apart (and more than two pages), or both with at
/// least three usable sampled pages, no visual alignment, and text layers that
/// share almost nothing (Jaccard under 0.3). Used only to veto a title match;
/// it never creates one.
pub fn content_contradicts(a: &crate::pdf_similarity::PdfFingerprint, b: &crate::pdf_similarity::PdfFingerprint) -> bool {
    let (pa, pb) = (a.page_count as f32, b.page_count as f32);
    let diff = (pa - pb).abs();
    if diff > 2.0 && diff > 0.10 * pa.max(pb) {
        return true;
    }
    if a.usable_pages() >= 3 && b.usable_pages() >= 3 && crate::pdf_similarity::visual_match(a, b).is_none() {
        if let Some(j) = crate::pdf_similarity::jaccard(&a.minhash, &b.minhash) {
            return a.has_text && b.has_text && j < 0.3;
        }
    }
    false
}

/// A title that is not a title: the same embedded string on at least
/// [`TEMPLATE_TITLE_BOOKS`] books holding *different* files, which does not
/// resemble most of those books' file names. It is what a template left behind
/// (`Nathanael-Roux_Affinity-Publisher_Template-PHB_v0.1` on 196 unrelated
/// PDFs; `Terry Herc - 1d100 Derelict Spaceships - Letter` on several). Such a
/// title is no evidence, as an `Untitled` is none. Returns record indices.
///
/// A title that resembles the books' file names (the 15 volumes of one
/// periodical, `Collected Poems` five times) is a real title and stays; those
/// are for the content veto to separate.
pub fn template_leftover_titles(records: &[BookRecord], content_groups: &[Vec<i32>], index: &HashMap<i32, usize>) -> HashSet<usize> {
    let mut same_file = UnionFind::new(records.len());
    for g in content_groups {
        let ids: Vec<usize> = g.iter().filter_map(|id| index.get(id).copied()).collect();
        for w in ids.windows(2) {
            same_file.union(w[0], w[1]);
        }
    }
    let mut by_title: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, r) in records.iter().enumerate() {
        let key = plain(&fold(&r.title.replace('_', " ")));
        if !key.is_empty() {
            by_title.entry(key).or_default().push(i);
        }
    }
    let words = |s: &str| -> HashSet<String> { plain(&fold(&s.replace('_', " "))).split(' ').filter(|w| !w.is_empty()).map(str::to_string).collect() };
    let mut out = HashSet::new();
    for (key, members) in by_title {
        if members.len() < TEMPLATE_TITLE_BOOKS {
            continue;
        }
        let distinct: HashSet<usize> = members.iter().map(|&i| same_file.find(i)).collect();
        if distinct.len() < TEMPLATE_TITLE_BOOKS {
            continue;
        }
        let title_words = words(&key);
        let resembling = members
            .iter()
            .filter(|&&i| {
                records[i].stems.is_empty() || records[i].stems.iter().any(|stem| {
                    let sw = words(stem);
                    let inter = sw.intersection(&title_words).count();
                    let union = sw.union(&title_words).count().max(1);
                    inter * 2 >= union || (!title_words.is_empty() && title_words.is_subset(&sw)) || (!sw.is_empty() && sw.is_subset(&title_words))
                })
            })
            .count();
        if resembling * 2 <= members.len() {
            out.extend(members);
        }
    }
    out
}

/// Books that must carry one title, with different files, before it is called
/// a template leftover.
pub const TEMPLATE_TITLE_BOOKS: usize = 5;

/// Of two books' author lists, the best way they overlap: `Some(true)` an
/// equal author, `Some(false)` only an initials-compatible one.
fn author_overlap(a: &[NormAuthor], b: &[NormAuthor]) -> Option<bool> {
    let mut best = None;
    for x in a {
        for y in b {
            if x == y {
                return Some(true);
            }
            if x.compatible(y) {
                best = Some(false);
            }
        }
    }
    best
}

fn fuzzy_edges(records: &[BookRecord], prepared: &[Prepared], exact_pairs: &HashSet<(usize, usize)>, veto: &dyn Fn(usize, usize) -> bool, edges: &mut Vec<Evidence>) {
    // Block by surname.
    let mut blocks: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, p) in prepared.iter().enumerate() {
        if p.keys.is_none() {
            continue;
        }
        let mut seen: Vec<String> = Vec::new();
        for a in &p.authors {
            let b = a.block();
            if !seen.contains(&b) {
                blocks.entry(b.clone()).or_default().push(i);
                seen.push(b);
            }
        }
    }
    let mut seen_pairs: HashSet<(usize, usize)> = HashSet::new();
    let mut emit = |i: usize, j: usize, base: f32, rule: &str, edges: &mut Vec<Evidence>| {
        let (lo, hi) = (i.min(j), i.max(j));
        if lo == hi || exact_pairs.contains(&(lo, hi)) {
            return;
        }
        let Some(equal_author) = author_overlap(&prepared[lo].authors, &prepared[hi].authors) else { return };
        if veto(lo, hi) {
            return;
        }
        if !seen_pairs.insert((lo, hi)) {
            return;
        }
        let confidence = if equal_author { base + 0.05 } else { base - 0.05 };
        edges.push(Evidence { a: records[lo].id, b: records[hi].id, strength: Strength::Fuzzy, confidence, detail: format!("{rule}; author {}", if equal_author { "equal" } else { "agrees (initials)" }) });
    };

    let mut block_keys: Vec<&String> = blocks.keys().collect();
    block_keys.sort();
    for bk in block_keys {
        let members = &blocks[bk];
        if members.len() < 2 {
            continue;
        }
        // Books sharing a core key.
        let mut by_core: HashMap<&str, Vec<usize>> = HashMap::new();
        for &i in members {
            by_core.entry(prepared[i].keys.as_ref().unwrap().core.as_str()).or_default().push(i);
        }
        for (core, ids) in &by_core {
            let ids = &ids[..ids.len().min(500)];
            for (x, &i) in ids.iter().enumerate() {
                for &j in &ids[x + 1..] {
                    emit(i, j, 0.75, &format!("titles equal apart from edition/bracket noise (\"{core}\")"), edges);
                }
            }
        }
        // A subtitle on one side only.
        for &i in members {
            let k = prepared[i].keys.as_ref().unwrap();
            if let Some(short) = &k.short {
                if let Some(ids) = by_core.get(short.as_str()) {
                    for &j in ids.iter().take(500) {
                        emit(i, j, 0.60, &format!("one title is the other plus a subtitle (\"{short}\")"), edges);
                    }
                }
            }
        }
        // A small edit apart.
        if by_core.len() <= MAX_EDIT_BLOCK {
            let mut by_sig: HashMap<&str, Vec<(&str, &Vec<usize>)>> = HashMap::new();
            for (core, ids) in &by_core {
                if core.chars().count() >= 12 {
                    by_sig.entry(prepared[ids[0]].sig.as_str()).or_default().push((*core, ids));
                }
            }
            for list in by_sig.values_mut() {
                list.sort_by_key(|(core, _)| core.chars().count());
                let chars: Vec<Vec<char>> = list.iter().map(|(c, _)| c.chars().collect()).collect();
                for x in 0..list.len() {
                    for y in x + 1..list.len() {
                        if chars[y].len() - chars[x].len() > 2 {
                            break;
                        }
                        let len = chars[x].len().max(chars[y].len());
                        if let Some(d) = levenshtein_within(&chars[x], &chars[y], 2) {
                            if d >= 1 && d * 8 <= len && !titles_differ_by_designator(list[x].0, list[y].0) {
                                let rule = format!("titles {d} edit(s) apart (\"{}\" / \"{}\")", list[x].0, list[y].0);
                                for &i in list[x].1.iter().take(500) {
                                    for &j in list[y].1.iter().take(500) {
                                        emit(i, j, if d == 1 { 0.65 } else { 0.60 }, &rule, edges);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// Reading a library
// ---------------------------------------------------------------------

/// Every book's id, title, authors and identifiers: four table scans, no
/// per-book queries.
pub fn load_records(cache: &Cache) -> Result<Vec<BookRecord>> {
    let conn = cache.backend.conn.lock().unwrap();
    let mut authors: HashMap<i32, Vec<String>> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT l.book, a.name FROM books_authors_link l JOIN authors a ON a.id = l.author ORDER BY l.id")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, String>(1)?)))? {
            let (book, name) = row?;
            authors.entry(book).or_default().push(name);
        }
    }
    let mut identifiers: HashMap<i32, Vec<(String, String)>> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT book, type, val FROM identifiers")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))? {
            let (book, kind, val) = row?;
            identifiers.entry(book).or_default().push((kind, val));
        }
    }
    let mut stems: HashMap<i32, Vec<String>> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT book, name FROM data")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, String>(1)?)))? {
            let (book, name) = row?;
            stems.entry(book).or_default().push(name);
        }
    }
    let mut records = Vec::new();
    let mut stmt = conn.prepare("SELECT id, title FROM books ORDER BY id")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, String>(1)?)))? {
        let (id, title) = row?;
        records.push(BookRecord { id, title, authors: authors.remove(&id).unwrap_or_default(), identifiers: identifiers.remove(&id).unwrap_or_default(), stems: stems.remove(&id).unwrap_or_default() });
    }
    Ok(records)
}

/// What the scan knows about a book's PDF, for showing and for choosing
/// which copy to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PdfBookInfo {
    pub page_count: u32,
    pub has_text: bool,
    pub size: i64,
}

/// A scan's groups, and what it knows about the library's PDFs.
pub struct ScanOutcome {
    pub groups: Vec<Group>,
    /// Fingerprinted PDFs, by book.
    pub pdf: HashMap<i32, PdfBookInfo>,
    pub status: crate::pdf_similarity::PdfStatus,
    /// Zero-byte book files, `(book id, format)`. Never grouped as content
    /// duplicates (they all share the hash of nothing); reported separately.
    pub empty_files: Vec<(i32, String)>,
}

/// Every duplicate group in the library at `min` strength or stronger.
pub fn scan_library(cache: &Cache, min: Strength) -> Result<Vec<Group>> {
    Ok(scan_library_full(cache, min)?.groups)
}

/// [`scan_library`], and the PDF facts the response needs. Only PDFs that
/// have been fingerprinted take part in page-content matching; `status` says
/// how many have not.
pub fn scan_library_full(cache: &Cache, min: Strength) -> Result<ScanOutcome> {
    let records = load_records(cache)?;
    let content: Vec<Vec<i32>> = if min <= Strength::Content {
        crate::copy_to_library::scan_library_for_content_duplicates(cache)?
    } else {
        Vec::new()
    };
    let loaded = cache.checksums().pdf_fingerprints()?;
    let mut pdf = HashMap::new();
    for l in &loaded {
        for &book in &l.books {
            pdf.insert(book, PdfBookInfo { page_count: l.fingerprint.page_count, has_text: l.fingerprint.has_text, size: l.size });
        }
    }
    let edges = if min <= Strength::Exact { crate::pdf_similarity::edges_for(&loaded).0 } else { Vec::new() };
    let groups = find_groups_vetoed(&records, &content, &edges, &loaded, min);
    Ok(ScanOutcome { groups, pdf, status: cache.checksums().pdf_status()?, empty_files: cache.checksums().empty_files("format")? })
}

/// Which copy of a PDF group to keep: the one with a text layer, then the
/// one with more pages, then the larger file, then the lowest id. An
/// optimizer shrinks images, so of two re-saves the larger is the original;
/// of a scan and its OCR the text layer is what was added on purpose. A book
/// with no fingerprint ranks last.
pub fn best_pdf_destination(ids: &[i32], pdf: &HashMap<i32, PdfBookInfo>) -> Option<i32> {
    let rank = |id: &i32| pdf.get(id).map(|p| (true, p.has_text, p.page_count, p.size)).unwrap_or((false, false, 0, 0));
    ids.iter().copied().max_by(|a, b| rank(a).cmp(&rank(b)).then(b.cmp(a)))
}

/// An index of a library's exact `(title, author)` keys, for asking "is
/// this book already here" without rescanning.
pub struct ExactIndex {
    map: HashMap<(String, String), Vec<i32>>,
}

impl ExactIndex {
    pub fn build(cache: &Cache) -> Result<Self> {
        Ok(Self::from_records(&load_records(cache)?))
    }

    pub fn from_records(records: &[BookRecord]) -> Self {
        let mut map: HashMap<(String, String), Vec<i32>> = HashMap::new();
        for r in records {
            for key in exact_keys(&r.title, &r.authors) {
                let ids = map.entry(key).or_default();
                if ids.last() != Some(&r.id) {
                    ids.push(r.id);
                }
            }
        }
        ExactIndex { map }
    }

    /// Books with the same normalised title and any overlapping author.
    pub fn lookup(&self, title: &str, authors: &[String]) -> HashSet<i32> {
        exact_keys(title, authors).iter().filter_map(|k| self.map.get(k)).flatten().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: i32, title: &str, authors: &[&str], identifiers: &[(&str, &str)]) -> BookRecord {
        BookRecord { id, title: title.to_string(), authors: authors.iter().map(|s| s.to_string()).collect(), identifiers: identifiers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), stems: Vec::new() }
    }

    fn groups(records: &[BookRecord], min: Strength) -> Vec<Vec<i32>> {
        find_groups(records, &[], min).into_iter().map(|g| g.book_ids).collect()
    }

    /// (title a, authors a, title b, authors b): one group, whatever the tier.
    #[test]
    fn messy_pairs_that_are_the_same_book_group() {
        let cases: &[(&str, &[&str], &str, &[&str])] = &[
            ("The Hobbit", &["J.R.R. Tolkien"], "Hobbit, The", &["J.R.R. Tolkien"]),
            ("The Hobbit", &["J.R.R. Tolkien"], "The Hobbit (z-lib.org)", &["J.R.R. Tolkien"]),
            ("The Hobbit [Retail]", &["J.R.R. Tolkien"], "The Hobbit (epub)", &["J.R.R. Tolkien"]),
            ("The Hobbit - z-lib.org", &["Tolkien"], "A Hobbit", &["Tolkien"]),
            ("The Hobbit", &["Tolkien, J.R.R."], "The Hobbit", &["J. R. R. Tolkien"]),
            ("The Hobbit", &["J R R Tolkien"], "The Hobbit", &["JRR Tolkien"]),
            ("Café Society", &["Marie Dupré"], "Cafe Society", &["Marie Dupre"]),
            ("Ender's Game", &["Orson Scott Card"], "Enders Game", &["Card, Orson Scott"]),
            ("Pride & Prejudice", &["Jane Austen"], "Pride and Prejudice", &["Jane Austen"]),
            ("Sapiens: A Brief History of Humankind", &["Yuval Noah Harari"], "Sapiens", &["Yuval Noah Harari"]),
            ("Dune - Frank Herbert", &["Frank Herbert"], "Dune", &["Frank Herbert"]),
            ("Das Parfum", &["Patrick Süskind"], "Parfum", &["Patrick Suskind"]),
            ("Les Misérables", &["Victor Hugo"], "Les Miserables", &["Hugo, Victor"]),
            ("Dune (Illustrated Edition)", &["Frank Herbert"], "Dune", &["Frank Herbert"]),
            ("Emma (1815)", &["Jane Austen"], "Emma", &["Jane Austen"]),
            ("The Silmarillion, 2nd Edition", &["J. R. R. Tolkien"], "The Silmarillion", &["John Ronald Reuel Tolkien"]),
            ("The Fellowship of the Ring", &["Tolkien"], "The Fellowship of the Ringe", &["Tolkien"]),
            ("Foundation", &["Isaac Asimov"], "Foundation", &["Asimov, Isaac", "Somebody Else"]),
            ("Good Omens", &["Terry Pratchett & Neil Gaiman"], "Good Omens", &["Neil Gaiman"]),
        ];
        for (ta, aa, tb, ab) in cases {
            let recs = vec![rec(1, ta, aa, &[]), rec(2, tb, ab, &[])];
            assert_eq!(groups(&recs, Strength::Fuzzy), vec![vec![1, 2]], "should group: {ta:?} {aa:?} / {tb:?} {ab:?}");
        }
    }

    #[test]
    fn pairs_that_are_different_books_do_not_group() {
        let cases: &[(&str, &[&str], &str, &[&str])] = &[
            ("Collected Poems", &["W. H. Auden"], "Collected Poems", &["Philip Larkin"]),
            ("Collected Poems", &["Unknown"], "Collected Poems", &["Unknown"]),
            ("Collected Poems", &[], "Collected Poems", &[]),
            ("Collected Poems", &["Unknown Author"], "Collected Poems", &["Anonymous"]),
            ("Dune", &["Frank Herbert"], "Dune", &["Unknown"]),
            ("Foundation", &["John Smith"], "Foundation", &["Jane Smith"]),
            ("Foundation", &["Smith, John Sr."], "Foundation", &["John Smith Jr."]),
            ("Harry Potter, Vol. 1", &["J.K. Rowling"], "Harry Potter, Vol. 2", &["J.K. Rowling"]),
            ("Henry IV, Part I", &["William Shakespeare"], "Henry IV, Part II", &["William Shakespeare"]),
            ("The Dark Tower Book One", &["Stephen King"], "The Dark Tower Book Two", &["Stephen King"]),
            ("Dune: Part One", &["Frank Herbert"], "Dune: Part Two", &["Frank Herbert"]),
            ("Dune: Book 1", &["Frank Herbert"], "Dune: Book 2", &["Frank Herbert"]),
            ("Fahrenheit 451", &["Ray Bradbury"], "Fahrenheit 452", &["Ray Bradbury"]),
            ("Discworld: Mort", &["Terry Pratchett"], "Discworld: Eric", &["Terry Pratchett"]),
            ("Harry Potter and the Chamber of Secrets", &["J.K. Rowling"], "Harry Potter and the Prisoner of Azkaban", &["J.K. Rowling"]),
            ("Document1", &["Jane Austen"], "Document1", &["Jane Austen"]),
            ("Untitled", &["Jane Austen"], "Untitled", &["Jane Austen"]),
            ("1984", &["George Orwell"], "1985", &["George Orwell"]),
        ];
        for (ta, aa, tb, ab) in cases {
            let recs = vec![rec(1, ta, aa, &[]), rec(2, tb, ab, &[])];
            assert_eq!(groups(&recs, Strength::Fuzzy), Vec::<Vec<i32>>::new(), "should NOT group: {ta:?} {aa:?} / {tb:?} {ab:?}");
        }
    }

    #[test]
    fn isbn_10_and_13_of_the_same_book_group_on_the_identifier_alone() {
        // 0261102214 is the ISBN-10 of 9780261102217.
        let recs = vec![rec(1, "A title", &["Someone"], &[("isbn", "0-261-10221-4")]), rec(2, "Completely different", &["Nobody"], &[("isbn", "978 0 261 10221 7")])];
        let g = find_groups(&recs, &[], Strength::Fuzzy);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].book_ids, vec![1, 2]);
        assert_eq!(g[0].strength, Strength::Identifier);
        assert!(g[0].evidence[0].detail.contains("isbn:9780261102217"));
    }

    #[test]
    fn junk_isbns_do_not_group() {
        for junk in ["0000000000", "9780000000002", "1234567890", "9781234567890", "123", "", "978026110221X", "9770261102217"] {
            let recs = vec![rec(1, "A", &[], &[("isbn", junk)]), rec(2, "B", &[], &[("isbn", junk)])];
            assert!(groups(&recs, Strength::Fuzzy).is_empty(), "junk isbn {junk:?} must not group");
        }
    }

    #[test]
    fn isbn_normalisation() {
        assert_eq!(normalize_isbn("0-261-10221-4").as_deref(), Some("9780261102217"));
        assert_eq!(normalize_isbn("978-0-261-10221-7").as_deref(), Some("9780261102217"));
        assert_eq!(normalize_isbn("080442957x").as_deref(), Some("9780804429573"));
        assert_eq!(normalize_isbn("0261102215"), None);
    }

    #[test]
    fn other_strong_identifiers_group_and_weak_ones_do_not() {
        let recs = vec![
            rec(1, "A", &[], &[("asin", "B000FA5ZZ0")]),
            rec(2, "B", &[], &[("amazon", "b000fa5zz0")]),
            rec(3, "C", &[], &[("goodreads", "12345")]),
            rec(4, "D", &[], &[("goodreads", "12345")]),
            rec(5, "E", &[], &[("google", "x")]),
            rec(6, "F", &[], &[("google", "x")]),
            rec(7, "G", &[], &[("isbn", "")]),
            rec(8, "H", &[], &[("isbn", "")]),
            rec(9, "I", &[], &[("goodreads", "0")]),
            rec(10, "J", &[], &[("goodreads", "0")]),
            rec(11, "K", &[], &[("mybadge", "same")]),
            rec(12, "L", &[], &[("mybadge", "same")]),
        ];
        assert_eq!(groups(&recs, Strength::Fuzzy), vec![vec![1, 2], vec![3, 4]]);
    }

    #[test]
    fn an_identifier_shared_by_a_crowd_is_not_evidence() {
        let recs: Vec<BookRecord> = (1..=(MAX_IDENTIFIER_BUCKET as i32 + 1)).map(|i| rec(i, &format!("Book {i}"), &[], &[("isbn", "9780261102217")])).collect();
        assert!(groups(&recs, Strength::Fuzzy).is_empty());
    }

    #[test]
    fn groups_report_the_strongest_reason_and_merge_overlapping_evidence() {
        let recs = vec![
            rec(1, "The Hobbit", &["J.R.R. Tolkien"], &[]),
            rec(2, "Hobbit, The", &["Tolkien, J.R.R."], &[("isbn", "9780261102217")]),
            rec(3, "The Hobbit (Illustrated)", &["J. R. R. Tolkien"], &[]),
            rec(4, "Other", &["Someone Else"], &[("isbn", "0261102214")]),
        ];
        let g = find_groups(&recs, &[], Strength::Fuzzy);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].book_ids, vec![1, 2, 3, 4]);
        assert_eq!(g[0].strength, Strength::Identifier);
        assert!(g[0].evidence.iter().any(|e| e.strength == Strength::Exact));
        assert!(g[0].evidence.iter().any(|e| e.strength == Strength::Identifier));
    }

    #[test]
    fn content_groups_are_the_strongest_and_a_minimum_filters_weaker_edges_before_merging() {
        let recs = vec![rec(1, "A Book", &["X Writer"], &[]), rec(2, "Unrelated", &[], &[]), rec(3, "A Book", &["X Writer"], &[]), rec(4, "Unrelated Too", &[], &[])];
        let g = find_groups(&recs, &[vec![2, 4]], Strength::Fuzzy);
        assert_eq!(g.iter().map(|g| (g.book_ids.clone(), g.strength)).collect::<Vec<_>>(), vec![(vec![2, 4], Strength::Content), (vec![1, 3], Strength::Exact)]);
        assert_eq!(g[0].confidence, 1.0);
        let only_content = find_groups(&recs, &[vec![2, 4]], Strength::Content);
        assert_eq!(only_content.len(), 1);
        let none_exact = find_groups(&[rec(1, "Dune (Illustrated Edition)", &["Frank Herbert"], &[]), rec(2, "Dune", &["Frank Herbert"], &[])], &[], Strength::Exact);
        assert!(none_exact.is_empty(), "a fuzzy-only pair is not reported at min=exact");
    }

    #[test]
    fn a_fuzzy_edge_cannot_bridge_groups_when_only_exact_is_wanted() {
        let recs = vec![
            rec(1, "Dune", &["Frank Herbert"], &[]),
            rec(2, "Dune", &["Frank Herbert"], &[]),
            rec(3, "Dune (Illustrated Edition)", &["Frank Herbert"], &[]),
        ];
        assert_eq!(groups(&recs, Strength::Exact), vec![vec![1, 2]]);
        assert_eq!(groups(&recs, Strength::Fuzzy), vec![vec![1, 2, 3]]);
    }

    #[test]
    fn exact_matches_report_exact_reason_and_confidence() {
        let recs = vec![rec(1, "The Hobbit", &["J.R.R. Tolkien"], &[]), rec(2, "Hobbit, The", &["Tolkien, J.R.R."], &[])];
        let g = find_groups(&recs, &[], Strength::Fuzzy);
        assert_eq!((g[0].strength, g[0].strength.reason()), (Strength::Exact, "title_author_exact"));
        assert!(g[0].confidence >= 0.9);
    }

    /// The trial's false positives: titles one small edit apart that name
    /// different books (underscores are separators, so they are the same
    /// words as the spaced forms).
    #[test]
    fn titles_a_designator_apart_do_not_group() {
        let cases: &[(&str, &str)] = &[
            ("100_Elven_Names_-_Female", "100_Elven_Names_-_Male"),
            ("Dragon_Templates_A_to_H", "Dragon_Templates_P_to_Z"),
            ("Helpful_List_-_Mori_Names_-_Female", "Helpful_List_-_Mori_Names_-_Male"),
            ("Some_Magazine_Special__1_00", "Some_Magazine_Special__1_01"),
            ("Monster Manual Players Guide Men Names", "Monster Manual Players Guide Women Names"),
            ("Campaign Setting Handbook 3rd", "Campaign Setting Handbook 4th"),
            ("The Great Adventures Series Volume", "The Great Adventures Series Volumes"),
        ];
        for (ta, tb) in cases {
            let recs = vec![rec(1, ta, &["Some Author"], &[]), rec(2, tb, &["Some Author"], &[])];
            assert!(groups(&recs, Strength::Fuzzy).is_empty(), "must not group: {ta:?} / {tb:?}");
        }
    }

    #[test]
    fn spelling_and_subtitle_variants_still_group() {
        let cases: &[(&str, &str)] = &[
            ("The_Fellowship_of_the_Ring", "Fellowship of the Ringe"),
            ("The Colour of Magic", "The Color of Magic"),
            ("Sapiens_-_A_Brief_History_of_Humankind", "Sapiens"),
            ("the hobbit: there and back again", "Hobbit, The: There and Back Again!"),
        ];
        for (ta, tb) in cases {
            let recs = vec![rec(1, ta, &["Some Author"], &[]), rec(2, tb, &["Some Author"], &[])];
            assert_eq!(groups(&recs, Strength::Fuzzy), vec![vec![1, 2]], "should group: {ta:?} / {tb:?}");
        }
    }

    #[test]
    fn designator_veto_is_data_driven() {
        assert!(titles_differ_by_designator("names female", "names male"));
        assert!(titles_differ_by_designator("templates a to h", "templates p to z"));
        assert!(titles_differ_by_designator("special 1 00", "special 1 01"));
        assert!(!titles_differ_by_designator("colour of magic", "color of magic"));
        assert!(!titles_differ_by_designator("the ring", "ring"));
    }

    #[test]
    fn junk_authors_and_titles_are_not_evidence() {
        // The same junk author on two different books must not group them,
        // and a print-job title never keys a group.
        let recs = vec![rec(1, "Dragon Lore", &["Admin"], &[]), rec(2, "Dragon Lore", &["Admin"], &[])];
        assert!(groups(&recs, Strength::Fuzzy).is_empty());
        let recs = vec![rec(1, "ACDSee 32 print job", &["RDL_python"], &[]), rec(2, "ACDSee 32 print job", &["RDL_python"], &[])];
        assert!(groups(&recs, Strength::Fuzzy).is_empty());
        let recs = vec![rec(1, "F:\\On Hold\\1-45.tif", &["Some Person"], &[]), rec(2, "F:\\On Hold\\1-45.tif", &["Some Person"], &[])];
        assert!(groups(&recs, Strength::Fuzzy).is_empty());
        // Real authors still group.
        let recs = vec![rec(1, "Dragon Lore", &["Ann Lee"], &[]), rec(2, "Dragon Lore", &["Ann Lee"], &[])];
        assert_eq!(groups(&recs, Strength::Fuzzy), vec![vec![1, 2]]);
    }

    fn pdf_edge(a: i32, b: i32) -> crate::pdf_similarity::PdfEdge {
        crate::pdf_similarity::PdfEdge { a, b, found: crate::pdf_similarity::PdfMatch { kind: crate::pdf_similarity::PdfKind::OcrVariant, confidence: 0.75, detail: String::new() } }
    }

    fn pdf_groups(n: i32, edges: &[(i32, i32)], content: &[Vec<i32>]) -> Vec<Vec<i32>> {
        let recs: Vec<BookRecord> = (1..=n).map(|i| rec(i, &format!("Book {i}"), &[&format!("Author {i}")], &[])).collect();
        let edges: Vec<_> = edges.iter().map(|&(a, b)| pdf_edge(a, b)).collect();
        let mut g: Vec<Vec<i32>> = find_groups_with(&recs, content, &edges, Strength::PdfText).into_iter().map(|g| g.book_ids).collect();
        g.sort();
        g
    }

    /// A chain of weak pairs (`1~2`, `2~3`, ... `9~10`) used to be one group
    /// of ten through transitivity. Page-content groups are cliques.
    fn lib_fp(book: i32, pages: u32) -> crate::pdf_similarity::LibraryFingerprint {
        use crate::pdf_similarity::{PageHash, PdfFingerprint};
        let mut bits = [0u64; 4];
        for (k, w) in bits.iter_mut().enumerate() {
            *w = (book as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(k as u32 * 7) ^ (k as u64);
        }
        let fingerprint = PdfFingerprint { page_count: pages, pages: vec![PageHash::new(0, bits)], text_chars: 0, has_text: false, scanned: false, minhash: Vec::new() };
        crate::pdf_similarity::LibraryFingerprint { hash: format!("h{book}"), books: vec![book], fingerprint, size: 1 }
    }

    /// Fifteen volumes of one periodical share a title and author but not
    /// content: their PDFs plainly differ in length, so content vetoes the
    /// title match. Volumes whose content does not contradict still group.
    #[test]
    fn content_vetoes_a_title_match_between_different_pdfs() {
        let recs: Vec<BookRecord> = (1..=4).map(|i| rec(i, "The Civil Engineer and Architect's Journal", &["William Laxton"], &[])).collect();
        let fps: Vec<_> = [(1, 100), (2, 102), (3, 300), (4, 500)].into_iter().map(|(b, p)| lib_fp(b, p)).collect();
        let mut got: Vec<Vec<i32>> = find_groups_vetoed(&recs, &[], &[], &fps, Strength::Fuzzy).into_iter().map(|g| g.book_ids).collect();
        got.sort();
        assert_eq!(got, vec![vec![1, 2]], "only the two of similar length group");
        // Without fingerprints there is nothing to contradict: all four group.
        assert_eq!(groups(&recs, Strength::Fuzzy), vec![vec![1, 2, 3, 4]]);
        // A PDF edge between 1 and 3 (the comparison matched them) overrides the veto.
        let edge = pdf_edge(1, 3);
        let got = find_groups_vetoed(&recs, &[], &[edge], &fps, Strength::Fuzzy);
        assert!(got.iter().any(|g| g.book_ids.contains(&1) && g.book_ids.contains(&3)));
    }

    #[test]
    fn content_veto_applies_to_fuzzy_pairs_too() {
        let recs = vec![rec(1, "Sapiens: A Brief History", &["Yuval Harari"], &[]), rec(2, "Sapiens", &["Yuval Harari"], &[])];
        let fps = vec![lib_fp(1, 200), lib_fp(2, 20)];
        assert!(find_groups_vetoed(&recs, &[], &[], &fps, Strength::Fuzzy).is_empty());
        let fps = vec![lib_fp(1, 200), lib_fp(2, 201)];
        assert_eq!(find_groups_vetoed(&recs, &[], &[], &fps, Strength::Fuzzy).len(), 1);
    }

    /// A title left behind by a template, on many different files that do not
    /// look like it, is not evidence; a real shared title is.
    #[test]
    fn a_template_leftover_title_is_not_evidence() {
        let mut recs: Vec<BookRecord> = (1..=6).map(|i| rec(i, "Nathanael-Roux_Affinity-Publisher_Template-PHB_v0.1", &["Ennead Games"], &[])).collect();
        for (r, stem) in recs.iter_mut().zip(["Tome of Fiends", "Book of Lairs", "Arcane Compendium", "Bestiary Two", "Realm Atlas", "Hero Handbook"]) {
            r.stems = vec![stem.to_string()];
        }
        assert!(groups(&recs, Strength::Fuzzy).is_empty(), "template title grouped unrelated files");
        // The same shape with titles that resemble the file names is a real title.
        let mut poems: Vec<BookRecord> = (1..=6).map(|i| rec(i, "Collected Poems", &["Ann Lee"], &[])).collect();
        for (i, r) in poems.iter_mut().enumerate() {
            r.stems = vec![format!("Collected Poems vol {i}")];
        }
        assert_eq!(groups(&poems, Strength::Fuzzy), vec![vec![1, 2, 3, 4, 5, 6]]);
        // Fewer than five is never called a template, and the same file five times is one file.
        let few: Vec<BookRecord> = recs.iter().take(4).cloned().collect();
        assert_eq!(groups(&few, Strength::Fuzzy).len(), 1);
    }

    #[test]
    fn a_chain_of_pdf_pairs_is_not_one_group() {
        let chain: Vec<(i32, i32)> = (1..10).map(|i| (i, i + 1)).collect();
        let groups = pdf_groups(10, &chain, &[]);
        assert!(groups.iter().all(|g| g.len() <= 3), "{groups:?}");
        assert!(!groups.is_empty());
    }

    #[test]
    fn a_full_clique_of_pdf_pairs_is_one_group() {
        let mut edges = Vec::new();
        for a in 1..=6 {
            for b in a + 1..=6 {
                edges.push((a, b));
            }
        }
        assert_eq!(pdf_groups(6, &edges, &[]), vec![vec![1, 2, 3, 4, 5, 6]]);
    }

    /// A scan, its OCR and an optimized copy may have only two of the three
    /// edges and are still one group; a pair is a group.
    #[test]
    fn a_trio_may_chain_and_a_pair_is_a_group() {
        assert_eq!(pdf_groups(3, &[(1, 2), (2, 3)], &[]), vec![vec![1, 2, 3]]);
        assert_eq!(pdf_groups(2, &[(1, 2)], &[]), vec![vec![1, 2]]);
    }

    /// Books holding identical files are one node: the fingerprint edge
    /// names only one of them, and the clique test must not be defeated by that.
    #[test]
    fn books_with_identical_files_count_as_one_node_in_a_clique() {
        // 1 and 2 are the same file; 3 and 4 are copies of it, as (1,3) (1,4) (3,4).
        assert_eq!(pdf_groups(4, &[(1, 3), (1, 4), (3, 4)], &[vec![1, 2]]), vec![vec![1, 2, 3, 4]]);
    }

    #[test]
    fn fuzzy_matches_carry_a_lower_confidence_than_exact() {
        let recs = vec![rec(1, "Sapiens: A Brief History", &["Yuval Harari"], &[]), rec(2, "Sapiens", &["Yuval Harari"], &[])];
        let g = find_groups(&recs, &[], Strength::Fuzzy);
        assert_eq!(g[0].strength, Strength::Fuzzy);
        assert!(g[0].confidence < 0.9 && g[0].confidence > 0.5);
    }

    #[test]
    fn title_keys_table() {
        let cases = [
            ("The Hobbit (z-lib.org)", "hobbit"),
            ("Hobbit, The", "hobbit"),
            ("A Tale of Two Cities [epub]", "tale of two cities"),
            ("Café", "cafe"),
            ("Ørsted & Søn", "orsted and son"),
            ("Dune (Vol. 2)", "dune vol 2"),
            ("L'Étranger", "etranger"),
            ("Discworld, The: Mort", "discworld mort"),
        ];
        for (title, want) in cases {
            assert_eq!(title_keys(title).unwrap().exact, want, "{title}");
        }
        assert!(title_keys("Untitled").is_none());
        assert!(title_keys("   ").is_none());
        assert!(title_keys("2024").is_none());
        assert_eq!(title_keys("The").unwrap().exact, "the", "an article alone is kept");
    }

    #[test]
    fn author_parsing_table() {
        let key = |s: &str| parse_authors(&[s.to_string()]).into_iter().map(|a| a.key()).collect::<Vec<_>>();
        assert_eq!(key("Tolkien, J.R.R."), vec!["j r r tolkien"]);
        assert_eq!(key("J. R. R. Tolkien"), vec!["j r r tolkien"]);
        assert_eq!(key("JRR Tolkien"), vec!["j r r tolkien"]);
        assert_eq!(key("Dr. Jane Doe, PhD"), vec!["jane doe"]);
        assert_eq!(key("Jean-Paul Sartre"), vec!["jean paul sartre"]);
        assert_eq!(key("A. Author & B. Writer"), vec!["a author", "b writer"]);
        assert_eq!(key("Smith, John Jr."), vec!["john smith jr"]);
        assert_eq!(key("Unknown"), Vec::<String>::new());
        assert_eq!(key("unbekannt"), Vec::<String>::new());
        assert_eq!(key(""), Vec::<String>::new());
    }

    #[test]
    fn author_compatibility() {
        let one = |s: &str| parse_authors(&[s.to_string()]).remove(0);
        assert!(one("J. R. R. Tolkien").compatible(&one("John Ronald Reuel Tolkien")));
        assert!(one("J. Smith").compatible(&one("John Smith")));
        assert!(!one("Jane Smith").compatible(&one("John Smith")));
        assert!(!one("Smith").compatible(&one("John Smith")));
        assert!(!one("John Smith Jr.").compatible(&one("John Smith")));
    }

    #[test]
    fn levenshtein_band() {
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert_eq!(levenshtein_within(&c("kitten sitting"), &c("kitten sitting"), 2), Some(0));
        assert_eq!(levenshtein_within(&c("the fellowship"), &c("the fellowshp"), 2), Some(1));
        assert_eq!(levenshtein_within(&c("abcdefgh"), &c("abcdxxgh"), 2), Some(2));
        assert_eq!(levenshtein_within(&c("abcdefgh"), &c("abxxxxgh"), 2), None);
        assert_eq!(levenshtein_within(&c("abc"), &c("abcdef"), 2), None);
    }

    #[test]
    fn exact_index_is_the_add_time_check() {
        let recs = vec![rec(1, "The Hobbit", &["J.R.R. Tolkien"], &[]), rec(2, "Untitled", &["Unknown"], &[]), rec(3, "Scan0042", &["Unknown"], &[])];
        let idx = ExactIndex::from_records(&recs);
        assert_eq!(idx.lookup("Hobbit, The (epub)", &["Tolkien, J. R. R.".to_string()]), HashSet::from([1]));
        assert!(idx.lookup("The Hobbit", &["Somebody Else".to_string()]).is_empty());
        assert!(idx.lookup("Scan0042", &["Unknown".to_string()]).is_empty(), "an unknown author never matches");
        assert!(idx.lookup("The Hobbit", &[]).is_empty());
    }

    #[test]
    fn min_parses() {
        assert_eq!(Strength::parse_min("fuzzy"), Some(Strength::Fuzzy));
        assert_eq!(Strength::parse_min("EXACT"), Some(Strength::Exact));
        assert_eq!(Strength::parse_min("identifier"), Some(Strength::Identifier));
        assert_eq!(Strength::parse_min("content"), Some(Strength::Content));
        assert_eq!(Strength::parse_min("nope"), None);
    }

    /// 50k generated books: realistic spread of authors, every 25th book
    /// also recorded under a messy second title. Prints the timing.
    /// `cargo test -p calibre_db --lib dedupe::tests::scale -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn scale_50k_books() {
        let mut recs = Vec::with_capacity(52_000);
        let mut id = 0;
        for i in 0..50_000usize {
            id += 1;
            // ~5k authors, so the average block is 10 books and a few
            // prolific ones have hundreds.
            let a = if i % 97 == 0 { i % 7 } else { i % 5000 };
            let title = format!("The {} of {} {}", ["Book", "Tale", "Story", "Chronicle", "Saga", "Legend"][i % 6], ["Winter", "Summer", "Shadows", "Dragons", "Rivers", "Storms", "Glass", "Iron"][(i / 6) % 8], i / 48);
            recs.push(rec(id, &title, &[&format!("Firstname{} Surname{}", a % 31, a)], &[]));
            if i % 25 == 0 {
                id += 1;
                recs.push(rec(id, &format!("{} (z-lib.org)", title.replacen("The ", "", 1) + ", The"), &[&format!("Surname{}, Firstname{}", a, a % 31)], &[]));
            }
        }
        let start = std::time::Instant::now();
        let g = find_groups(&recs, &[], Strength::Fuzzy);
        eprintln!("dedupe scale: {} books -> {} groups in {:?}", recs.len(), g.len(), start.elapsed());
        assert!(g.len() >= 2000, "the planted duplicates were found");
    }
}
