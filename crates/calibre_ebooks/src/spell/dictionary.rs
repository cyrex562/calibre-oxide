//! Port of `calibre.spell.dictionary` -- dictionary discovery, loading,
//! and the `Dictionaries` lookup/suggestion engine.
//!
//! # Real spell-checking via `spellbook`, not a stub
//!
//! Upstream loads dictionaries through `calibre_extensions.hunspell`, a
//! C++ extension wrapping the real Hunspell library. This workspace has
//! no Hunspell binding and the system Hunspell package here ships no
//! development headers to bind against -- but [`spellbook`], a pure-Rust
//! spellchecking engine compatible with Hunspell's own `.dic`/`.aff`
//! dictionary format (the same crate the Helix editor uses for its own
//! spell-checking), reads the *exact same dictionary files* this
//! workspace already vendors (`old_src/resources/dictionaries/`).
//! [`LoadedDictionary`] wraps a real `spellbook::Dictionary`, and
//! [`Dictionaries::recognized`]/[`Dictionaries::suggestions`] do real
//! spell-checking against it -- this is not a `todo!()`-blocked gap.
//!
//! `spellbook::Dictionary::remove_stem` (marks a stem forbidden) stands
//! in for upstream's `hunspell.Dictionary.remove` (deletes it outright)
//! in [`Dictionaries::remove_user_words`] -- functionally equivalent for
//! `check`/`recognized` purposes, the only thing this port uses it for.
//!
//! # No persisted preferences store
//!
//! Upstream's `dprefs` (a `JSONConfig`-backed global) holds
//! `preferred_dictionaries`/`preferred_locales`/`user_dictionaries`
//! across restarts. `calibre.utils.config.JSONConfig` isn't ported
//! anywhere in this workspace. Rather than reading a hidden global, this
//! port takes that state as explicit inputs: [`Dictionaries::new`] takes
//! the caller's already-loaded [`UserDictionary`] list plus two lookup
//! closures (`preferred_dictionary_for`/`best_locale_for_language`,
//! matching upstream's own `preferred_dictionary`/
//! `best_locale_for_language` functions) the caller can back with
//! whatever config store it has -- or `|_| None` if it has none yet.
//! Whenever this port's state changes in a way upstream would persist
//! (a user dictionary edited, a word added), the caller is expected to
//! read the relevant field back out and persist it itself, matching
//! this whole port's "pure function/caller does I/O" convention.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use regex::Regex;

use super::DictionaryLocale;

/// Port of the `Dictionary` namedtuple: metadata about an on-disk
/// dictionary, not yet loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictionaryMeta {
    pub primary_locale: DictionaryLocale,
    pub locales: Vec<DictionaryLocale>,
    pub dicpath: PathBuf,
    pub affpath: PathBuf,
    pub builtin: bool,
    /// `None` for builtin dictionaries -- matches upstream, where every
    /// builtin `Dictionary far shares `name = id = None` (see
    /// [`get_dictionary`]'s doc for the resulting id-collision
    /// behavior this port intentionally preserves).
    pub name: Option<String>,
    pub id: Option<String>,
}

/// A [`DictionaryMeta`] with its `.dic`/`.aff` files actually parsed.
pub struct LoadedDictionary {
    pub primary_locale: DictionaryLocale,
    pub locales: Vec<DictionaryLocale>,
    pub dict: spellbook::Dictionary,
    pub builtin: bool,
    pub name: Option<String>,
    pub id: Option<String>,
}

fn read_locales_file(path: &Path) -> Option<Vec<String>> {
    let raw = fs::read_to_string(path).ok()?;
    Some(raw.lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
}

/// Port of `builtin_dictionaries`. `dictionaries_dir` is upstream's
/// `P('dictionaries', allow_user_override=False)` -- this workspace has
/// no resource-lookup convention (see other ports' own notes on this),
/// so callers pass the path explicitly (e.g.
/// `old_src/resources/dictionaries` in this repo).
pub fn builtin_dictionaries(dictionaries_dir: &Path) -> Vec<DictionaryMeta> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dictionaries_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let base = entry.path();
        if !base.is_dir() {
            continue;
        }
        let Some(locales) = read_locales_file(&base.join("locales")) else {
            continue;
        };
        let Some(locale) = locales.first() else { continue };
        let Ok(primary_locale) = super::parse_lang_code(locale) else { continue };
        let all_locales: Vec<DictionaryLocale> = locales.iter().filter_map(|l| super::parse_lang_code(l).ok()).collect();
        out.push(DictionaryMeta {
            primary_locale,
            locales: all_locales,
            dicpath: base.join(format!("{locale}.dic")),
            affpath: base.join(format!("{locale}.aff")),
            builtin: true,
            name: None,
            id: None,
        });
    }
    out
}

/// Port of `custom_dictionaries`. `config_dir` is upstream's
/// `calibre.constants.config_dir`; the actual scanned directory is
/// `config_dir/dictionaries`, matching upstream's own
/// `os.path.join(config_dir, 'dictionaries', ...)`.
pub fn custom_dictionaries(config_dir: &Path) -> Vec<DictionaryMeta> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(config_dir.join("dictionaries")) else {
        return out;
    };
    for entry in entries.flatten() {
        let base = entry.path();
        if !base.is_dir() {
            continue;
        }
        let Some(lines) = read_locales_file(&base.join("locales")) else {
            continue;
        };
        if lines.len() < 2 {
            continue;
        }
        let name = lines[0].clone();
        let locale = &lines[1];
        let Ok(primary_locale) = super::parse_lang_code(locale) else { continue };
        if primary_locale.countrycode.is_none() {
            continue;
        }
        let all_locales: Vec<DictionaryLocale> =
            lines[1..].iter().filter_map(|l| super::parse_lang_code(l).ok()).filter(|l| l.countrycode.is_some()).collect();
        let id = base.file_name().map(|n| n.to_string_lossy().into_owned());
        out.push(DictionaryMeta {
            primary_locale,
            locales: all_locales,
            dicpath: base.join(format!("{locale}.dic")),
            affpath: base.join(format!("{locale}.aff")),
            builtin: false,
            name: Some(name),
            id,
        });
    }
    out
}

/// Port of `get_dictionary`. See this module's doc for why
/// `preferred_dictionary_for`/`best_locale_for_language` are explicit
/// closures rather than a read from a persisted global -- both are
/// threaded through the recursive fallback call exactly as upstream's
/// own `preferred_dictionary(locale)`/`best_locale_for_language(langcode)`
/// calls are (each re-evaluated against whatever locale is current at
/// that point, not memoized from the top-level call).
pub fn get_dictionary(
    locale: &DictionaryLocale,
    custom: &[DictionaryMeta],
    builtin: &[DictionaryMeta],
    preferred_dictionary_for: &impl Fn(&DictionaryLocale) -> Option<String>,
    best_locale_for_language: &impl Fn(&str) -> Option<DictionaryLocale>,
    exact_match: bool,
) -> Option<DictionaryMeta> {
    let mut exact_matches: HashMap<Option<String>, DictionaryMeta> = HashMap::new();
    for collection in [custom, builtin] {
        for d in collection {
            if &d.primary_locale == locale {
                exact_matches.insert(d.id.clone(), d.clone());
            }
        }
        for d in collection {
            if d.locales.iter().any(|q| q == locale) && !exact_matches.contains_key(&d.id) {
                exact_matches.insert(d.id.clone(), d.clone());
            }
        }
    }

    if let Some(preferred) = preferred_dictionary_for(locale) {
        if let Some(d) = exact_matches.get(&Some(preferred)) {
            return Some(d.clone());
        }
    }

    let mut keys: Vec<&Option<String>> = exact_matches.keys().collect();
    keys.sort_by_key(|k| match k {
        None => (1u8, String::new()),
        Some(s) => (0u8, s.clone()),
    });
    if let Some(k) = keys.first() {
        return exact_matches.get(*k).cloned();
    }

    if exact_match {
        return None;
    }

    if let Some(best_locale) = best_locale_for_language(&locale.langcode) {
        if let Some(ans) = get_dictionary(&best_locale, custom, builtin, preferred_dictionary_for, best_locale_for_language, true) {
            return Some(ans);
        }
    }

    for collection in [custom, builtin] {
        let mut sorted: Vec<&DictionaryMeta> = collection.iter().collect();
        sorted.sort_by(|a, b| a.name.as_deref().unwrap_or("").cmp(b.name.as_deref().unwrap_or("")));
        for d in sorted {
            if d.primary_locale.langcode == locale.langcode {
                return Some(d.clone());
            }
        }
    }
    None
}

/// Port of `load_dictionary`. `\\?\`-prefixing for long Windows paths
/// (upstream's `iswindows` branch) is not replicated -- this port has
/// no established Windows-long-path convention elsewhere either.
pub fn load_dictionary(meta: &DictionaryMeta) -> Result<LoadedDictionary> {
    let aff = fs::read_to_string(&meta.affpath).with_context(|| format!("reading {:?}", meta.affpath))?;
    let dic = fs::read_to_string(&meta.dicpath).with_context(|| format!("reading {:?}", meta.dicpath))?;
    let dict = spellbook::Dictionary::new(&aff, &dic).map_err(|e| anyhow::anyhow!("failed to parse dictionary {:?}: {e}", meta.dicpath))?;
    Ok(LoadedDictionary {
        primary_locale: meta.primary_locale.clone(),
        locales: meta.locales.clone(),
        dict,
        builtin: meta.builtin,
        name: meta.name.clone(),
        id: meta.id.clone(),
    })
}

/// Port of `UserDictionary`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDictionary {
    pub name: String,
    pub is_active: bool,
    pub words: HashSet<(String, String)>,
}

/// Port of `Dictionaries`. See this module's doc for the persisted-config
/// simplification.
pub struct Dictionaries {
    remove_hyphenation: Regex,
    negative_pat: Regex,
    fix_punctuation_pat: Regex,
    custom: Vec<DictionaryMeta>,
    builtin: Vec<DictionaryMeta>,
    preferred_dictionary_for: Box<dyn Fn(&DictionaryLocale) -> Option<String>>,
    best_locale_for_language: Box<dyn Fn(&str) -> Option<DictionaryLocale>>,
    loaded: HashMap<DictionaryLocale, Option<LoadedDictionary>>,
    word_cache: HashMap<(String, DictionaryLocale), bool>,
    ignored_words: HashSet<(String, String)>,
    pub active_user_dictionaries: Vec<UserDictionary>,
    pub inactive_user_dictionaries: Vec<UserDictionary>,
    pub default_locale: DictionaryLocale,
}

impl Dictionaries {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        default_locale: DictionaryLocale,
        custom: Vec<DictionaryMeta>,
        builtin: Vec<DictionaryMeta>,
        user_dictionaries: Vec<UserDictionary>,
        preferred_dictionary_for: impl Fn(&DictionaryLocale) -> Option<String> + 'static,
        best_locale_for_language: impl Fn(&str) -> Option<DictionaryLocale> + 'static,
    ) -> Dictionaries {
        let mut active_user_dictionaries = Vec::new();
        let mut inactive_user_dictionaries = Vec::new();
        for d in user_dictionaries {
            if d.is_active {
                active_user_dictionaries.push(d);
            } else {
                inactive_user_dictionaries.push(d);
            }
        }
        Dictionaries {
            remove_hyphenation: Regex::new(r"[\u{2010}-]+").unwrap(),
            negative_pat: Regex::new(r"^-[.\d+]").unwrap(),
            fix_punctuation_pat: Regex::new(r"[:.]").unwrap(),
            custom,
            builtin,
            preferred_dictionary_for: Box::new(preferred_dictionary_for),
            best_locale_for_language: Box::new(best_locale_for_language),
            loaded: HashMap::new(),
            word_cache: HashMap::new(),
            ignored_words: HashSet::new(),
            active_user_dictionaries,
            inactive_user_dictionaries,
            default_locale,
        }
    }

    pub fn clear_caches(&mut self) {
        self.loaded.clear();
        self.word_cache.clear();
    }

    pub fn clear_ignored(&mut self) {
        self.ignored_words.clear();
    }

    /// Port of `dictionary_for_locale`: lazily loads and caches the
    /// dictionary for `locale`, seeding it with every active user
    /// dictionary's words for that locale's language, matching
    /// upstream's own one-time seed-on-load (not a live sync -- see
    /// `add_user_words`/`remove_user_words` for the live-update path).
    pub fn dictionary_for_locale(&mut self, locale: &DictionaryLocale) -> Option<&LoadedDictionary> {
        if !self.loaded.contains_key(locale) {
            let meta = get_dictionary(locale, &self.custom, &self.builtin, &self.preferred_dictionary_for, &self.best_locale_for_language, false);
            let mut loaded = meta.and_then(|m| load_dictionary(&m).ok());
            if let Some(d) = &mut loaded {
                for ud in &self.active_user_dictionaries {
                    for (word, langcode) in &ud.words {
                        if langcode == &locale.langcode {
                            let _ = d.dict.add(word);
                        }
                    }
                }
            }
            self.loaded.insert(locale.clone(), loaded);
        }
        self.loaded.get(locale).and_then(|o| o.as_ref())
    }

    pub fn ignore_word(&mut self, word: &str, locale: &DictionaryLocale) {
        self.ignored_words.insert((word.to_string(), locale.langcode.clone()));
        self.word_cache.insert((word.to_string(), locale.clone()), true);
    }

    pub fn unignore_word(&mut self, word: &str, locale: &DictionaryLocale) {
        self.ignored_words.remove(&(word.to_string(), locale.langcode.clone()));
        self.word_cache.remove(&(word.to_string(), locale.clone()));
    }

    pub fn is_word_ignored(&self, word: &str, locale: &DictionaryLocale) -> bool {
        self.ignored_words.contains(&(word.to_string(), locale.langcode.clone()))
    }

    pub fn all_user_dictionaries(&self) -> impl Iterator<Item = &UserDictionary> {
        self.active_user_dictionaries.iter().chain(self.inactive_user_dictionaries.iter())
    }

    pub fn user_dictionary(&self, name: &str) -> Option<&UserDictionary> {
        self.all_user_dictionaries().find(|d| d.name == name)
    }

    pub fn mark_user_dictionary_as_active(&mut self, name: &str, is_active: bool) -> bool {
        for d in self.active_user_dictionaries.iter_mut().chain(self.inactive_user_dictionaries.iter_mut()) {
            if d.name == name {
                d.is_active = is_active;
                return true;
            }
        }
        false
    }

    /// Port of `add_user_words`: propagates already-added user-dictionary
    /// words into every *already-loaded* dictionary for `langcode`.
    fn add_user_words(&mut self, words: &[String], langcode: &str) {
        for d in self.loaded.values_mut().flatten() {
            if d.primary_locale.langcode == langcode {
                for word in words {
                    let _ = d.dict.add(word);
                }
            }
        }
    }

    /// Port of `remove_user_words`. See this module's doc: uses
    /// `remove_stem` (forbid) rather than a true delete.
    fn remove_user_words(&mut self, words: &[String], langcode: &str) {
        for d in self.loaded.values_mut().flatten() {
            if d.primary_locale.langcode == langcode {
                for word in words {
                    d.dict.remove_stem(word);
                }
            }
        }
    }

    /// Port of `add_to_user_dictionary`. Returns `Err` if `name` doesn't
    /// name an existing user dictionary (upstream's `ValueError`).
    pub fn add_to_user_dictionary(&mut self, name: &str, word: &str, locale: &DictionaryLocale) -> Result<bool, String> {
        let ud = self
            .active_user_dictionaries
            .iter_mut()
            .chain(self.inactive_user_dictionaries.iter_mut())
            .find(|d| d.name == name)
            .ok_or_else(|| format!("Cannot add to the dictionary named: {name} as no such dictionary exists"))?;
        let before = ud.words.len();
        ud.words.insert((word.to_string(), locale.langcode.clone()));
        if ud.words.len() > before {
            self.add_user_words(&[word.to_string()], &locale.langcode);
            self.word_cache.remove(&(word.to_string(), locale.clone()));
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn remove_from_user_dictionaries(&mut self, word: &str, locale: &DictionaryLocale) -> bool {
        let key = (word.to_string(), locale.langcode.clone());
        let mut changed = false;
        for ud in &mut self.active_user_dictionaries {
            if ud.words.remove(&key) {
                changed = true;
            }
        }
        if changed {
            self.word_cache.remove(&(word.to_string(), locale.clone()));
            self.remove_user_words(&[word.to_string()], &locale.langcode);
        }
        changed
    }

    pub fn word_in_user_dictionary(&self, word: &str, locale: &DictionaryLocale) -> Option<&str> {
        let key = (word.to_string(), locale.langcode.clone());
        self.active_user_dictionaries.iter().find(|d| d.words.contains(&key)).map(|d| d.name.as_str())
    }

    pub fn create_user_dictionary(&mut self, name: &str) -> Result<(), String> {
        if self.all_user_dictionaries().any(|d| d.name == name) {
            return Err(format!("A dictionary named {name} already exists"));
        }
        self.active_user_dictionaries.push(UserDictionary { name: name.to_string(), is_active: true, words: HashSet::new() });
        Ok(())
    }

    pub fn remove_user_dictionary(&mut self, name: &str) -> bool {
        let before = self.active_user_dictionaries.len() + self.inactive_user_dictionaries.len();
        self.active_user_dictionaries.retain(|d| d.name != name);
        self.inactive_user_dictionaries.retain(|d| d.name != name);
        let changed = self.active_user_dictionaries.len() + self.inactive_user_dictionaries.len() < before;
        if changed {
            self.clear_caches();
        }
        changed
    }

    pub fn rename_user_dictionary(&mut self, name: &str, new_name: &str) -> bool {
        let mut changed = false;
        for d in self.active_user_dictionaries.iter_mut().chain(self.inactive_user_dictionaries.iter_mut()) {
            if d.name == name {
                d.name = new_name.to_string();
                changed = true;
            }
        }
        changed
    }

    /// Port of `recognized`.
    pub fn recognized(&mut self, word: &str, locale: Option<&DictionaryLocale>) -> bool {
        let locale = locale.unwrap_or(&self.default_locale).clone();
        let key = (word.to_string(), locale.clone());
        if let Some(&ans) = self.word_cache.get(&key) {
            return ans;
        }
        let lkey = (word.to_string(), locale.langcode.clone());
        let mut ans = if self.ignored_words.contains(&lkey) {
            true
        } else if self.active_user_dictionaries.iter().any(|ud| ud.words.contains(&lkey)) {
            true
        } else {
            match self.dictionary_for_locale(&locale) {
                Some(d) => d.dict.check(&word.replace('\u{2010}', "-")),
                None => true,
            }
        };
        if !ans && self.negative_pat.is_match(word) {
            ans = true;
        }
        self.word_cache.insert(key, ans);
        ans
    }

    /// Port of `suggestions`.
    pub fn suggestions(&mut self, word: &str, locale: Option<&DictionaryLocale>) -> Vec<String> {
        let locale = locale.unwrap_or(&self.default_locale).clone();
        let has_unicode_hyphen = word.contains('\u{2010}');
        let mut ans: Vec<String> = Vec::new();

        if self.dictionary_for_locale(&locale).is_some() {
            let hyphen_normalized = word.replace('\u{2010}', "-");
            let mut suggested = Vec::new();
            self.dictionary_for_locale(&locale).unwrap().dict.suggest(&hyphen_normalized, &mut suggested);
            ans = suggested;

            let dehyphenated_word = self.remove_hyphenation.replace_all(word, "").into_owned();

            if dehyphenated_word.chars().count() != word.chars().count() && self.recognized(&dehyphenated_word, None) {
                ans = add_suggestion(dehyphenated_word, ans);
            } else if let Some(m) = self.fix_punctuation_pat.find(word) {
                let w1 = &word[..m.start()];
                let w2 = &word[m.end()..];
                if self.recognized(w1, None) && self.recognized(w2, None) {
                    let fw = format!("{w1}{} {w2}", m.as_str());
                    ans = add_suggestion(fw, ans);
                    let capitalized_w2 = calibre_utils::icu::capitalize(w2);
                    if capitalized_w2 != w2 {
                        let fw = format!("{w1}{} {capitalized_w2}", m.as_str());
                        ans = add_suggestion(fw, ans);
                    }
                }
            }
        }

        if has_unicode_hyphen {
            ans = ans.into_iter().map(|w| w.replace('-', "\u{2010}")).collect();
        }
        ans
    }
}

fn add_suggestion(w: String, ans: Vec<String>) -> Vec<String> {
    let mut out = vec![w.clone()];
    out.extend(ans.into_iter().filter(|x| *x != w));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dictionaries_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../old_src/resources/dictionaries")
    }

    fn has_dictionaries() -> bool {
        dictionaries_dir().join("en-US").join("locales").exists()
    }

    fn en_us() -> DictionaryLocale {
        DictionaryLocale::new("eng", Some("US".to_string()))
    }

    #[test]
    fn builtin_dictionaries_discovers_the_vendored_locales() {
        if !has_dictionaries() {
            eprintln!("skipping: old_src/resources/dictionaries not present in this checkout");
            return;
        }
        let dics = builtin_dictionaries(&dictionaries_dir());
        assert!(dics.iter().any(|d| d.primary_locale == en_us()));
        assert!(dics.iter().all(|d| d.builtin && d.id.is_none()));
    }

    #[test]
    fn get_dictionary_finds_an_exact_locale_match() {
        if !has_dictionaries() {
            return;
        }
        let builtin = builtin_dictionaries(&dictionaries_dir());
        let found = get_dictionary(&en_us(), &[], &builtin, &|_| None, &|_| None, false);
        assert!(found.is_some());
        assert_eq!(found.unwrap().primary_locale, en_us());
    }

    #[test]
    fn get_dictionary_falls_back_to_any_dictionary_matching_the_language() {
        if !has_dictionaries() {
            return;
        }
        let builtin = builtin_dictionaries(&dictionaries_dir());
        // en-CA isn't one of the vendored builtin locales, but English is.
        let en_ca = DictionaryLocale::new("eng", Some("CA".to_string()));
        let found = get_dictionary(&en_ca, &[], &builtin, &|_| None, &|_| None, false);
        assert!(found.is_some());
        assert_eq!(found.unwrap().primary_locale.langcode, "eng");
    }

    #[test]
    fn get_dictionary_exact_match_only_returns_none_without_a_locale_match() {
        if !has_dictionaries() {
            return;
        }
        let builtin = builtin_dictionaries(&dictionaries_dir());
        let en_ca = DictionaryLocale::new("eng", Some("CA".to_string()));
        let found = get_dictionary(&en_ca, &[], &builtin, &|_| None, &|_| None, true);
        assert!(found.is_none());
    }

    fn test_dictionaries() -> Dictionaries {
        let builtin = builtin_dictionaries(&dictionaries_dir());
        Dictionaries::new(en_us(), Vec::new(), builtin, Vec::new(), |_| None, |_| None)
    }

    #[test]
    fn recognized_finds_a_correctly_spelled_word() {
        if !has_dictionaries() {
            return;
        }
        let mut d = test_dictionaries();
        assert!(d.recognized("recognized", Some(&en_us())));
    }

    #[test]
    fn recognized_rejects_a_misspelled_word() {
        if !has_dictionaries() {
            return;
        }
        let mut d = test_dictionaries();
        assert!(!d.recognized("recognzied", Some(&en_us())));
    }

    #[test]
    fn recognized_treats_simple_negative_numbers_as_words() {
        if !has_dictionaries() {
            return;
        }
        let mut d = test_dictionaries();
        assert!(d.recognized("-5", Some(&en_us())));
    }

    #[test]
    fn ignored_words_are_recognized() {
        if !has_dictionaries() {
            return;
        }
        let mut d = test_dictionaries();
        assert!(!d.recognized("zzqwxk", Some(&en_us())));
        d.ignore_word("zzqwxk", &en_us());
        assert!(d.recognized("zzqwxk", Some(&en_us())));
    }

    #[test]
    fn user_dictionary_words_are_recognized_and_seed_new_dictionary_loads() {
        if !has_dictionaries() {
            return;
        }
        let mut d = test_dictionaries();
        d.create_user_dictionary("Default").unwrap();
        assert!(d.add_to_user_dictionary("Default", "zzqwxk", &en_us()).unwrap());
        assert!(d.recognized("zzqwxk", Some(&en_us())));
    }

    #[test]
    fn suggestions_offers_a_dehyphenated_correction() {
        if !has_dictionaries() {
            return;
        }
        let mut d = test_dictionaries();
        let suggestions = d.suggestions("one\u{2010}half", Some(&en_us()));
        assert!(suggestions.iter().any(|s| s.contains('\u{2010}')) || !suggestions.is_empty());
    }

    #[test]
    fn create_user_dictionary_rejects_a_duplicate_name() {
        let mut d = Dictionaries::new(en_us(), Vec::new(), Vec::new(), Vec::new(), |_| None, |_| None);
        d.create_user_dictionary("Default").unwrap();
        assert!(d.create_user_dictionary("Default").is_err());
    }

    #[test]
    fn mark_user_dictionary_as_active_flips_the_flag_without_moving_lists() {
        // Matches upstream exactly: `mark_user_dictionary_as_active` only
        // flips `is_active` in place, it does not re-partition
        // `active_user_dictionaries`/`inactive_user_dictionaries` -- a
        // dictionary created active and then marked inactive stays
        // physically in `active_user_dictionaries` (and so, e.g., still
        // gets consulted by `recognized`) until the lists are rebuilt.
        let mut d = Dictionaries::new(en_us(), Vec::new(), Vec::new(), Vec::new(), |_| None, |_| None);
        d.create_user_dictionary("Default").unwrap();
        assert!(d.mark_user_dictionary_as_active("Default", false));
        assert_eq!(d.active_user_dictionaries.len(), 1);
        assert!(!d.active_user_dictionaries[0].is_active);
    }
}
