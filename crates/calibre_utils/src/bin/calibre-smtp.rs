//! `calibre-smtp` — the console front end to `calibre_utils::smtp` (#813).
//!
//! The engine has been merged and tested for a while; its only caller was
//! `POST /share/email` in the content server, so there was no way to send
//! a book from a shell. This is the wrapper.
//!
//! One narrowing inherited from the engine and disclosed rather than
//! hidden: it can only send through a relay. Upstream also supports
//! direct-to-MX delivery, which `smtp.rs` does not implement, so
//! `--relay` is required here instead of being an option that silently
//! changes the delivery path.

use anyhow::{bail, Context, Result};
use calibre_utils::smtp::{create_mail, send_via_relay, Encryption, MailAttachment, RelayConfig};
use clap::Parser;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "calibre-smtp")]
#[command(about = "Send a message, optionally with a book attached, through an SMTP relay", long_about = None)]
struct Args {
    /// Sender address.
    #[arg(short, long, required = true)]
    from: String,

    /// Recipient address.
    #[arg(short, long, required = true)]
    to: String,

    /// Subject line.
    #[arg(short, long, default_value = "")]
    subject: String,

    /// Message body. Read from stdin if neither this nor --attach is given.
    #[arg(long)]
    text: Option<String>,

    /// A file to attach — usually the book being sent.
    #[arg(short, long, value_name = "FILE")]
    attach: Option<PathBuf>,

    /// SMTP relay hostname. Required: direct-to-MX delivery is not
    /// implemented (see the module doc).
    #[arg(long, required = true)]
    relay: String,

    /// Relay port. Defaults to 465 for --encryption ssl, 587 otherwise.
    #[arg(long)]
    port: Option<u16>,

    #[arg(long)]
    username: Option<String>,

    /// Relay password. Prefer CALIBRE_SMTP_PASSWORD, which keeps it out
    /// of the shell history and the process list.
    #[arg(long)]
    password: Option<String>,

    /// tls (STARTTLS), ssl (implicit TLS), or none.
    #[arg(long, default_value = "tls")]
    encryption: String,

    /// Give up after this many seconds.
    #[arg(long)]
    timeout: Option<u64>,
}

fn encryption_from(name: &str) -> Result<Encryption> {
    match name.to_ascii_lowercase().as_str() {
        "tls" | "starttls" => Ok(Encryption::Tls),
        "ssl" => Ok(Encryption::Ssl),
        "none" => Ok(Encryption::None),
        other => bail!("unknown --encryption {other:?}: expected tls, ssl or none"),
    }
}

/// Guesses an attachment's MIME type from its extension.
///
/// `create_mail` falls back to `application/octet-stream` for anything it
/// cannot parse, so a wrong guess degrades rather than fails — but
/// getting the common book types right is what makes the mail open
/// correctly on a device.
fn content_type_for(path: &std::path::Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    match ext.as_str() {
        "epub" => "application/epub+zip",
        "kepub" => "application/epub+zip",
        "mobi" | "azw" | "azw3" | "prc" => "application/x-mobipocket-ebook",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "fb2" => "application/x-fictionbook+xml",
        "djvu" => "image/vnd.djvu",
        "rtf" => "application/rtf",
        "zip" | "cbz" => "application/zip",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn main() -> Result<()> {
    let args = Args::parse();

    let attachment = match &args.attach {
        Some(path) => {
            if !path.is_file() {
                bail!("{} is not a file", path.display());
            }
            Some(MailAttachment {
                data: std::fs::read(path).with_context(|| format!("reading {}", path.display()))?,
                content_type: content_type_for(path),
                filename: path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            })
        }
        None => None,
    };

    // `create_mail` requires at least one of text/attachment. Reading
    // stdin when neither was given is what makes this usable in a pipe,
    // and beats erroring on the common `echo … | calibre-smtp` shape.
    let text = match (&args.text, &attachment) {
        (Some(text), _) => Some(text.clone()),
        (None, None) => {
            let mut buffer = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buffer)?;
            if buffer.trim().is_empty() {
                bail!("nothing to send: pass --text, pipe a body on stdin, or use --attach");
            }
            Some(buffer)
        }
        (None, Some(_)) => None,
    };

    let message = create_mail(&args.from, &args.to, &args.subject, text.as_deref(), attachment)?;

    // The environment variable is preferred so a password need not appear
    // in shell history or in `ps` output.
    let password = std::env::var("CALIBRE_SMTP_PASSWORD").ok().or_else(|| args.password.clone());

    let config = RelayConfig {
        relay: args.relay.clone(),
        port: args.port,
        username: args.username.clone(),
        password,
        encryption: encryption_from(&args.encryption)?,
        timeout: args.timeout.map(Duration::from_secs),
    };

    send_via_relay(&message, &config).with_context(|| format!("sending through {}", args.relay))?;
    eprintln!("Sent to {}", args.to);
    Ok(())
}
