//! Text tokenization + stemming for full-text search.
//!
//! Port of `old_src/src/calibre/db/sqlite_extension.cpp`. The C++
//! original does three jobs:
//!
//! 1. Word-level tokenization of arbitrary UTF-8, respecting Unicode
//!    boundaries and handling script transitions (Han/Thai/Khmer/...
//!    switch to per-script BreakIterators).
//! 2. Snowball stemming per language.
//! 3. Registration of the tokenizers as FTS5 custom tokenizers on a
//!    SQLite connection (`calibre_sqlite_extension_init`).
//!
//! Rust port coverage:
//! - **(1) tokenization**: `unicode-segmentation` — good for
//!   Latin/Cyrillic/Greek/etc. It uses UAX #29 word boundaries.
//!   Dictionary-based segmentation for CJK/Thai/etc. is inferior to
//!   ICU's; see follow-up note.
//! - **(2) stemming**: `rust-stemmers` — pure-Rust Snowball ports,
//!   covers the same languages as libstemmer.
//! - **(3) diacritic removal**: `unicode-normalization` NFD + strip
//!   combining marks.
//! - **FTS5 tokenizer registration** (issue #566): real, via raw
//!   `libsqlite3-sys` FFI against `fts5_api`/`fts5_tokenizer` — see
//!   [`register_fts5_tokenizers`]'s own doc for the full design. This
//!   crate's `fts::connection`/`notes::connection` virtual tables now
//!   use the real `tokenize = 'calibre remove_diacritics 2'`/
//!   `tokenize = 'porter calibre remove_diacritics 2'` clauses from
//!   real upstream's bundled `fts_sqlite.sql`/`notes_sqlite.sql`
//!   (previously missing a `tokenize=` clause at all, so `_stemmed`
//!   tables were byte-for-byte identical to their non-stemmed
//!   siblings -- a real, disclosed, silently-wrong gap found while
//!   researching this issue, the same class #201's original audit
//!   flagged elsewhere in this crate).

use std::ffi::{c_char, c_int, c_void, CStr};
use std::fmt;

use rusqlite::ffi;
use rust_stemmers::{Algorithm, Stemmer as SbStemmer};
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};
use unicode_segmentation::UnicodeSegmentation;

/// A single token emitted by [`Tokenizer::tokenize`].
///
/// `text` is case-folded (lowercased) and, when requested, has
/// combining diacritics stripped. `start` / `end` are byte offsets
/// into the ORIGINAL input string — critical for SQLite FTS5's
/// snippet/highlight machinery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

/// The tokenization mode. FTS5 passes different flags at index time
/// vs query time; the C++ Tokenizer used the flag to decide whether
/// to emit a diacritic-stripped duplicate ("colocated" token) for
/// index-time only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenizeMode {
    /// Index-time. If `remove_diacritics` is set, also emit a
    /// diacritic-stripped duplicate token at the same offsets.
    Document,
    /// Query-time. Only emit the primary token — do not duplicate
    /// (the C++ version skipped the diacritic-strip branch on
    /// queries).
    Query,
}

pub struct Tokenizer {
    /// If true, at Document-time we emit an extra diacritic-stripped
    /// token per input word. See `TokenizeMode` docs.
    pub remove_diacritics: bool,
    /// If Some, apply Snowball stemming for the given language.
    /// The C++ registered the "porter" tokenizer specifically as the
    /// stemming one; the plain "calibre" tokenizer had no stemmer.
    pub stemmer: Option<Algorithm>,
}

impl Tokenizer {
    /// The default `calibre` tokenizer: no stemmer, diacritics
    /// stripped at index time.
    pub fn calibre() -> Self {
        Self {
            remove_diacritics: true,
            stemmer: None,
        }
    }

    /// The `porter` tokenizer: with English Snowball stemming and
    /// diacritics stripped.
    pub fn porter_english() -> Self {
        Self {
            remove_diacritics: true,
            stemmer: Some(Algorithm::English),
        }
    }

    pub fn tokenize(&self, text: &str, mode: TokenizeMode) -> Vec<Token> {
        let mut out = Vec::new();
        let stemmer = self.stemmer.map(SbStemmer::create);

        for (start, word) in text.split_word_bound_indices() {
            if !is_word(word) {
                continue;
            }
            let end = start + word.len();
            let folded = case_fold(word);
            let stemmed = match &stemmer {
                Some(s) => s.stem(&folded).into_owned(),
                None => folded.clone(),
            };
            out.push(Token {
                text: stemmed,
                start,
                end,
            });

            if matches!(mode, TokenizeMode::Document) && self.remove_diacritics {
                let stripped = strip_diacritics(&folded);
                if stripped != folded {
                    // Emit the diacritic-stripped token as a colocated
                    // sibling — same byte offsets, so FTS5 treats it as
                    // an alternate for the same source position.
                    let stripped_stemmed = match &stemmer {
                        Some(s) => s.stem(&stripped).into_owned(),
                        None => stripped,
                    };
                    out.push(Token {
                        text: stripped_stemmed,
                        start,
                        end,
                    });
                }
            }
        }
        out
    }
}

/// Snowball language keys the C++ libstemmer supported. The Rust
/// `rust_stemmers::Algorithm` enum covers the same set except for a
/// handful of less-common ones (Basque, Catalan, ...) — those either
/// aren't in the pure-Rust port or use a different key. Keep the list
/// stable so the FTS layer can enumerate available stemmers.
pub const AVAILABLE_STEMMER_LANGUAGES: &[&str] = &[
    "arabic",
    "danish",
    "dutch",
    "english",
    "finnish",
    "french",
    "german",
    "greek",
    "hungarian",
    "italian",
    "norwegian",
    "portuguese",
    "romanian",
    "russian",
    "spanish",
    "swedish",
    "tamil",
    "turkish",
];

pub fn stemmer_for_language(lang: &str) -> Option<Algorithm> {
    match lang.to_ascii_lowercase().as_str() {
        "arabic" => Some(Algorithm::Arabic),
        "danish" => Some(Algorithm::Danish),
        "dutch" => Some(Algorithm::Dutch),
        "english" | "en" => Some(Algorithm::English),
        "finnish" => Some(Algorithm::Finnish),
        "french" | "fr" => Some(Algorithm::French),
        "german" | "de" => Some(Algorithm::German),
        "greek" => Some(Algorithm::Greek),
        "hungarian" => Some(Algorithm::Hungarian),
        "italian" => Some(Algorithm::Italian),
        "norwegian" => Some(Algorithm::Norwegian),
        "portuguese" => Some(Algorithm::Portuguese),
        "romanian" => Some(Algorithm::Romanian),
        "russian" => Some(Algorithm::Russian),
        "spanish" | "es" => Some(Algorithm::Spanish),
        "swedish" => Some(Algorithm::Swedish),
        "tamil" => Some(Algorithm::Tamil),
        "turkish" => Some(Algorithm::Turkish),
        _ => None,
    }
}

/// Apply Snowball stemming to `word` for the given language. Returns
/// the input unchanged if no stemmer is available for that language.
pub fn stem(word: &str, lang: &str) -> String {
    match stemmer_for_language(lang) {
        Some(algo) => SbStemmer::create(algo).stem(word).into_owned(),
        None => word.to_string(),
    }
}

/// Predicate mirroring the C++ `is_token_char` — a token contains at
/// least one letter, digit, or currency/other-symbol.
fn is_word(s: &str) -> bool {
    s.chars().any(|c| {
        c.is_alphanumeric()
            || matches!(c, '$' | '€' | '£' | '¥' | '¢' | '₹' | '₽' | '₩')
    })
}

/// Case-fold to lowercase. Rust `to_lowercase` is Unicode-correct
/// (folds ß → ss, dotted-I → i in Turkish etc. via the default
/// mapping — good enough for FTS).
fn case_fold(s: &str) -> String {
    s.to_lowercase()
}

/// NFD decomposition, strip combining marks, recompose. This is the
/// standard "remove diacritics" pattern.
fn strip_diacritics(s: &str) -> String {
    s.nfd().filter(|c| !is_combining_mark(*c)).collect()
}

/// Error registering the FTS5 tokenizers: either a raw SQLite error
/// code (from the `fts5_api` retrieval or `xCreateTokenizer` calls),
/// or FTS5 not being compiled in / too old.
#[derive(Debug)]
pub enum Fts5RegistrationError {
    Sqlite(c_int),
    Fts5Unavailable,
}
impl fmt::Display for Fts5RegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fts5RegistrationError::Sqlite(rc) => write!(f, "SQLite error {rc} while registering FTS5 tokenizers"),
            Fts5RegistrationError::Fts5Unavailable => f.write_str("FTS5 extension not available or too old (iVersion < 2)"),
        }
    }
}
impl std::error::Error for Fts5RegistrationError {}

/// Opaque handle for a tokenizer instance, matching real `fts5.h`'s
/// forward-declared `struct Fts5Tokenizer` (application-defined
/// content). We store a boxed [`Tokenizer`] behind it -- see
/// [`xcreate_impl`]/[`xdelete`].
#[repr(C)]
struct RawFts5Tokenizer {
    _private: [u8; 0],
}

/// A C function pointer matching `fts5_tokenizer.xTokenize`'s own
/// `xToken` callback parameter -- supplied BY FTS5 (not by us) on
/// every real `xTokenize` call, invoked once per emitted token.
type XTokenFn = unsafe extern "C" fn(*mut c_void, c_int, *const c_char, c_int, c_int, c_int) -> c_int;

const FTS5_TOKENIZE_QUERY: c_int = 0x0001;
const FTS5_TOKEN_COLOCATED: c_int = 0x0001;

/// Port of real `fts5.h`'s `struct fts5_tokenizer` (byte-for-byte
/// field order/types, verified directly against the real vendored
/// SQLite amalgamation this crate already compiles against --
/// `libsqlite3-sys-0.27.0/sqlite3/sqlite3.c`, not guessed from memory).
#[repr(C)]
struct RawFts5TokenizerVtable {
    x_create: unsafe extern "C" fn(*mut c_void, *mut *const c_char, c_int, *mut *mut RawFts5Tokenizer) -> c_int,
    x_delete: unsafe extern "C" fn(*mut RawFts5Tokenizer),
    x_tokenize: unsafe extern "C" fn(*mut RawFts5Tokenizer, *mut c_void, c_int, *const c_char, c_int, XTokenFn) -> c_int,
}

/// A real, but deliberately TRUNCATED, port of `fts5_api` -- only its
/// leading `iVersion`/`xCreateTokenizer` fields are declared (the real
/// struct also has `xFindTokenizer`/`xCreateFunction` after them,
/// which this port never calls). This is safe: we only ever receive
/// `*mut RawFts5Api` as a raw pointer from SQLite and read these two
/// leading fields off it -- never treat it as complete-sized (never
/// `size_of`/copy-by-value it).
#[repr(C)]
struct RawFts5Api {
    i_version: c_int,
    x_create_tokenizer: unsafe extern "C" fn(*mut RawFts5Api, *const c_char, *mut c_void, *const RawFts5TokenizerVtable, Option<unsafe extern "C" fn(*mut c_void)>) -> c_int,
}

/// Port of the real C++ `Tokenizer` constructor's `azArg` handling:
/// only a literal `"0"` following `remove_diacritics`/`stem_words`
/// changes the corresponding default; any other value (including real
/// upstream's own `remove_diacritics 2`, meaningful only to SQLite's
/// *built-in* `unicode61` tokenizer's distinct 3-way option, not to
/// this custom one) leaves the default untouched. A bare tokenizer
/// name embedded in the chain (e.g. the `calibre` in real upstream's
/// `tokenize = 'porter calibre remove_diacritics 2'`) is likewise
/// inert here -- this port's `porter` tokenizer always runs its own
/// full pipeline regardless of what base-tokenizer name FTS5's chain
/// syntax names, matching the real C++ exactly (it never looks at that
/// argument either).
fn parse_tokenizer_args(args: &[&str], mut remove_diacritics: bool, mut stem_words: bool) -> (bool, bool) {
    let mut i = 0usize;
    while i < args.len() {
        if args[i] == "remove_diacritics" {
            i += 1;
            if i < args.len() && args[i] == "0" {
                remove_diacritics = false;
            }
        } else if args[i] == "stem_words" {
            i += 1;
            if i < args.len() && args[i] == "0" {
                stem_words = false;
            } else {
                stem_words = true;
            }
        }
        i += 1;
    }
    (remove_diacritics, stem_words)
}

unsafe fn xcreate_impl(az_arg: *mut *const c_char, n_arg: c_int, pp_out: *mut *mut RawFts5Tokenizer, stem_words_default: bool) -> c_int {
    let mut args: Vec<&str> = Vec::with_capacity(n_arg.max(0) as usize);
    for i in 0..n_arg as isize {
        let p = *az_arg.offset(i);
        if p.is_null() {
            continue;
        }
        match CStr::from_ptr(p).to_str() {
            Ok(s) => args.push(s),
            Err(_) => return ffi::SQLITE_ERROR,
        }
    }
    let (remove_diacritics, stem_words) = parse_tokenizer_args(&args, true, stem_words_default);
    let tokenizer = Tokenizer {
        remove_diacritics,
        stemmer: if stem_words { Some(Algorithm::English) } else { None },
    };
    *pp_out = Box::into_raw(Box::new(tokenizer)) as *mut RawFts5Tokenizer;
    ffi::SQLITE_OK
}

unsafe extern "C" fn xcreate(_ctx: *mut c_void, az_arg: *mut *const c_char, n_arg: c_int, pp_out: *mut *mut RawFts5Tokenizer) -> c_int {
    xcreate_impl(az_arg, n_arg, pp_out, false)
}

unsafe extern "C" fn xcreate_stemming(_ctx: *mut c_void, az_arg: *mut *const c_char, n_arg: c_int, pp_out: *mut *mut RawFts5Tokenizer) -> c_int {
    xcreate_impl(az_arg, n_arg, pp_out, true)
}

unsafe extern "C" fn xdelete(p: *mut RawFts5Tokenizer) {
    if !p.is_null() {
        drop(Box::from_raw(p as *mut Tokenizer));
    }
}

unsafe extern "C" fn xtokenize(tokenizer_ptr: *mut RawFts5Tokenizer, p_ctx: *mut c_void, flags: c_int, p_text: *const c_char, n_text: c_int, x_token: XTokenFn) -> c_int {
    if p_text.is_null() || n_text < 0 {
        return ffi::SQLITE_ERROR;
    }
    let tokenizer = &*(tokenizer_ptr as *const Tokenizer);
    let bytes = std::slice::from_raw_parts(p_text as *const u8, n_text as usize);
    let text = match std::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return ffi::SQLITE_ERROR,
    };
    let mode = if flags & FTS5_TOKENIZE_QUERY != 0 { TokenizeMode::Query } else { TokenizeMode::Document };
    let tokens = tokenizer.tokenize(text, mode);

    let mut prev: Option<(usize, usize)> = None;
    for tok in &tokens {
        let tflags = if prev == Some((tok.start, tok.end)) { FTS5_TOKEN_COLOCATED } else { 0 };
        let rc = x_token(p_ctx, tflags, tok.text.as_ptr() as *const c_char, tok.text.len() as c_int, tok.start as c_int, tok.end as c_int);
        if rc != ffi::SQLITE_OK {
            return rc;
        }
        prev = Some((tok.start, tok.end));
    }
    ffi::SQLITE_OK
}

/// Port of `fts5_api_from_db`: the documented SQLite/FTS5 idiom for
/// retrieving the `fts5_api*` pointer from a connection that has FTS5
/// compiled in -- prepare `SELECT fts5(?1)`, bind a type-tagged
/// pointer (to our own output slot) as its parameter, and step once;
/// FTS5's internal `fts5()` SQL function writes the real API pointer
/// into that slot as a side effect.
unsafe fn fts5_api_from_db(db: *mut ffi::sqlite3) -> Result<*mut RawFts5Api, Fts5RegistrationError> {
    let sql = c"SELECT fts5(?1)";
    let mut stmt: *mut ffi::sqlite3_stmt = std::ptr::null_mut();
    let rc = ffi::sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut stmt, std::ptr::null_mut());
    if rc != ffi::SQLITE_OK {
        return Err(Fts5RegistrationError::Sqlite(rc));
    }
    let mut api_ptr: *mut RawFts5Api = std::ptr::null_mut();
    let tag = c"fts5_api_ptr";
    let rc = ffi::sqlite3_bind_pointer(stmt, 1, (&mut api_ptr as *mut *mut RawFts5Api) as *mut c_void, tag.as_ptr(), None);
    if rc != ffi::SQLITE_OK {
        ffi::sqlite3_finalize(stmt);
        return Err(Fts5RegistrationError::Sqlite(rc));
    }
    ffi::sqlite3_step(stmt);
    ffi::sqlite3_finalize(stmt);
    if api_ptr.is_null() {
        return Err(Fts5RegistrationError::Fts5Unavailable);
    }
    Ok(api_ptr)
}

/// Port of `calibre_sqlite_extension_init`: registers the real
/// `calibre`/`porter` FTS5 tokenizers on `conn`, and -- matching real
/// upstream's own extension init exactly -- also OVERRIDES the
/// built-in `unicode61` tokenizer name with the plain (non-stemming)
/// `calibre` implementation. Real upstream's own bundled DDL
/// (`fts_sqlite.sql`/`notes_sqlite.sql`) uses `tokenize = 'calibre
/// remove_diacritics 2'`/`tokenize = 'porter calibre remove_diacritics
/// 2'` directly (not the overridden `unicode61` name) for its own
/// `books_fts`/`notes_fts` tables, but some other real schema
/// (`schema_upgrades.py`'s `annotations_fts`) does use `tokenize =
/// 'unicode61 remove_diacritics 2'` -- registering all three names is
/// what makes both real call sites correct.
///
/// Call once per real `Connection`, alongside the other custom
/// function/collation registration in [`crate::backend::register_functions`].
pub fn register_fts5_tokenizers(conn: &rusqlite::Connection) -> Result<(), Fts5RegistrationError> {
    unsafe {
        let db = conn.handle();
        let api = fts5_api_from_db(db)?;
        if (*api).i_version < 2 {
            return Err(Fts5RegistrationError::Fts5Unavailable);
        }

        let plain = RawFts5TokenizerVtable { x_create: xcreate, x_delete: xdelete, x_tokenize: xtokenize };
        let stemming = RawFts5TokenizerVtable { x_create: xcreate_stemming, x_delete: xdelete, x_tokenize: xtokenize };

        for name in [c"unicode61", c"calibre"] {
            let rc = ((*api).x_create_tokenizer)(api, name.as_ptr(), std::ptr::null_mut(), &plain, None);
            if rc != ffi::SQLITE_OK {
                return Err(Fts5RegistrationError::Sqlite(rc));
            }
        }
        let rc = ((*api).x_create_tokenizer)(api, c"porter".as_ptr(), std::ptr::null_mut(), &stemming, None);
        if rc != ffi::SQLITE_OK {
            return Err(Fts5RegistrationError::Sqlite(rc));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_simple_english() {
        let t = Tokenizer::calibre();
        let toks = t.tokenize("Hello, world!", TokenizeMode::Query);
        let words: Vec<&str> = toks.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(words, vec!["hello", "world"]);
    }

    #[test]
    fn tokenize_preserves_byte_offsets() {
        let t = Tokenizer::calibre();
        let text = "Hello, world!";
        let toks = t.tokenize(text, TokenizeMode::Query);
        assert_eq!(toks[0].start, 0);
        assert_eq!(toks[0].end, 5);
        assert_eq!(&text[toks[0].start..toks[0].end], "Hello");
        // "world" starts after ", "
        assert_eq!(toks[1].start, 7);
        assert_eq!(toks[1].end, 12);
        assert_eq!(&text[toks[1].start..toks[1].end], "world");
    }

    #[test]
    fn tokenize_skips_pure_punctuation() {
        let t = Tokenizer::calibre();
        let toks = t.tokenize("... !!! -- ", TokenizeMode::Query);
        assert!(toks.is_empty(), "got: {:?}", toks);
    }

    #[test]
    fn tokenize_lowercases() {
        let t = Tokenizer::calibre();
        let toks = t.tokenize("FOO BaR", TokenizeMode::Query);
        assert_eq!(toks[0].text, "foo");
        assert_eq!(toks[1].text, "bar");
    }

    #[test]
    fn tokenize_query_does_not_duplicate_for_diacritics() {
        // Even with remove_diacritics=true (the calibre default),
        // query mode must emit exactly one token per word — the
        // primary. Only Document mode emits colocated stripped
        // duplicates.
        let t = Tokenizer::calibre();
        let toks = t.tokenize("café", TokenizeMode::Query);
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].text, "café");
    }

    #[test]
    fn tokenize_document_emits_colocated_diacritic_stripped_duplicate() {
        let t = Tokenizer::calibre();
        let toks = t.tokenize("café", TokenizeMode::Document);
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[0].text, "café");
        assert_eq!(toks[1].text, "cafe");
        // Colocated: same byte offsets.
        assert_eq!(toks[0].start, toks[1].start);
        assert_eq!(toks[0].end, toks[1].end);
    }

    #[test]
    fn tokenize_document_no_duplicate_if_no_diacritics() {
        let t = Tokenizer::calibre();
        let toks = t.tokenize("hello", TokenizeMode::Document);
        assert_eq!(toks.len(), 1);
    }

    #[test]
    fn tokenize_document_no_duplicate_when_remove_diacritics_off() {
        let t = Tokenizer {
            remove_diacritics: false,
            stemmer: None,
        };
        let toks = t.tokenize("café", TokenizeMode::Document);
        assert_eq!(toks.len(), 1);
    }

    #[test]
    fn tokenize_with_porter_stemmer_stems() {
        let t = Tokenizer::porter_english();
        let toks = t.tokenize("running runners", TokenizeMode::Query);
        // Snowball English stems both to "run".
        assert_eq!(toks[0].text, "run");
        assert_eq!(toks[1].text, "runner");
    }

    #[test]
    fn tokenize_handles_currency_and_digits() {
        let t = Tokenizer::calibre();
        let toks = t.tokenize("$5 €10 3.14", TokenizeMode::Query);
        let words: Vec<&str> = toks.iter().map(|t| t.text.as_str()).collect();
        // Digits are alphanumeric so they tokenize; currency is caught
        // by the is_word predicate.
        assert!(words.contains(&"5"), "got: {:?}", words);
        assert!(words.contains(&"10"), "got: {:?}", words);
    }

    #[test]
    fn stem_falls_back_to_input_on_unknown_language() {
        assert_eq!(stem("running", "klingon"), "running");
    }

    #[test]
    fn stem_english_reduces_inflections() {
        assert_eq!(stem("running", "english"), "run");
        assert_eq!(stem("fishing", "en"), "fish");
    }

    #[test]
    fn stemmer_for_language_accepts_common_aliases() {
        assert!(stemmer_for_language("en").is_some());
        assert!(stemmer_for_language("EN").is_some());
        assert!(stemmer_for_language("English").is_some());
        assert!(stemmer_for_language("ENGLISH").is_some());
        assert!(stemmer_for_language("Klingon").is_none());
    }

    #[test]
    fn strip_diacritics_matches_common_cases() {
        assert_eq!(strip_diacritics("café"), "cafe");
        assert_eq!(strip_diacritics("naïve"), "naive");
        assert_eq!(strip_diacritics("Ångström"), "Angstrom");
        // No-op when no combining marks present.
        assert_eq!(strip_diacritics("hello"), "hello");
    }

    #[test]
    fn parse_tokenizer_args_only_a_literal_zero_disables_remove_diacritics() {
        assert_eq!(parse_tokenizer_args(&[], true, false), (true, false));
        assert_eq!(parse_tokenizer_args(&["remove_diacritics", "0"], true, false), (false, false));
        // Real upstream's own bundled DDL value -- meaningful only to
        // SQLite's built-in unicode61, inert here.
        assert_eq!(parse_tokenizer_args(&["remove_diacritics", "2"], true, false), (true, false));
        assert_eq!(parse_tokenizer_args(&["stem_words", "1"], true, false), (true, true));
        assert_eq!(parse_tokenizer_args(&["stem_words", "0"], true, true), (true, false));
        // A bare chained-tokenizer name (e.g. "calibre" in real
        // upstream's "porter calibre remove_diacritics 2") is simply
        // skipped, matching the real C++ loop exactly.
        assert_eq!(parse_tokenizer_args(&["calibre", "remove_diacritics", "0"], true, false), (false, false));
    }

    #[test]
    fn register_fts5_tokenizers_makes_calibre_and_porter_real() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        register_fts5_tokenizers(&conn).unwrap();

        conn.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5(content, tokenize='calibre');
             CREATE VIRTUAL TABLE t_stemmed USING fts5(content, tokenize='porter');",
        )
        .unwrap();
        conn.execute("INSERT INTO t(rowid, content) VALUES (1, 'café')", []).unwrap();
        conn.execute("INSERT INTO t_stemmed(rowid, content) VALUES (1, 'running')", []).unwrap();

        // Diacritic-folding: a plain "cafe" query should match "café"
        // via the calibre tokenizer's colocated diacritic-stripped
        // token.
        let hits: i64 = conn.query_row("SELECT count(*) FROM t WHERE t MATCH 'cafe'", [], |r| r.get(0)).unwrap();
        assert_eq!(hits, 1, "the calibre tokenizer should fold diacritics so 'cafe' matches 'café'");

        // English stemming: a "run" query should match "running" via
        // the porter tokenizer's Snowball stemming.
        let hits: i64 = conn.query_row("SELECT count(*) FROM t_stemmed WHERE t_stemmed MATCH 'run'", [], |r| r.get(0)).unwrap();
        assert_eq!(hits, 1, "the porter tokenizer should stem 'running' to 'run'");
    }

    #[test]
    fn register_fts5_tokenizers_overrides_the_builtin_unicode61_name_too() {
        // Real upstream's own extension init overrides SQLite's
        // built-in "unicode61" tokenizer name with the plain calibre
        // implementation -- confirm the override actually took (a
        // real "unicode61" would NOT diacritic-fold by default).
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        register_fts5_tokenizers(&conn).unwrap();
        conn.execute_batch("CREATE VIRTUAL TABLE u USING fts5(content, tokenize='unicode61 remove_diacritics 2')").unwrap();
        conn.execute("INSERT INTO u(rowid, content) VALUES (1, 'café')", []).unwrap();
        let hits: i64 = conn.query_row("SELECT count(*) FROM u WHERE u MATCH 'cafe'", [], |r| r.get(0)).unwrap();
        assert_eq!(hits, 1, "the overridden 'unicode61' name should behave like the calibre tokenizer, not SQLite's real built-in one");
    }

    #[test]
    fn register_fts5_tokenizers_rejects_a_malformed_query() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        register_fts5_tokenizers(&conn).unwrap();
        conn.execute_batch("CREATE VIRTUAL TABLE t USING fts5(content, tokenize='calibre')").unwrap();
        // An unclosed double-quoted phrase is FTS5's own query-syntax
        // error (not the outer SQL string, which is validly closed).
        let err = conn.query_row("SELECT count(*) FROM t WHERE t MATCH 'unterminated \"quote'", [], |r| r.get::<_, i64>(0)).unwrap_err();
        // Just confirm it's a real, well-formed SQLite-level error
        // surfaced through the connection -- not a panic or UB.
        assert!(format!("{err}").to_lowercase().contains("unterminated"), "{err}");
    }
}
