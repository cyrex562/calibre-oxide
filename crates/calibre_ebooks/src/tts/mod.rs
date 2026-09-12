//! Port of `old_src/src/calibre/utils/tts` (issue #77): calibre's
//! Piper-based neural text-to-speech engine.
//!
//! # Real scope, split (issue #77's own "2 files" framing understated
//! the real size)
//!
//! `piper.py`/`piper.cpp` together wrap a full neural TTS pipeline:
//! eSpeak-ng phonemization, a Piper ONNX neural vocoder, and (in the
//! GUI) Qt audio playback. None of that infrastructure previously
//! existed in this workspace. A feasibility spike (multiple standalone
//! scratch-project builds, run *before* committing to any approach)
//! found: a real FFI eSpeak-ng binding (`espeak-rs`/`espeak-rs-sys`)
//! has real, non-environmental build bugs in its own vendored
//! eSpeak-ng snapshot; [`espeak-ng`](https://crates.io/crates/espeak-ng)
//! -- a genuine **pure-Rust port** of eSpeak NG, not an FFI wrapper --
//! builds cleanly and produces correct real IPA phonemization out of
//! the box; and `tract-onnx` (pure-Rust ONNX inference, the original
//! plan for the vocoder) fails real symbolic shape inference on
//! Piper's actual VITS-style attention graph, so [`vocoder`] uses
//! [`ort`](https://crates.io/crates/ort) (real ONNX Runtime bindings,
//! the same choice the existing `piper-rs` crate makes) instead --
//! see [`vocoder`]'s own module doc for the full story. Split into
//! #610 (this module's `piper` submodule: voice config + real
//! phonemization), #611 ([`vocoder`]: ONNX neural vocoder inference),
//! #612 ([`stream`]: streaming synthesis orchestration, the real
//! `Piper` class). All three are done -- #77 is fully ported. Qt audio
//! playback (`play_wav_data`/`play_pcm_data`) is out of scope, matching
//! this project's established GUI-plumbing narrowing -- this port's
//! job ends at producing real PCM audio samples.
//!
//! # `piper` submodule scope (#610)
//!
//! [`VoiceConfig`]/[`translate_voice_config`]/[`load_voice_config`]/
//! [`create_voice_config`] (port of `piper.py`'s voice-config JSON
//! parsing) and real phonemization + phoneme-to-ID-sequence building
//! ([`text_to_sentence_ids`]), replicating `piper.cpp`'s own
//! `start()`'s real algorithm exactly: per-sentence phoneme-ID
//! sequences, each wrapped `BOS, PAD, <id, PAD>*, EOS`, ready to feed
//! into [`vocoder::Vocoder`].
//!
//! **Disclosed narrowing**: real `piper.cpp` iterates eSpeak-ng's
//! low-level per-*clause* API (`espeak_TextToPhonemesWithTerminator`),
//! re-inserting the literal terminator character (`,`/`;`/`:`/`.`/
//! `?`/`!`) into the phoneme string at each clause boundary *before*
//! doing the char-by-char `phoneme_id_map` lookup -- meaning a comma's
//! own mapped IDs (a real Piper voice's `phoneme_id_map` typically has
//! an entry for `,`, used for the vocoder's pause/prosody) get emitted
//! into the ID stream. This port instead splits the input text into
//! clauses itself ([`split_clauses`], a direct, real, simple
//! punctuation-based split matching upstream's own `CLAUSE_TYPE_CLAUSE`/
//! `CLAUSE_TYPE_SENTENCE` distinction) and phonemizes each clause
//! separately via the `espeak-ng` crate's own `text_to_phonemes` (its
//! internal per-clause splitting is a simplified approximation --
//! confirmed by reading its own doc comments -- not a faithful `Vec<
//! Clause>` with real terminator classification, so this port does not
//! rely on it), then re-inserts the same literal terminator character
//! this port's own splitter recorded. The one real difference from
//! upstream: multi-character terminator classes upstream's own
//! `categorize_terminator` doesn't distinguish either (e.g. an
//! ellipsis) fall back to no terminator character here too, same as
//! upstream.

pub mod batch;
pub mod piper;
pub mod stream;
pub mod vocoder;

pub use piper::{
    create_voice_config, load_voice_config, sentence_phonemes_to_ids, split_clauses,
    text_to_sentence_ids, text_to_sentence_phonemes, translate_voice_config, Clause,
    ClauseTerminator, VoiceConfig,
};
pub use batch::{text_to_raw_audio_data, BatchSynthesis, Utterance};
pub use stream::{PcmSamples, Piper, PiperEvent, SynthesisResult};
pub use vocoder::{to_i16_samples, Vocoder};
