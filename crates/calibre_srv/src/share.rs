//! `POST /share/email` -- a real, **new** route (no upstream
//! `calibre.srv` route exists for this either: real upstream's own
//! "Connect & share" feature is a desktop-GUI-only "send to device/
//! email" action driven directly from `calibre.utils.smtp`, never
//! exposed over HTTP). This is the first real HTTP-reachable caller of
//! `calibre_utils::smtp` (issue #468) -- confirmed via a full grep of
//! `calibre_srv` finding zero prior references.
//!
//! # Scope
//!
//! Sends one of a book's own formats as an email attachment through a
//! caller-supplied SMTP relay. "Export to folder" (#724's other named
//! feature) needs no new route at all: `BookDetailsPanel.vue`'s
//! existing per-format download links (`ajax::book_json`'s own
//! `main_format`/`other_formats` URLs) already let a browser save a
//! format to any folder the user picks via its own save dialog -- a
//! real, already-shipped path, not a gap.
//!
//! # Real, disclosed narrowing: no persisted SMTP account
//!
//! Issue #724's own body flags that real SMTP account settings need
//! somewhere to live, "likely ties into #721's settings epic." That
//! epic doesn't exist yet, so this route accepts the relay
//! configuration in the request body on every call rather than
//! blocking on it -- a real, working feature today, with real account
//! *persistence* a genuine, separate follow-up once #721 lands (the
//! client can store its own relay config locally in the meantime).
//!
//! # SSRF: the relay host is validated the same way `news.rs` validates feed URLs
//!
//! An earlier version of this doc comment argued no SSRF check was
//! needed here because a send-only SMTP connection doesn't leak a
//! response body back to the caller the way a fetched page would.
//! That reasoning was incomplete: letting a caller point this server
//! at an arbitrary internal host:port and have it speak
//! attacker-influenced bytes (a crafted `from`/`to`/subject/body) is a
//! real "blind" SSRF / protocol-smuggling vector even with no response
//! leakage -- e.g. probing which internal ports are open (a
//! differential-error-message oracle), or smuggling a line-based
//! command to a non-SMTP internal service (Redis, Memcached) that
//! happens to tolerate malformed input enough to act on an early line.
//! The impact doesn't require the caller to read a response, only that
//! the server dials and writes to a host of the caller's choosing.
//!
//! So the relay host is DNS-resolved and checked against the same
//! disallowed-address list `news::validate_feed_url` uses (see
//! [`crate::net_guard`]) before `RelayConfig` is ever constructed.
//! `send_via_relay`'s own error detail is also not surfaced to the
//! client (logged server-side instead, generic message returned) --
//! even for a *permitted* destination, echoing back "connection
//! refused" vs. "timed out" vs. "TLS handshake failed" would let a
//! caller fingerprint what's listening on a given public host:port,
//! which this route has no business revealing.

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use calibre_utils::smtp::{create_mail, send_via_relay, Encryption, MailAttachment, RelayConfig};

use crate::errors::ServerError;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct RelayConfigBody {
    relay: String,
    port: Option<u16>,
    username: Option<String>,
    password: Option<String>,
    /// `"tls"` (STARTTLS, default), `"ssl"` (implicit TLS), or `"none"`.
    #[serde(default)]
    encryption: Option<String>,
}

fn parse_encryption(raw: Option<&str>) -> Result<Encryption, ServerError> {
    match raw.map(str::to_lowercase).as_deref() {
        None | Some("tls") => Ok(Encryption::Tls),
        Some("ssl") => Ok(Encryption::Ssl),
        Some("none") => Ok(Encryption::None),
        Some(other) => Err(ServerError::BadRequest(format!("Invalid encryption: {other:?} (expected tls, ssl, or none)"))),
    }
}

#[derive(Debug, Deserialize)]
pub struct ShareEmailBody {
    book_id: i32,
    format: String,
    from: String,
    to: String,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    text: Option<String>,
    relay: RelayConfigBody,
}

#[derive(Debug, Deserialize)]
pub struct ShareQuery {
    library_id: Option<String>,
}

/// `POST /share/email`.
pub async fn share_email(State(state): State<AppState>, Query(q): Query<ShareQuery>, Json(body): Json<ShareEmailBody>) -> Result<Json<Value>, ServerError> {
    let cache = state.cache_for(q.library_id.as_deref()).ok_or_else(|| ServerError::NotFound(format!("no library named {:?}", q.library_id.unwrap_or_default())))?;
    let encryption = parse_encryption(body.relay.encryption.as_deref())?;

    // Default ports mirror lettre's own per-encryption defaults
    // (`SmtpTransport::relay`/`starttls_relay`/`builder_dangerous`) --
    // only used here to pick a port for the DNS-resolution check, the
    // real connection still goes through `send_via_relay`/`RelayConfig`
    // unchanged.
    let guard_port = body.relay.port.unwrap_or(match encryption {
        Encryption::Ssl => 465,
        Encryption::Tls => 587,
        Encryption::None => 25,
    });
    crate::net_guard::resolve_and_check(&body.relay.relay, guard_port).await.map_err(|e| ServerError::BadRequest(format!("relay {}: {e}", body.relay.relay)))?;

    tokio::task::spawn_blocking(move || -> Result<(), ServerError> {
        let fmt_lower = body.format.to_lowercase();
        let ids: std::collections::HashSet<i32> = std::iter::once(body.book_id).collect();
        let rows = cache.get_data_as_dict(None, true, Some(&ids), false).map_err(|e| ServerError::InternalServerError(e.to_string()))?;
        let row = rows.into_iter().next().ok_or_else(|| ServerError::book_not_found(body.book_id, "default"))?;
        let path_str = row.get(format!("fmt_{fmt_lower}")).and_then(|v| v.as_str()).ok_or_else(|| ServerError::NotFound(format!("No {fmt_lower} format for book {}", body.book_id)))?;
        let path = std::path::PathBuf::from(path_str);
        let data = std::fs::read(&path).map_err(|e| ServerError::InternalServerError(format!("failed to read {path:?}: {e}")))?;

        let title = row.get("title").and_then(|v| v.as_str()).map(str::to_string).unwrap_or_else(|| format!("Book {}", body.book_id));
        let filename = format!("{title}.{fmt_lower}");
        let content_type = mime_guess::from_path(&filename).first_or_octet_stream().to_string();
        let subject = body.subject.unwrap_or_else(|| title.clone());

        let msg = create_mail(&body.from, &body.to, &subject, body.text.as_deref(), Some(MailAttachment { data, content_type, filename })).map_err(|e| ServerError::BadRequest(e.to_string()))?;

        let cfg = RelayConfig { relay: body.relay.relay, port: body.relay.port, username: body.relay.username, password: body.relay.password, encryption, timeout: Some(std::time::Duration::from_secs(30)) };
        send_via_relay(&msg, &cfg).map_err(|e| {
            // Detail logged server-side only -- see this module's doc
            // comment on why leaking the raw error (connection
            // refused vs. timed out vs. TLS failure, etc.) back to the
            // client would make this route usable as a scan oracle.
            eprintln!("share_email: send_via_relay failed: {e:#}");
            ServerError::InternalServerError("failed to send mail".to_string())
        })?;
        Ok(())
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))??;

    Ok(Json(json!({"ok": true})))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use calibre_db::cache::Cache;

    fn add_test_book(dir: &std::path::Path, cache: &Cache, title: &str) -> i32 {
        let source = dir.join(format!("{title}.txt"));
        std::fs::write(&source, "Hello, world!").unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["Author".to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    fn test_app() -> (tempfile::TempDir, axum::Router, i32) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let book_id = add_test_book(dir.path(), &cache, "Test Book");
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
        };
        let router = crate::test_router(state);
        (dir, router, book_id)
    }

    async fn post_json(router: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) };
        (status, value)
    }

    #[tokio::test]
    async fn share_email_404s_for_a_format_the_book_does_not_have() {
        let (_dir, router, book_id) = test_app();
        let (status, body) = post_json(
            &router,
            "/share/email",
            serde_json::json!({"book_id": book_id, "format": "pdf", "from": "me@example.com", "to": "you@example.com", "relay": {"relay": "127.0.0.1"}}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    }

    #[tokio::test]
    async fn share_email_404s_for_an_unknown_book() {
        let (_dir, router, _book_id) = test_app();
        let (status, _) = post_json(
            &router,
            "/share/email",
            serde_json::json!({"book_id": 999999, "format": "txt", "from": "me@example.com", "to": "you@example.com", "relay": {"relay": "127.0.0.1"}}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn share_email_rejects_an_invalid_encryption_value() {
        let (_dir, router, book_id) = test_app();
        let (status, _) = post_json(
            &router,
            "/share/email",
            serde_json::json!({"book_id": book_id, "format": "txt", "from": "me@example.com", "to": "you@example.com", "relay": {"relay": "127.0.0.1", "encryption": "bogus"}}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn share_email_rejects_a_relay_that_resolves_to_a_private_address() {
        let (_dir, router, book_id) = test_app();
        let (status, body) = post_json(
            &router,
            "/share/email",
            serde_json::json!({"book_id": book_id, "format": "txt", "from": "me@example.com", "to": "you@example.com", "relay": {"relay": "10.0.0.5"}}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }

    #[tokio::test]
    async fn share_email_rejects_a_relay_that_resolves_to_a_link_local_address() {
        let (_dir, router, book_id) = test_app();
        let (status, body) = post_json(
            &router,
            "/share/email",
            serde_json::json!({"book_id": book_id, "format": "txt", "from": "me@example.com", "to": "you@example.com", "relay": {"relay": "169.254.169.254"}}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }

    #[tokio::test]
    async fn share_email_rejects_an_invalid_from_address() {
        let (_dir, router, book_id) = test_app();
        let (status, _) = post_json(
            &router,
            "/share/email",
            serde_json::json!({"book_id": book_id, "format": "txt", "from": "not-an-email", "to": "you@example.com", "relay": {"relay": "127.0.0.1"}}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn share_email_reports_a_send_failure_generically_without_leaking_the_relay_error() {
        use std::net::TcpListener;

        // Bind then immediately drop the listener -- the port is real
        // and resolvable (loopback) but nothing is listening, so
        // `send_via_relay` fails with a real "connection refused".
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let (_dir, router, book_id) = test_app();
        let req = Request::builder()
            .method("POST")
            .uri("/share/email")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "book_id": book_id, "format": "txt", "from": "me@example.com", "to": "you@example.com",
                    "relay": {"relay": addr.ip().to_string(), "port": addr.port(), "encryption": "none"},
                })
                .to_string(),
            ))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(text, "failed to send mail", "raw relay error detail must not reach the client: {text}");
    }

    #[tokio::test]
    async fn share_email_sends_a_real_message_through_a_real_local_smtp_server() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        // A minimal real SMTP server: accepts one connection, plays
        // along with the real dialog lettre's SmtpTransport actually
        // speaks (EHLO/MAIL FROM/RCPT TO/DATA/QUIT), and records the
        // raw DATA payload so the test can assert on real message
        // content, not just "no error was returned."
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            writer.write_all(b"220 test.local ESMTP\r\n").unwrap();
            let mut data = String::new();
            let mut in_data = false;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                if in_data {
                    if line.trim_end() == "." {
                        in_data = false;
                        writer.write_all(b"250 OK\r\n").unwrap();
                        continue;
                    }
                    data.push_str(&line);
                    continue;
                }
                let upper = line.to_uppercase();
                if upper.starts_with("EHLO") {
                    writer.write_all(b"250-test.local\r\n250 OK\r\n").unwrap();
                } else if upper.starts_with("MAIL FROM") || upper.starts_with("RCPT TO") {
                    writer.write_all(b"250 OK\r\n").unwrap();
                } else if upper.starts_with("DATA") {
                    writer.write_all(b"354 Send data\r\n").unwrap();
                    in_data = true;
                } else if upper.starts_with("QUIT") {
                    writer.write_all(b"221 Bye\r\n").unwrap();
                    break;
                } else {
                    writer.write_all(b"250 OK\r\n").unwrap();
                }
            }
            data
        });

        let (_dir, router, book_id) = test_app();
        let (status, body) = post_json(
            &router,
            "/share/email",
            serde_json::json!({
                "book_id": book_id, "format": "txt", "from": "me@example.com", "to": "you@example.com",
                "subject": "A real subject", "relay": {"relay": addr.ip().to_string(), "port": addr.port(), "encryption": "none"},
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let data = handle.join().unwrap();
        assert!(data.contains("A real subject"), "{data}");
        assert!(data.contains("Test Book.txt"), "{data}");
    }
}
