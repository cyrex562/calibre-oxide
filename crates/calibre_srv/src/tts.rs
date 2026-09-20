//! `POST /tts/synthesize` -- a real, **new** route (issue #756, part
//! of #747's tracking epic): synthesize speech for arbitrary text on
//! demand, for the reader's own "Read aloud" mode.
//!
//! # Real prerequisite check, resolved
//!
//! Two real TTS entry points already exist (#77 epic): the low-level
//! streaming [`calibre_ebooks::tts::stream::Piper`], and the
//! higher-level [`calibre_ebooks::tts::batch::text_to_raw_audio_data`]
//! (multiple texts through one voice load, real, tested, already the
//! real caller `oeb::polish::tts::embed_tts` uses for its own
//! whole-book narration). This route reuses `text_to_raw_audio_data`
//! with a single input text per call -- it already wraps `Piper` for
//! exactly the "synthesize on demand" shape this issue's own body
//! calls out as the better fit (as opposed to `embed_tts`'s own
//! ahead-of-time whole-book pipeline), without this route needing to
//! drive `Piper`'s raw event channel itself.
//!
//! # Real, disclosed narrowing: collect-then-respond, not chunked streaming
//!
//! "Streaming" in `stream::Piper`'s own name refers to per-sentence
//! synthesis, not HTTP chunked transfer -- this route still waits for
//! the full requested text to finish synthesizing before responding
//! with one complete WAV body. A real chunked/`Range`-served response
//! (so playback could start before synthesis of later sentences
//! finishes) is real, separable follow-up work; for the short,
//! per-spine-file text lengths a real reader page actually contains,
//! the round-trip latency this avoids optimizing is small.
//!
//! # Real, disclosed narrowing: no voice resolution/download
//!
//! Matches `calibre_ebooks::tts::batch`'s own module doc: this crate
//! has no `(lang, voice_name) -> (config_path, model_path)` resolution
//! or voice-downloading machinery anywhere. The server operator
//! configures exactly one voice via `--tts-voice <path-to-voice.onnx>`
//! (its sibling `<path>.json` is used as the config); `/tts/synthesize`
//! 503s with a clear message if no voice was configured, rather than
//! silently returning empty audio.

use std::path::PathBuf;
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;

use calibre_ebooks::tts::batch::text_to_raw_audio_data;
use calibre_ebooks::tts::transcode::wav_from_pcm16le;

use crate::errors::ServerError;
use crate::AppState;

/// A single server-wide TTS voice (see this module's own doc for why
/// there's no per-language/per-book selection yet).
#[derive(Debug, Clone)]
pub struct TtsVoiceConfig {
    pub config_path: PathBuf,
    pub model_path: PathBuf,
}

impl TtsVoiceConfig {
    /// `model_path` is the `.onnx` file itself; its config lives at
    /// the same path with `.json` appended, matching real Piper's own
    /// on-disk voice-file convention (`voice.onnx` + `voice.onnx.json`).
    pub fn from_model_path(model_path: PathBuf) -> Self {
        let mut config_path = model_path.clone().into_os_string();
        config_path.push(".json");
        TtsVoiceConfig { config_path: PathBuf::from(config_path), model_path }
    }
}

#[derive(Debug, Deserialize)]
pub struct SynthesizeBody {
    text: String,
}

/// `POST /tts/synthesize`.
pub async fn synthesize(State(state): State<AppState>, Json(body): Json<SynthesizeBody>) -> Result<Response, ServerError> {
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return Err(ServerError::BadRequest("text must not be empty".to_string()));
    }
    let Some(voice) = state.tts_voice.clone() else {
        return Err(ServerError::ServiceUnavailable("TTS is not configured on this server -- start it with --tts-voice <path-to-voice.onnx>".to_string()));
    };

    let wav = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let synthesized = text_to_raw_audio_data(&voice.config_path, &voice.model_path, [text.as_str()], 0.0, 0.0, Duration::from_secs(30))?;
        let utterance = synthesized.utterances.into_iter().next().unwrap_or(calibre_ebooks::tts::batch::Utterance { audio: Vec::new(), duration: 0.0 });
        wav_from_pcm16le(&utterance.audio, synthesized.sample_rate, 1)
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
    .map_err(|e| ServerError::InternalServerError(format!("{e:#}")))?;

    let mut resp = Response::new(Body::from(wav));
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    fn test_app(tts_voice: Option<super::TtsVoiceConfig>) -> (tempfile::TempDir, axum::Router) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let state = crate::AppState {
            libraries: None,
            cache: std::sync::Arc::new(cache),
            opts: std::sync::Arc::new(crate::opts::ServerOptions::default()),
            auth: None,
            changes: crate::web_socket::new_change_broadcaster(),
            reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()),
            book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))),
            render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()),
            news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()),
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()),
            news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()),
            tts_voice: tts_voice.map(std::sync::Arc::new), plugin_store: None,
        };
        let router = crate::test_router(state);
        (dir, router)
    }

    async fn post_json(router: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, axum::body::Bytes, Option<String>) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let content_type = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, bytes, content_type)
    }

    #[tokio::test]
    async fn synthesize_503s_when_no_voice_is_configured() {
        let (_dir, router) = test_app(None);
        let (status, _, _) = post_json(&router, "/tts/synthesize", serde_json::json!({"text": "hello"})).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn synthesize_rejects_empty_text() {
        // Independent of whether a voice is configured -- empty text
        // is a real, always-a-400 input error, not a 503, so this
        // deliberately runs with no voice at all.
        let (_dir, router) = test_app(None);
        let (status, _, _) = post_json(&router, "/tts/synthesize", serde_json::json!({"text": "   "})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_real_voice_synthesizes_a_real_playable_wav() {
        let Ok(voice_path) = std::env::var("CALIBRE_OXIDE_TEST_PIPER_VOICE") else {
            eprintln!("skipping: set CALIBRE_OXIDE_TEST_PIPER_VOICE to a real .onnx voice to run this test");
            return;
        };
        let (_dir, router) = test_app(Some(super::TtsVoiceConfig::from_model_path(std::path::PathBuf::from(voice_path))));
        let (status, bytes, content_type) = post_json(&router, "/tts/synthesize", serde_json::json!({"text": "Hello, this is a real test."})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type.as_deref(), Some("audio/wav"));
        assert!(bytes.len() > 44, "expected real PCM audio beyond the WAV header, got {} bytes", bytes.len());
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
    }
}
