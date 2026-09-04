//! Port of `calibre.utils.smtp` (issue #468): compose and send an
//! email via an SMTP relay -- the "Send to device/email" feature's
//! delivery mechanism.
//!
//! Redesigned around the `lettre` crate rather than porting
//! `smtp.py`'s own hand-rolled MIME composition (`create_mail`) and
//! `polyglot.smtplib`-based delivery (`sendmail`) line for line, per
//! this issue's own filed scope: `smtplib.py` (a lightly-patched copy
//! of Python's stdlib `smtplib`) is not calibre-specific logic to
//! port, and `lettre` already implements a real, well-tested SMTP
//! client (STARTTLS/implicit-TLS/AUTH) that upstream's own code
//! exists only to wrap.
//!
//! # Disclosed narrowing vs. upstream
//!
//! - `sendmail_direct` (direct delivery by resolving the recipient
//!   domain's own MX records and connecting to it directly, bypassing
//!   any relay) is **not ported**. It needs a DNS resolver dependency
//!   this crate doesn't otherwise have, and no caller in this port
//!   uses it -- upstream's own docs note it's the least reliable path
//!   (no relay-side spam/deliverability handling); a real caller
//!   should configure a relay instead. [`send_via_relay`] is the only
//!   send path this port provides.
//! - The maildir-backed failed-delivery retry queue and the CLI's
//!   `--fork`-and-deliver-in-background orchestration
//!   (`smtp.py`'s `main`) aren't ported -- no send-to-device/email
//!   feature exists yet anywhere in this port to drive that
//!   architecture decision. [`send_via_relay`] returns a real
//!   `Result`; a future caller can build its own retry/queue logic on
//!   top, the same way any Rust caller of a fallible send function
//!   would.
//! - `safe_localhost`/`sanitize_hostname` (computing a EHLO/HELO
//!   hostname for the *direct*-delivery path) aren't ported for the
//!   same reason as `sendmail_direct` -- `lettre`'s relay transports
//!   compute their own `ClientId` by default, overridable via
//!   [`RelayConfig`] if a caller ever needs to.

use anyhow::{Context, Result};
use lettre::message::header::ContentType;
use lettre::message::{Attachment, Message, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{SmtpTransport, Transport};
use std::time::Duration;

/// A file to attach to a composed email -- port of `create_mail`'s
/// `attachment_data`/`attachment_type`/`attachment_name` trio.
pub struct MailAttachment {
    pub data: Vec<u8>,
    /// A MIME type such as `"application/epub+zip"`. Falls back to
    /// `application/octet-stream` if it doesn't parse, matching
    /// upstream's own `except Exception: maintype, subtype =
    /// 'application', 'octet-stream'`.
    pub content_type: String,
    pub filename: String,
}

/// Port of `create_mail`/`compose_mail`: build a real MIME message,
/// either plain text or (with an attachment) `multipart/mixed`. At
/// least one of `text`/`attachment` must be given, matching upstream's
/// own `assert text or attachment_data`.
pub fn create_mail(from: &str, to: &str, subject: &str, text: Option<&str>, attachment: Option<MailAttachment>) -> Result<Message> {
    if text.is_none() && attachment.is_none() {
        anyhow::bail!("create_mail requires text, an attachment, or both");
    }
    let builder = Message::builder().from(from.parse().context("invalid From address")?).to(to.parse().context("invalid To address")?).subject(subject);

    let body_text = text.unwrap_or_default().to_string();

    let message = match attachment {
        None => builder.header(ContentType::TEXT_PLAIN).body(body_text)?,
        Some(att) => {
            let content_type = ContentType::parse(&att.content_type).unwrap_or_else(|_| ContentType::parse("application/octet-stream").unwrap());
            let mut multipart = MultiPart::mixed().singlepart(SinglePart::plain(body_text));
            multipart = multipart.singlepart(Attachment::new(att.filename).body(att.data, content_type));
            builder.multipart(multipart)?
        }
    };
    Ok(message)
}

/// Port of upstream's `encryption` choice (`TLS`/`SSL`/`NONE`).
/// `TLS` is STARTTLS (upgrade a plaintext connection, upstream's
/// default, typically port 587); `Ssl` is implicit TLS from the first
/// byte (typically port 465); `None` is plaintext -- upstream itself
/// warns this is "highly insecure", kept only because a real relay
/// (e.g. a local mail server on the same trusted host) may
/// legitimately run without TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encryption {
    Tls,
    Ssl,
    None,
}

/// Port of `sendmail`'s relay-branch parameters (`relay`, `username`,
/// `password`, `encryption`, `port`, `timeout`). `id_is_uuid`-style
/// direct delivery isn't representable here -- see this module's
/// disclosed narrowing.
pub struct RelayConfig {
    pub relay: String,
    /// `None` picks the same default upstream does: 465 for `Ssl`,
    /// 587 (lettre's own STARTTLS default) otherwise.
    pub port: Option<u16>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub encryption: Encryption,
    pub timeout: Option<Duration>,
}

/// Port of `sendmail`'s relay branch: send an already-composed
/// message through the configured SMTP relay.
pub fn send_via_relay(msg: &Message, cfg: &RelayConfig) -> Result<()> {
    let mut builder = match cfg.encryption {
        Encryption::Ssl => SmtpTransport::relay(&cfg.relay)?,
        Encryption::Tls => SmtpTransport::starttls_relay(&cfg.relay)?,
        Encryption::None => SmtpTransport::builder_dangerous(&cfg.relay),
    };
    if let Some(port) = cfg.port {
        builder = builder.port(port);
    }
    if let (Some(u), Some(p)) = (&cfg.username, &cfg.password) {
        builder = builder.credentials(Credentials::new(u.clone(), p.clone()));
    }
    if let Some(timeout) = cfg.timeout {
        builder = builder.timeout(Some(timeout));
    }
    let mailer = builder.build();
    mailer.send(msg).context("failed to send mail")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    #[test]
    fn create_mail_builds_a_real_plain_text_message() {
        let msg = create_mail("from@example.com", "to@example.com", "hello", Some("body text"), None).unwrap();
        let raw = String::from_utf8(msg.formatted()).unwrap();
        assert!(raw.contains("From: from@example.com"));
        assert!(raw.contains("To: to@example.com"));
        assert!(raw.contains("Subject: hello"));
        assert!(raw.contains("body text"));
    }

    #[test]
    fn create_mail_builds_a_real_multipart_message_with_an_attachment() {
        let att = MailAttachment { data: b"pretend epub bytes".to_vec(), content_type: "application/epub+zip".to_string(), filename: "book.epub".to_string() };
        let msg = create_mail("from@example.com", "to@example.com", "your book", Some("enjoy"), Some(att)).unwrap();
        let raw = String::from_utf8(msg.formatted()).unwrap();
        assert!(raw.contains("multipart/mixed"));
        assert!(raw.contains("book.epub"));
        assert!(raw.contains("application/epub+zip"));
    }

    #[test]
    fn create_mail_falls_back_to_octet_stream_for_an_unparseable_content_type() {
        let att = MailAttachment { data: b"x".to_vec(), content_type: "not a mime type".to_string(), filename: "f.bin".to_string() };
        let msg = create_mail("from@example.com", "to@example.com", "s", None, Some(att)).unwrap();
        let raw = String::from_utf8(msg.formatted()).unwrap();
        assert!(raw.contains("application/octet-stream"));
    }

    #[test]
    fn create_mail_rejects_neither_text_nor_attachment() {
        assert!(create_mail("from@example.com", "to@example.com", "s", None, None).is_err());
    }

    #[test]
    fn create_mail_rejects_an_invalid_address() {
        assert!(create_mail("not-an-address", "to@example.com", "s", Some("x"), None).is_err());
    }

    /// A minimal, real (not mocked) line-based SMTP server: enough of
    /// RFC 5321 to negotiate EHLO/MAIL FROM/RCPT TO/DATA/QUIT with a
    /// real `lettre` `SmtpTransport` and capture what it actually
    /// sent over the wire, matching this project's established
    /// "verify against a real running server" testing discipline
    /// rather than mocking the transport.
    fn spawn_fake_smtp_server() -> (u16, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut writer = stream.try_clone().unwrap();
            let mut reader = BufReader::new(stream);
            writer.write_all(b"220 fake.smtp.test ESMTP\r\n").unwrap();
            let mut data = String::new();
            let mut in_data = false;
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                if in_data {
                    if line.trim_end_matches(['\r', '\n']) == "." {
                        in_data = false;
                        writer.write_all(b"250 OK: message accepted\r\n").unwrap();
                        tx.send(data.clone()).ok();
                        continue;
                    }
                    data.push_str(&line);
                    continue;
                }
                let upper = line.to_ascii_uppercase();
                if upper.starts_with("EHLO") {
                    writer.write_all(b"250-fake.smtp.test\r\n250 8BITMIME\r\n").unwrap();
                } else if upper.starts_with("MAIL FROM") {
                    writer.write_all(b"250 OK\r\n").unwrap();
                } else if upper.starts_with("RCPT TO") {
                    writer.write_all(b"250 OK\r\n").unwrap();
                } else if upper.starts_with("DATA") {
                    writer.write_all(b"354 Send message\r\n").unwrap();
                    in_data = true;
                } else if upper.starts_with("QUIT") {
                    writer.write_all(b"221 Bye\r\n").unwrap();
                    break;
                } else {
                    writer.write_all(b"250 OK\r\n").unwrap();
                }
            }
        });
        (port, rx)
    }

    #[test]
    fn send_via_relay_really_delivers_a_message_over_a_real_tcp_connection() {
        let (port, rx) = spawn_fake_smtp_server();
        let msg = create_mail("from@example.com", "to@example.com", "real send test", Some("hello over the wire"), None).unwrap();
        let cfg = RelayConfig { relay: "127.0.0.1".to_string(), port: Some(port), username: None, password: None, encryption: Encryption::None, timeout: Some(Duration::from_secs(5)) };
        send_via_relay(&msg, &cfg).unwrap();

        let received = rx.recv_timeout(Duration::from_secs(5)).expect("fake server should have received a DATA payload");
        assert!(received.contains("hello over the wire"), "server received: {received}");
        assert!(received.contains("Subject: real send test"), "server received: {received}");
    }

    #[test]
    fn send_via_relay_surfaces_a_real_error_when_the_relay_refuses_the_connection() {
        // Nothing is listening on this port.
        let msg = create_mail("from@example.com", "to@example.com", "s", Some("x"), None).unwrap();
        let cfg = RelayConfig { relay: "127.0.0.1".to_string(), port: Some(1), username: None, password: None, encryption: Encryption::None, timeout: Some(Duration::from_millis(500)) };
        assert!(send_via_relay(&msg, &cfg).is_err());
    }
}
