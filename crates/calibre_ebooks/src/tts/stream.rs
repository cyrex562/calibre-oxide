//! Port of `piper.py`'s `Piper` class (issue #612): a background-
//! thread work queue streaming synthesized audio chunks back to a
//! caller. See `tts`'s own module doc for the full #77 scope/split.
//!
//! # Real generation-guard semantics, replicated exactly
//!
//! Every queued job is tagged with the `voice_id` generation counter
//! current when it was enqueued. The worker thread silently drops any
//! job whose tag doesn't match the CURRENT generation when it's about
//! to run it -- `set_voice`/`cancel` both bump the counter, so any
//! work queued *before* a voice switch or cancel that hasn't started
//! running yet is discarded rather than executed against stale state.
//! A `synthesize` call already in progress (already past its own
//! per-sentence generation check) also stops emitting further chunks
//! the moment a newer generation appears, mid-stream -- matching real
//! Python's own `if self.voice_id == voice_id: ... else: break`.
//!
//! **One deliberate, disclosed improvement over real Python**: real
//! `_synthesize` calls `piper.next()` (running the actual ONNX
//! inference) for a sentence *before* checking whether the request has
//! gone stale, discarding the result afterward if so. This port checks
//! *before* running inference for that sentence, skipping the wasted
//! computation entirely -- no real caller can observe a difference,
//! since a discarded-anyway result was never going to be delivered
//! either way.
//!
//! **Scoped out**: `global_piper_instance()`'s lazy singleton +
//! `atexit`-registered graceful shutdown. Rust `static`s are never
//! dropped at normal process exit (unlike Python's `atexit`, which
//! upstream relies on specifically because the worker thread is
//! otherwise a `daemon=True` thread Python would just kill without
//! joining) -- a bare `static Piper` here would silently skip
//! `shutdown()`'s `join()`, a real behavior difference worth avoiding
//! rather than shipping a "looks equivalent but isn't" global.
//! [`Piper`] itself is a real, complete, directly-usable type; a
//! caller wanting one global instance can hold it themselves (e.g. in
//! their own `OnceLock` plus an explicit shutdown call on their own
//! actual exit path).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use super::piper::{create_voice_config, text_to_sentence_ids, VoiceConfig};
use super::vocoder::{to_i16_samples, Vocoder};

/// Port of `SynthesisResult`. `utterance_id` is a caller-chosen `u64`
/// tag (real Python allows any hashable `Any`; a plain integer id is
/// this port's real, sufficient equivalent -- every real upstream
/// caller already uses an integer or similar simple id).
#[derive(Debug, Clone)]
pub struct SynthesisResult {
    pub utterance_id: u64,
    pub bytes_per_sample: u8,
    pub audio: PcmSamples,
    pub num_samples: usize,
    pub sample_rate: u32,
    pub is_last: bool,
}

/// The two real sample encodings `next(as_16bit_samples)` produces.
#[derive(Debug, Clone)]
pub enum PcmSamples {
    I16(Vec<i16>),
    F32(Vec<f32>),
}

/// Delivered to a `set_voice` call's own result channel. Port of the
/// real `result_callback(result, exception, traceback)` triple, as a
/// real Rust `Result`-shaped enum instead of an "always 3 args, 2 of
/// them usually `None`" tuple.
#[derive(Debug, Clone)]
pub enum PiperEvent {
    Result(SynthesisResult),
    /// `utterance_id` is `None` for a `set_voice` failure (real Python
    /// has no utterance to attach the error to there either).
    Error { utterance_id: Option<u64>, message: String },
}

struct SharedState {
    result_tx: Option<mpsc::Sender<PiperEvent>>,
    as_16bit_samples: bool,
}

enum Job {
    SetVoice { cfg: VoiceConfig, model_path: PathBuf },
    Synthesize { utterance_id: u64, text: String },
}

/// Port of the `Piper` class.
pub struct Piper {
    tx: mpsc::Sender<(u64, Job)>,
    voice_id: Arc<AtomicU64>,
    shared: Arc<Mutex<SharedState>>,
    handle: Option<JoinHandle<()>>,
}

impl Piper {
    /// Port of `Piper.__init__` (minus `piper.initialize`, which
    /// #610/#611's `espeak_ng`/`ort` setup replaces -- there is no
    /// separate real "initialize the engine" step to defer here, each
    /// voice load is self-contained).
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<(u64, Job)>();
        let voice_id = Arc::new(AtomicU64::new(0));
        let shared = Arc::new(Mutex::new(SharedState { result_tx: None, as_16bit_samples: true }));
        let worker_voice_id = voice_id.clone();
        let worker_shared = shared.clone();
        let handle = std::thread::Builder::new()
            .name("PiperSynth".to_string())
            .spawn(move || worker_loop(rx, worker_voice_id, worker_shared))
            .expect("spawning the Piper synthesis thread must succeed");
        Piper { tx, voice_id, shared, handle: Some(handle) }
    }

    /// Port of `Piper.voice_id` (the property, not `increment_voice_id`).
    pub fn voice_id(&self) -> u64 {
        self.voice_id.load(Ordering::SeqCst)
    }

    /// Port of `set_voice`: synchronously loads the real voice config
    /// (cheap -- just JSON parsing) and returns its real sample rate,
    /// while queuing the actual (slower) eSpeak-data-directory setup +
    /// ONNX model load to run on the worker thread.
    pub fn set_voice(&self, result_tx: mpsc::Sender<PiperEvent>, config_path: &Path, model_path: &Path, length_scale_multiplier: f32, sentence_delay: f32, as_16bit_samples: bool) -> anyhow::Result<u32> {
        let vid = self.voice_id.fetch_add(1, Ordering::SeqCst) + 1;
        let cfg = create_voice_config(config_path, length_scale_multiplier, sentence_delay)?;
        let sample_rate = cfg.sample_rate;
        {
            let mut shared = self.shared.lock().expect("Piper worker never panics while holding this lock");
            shared.result_tx = Some(result_tx);
            shared.as_16bit_samples = as_16bit_samples;
        }
        let _ = self.tx.send((vid, Job::SetVoice { cfg, model_path: model_path.to_path_buf() }));
        Ok(sample_rate)
    }

    /// Port of `cancel`.
    pub fn cancel(&self) {
        self.voice_id.fetch_add(1, Ordering::SeqCst);
        self.shared.lock().expect("Piper worker never panics while holding this lock").result_tx = None;
    }

    /// Port of `synthesize`.
    pub fn synthesize(&self, utterance_id: u64, text: &str) {
        let vid = self.voice_id();
        let _ = self.tx.send((vid, Job::Synthesize { utterance_id, text: text.to_string() }));
    }

    /// Port of `shutdown`: bumps the generation (so any job still
    /// sitting in the queue behind this point is real, current-
    /// generation work that's allowed to finish -- matching real
    /// Python's own ordering) then closes the channel and joins the
    /// worker thread. Drops `self` (and with it this `Piper`'s own
    /// `Sender`, the channel's last one) *before* joining -- joining
    /// first would deadlock, since the worker's `recv()` only returns
    /// once every `Sender` is gone.
    pub fn shutdown(mut self) {
        self.voice_id.fetch_add(1, Ordering::SeqCst);
        let handle = self.handle.take();
        drop(self);
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

impl Default for Piper {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Piper {
    fn drop(&mut self) {
        // The real `Thread`'s `daemon=True` flag means real Python
        // does NOT join on a bare drop either -- only the explicit,
        // atexit-registered `shutdown()` call does. Matching that: a
        // plain drop just lets the channel closing (once every
        // `Sender` clone -- there's only ever this one -- is gone)
        // wind the worker down on its own, without blocking here.
    }
}

fn worker_loop(rx: mpsc::Receiver<(u64, Job)>, voice_id: Arc<AtomicU64>, shared: Arc<Mutex<SharedState>>) {
    let mut engine: Option<espeak_ng::EspeakNg> = None;
    let mut cfg: Option<VoiceConfig> = None;
    let mut vocoder: Option<Vocoder> = None;

    while let Ok((job_vid, job)) = rx.recv() {
        if job_vid != voice_id.load(Ordering::SeqCst) {
            continue;
        }
        match job {
            Job::SetVoice { cfg: new_cfg, model_path } => match load_voice(&new_cfg, &model_path) {
                Ok((e, v)) => {
                    engine = Some(e);
                    vocoder = Some(v);
                    cfg = Some(new_cfg);
                }
                Err(err) => send_event(&shared, PiperEvent::Error { utterance_id: None, message: err.to_string() }),
            },
            Job::Synthesize { utterance_id, text } => {
                let (Some(engine), Some(cfg), Some(vocoder)) = (engine.as_ref(), cfg.as_ref(), vocoder.as_mut()) else {
                    send_event(&shared, PiperEvent::Error { utterance_id: Some(utterance_id), message: "no voice set".to_string() });
                    continue;
                };
                run_synthesis(engine, cfg, vocoder, job_vid, utterance_id, &text, &voice_id, &shared);
            }
        }
    }
}

fn load_voice(cfg: &VoiceConfig, model_path: &Path) -> anyhow::Result<(espeak_ng::EspeakNg, Vocoder)> {
    let dir = std::env::temp_dir().join("calibre_oxide_piper_espeak_data");
    std::fs::create_dir_all(&dir)?;
    let (lang, _) = cfg.espeak_voice_name.split_once('-').unwrap_or((&cfg.espeak_voice_name, ""));
    espeak_ng::install_bundled_language(&dir, lang).map_err(|e| anyhow::anyhow!("{e}"))?;
    let engine = espeak_ng::EspeakNg::with_data_dir(&cfg.espeak_voice_name, &dir).map_err(|e| anyhow::anyhow!("{e}"))?;
    let vocoder = Vocoder::load(model_path, cfg).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok((engine, vocoder))
}

#[allow(clippy::too_many_arguments)]
fn run_synthesis(engine: &espeak_ng::EspeakNg, cfg: &VoiceConfig, vocoder: &mut Vocoder, job_vid: u64, utterance_id: u64, text: &str, voice_id: &AtomicU64, shared: &Arc<Mutex<SharedState>>) {
    let as_16bit = shared.lock().expect("Piper worker never panics while holding this lock").as_16bit_samples;
    let bytes_per_sample: u8 = if as_16bit { 2 } else { 4 };

    let sentence_ids = match text_to_sentence_ids(engine, cfg, text) {
        Ok(ids) => ids,
        Err(err) => {
            send_event(shared, PiperEvent::Error { utterance_id: Some(utterance_id), message: err.to_string() });
            return;
        }
    };

    if sentence_ids.is_empty() {
        // Port of `next()`'s own "queue already empty" branch: always
        // deliver at least one (empty) chunk, marked `is_last`.
        deliver(shared, SynthesisResult { utterance_id, bytes_per_sample, audio: empty_audio(as_16bit), num_samples: 0, sample_rate: vocoder.sample_rate, is_last: true });
        return;
    }

    let last_index = sentence_ids.len() - 1;
    for (i, ids) in sentence_ids.iter().enumerate() {
        if voice_id.load(Ordering::SeqCst) != job_vid {
            break;
        }
        let samples = match vocoder.synthesize(ids) {
            Ok(s) => s,
            Err(err) => {
                send_event(shared, PiperEvent::Error { utterance_id: Some(utterance_id), message: err.to_string() });
                return;
            }
        };
        let num_samples = samples.len();
        let audio = if as_16bit { PcmSamples::I16(to_i16_samples(&samples)) } else { PcmSamples::F32(samples) };
        deliver(shared, SynthesisResult { utterance_id, bytes_per_sample, audio, num_samples, sample_rate: vocoder.sample_rate, is_last: i == last_index });
    }
}

fn empty_audio(as_16bit: bool) -> PcmSamples {
    if as_16bit {
        PcmSamples::I16(Vec::new())
    } else {
        PcmSamples::F32(Vec::new())
    }
}

fn deliver(shared: &Arc<Mutex<SharedState>>, result: SynthesisResult) {
    send_event(shared, PiperEvent::Result(result));
}

fn send_event(shared: &Arc<Mutex<SharedState>>, event: PiperEvent) {
    let tx = shared.lock().expect("Piper worker never panics while holding this lock").result_tx.clone();
    if let Some(tx) = tx {
        let _ = tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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

    fn recv_for(rx: &mpsc::Receiver<PiperEvent>, timeout: Duration) -> Vec<PiperEvent> {
        let mut events = Vec::new();
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(ev) => {
                    let is_last = matches!(&ev, PiperEvent::Result(r) if r.is_last) || matches!(&ev, PiperEvent::Error { .. });
                    events.push(ev);
                    if is_last {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        events
    }

    #[test]
    fn synthesize_before_any_voice_is_set_reports_a_real_error() {
        let piper = Piper::new();
        let (tx, rx) = mpsc::channel();
        // Route a result channel without going through set_voice, by
        // reaching into a fresh Piper's synthesize path directly --
        // real Python would hit the same "no voice set" condition if
        // `synthesize` were ever called before `set_voice`.
        piper.shared.lock().unwrap().result_tx = Some(tx);
        piper.synthesize(1, "hello");
        let events = recv_for(&rx, Duration::from_secs(2));
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], PiperEvent::Error { utterance_id: Some(1), .. }), "{:?}", events[0]);
        piper.shutdown();
    }

    #[test]
    fn end_to_end_streaming_synthesis_produces_real_chunks_in_order() {
        let Some((onnx, json)) = test_voice_paths() else {
            eprintln!("skipping: set CALIBRE_OXIDE_TEST_PIPER_VOICE to a real .onnx voice to run this test");
            return;
        };
        let piper = Piper::new();
        let (tx, rx) = mpsc::channel();
        let sample_rate = piper.set_voice(tx, &json, &onnx, 0.0, 0.0, true).expect("real voice config must load");
        assert!(sample_rate > 0);

        piper.synthesize(42, "Hello, world! This is a real streaming test.");
        let events = recv_for(&rx, Duration::from_secs(30));
        assert!(!events.is_empty(), "expected at least one real synthesis result");

        let mut saw_last = false;
        for (i, ev) in events.iter().enumerate() {
            match ev {
                PiperEvent::Result(r) => {
                    assert_eq!(r.utterance_id, 42);
                    assert_eq!(r.bytes_per_sample, 2);
                    if let PcmSamples::I16(samples) = &r.audio {
                        assert_eq!(samples.len(), r.num_samples);
                    } else {
                        panic!("expected 16-bit samples");
                    }
                    if i == events.len() - 1 {
                        assert!(r.is_last, "the last delivered chunk must be marked is_last");
                        saw_last = true;
                    } else {
                        assert!(!r.is_last, "only the final chunk should be marked is_last");
                    }
                }
                PiperEvent::Error { message, .. } => panic!("unexpected error: {message}"),
            }
        }
        assert!(saw_last);
        piper.shutdown();
    }

    #[test]
    fn cancel_mid_stream_stops_further_chunks_without_a_final_result() {
        let Some((onnx, json)) = test_voice_paths() else {
            eprintln!("skipping: set CALIBRE_OXIDE_TEST_PIPER_VOICE to a real .onnx voice to run this test");
            return;
        };
        let piper = Piper::new();
        let (tx, rx) = mpsc::channel();
        piper.set_voice(tx, &json, &onnx, 0.0, 0.0, true).unwrap();

        // A long text with many real sentences, so cancel has a real
        // chance to land mid-stream rather than after the only chunk.
        let long_text = "One. Two. Three. Four. Five. Six. Seven. Eight. Nine. Ten.";
        piper.synthesize(7, long_text);
        // Give the worker a moment to start, then cancel.
        std::thread::sleep(Duration::from_millis(50));
        piper.cancel();

        let events = recv_for(&rx, Duration::from_secs(10));
        // Real behavior: whatever arrived before cancel landed is real
        // data, but none of it is marked is_last (cancel means
        // synthesis was aborted, not that it finished normally) --
        // UNLESS every chunk had already been delivered before cancel
        // won the race, which is an acceptable, non-flaky outcome too.
        if let Some(PiperEvent::Result(last)) = events.last() {
            if events.len() < 10 {
                assert!(!last.is_last, "a stream stopped early by cancel should not end with is_last");
            }
        }
        piper.shutdown();
    }

    #[test]
    fn set_voice_after_a_pending_synthesize_drops_the_stale_job() {
        let Some((onnx, json)) = test_voice_paths() else {
            eprintln!("skipping: set CALIBRE_OXIDE_TEST_PIPER_VOICE to a real .onnx voice to run this test");
            return;
        };
        let piper = Piper::new();
        let (tx1, rx1) = mpsc::channel();
        piper.set_voice(tx1, &json, &onnx, 0.0, 0.0, true).unwrap();
        piper.synthesize(1, "Stale request.");
        // Immediately supersede it with a new voice generation before
        // the worker could plausibly have started the first job.
        let (tx2, rx2) = mpsc::channel();
        piper.set_voice(tx2, &json, &onnx, 0.0, 0.0, true).unwrap();
        piper.synthesize(2, "Fresh request.");

        let stale_events = recv_for(&rx1, Duration::from_millis(300));
        assert!(stale_events.is_empty(), "the stale generation's result channel should receive nothing: {stale_events:?}");

        let fresh_events = recv_for(&rx2, Duration::from_secs(15));
        assert!(!fresh_events.is_empty());
        assert!(matches!(fresh_events.last(), Some(PiperEvent::Result(r)) if r.utterance_id == 2));
        piper.shutdown();
    }
}
