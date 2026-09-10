//! Port of `piper.py`'s voice-config parsing and `piper.cpp`'s
//! `start()`'s real phoneme-to-ID algorithm. See `tts`'s own module
//! doc for the full scope/narrowing.

use std::collections::HashMap;
use std::path::Path;

use unicode_normalization::UnicodeNormalization;

/// Port of `piper.cpp`'s `ID_PAD`/`ID_BOS`/`ID_EOS` constants (the
/// Piper ONNX model's own fixed phoneme-ID vocabulary reserved slots).
const ID_PAD: i64 = 0;
const ID_BOS: i64 = 1;
const ID_EOS: i64 = 2;

const DEFAULT_LENGTH_SCALE: f32 = 1.0;
const DEFAULT_NOISE_SCALE: f32 = 0.667;
const DEFAULT_NOISE_W_SCALE: f32 = 0.8;

/// Port of `VoiceConfig`.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceConfig {
    pub espeak_voice_name: String,
    pub sample_rate: u32,
    /// Keyed by the phoneme character's codepoint (`ord(s[0])` in real
    /// Python -- only the *first* character of each JSON key is ever
    /// used, a real upstream quirk this replicates exactly).
    pub phoneme_id_map: HashMap<u32, Vec<i64>>,
    pub length_scale: f32,
    pub noise_scale: f32,
    pub noise_w: f32,
    pub num_speakers: u32,
    pub sentence_delay: f32,
    pub normalize_volume: bool,
}

/// Port of `translate_voice_config`. Unlike real Python (which would
/// raise `AttributeError` if the JSON has no `"inference"` object at
/// all, since it calls `.get()` on whatever `x.get('inference')`
/// returned, including `None`), missing/malformed fields here fall
/// back to the same real defaults rather than panicking -- a real,
/// disclosed robustness improvement, not a behavior this port
/// otherwise depends on (every real Piper voice config has an
/// `"inference"` object in practice).
pub fn translate_voice_config(x: &serde_json::Value) -> VoiceConfig {
    let mut phoneme_id_map: HashMap<u32, Vec<i64>> = HashMap::new();
    if let Some(map) = x.get("phoneme_id_map").and_then(|v| v.as_object()) {
        for (s, pids) in map {
            let Some(first_char) = s.chars().next() else { continue };
            let entry = phoneme_id_map.entry(first_char as u32).or_default();
            if let Some(arr) = pids.as_array() {
                entry.extend(arr.iter().filter_map(|v| v.as_i64()));
            }
        }
    }
    let inf = x.get("inference");
    let g_f32 = |prop: &str, default: f32| -> f32 { inf.and_then(|d| d.get(prop)).and_then(serde_json::Value::as_f64).map(|v| v as f32).unwrap_or(default) };

    let espeak_voice_name = x
        .get("espeak")
        .and_then(|e| e.get("voice"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("en-us")
        .to_string();
    let sample_rate = x.get("audio").and_then(|a| a.get("sample_rate")).and_then(serde_json::Value::as_u64).unwrap_or(22050) as u32;
    let num_speakers = x.get("num_speakers").and_then(serde_json::Value::as_u64).unwrap_or(1) as u32;

    VoiceConfig {
        espeak_voice_name,
        sample_rate,
        phoneme_id_map,
        length_scale: g_f32("length_scale", DEFAULT_LENGTH_SCALE),
        noise_scale: g_f32("noise_scale", DEFAULT_NOISE_SCALE),
        noise_w: g_f32("noise_w", DEFAULT_NOISE_W_SCALE),
        num_speakers,
        sentence_delay: 0.0,
        normalize_volume: false,
    }
}

/// Port of `load_voice_config`.
pub fn load_voice_config(path: &Path) -> anyhow::Result<VoiceConfig> {
    let data = std::fs::read(path)?;
    let value: serde_json::Value = serde_json::from_slice(&data)?;
    Ok(translate_voice_config(&value))
}

/// Port of `create_voice_config`.
pub fn create_voice_config(config_path: &Path, length_scale_multiplier: f32, sentence_delay: f32) -> anyhow::Result<VoiceConfig> {
    let mut cfg = load_voice_config(config_path)?;
    // maps -1..1 to 2..0.1, matching the real Python's own comment.
    let clamped = length_scale_multiplier.clamp(-1.0, 1.0);
    let m = (1.0 - clamped).max(0.1);
    cfg.sentence_delay = sentence_delay;
    cfg.length_scale *= m;
    Ok(cfg)
}

/// Port of `categorize_terminator`'s real return values, as a typed
/// enum. `is_sentence()` matches upstream's own `CLAUSE_TYPE_SENTENCE`
/// flag (period/question/exclamation end a *sentence*; comma/
/// semicolon/colon only end a *clause* within the current sentence).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClauseTerminator {
    None,
    Comma,
    Semicolon,
    Colon,
    Period,
    Question,
    Exclamation,
}

impl ClauseTerminator {
    fn from_char(c: char) -> Self {
        match c {
            ',' => ClauseTerminator::Comma,
            ';' => ClauseTerminator::Semicolon,
            ':' => ClauseTerminator::Colon,
            '.' => ClauseTerminator::Period,
            '?' => ClauseTerminator::Question,
            '!' => ClauseTerminator::Exclamation,
            _ => ClauseTerminator::None,
        }
    }

    /// The literal character `piper.cpp` appends into the phoneme
    /// string at this boundary (`categorize_terminator`'s return
    /// value).
    pub fn as_str(self) -> &'static str {
        match self {
            ClauseTerminator::None => "",
            ClauseTerminator::Comma => ",",
            ClauseTerminator::Semicolon => ";",
            ClauseTerminator::Colon => ":",
            ClauseTerminator::Period => ".",
            ClauseTerminator::Question => "?",
            ClauseTerminator::Exclamation => "!",
        }
    }

    /// Port of `(terminator & CLAUSE_TYPE_SENTENCE) != 0`.
    pub fn is_sentence_end(self) -> bool {
        matches!(self, ClauseTerminator::Period | ClauseTerminator::Question | ClauseTerminator::Exclamation)
    }
}

/// One clause of input text, with the punctuation that ended it (see
/// this module's own doc for why this port splits clauses itself
/// rather than using eSpeak-ng's own per-clause API/`Clause` type).
#[derive(Debug, Clone, PartialEq)]
pub struct Clause {
    pub text: String,
    pub terminator: ClauseTerminator,
}

/// Splits `text` at `,`/`;`/`:`/`.`/`?`/`!`, matching upstream's own
/// clause-boundary punctuation set. A trailing chunk with no
/// terminating punctuation gets [`ClauseTerminator::None`] (matching
/// `categorize_terminator`'s `""` for the final, unterminated segment
/// real `espeak_TextToPhonemesWithTerminator` reports at end of text).
pub fn split_clauses(text: &str) -> Vec<Clause> {
    let mut clauses = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        match c {
            ',' | ';' | ':' | '.' | '?' | '!' => {
                clauses.push(Clause { text: std::mem::take(&mut current), terminator: ClauseTerminator::from_char(c) });
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() || clauses.is_empty() {
        clauses.push(Clause { text: current, terminator: ClauseTerminator::None });
    }
    clauses
}

/// Port of `start()`'s clause-accumulation loop: phonemizes each
/// clause (via `engine`) and buckets them into per-*sentence* phoneme
/// strings, starting a new bucket only after a sentence-ending
/// terminator, appending each clause's own literal terminator
/// character into the accumulated string exactly as `piper.cpp` does.
/// Empty sentences are NOT filtered here (matching real Python's own
/// `sentence_phonemes` list, which can contain a trailing empty
/// string) -- [`text_to_sentence_ids`] filters them, matching
/// `piper.cpp`'s own `if (phonemes_str.empty()) continue;`.
pub fn text_to_sentence_phonemes(engine: &espeak_ng::EspeakNg, text: &str) -> espeak_ng::Result<Vec<String>> {
    let clauses = split_clauses(text);
    let mut sentences = vec![String::new()];
    for clause in clauses {
        let trimmed = clause.text.trim();
        if !trimmed.is_empty() {
            let phonemes = engine.text_to_phonemes(trimmed)?;
            sentences.last_mut().expect("always non-empty").push_str(&phonemes);
        }
        sentences.last_mut().expect("always non-empty").push_str(clause.terminator.as_str());
        if clause.terminator.is_sentence_end() {
            sentences.push(String::new());
        }
    }
    Ok(sentences)
}

/// Port of the phoneme-string-to-ID-sequence half of `start()`'s inner
/// loop: real Unicode NFD normalization (matching
/// `unicodedata.normalize('NFD', ...)`, needed so combining diacritics
/// in the IPA output are separate codepoints the voice's
/// `phoneme_id_map` can match individually), skipping `(lang)` switch
/// flags entirely, and wrapping the result `BOS, PAD, <id, PAD>*,
/// EOS`.
pub fn sentence_phonemes_to_ids(cfg: &VoiceConfig, phonemes: &str) -> Vec<i64> {
    let mut ids = vec![ID_BOS, ID_PAD];
    let mut in_lang_flag = false;
    for ch in phonemes.nfd() {
        if in_lang_flag {
            if ch == ')' {
                in_lang_flag = false;
            }
        } else if ch == '(' {
            in_lang_flag = true;
        } else if let Some(phoneme_ids) = cfg.phoneme_id_map.get(&(ch as u32)) {
            for &id in phoneme_ids {
                ids.push(id);
                ids.push(ID_PAD);
            }
        }
    }
    ids.push(ID_EOS);
    ids
}

/// Port of `start()` end to end: real text -> one real phoneme-ID
/// sequence per sentence, ready for #611's ONNX inference (each
/// `Vec<i64>` is exactly what `piper.cpp`'s `phoneme_id_queue` holds
/// per queued sentence).
pub fn text_to_sentence_ids(engine: &espeak_ng::EspeakNg, cfg: &VoiceConfig, text: &str) -> espeak_ng::Result<Vec<Vec<i64>>> {
    let sentences = text_to_sentence_phonemes(engine, text)?;
    Ok(sentences.into_iter().filter(|s| !s.is_empty()).map(|s| sentence_phonemes_to_ids(cfg, &s)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_engine() -> espeak_ng::EspeakNg {
        let dir = std::env::temp_dir().join("calibre_oxide_espeak_ng_test_data");
        std::fs::create_dir_all(&dir).unwrap();
        espeak_ng::install_bundled_language(&dir, "en").unwrap();
        espeak_ng::EspeakNg::with_data_dir("en-us", &dir).expect("bundled en-us data must load")
    }

    fn real_voice_config_json() -> serde_json::Value {
        // A real, representative Piper voice config shape (en_US-lessac-medium.onnx.json).
        serde_json::json!({
            "audio": {"sample_rate": 22050},
            "espeak": {"voice": "en-us"},
            "inference": {"noise_scale": 0.667, "length_scale": 1.0, "noise_w": 0.8},
            "num_speakers": 1,
            "phoneme_id_map": {
                "_": [0], "^": [1], "$": [2], " ": [3],
                "h": [43], "e": [16], "l": [45], "o": [50], ",": [6], ".": [4], "!": [5], "?": [7],
            }
        })
    }

    #[test]
    fn translate_voice_config_reads_every_real_field() {
        let cfg = translate_voice_config(&real_voice_config_json());
        assert_eq!(cfg.espeak_voice_name, "en-us");
        assert_eq!(cfg.sample_rate, 22050);
        assert_eq!(cfg.length_scale, 1.0);
        assert_eq!(cfg.noise_scale, 0.667);
        assert_eq!(cfg.noise_w, 0.8);
        assert_eq!(cfg.num_speakers, 1);
        assert_eq!(cfg.phoneme_id_map.get(&('h' as u32)), Some(&vec![43]));
        assert_eq!(cfg.phoneme_id_map.get(&(',' as u32)), Some(&vec![6]));
    }

    #[test]
    fn translate_voice_config_falls_back_to_real_defaults_when_fields_are_missing() {
        let cfg = translate_voice_config(&serde_json::json!({}));
        assert_eq!(cfg.espeak_voice_name, "en-us");
        assert_eq!(cfg.sample_rate, 22050);
        assert_eq!(cfg.length_scale, DEFAULT_LENGTH_SCALE);
        assert_eq!(cfg.noise_scale, DEFAULT_NOISE_SCALE);
        assert_eq!(cfg.noise_w, DEFAULT_NOISE_W_SCALE);
        assert_eq!(cfg.num_speakers, 1);
        assert!(cfg.phoneme_id_map.is_empty());
    }

    #[test]
    fn translate_voice_config_concatenates_ids_for_keys_sharing_a_first_char() {
        // Real upstream quirk: only the first character of each JSON
        // key is used, so distinct multi-char keys sharing a first
        // char get their ID lists concatenated under one codepoint.
        let x = serde_json::json!({"phoneme_id_map": {"a": [1, 2], "ab": [3]}});
        let cfg = translate_voice_config(&x);
        assert_eq!(cfg.phoneme_id_map.get(&('a' as u32)), Some(&vec![1, 2, 3]));
    }

    #[test]
    fn create_voice_config_applies_the_real_length_scale_multiplier_formula() {
        let dir = std::env::temp_dir().join("calibre_oxide_tts_voice_config_test.json");
        std::fs::write(&dir, serde_json::to_vec(&real_voice_config_json()).unwrap()).unwrap();

        // multiplier=0 -> m=1 -> unchanged.
        let cfg = create_voice_config(&dir, 0.0, 0.2).unwrap();
        assert_eq!(cfg.length_scale, 1.0);
        assert_eq!(cfg.sentence_delay, 0.2);

        // multiplier=1 (clamped ceiling) -> m=max(0.1, 1-1)=0.1 -> length_scale *= 0.1.
        let cfg = create_voice_config(&dir, 1.0, 0.2).unwrap();
        assert!((cfg.length_scale - 0.1).abs() < 1e-6);

        // multiplier=-1 (clamped floor) -> m=max(0.1, 1-(-1))=2 -> length_scale *= 2.
        let cfg = create_voice_config(&dir, -1.0, 0.2).unwrap();
        assert!((cfg.length_scale - 2.0).abs() < 1e-6);

        // Out-of-range multipliers clamp the same as the real min/max chain.
        let cfg = create_voice_config(&dir, 5.0, 0.2).unwrap();
        assert!((cfg.length_scale - 0.1).abs() < 1e-6);

        std::fs::remove_file(&dir).ok();
    }

    #[test]
    fn split_clauses_classifies_every_real_terminator_and_keeps_a_trailing_remainder() {
        let clauses = split_clauses("Hello, world! Are you there? Yes: indeed; quite so. Done");
        let rendered: Vec<(String, ClauseTerminator)> = clauses.into_iter().map(|c| (c.text.trim().to_string(), c.terminator)).collect();
        assert_eq!(
            rendered,
            vec![
                ("Hello".to_string(), ClauseTerminator::Comma),
                ("world".to_string(), ClauseTerminator::Exclamation),
                ("Are you there".to_string(), ClauseTerminator::Question),
                ("Yes".to_string(), ClauseTerminator::Colon),
                ("indeed".to_string(), ClauseTerminator::Semicolon),
                ("quite so".to_string(), ClauseTerminator::Period),
                ("Done".to_string(), ClauseTerminator::None),
            ]
        );
    }

    #[test]
    fn split_clauses_on_text_with_no_punctuation_is_one_unterminated_clause() {
        let clauses = split_clauses("just words");
        assert_eq!(clauses, vec![Clause { text: "just words".to_string(), terminator: ClauseTerminator::None }]);
    }

    #[test]
    fn sentence_phonemes_to_ids_wraps_with_bos_pad_eos_and_pads_between_every_id() {
        let mut phoneme_id_map = HashMap::new();
        phoneme_id_map.insert('h' as u32, vec![10]);
        phoneme_id_map.insert('i' as u32, vec![11, 12]); // a phoneme with 2 ids
        let cfg = VoiceConfig { espeak_voice_name: "en-us".into(), sample_rate: 22050, phoneme_id_map, length_scale: 1.0, noise_scale: 0.667, noise_w: 0.8, num_speakers: 1, sentence_delay: 0.0, normalize_volume: false };
        let ids = sentence_phonemes_to_ids(&cfg, "hi");
        assert_eq!(ids, vec![ID_BOS, ID_PAD, 10, ID_PAD, 11, ID_PAD, 12, ID_PAD, ID_EOS]);
    }

    #[test]
    fn sentence_phonemes_to_ids_skips_characters_inside_a_lang_switch_flag() {
        let mut phoneme_id_map = HashMap::new();
        phoneme_id_map.insert('h' as u32, vec![10]);
        phoneme_id_map.insert('x' as u32, vec![99]); // would match inside the flag if not skipped
        let cfg = VoiceConfig { espeak_voice_name: "en-us".into(), sample_rate: 22050, phoneme_id_map, length_scale: 1.0, noise_scale: 0.667, noise_w: 0.8, num_speakers: 1, sentence_delay: 0.0, normalize_volume: false };
        let ids = sentence_phonemes_to_ids(&cfg, "h(x)h");
        assert_eq!(ids, vec![ID_BOS, ID_PAD, 10, ID_PAD, 10, ID_PAD, ID_EOS]);
    }

    #[test]
    fn sentence_phonemes_to_ids_unknown_phonemes_are_silently_skipped() {
        let cfg = VoiceConfig { espeak_voice_name: "en-us".into(), sample_rate: 22050, phoneme_id_map: HashMap::new(), length_scale: 1.0, noise_scale: 0.667, noise_w: 0.8, num_speakers: 1, sentence_delay: 0.0, normalize_volume: false };
        let ids = sentence_phonemes_to_ids(&cfg, "abc");
        assert_eq!(ids, vec![ID_BOS, ID_PAD, ID_EOS]);
    }

    #[test]
    fn text_to_sentence_phonemes_groups_clauses_into_sentences_with_real_terminators() {
        let engine = test_engine();
        let sentences = text_to_sentence_phonemes(&engine, "Hello, world! This is a test.").unwrap();
        // Real: "Hello" and "world" are two clauses of the SAME sentence
        // (comma doesn't end a sentence); "This is a test" is a second
        // sentence. A real trailing empty bucket follows the last
        // sentence-ending terminator too, matching real Python's own
        // `sentence_phonemes.push_back("")` -- filtered out downstream
        // by `text_to_sentence_ids`, not here.
        assert_eq!(sentences.len(), 3);
        assert!(sentences[0].contains(','), "expected the comma terminator inside the first sentence: {:?}", sentences[0]);
        assert!(sentences[0].ends_with('!'), "expected the first sentence's own terminator: {:?}", sentences[0]);
        assert!(sentences[1].ends_with('.'), "expected the second sentence's own terminator: {:?}", sentences[1]);
        assert_eq!(sentences[2], "");
    }

    #[test]
    fn text_to_sentence_ids_produces_one_real_id_sequence_per_sentence() {
        let engine = test_engine();
        let cfg = translate_voice_config(&real_voice_config_json());
        let sentence_ids = text_to_sentence_ids(&engine, &cfg, "Hello. World.").unwrap();
        assert_eq!(sentence_ids.len(), 2, "expected two sentences worth of phoneme-id sequences");
        for ids in &sentence_ids {
            assert_eq!(ids.first(), Some(&ID_BOS));
            assert_eq!(ids.get(1), Some(&ID_PAD));
            assert_eq!(ids.last(), Some(&ID_EOS));
            assert!(ids.len() > 3, "expected real phoneme ids between BOS/PAD and EOS, got {ids:?}");
        }
    }

    #[test]
    fn text_to_sentence_ids_skips_empty_sentences() {
        let engine = test_engine();
        let cfg = translate_voice_config(&real_voice_config_json());
        // Trailing punctuation with nothing after it produces a real
        // empty final "sentence" bucket that must be filtered out.
        let sentence_ids = text_to_sentence_ids(&engine, &cfg, "Hello.").unwrap();
        assert_eq!(sentence_ids.len(), 1);
    }
}
