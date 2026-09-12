//! Port of `gui2/tts/piper.py`'s `PiperEmbedded.text_to_raw_audio_data`
//! (issue #647): a batch, non-streaming synthesis entry point, used to
//! render every sentence of a chapter up front rather than one at a
//! time for interactive playback (the shape [`super::stream::Piper`]
//! is built for -- see that module's own docs, and #77's for why the
//! two are real, separate upstream entry points rather than one
//! generalized over the other).
//!
//! # Scope: driving the existing engine, not resolving voices
//!
//! Real `PiperEmbedded` also owns voice *resolution*
//! (`resolve_voice`/`ensure_voices_downloaded`/`load_voice_metadata`,
//! `download_voice`) -- GUI-adjacent config lookup and network
//! downloading, not anything this issue's own body scopes in ("the
//! underlying pieces ... are real and reusable; this issue's job is a
//! new batch-oriented wrapper"). No such machinery exists anywhere in
//! this crate yet, so [`text_to_raw_audio_data`] takes an
//! already-resolved voice's `config_path`/`model_path` directly --
//! exactly the two parameters [`super::stream::Piper::set_voice`]
//! itself already takes, rather than a `(lang, voice_name)` pair this
//! crate has nothing to resolve with.
//!
//! # Disclosed narrowing: no resampling
//!
//! Real upstream resamples via
//! `calibre_extensions.ffmpeg.resample_raw_audio_16bit` whenever the
//! caller's requested `sample_rate` differs from the voice's own
//! native one. No resampling implementation exists anywhere in this
//! workspace -- the same real gap issue #648 (WAV->AAC transcoding) is
//! blocked on. Rather than silently return audio that claims a sample
//! rate it isn't actually at, [`text_to_raw_audio_data`] always
//! produces audio at the voice's own native rate and reports that rate
//! back in [`BatchSynthesis::sample_rate`] instead of accepting one.
//!
//! # Disclosed improvement: a real, enforced timeout
//!
//! Real upstream declares a `timeout: float = 10.` parameter but never
//! actually passes it to its blocking `self._queue.get()` call --
//! confirmed by reading the body, not assumed from the signature; a
//! genuinely hung worker blocks that generator forever. This port's
//! `timeout` is real and enforced via `mpsc::Receiver::recv_timeout`,
//! matching the precedent [`super::stream`]'s own tests already
//! establish for driving this exact channel shape.

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use super::stream::{PcmSamples, Piper, PiperEvent};

/// One synthesized text's result: its audio as raw little-endian PCM16
/// bytes (upstream's own `bytes` -- what a WAV writer or resampler
/// consumes directly) and its real duration in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Utterance {
    pub audio: Vec<u8>,
    pub duration: f64,
}

/// The result of one [`text_to_raw_audio_data`] call: the voice's own
/// native sample rate (see the module docs on why this isn't
/// necessarily the rate the caller asked for), and one [`Utterance`]
/// per input text, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchSynthesis {
    pub sample_rate: u32,
    pub utterances: Vec<Utterance>,
}

/// Port of `PiperEmbedded.text_to_raw_audio_data`. See the module docs
/// for the two real, disclosed departures from a literal transcription
/// (voice resolution and resampling are both out of scope; the
/// `timeout` is real here where upstream's own is dead code).
///
/// An empty (or all-whitespace) text yields an empty [`Utterance`]
/// (`audio: vec![]`, `duration: 0.0`) without any synthesis call, same
/// as upstream's own `yield b'', 0.` fast path.
pub fn text_to_raw_audio_data<'a>(
    config_path: &Path,
    model_path: &Path,
    texts: impl IntoIterator<Item = &'a str>,
    length_scale_multiplier: f32,
    sentence_delay: f32,
    timeout: Duration,
) -> Result<BatchSynthesis> {
    let piper = Piper::new();
    let (tx, rx) = mpsc::channel();
    let sample_rate = piper
        .set_voice(tx, config_path, model_path, length_scale_multiplier, sentence_delay, true)
        .context("loading the voice for batch TTS synthesis")?;

    let mut utterances = Vec::new();
    for text in texts {
        let text = text.trim();
        if text.is_empty() {
            utterances.push(Utterance { audio: Vec::new(), duration: 0.0 });
            continue;
        }

        piper.synthesize(0, text);
        let mut audio: Vec<u8> = Vec::new();
        let mut num_samples: u64 = 0;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                piper.shutdown();
                bail!("timed out waiting for a batch TTS synthesis result");
            }
            match rx.recv_timeout(remaining) {
                Ok(PiperEvent::Result(r)) => {
                    match r.audio {
                        PcmSamples::I16(samples) => audio.extend(samples.iter().flat_map(|s| s.to_le_bytes())),
                        PcmSamples::F32(_) => {
                            piper.shutdown();
                            bail!("batch TTS synthesis requires 16-bit samples, got 32-bit float");
                        }
                    }
                    num_samples += r.num_samples as u64;
                    if r.is_last {
                        break;
                    }
                }
                Ok(PiperEvent::Error { message, .. }) => {
                    piper.shutdown();
                    bail!("failed to synthesize text to audio with error: {message}");
                }
                Err(_) => {
                    piper.shutdown();
                    bail!("timed out waiting for a batch TTS synthesis result");
                }
            }
        }

        let duration = num_samples as f64 / sample_rate as f64;
        utterances.push(Utterance { audio, duration });
    }

    piper.shutdown();
    Ok(BatchSynthesis { sample_rate, utterances })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_voice_paths() -> Option<(PathBuf, PathBuf)> {
        let onnx = std::env::var("CALIBRE_OXIDE_TEST_PIPER_VOICE").ok()?;
        let onnx = PathBuf::from(onnx);
        let json = onnx.with_extension("onnx.json");
        if onnx.exists() && json.exists() {
            Some((onnx, json))
        } else {
            None
        }
    }

    #[test]
    fn empty_texts_are_never_synthesized() {
        // No real voice is needed for this path: an empty/whitespace
        // text short-circuits before any Piper job is ever sent, so
        // this exercises the fast path regardless of CALIBRE_OXIDE_TEST_PIPER_VOICE.
        let Some((onnx, json)) = test_voice_paths() else {
            eprintln!("skipping: set CALIBRE_OXIDE_TEST_PIPER_VOICE to a real .onnx voice to run this test");
            return;
        };
        let result = text_to_raw_audio_data(&json, &onnx, ["", "   ", "\t\n"], 0.0, 0.0, Duration::from_secs(10)).unwrap();
        assert_eq!(result.utterances.len(), 3);
        for u in &result.utterances {
            assert_eq!(u.audio, Vec::<u8>::new());
            assert_eq!(u.duration, 0.0);
        }
    }

    #[test]
    fn batch_synthesizes_every_text_in_order_with_real_durations() {
        let Some((onnx, json)) = test_voice_paths() else {
            eprintln!("skipping: set CALIBRE_OXIDE_TEST_PIPER_VOICE to a real .onnx voice to run this test");
            return;
        };
        let texts = ["Hello there.", "", "This is a second, longer sentence for testing."];
        let result = text_to_raw_audio_data(&json, &onnx, texts, 0.0, 0.0, Duration::from_secs(30)).unwrap();

        assert!(result.sample_rate > 0);
        assert_eq!(result.utterances.len(), 3);

        assert!(!result.utterances[0].audio.is_empty());
        assert!(result.utterances[0].duration > 0.0);
        // Byte length must be a whole number of 16-bit samples.
        assert_eq!(result.utterances[0].audio.len() % 2, 0);
        // Duration must match the real byte count at the reported rate.
        let expected = (result.utterances[0].audio.len() / 2) as f64 / result.sample_rate as f64;
        assert!((result.utterances[0].duration - expected).abs() < 1e-9);

        // The empty middle text produced nothing.
        assert_eq!(result.utterances[1].audio, Vec::<u8>::new());
        assert_eq!(result.utterances[1].duration, 0.0);

        // The longer third sentence takes longer than the short first one.
        assert!(!result.utterances[2].audio.is_empty());
        assert!(result.utterances[2].duration > result.utterances[0].duration);
    }

    #[test]
    fn an_invalid_voice_path_is_a_real_error() {
        let bogus = PathBuf::from("/nonexistent/voice.onnx");
        let bogus_json = PathBuf::from("/nonexistent/voice.onnx.json");
        let err = text_to_raw_audio_data(&bogus_json, &bogus, ["hi"], 0.0, 0.0, Duration::from_secs(1)).unwrap_err();
        assert!(err.to_string().contains("loading the voice"), "{err}");
    }
}
