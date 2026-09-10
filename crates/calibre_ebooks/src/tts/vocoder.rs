//! Port of `piper.cpp`'s `set_voice`/`next` real ONNX inference: the
//! Piper neural vocoder itself. See `tts`'s own module doc for the
//! full #77 scope/split.
//!
//! # ONNX runtime choice: `ort`, not `tract-onnx`
//!
//! #610's own plan assumed `tract-onnx` (pure Rust, no native
//! toolchain) for this piece. Verified otherwise before writing any
//! real code: a real downloaded Piper voice (`en_US-lessac-low`,
//! 63MB, fetched from HuggingFace's `rhasspy/piper-voices` for this
//! spike) fails `tract-onnx`'s symbolic shape inference with a genuine
//! volume-mismatch error inside a relative-position attention layer's
//! dynamic reshape (`/enc_p/encoder/attn_layers.0/Reshape_10`) --
//! confirmed to be a real `tract` limitation on this graph pattern,
//! not a wiring mistake (reproduced identically regardless of the
//! phoneme-sequence length used, and regardless of whether input
//! shapes were concretized before or left symbolic). Found that
//! [`piper-rs`](https://crates.io/crates/piper-rs) -- an existing,
//! real, published crate that already does Piper TTS in Rust -- uses
//! [`ort`](https://crates.io/crates/ort) (real ONNX Runtime bindings,
//! downloading a prebuilt `onnxruntime` shared library at build time
//! rather than requiring a system install or a from-source C++ build)
//! instead of `tract`, for exactly this reason. Verified `ort` loads
//! and correctly runs the same real voice model end to end (a real WAV
//! file was generated and manually reviewed before committing to this
//! approach).
//!
//! # A real upstream bug, found and NOT reproduced
//!
//! `piper.cpp`'s `next()` allocates its output buffer as `sizeof(float)
//! * (num_samples * num_of_silence_samples)` for the 32-bit-float
//! output path -- a `*` where the 16-bit-integer path just above it
//! (and simple arithmetic) clearly calls for `+`. Whenever
//! `sentence_delay > 0` (silence padding is active) this allocates a
//! buffer of the wrong size, then still writes `num_samples +
//! num_of_silence_samples` samples into it via `memset`/the fill loop
//! -- a real buffer-overflow bug in upstream's C++, not something this
//! port has any reason to keep. [`Vocoder::synthesize`] always sizes
//! its output as `num_samples + num_of_silence_samples`, matching the
//! (correct) 16-bit path's own formula.

use std::path::Path;

use ort::session::Session;
use ort::value::{Tensor, Value};

use super::piper::VoiceConfig;

/// A loaded Piper voice: an ONNX session plus the scalar synthesis
/// parameters `piper.cpp`'s `set_voice` reads off the real
/// `VoiceConfig`.
pub struct Vocoder {
    session: Session,
    pub sample_rate: u32,
    pub num_speakers: u32,
    pub noise_scale: f32,
    pub length_scale: f32,
    pub noise_w: f32,
    pub sentence_delay: f32,
}

impl Vocoder {
    /// Port of `set_voice`'s real model-loading half (the
    /// `espeak_voice_name`/phoneme-id-map half is #610's
    /// [`super::piper::VoiceConfig`], already resolved by the time a
    /// caller has one to pass here).
    pub fn load(model_path: &Path, cfg: &VoiceConfig) -> ort::Result<Self> {
        let session = Session::builder()?.commit_from_file(model_path)?;
        Ok(Vocoder {
            session,
            sample_rate: cfg.sample_rate,
            num_speakers: cfg.num_speakers,
            noise_scale: cfg.noise_scale,
            length_scale: cfg.length_scale,
            noise_w: cfg.noise_w,
            sentence_delay: cfg.sentence_delay,
        })
    }

    /// Port of `next()`'s real inference + audio-shaping algorithm for
    /// one sentence's phoneme-ID sequence (as produced by #610's
    /// [`super::piper::text_to_sentence_ids`]): builds the exact real
    /// input tensors (`input`/`input_lengths`/`scales`, plus `sid` only
    /// when `num_speakers > 1` -- confirmed via the real downloaded
    /// voice's own ONNX graph, which for a single-speaker voice
    /// declares only 3 inputs, `sid` isn't merely unused, it's absent),
    /// runs the model, then peak-normalizes the output (dividing by the
    /// maximum absolute sample value, falling back to `1.0` for
    /// near-silent output to avoid dividing by ~0) and appends real
    /// silence padding (`sample_rate * sentence_delay` zero samples).
    /// Returns `f32` samples in `-1.0..=1.0`; see [`to_i16_samples`]
    /// for the 16-bit conversion `piper.cpp`'s `as_16bit_samples=true`
    /// path performs.
    pub fn synthesize(&mut self, phoneme_ids: &[i64]) -> ort::Result<Vec<f32>> {
        let n = phoneme_ids.len();
        let input = Tensor::from_array(([1usize, n], phoneme_ids.to_vec()))?;
        let input_lengths = Tensor::from_array(([1usize], vec![n as i64]))?;
        let scales = Tensor::from_array(([3usize], vec![self.noise_scale, self.length_scale, self.noise_w]))?;

        let mut inputs: Vec<(&str, Value)> = vec![("input", input.into()), ("input_lengths", input_lengths.into()), ("scales", scales.into())];
        if self.num_speakers > 1 {
            let sid = Tensor::from_array(([1usize], vec![0i64]))?;
            inputs.push(("sid", sid.into()));
        }

        let outputs = self.session.run(inputs)?;
        let (_, samples) = outputs[0].try_extract_tensor::<f32>()?;

        let num_of_silence_samples = if self.sentence_delay > 0.0 { (self.sample_rate as f32 * self.sentence_delay) as usize } else { 0 };
        let maxval = samples.iter().fold(0.0f32, |a, &b| a.max(b.abs())).max(1e-8);

        let mut out = Vec::with_capacity(samples.len() + num_of_silence_samples);
        out.extend(samples.iter().map(|s| s / maxval));
        out.resize(out.len() + num_of_silence_samples, 0.0);
        Ok(out)
    }
}

/// Port of `next()`'s `as_16bit_samples=true` conversion: scales
/// already-peak-normalized `-1.0..=1.0` samples to the full `i16`
/// range.
pub fn to_i16_samples(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|&s| (s * i16::MAX as f32) as i16).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::piper::{text_to_sentence_ids, translate_voice_config};

    /// These tests need a real Piper `.onnx` voice model, which isn't
    /// (and shouldn't be) committed to this repo (20-60MB each). Point
    /// `CALIBRE_OXIDE_TEST_PIPER_VOICE` at a real downloaded
    /// `<voice>.onnx` file (with a matching `<voice>.onnx.json` next to
    /// it) to run them; they're skipped otherwise rather than failing
    /// CI on every machine that doesn't have one cached.
    fn test_voice_paths() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let onnx = std::env::var("CALIBRE_OXIDE_TEST_PIPER_VOICE").ok()?;
        let onnx = std::path::PathBuf::from(onnx);
        let json = onnx.with_extension("onnx.json");
        if onnx.exists() && json.exists() {
            Some((onnx, json))
        } else {
            None
        }
    }

    #[test]
    fn synthesize_produces_real_nonsilent_audio_for_a_real_voice() {
        let Some((onnx, json)) = test_voice_paths() else {
            eprintln!("skipping: set CALIBRE_OXIDE_TEST_PIPER_VOICE to a real .onnx voice to run this test");
            return;
        };
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
        let cfg = translate_voice_config(&value);

        let dir = std::env::temp_dir().join("calibre_oxide_vocoder_test_espeak_data");
        std::fs::create_dir_all(&dir).unwrap();
        espeak_ng::install_bundled_language(&dir, "en").unwrap();
        let engine = espeak_ng::EspeakNg::with_data_dir(&cfg.espeak_voice_name, &dir).unwrap();

        let mut vocoder = Vocoder::load(&onnx, &cfg).expect("real voice must load");
        assert_eq!(vocoder.sample_rate, cfg.sample_rate);

        let sentence_ids = text_to_sentence_ids(&engine, &cfg, "Hello, this is a real test.").unwrap();
        assert!(!sentence_ids.is_empty());

        let samples = vocoder.synthesize(&sentence_ids[0]).expect("real inference must succeed");
        assert!(!samples.is_empty(), "expected real synthesized audio samples");
        assert!(samples.iter().any(|&s| s.abs() > 0.01), "expected non-silent audio");
        assert!(samples.iter().all(|&s| s.is_finite() && s.abs() <= 1.0 + 1e-4), "expected peak-normalized samples in -1..=1");

        let i16_samples = to_i16_samples(&samples);
        assert_eq!(i16_samples.len(), samples.len());
        assert!(i16_samples.iter().any(|&s| s != 0));
    }

    #[test]
    fn synthesize_appends_real_silence_padding_for_sentence_delay() {
        let Some((onnx, json)) = test_voice_paths() else {
            eprintln!("skipping: set CALIBRE_OXIDE_TEST_PIPER_VOICE to a real .onnx voice to run this test");
            return;
        };
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
        let mut cfg = translate_voice_config(&value);
        cfg.sentence_delay = 0.5;

        let dir = std::env::temp_dir().join("calibre_oxide_vocoder_test_espeak_data");
        std::fs::create_dir_all(&dir).unwrap();
        espeak_ng::install_bundled_language(&dir, "en").unwrap();
        let engine = espeak_ng::EspeakNg::with_data_dir(&cfg.espeak_voice_name, &dir).unwrap();

        let mut vocoder = Vocoder::load(&onnx, &cfg).expect("real voice must load");
        let sentence_ids = text_to_sentence_ids(&engine, &cfg, "Hi.").unwrap();
        let samples = vocoder.synthesize(&sentence_ids[0]).unwrap();

        let expected_silence = (cfg.sample_rate as f32 * 0.5) as usize;
        assert_eq!(&samples[samples.len() - expected_silence..], vec![0.0f32; expected_silence].as_slice());
    }
}
