//! Port of `calibre.srv.opts`: the content server's configuration
//! options. Upstream generates an `optparse` CLI parser and a
//! JSON-config reader/writer from one shared `raw_options` list; this
//! port is a single `clap`-derived struct (matching this whole
//! project's established CLI convention, see `docs/AGENT_PORTING_GUIDE.md`)
//! that doubles as the JSON-serializable config shape via `serde`.
//!
//! `server_config()`/`change_settings()` (a memoized global singleton
//! + in-place mutator) aren't ported -- there's no established
//! mutable-global-config pattern anywhere else in this workspace either;
//! callers just own a [`ServerOptions`] value directly.

use std::path::Path;

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};

/// Port of `raw_options`/`Options`.
#[derive(Debug, Clone, Parser, Serialize, Deserialize)]
#[command(about = "calibre-oxide content server")]
pub struct ServerOptions {
    /// Path to the SSL certificate file
    #[arg(long)]
    pub ssl_certfile: Option<String>,
    /// Path to the SSL private key file
    #[arg(long)]
    pub ssl_keyfile: Option<String>,
    /// Time (in seconds) after which an idle connection is closed
    #[arg(long, default_value_t = 120.0)]
    pub timeout: f64,
    /// Time (in seconds) to wait for a response from the server when making queries
    #[arg(long, default_value_t = 60.0)]
    pub ajax_timeout: f64,
    /// Total time in seconds to wait for clean shutdown
    #[arg(long, default_value_t = 5.0)]
    pub shutdown_timeout: f64,
    /// Socket pre-allocation, for example, with systemd socket activation
    #[arg(long, default_value_t = true)]
    pub allow_socket_preallocation: bool,
    /// Max. size of single HTTP header (in KB)
    #[arg(long, default_value_t = 8.0)]
    pub max_header_line_size: f64,
    /// Max. allowed size for files uploaded to the server (in MB)
    #[arg(long, default_value_t = 500.0)]
    pub max_request_body_size: f64,
    /// Minimum size for which responses use data compression (in bytes)
    #[arg(long, default_value_t = 1024)]
    pub compress_min_size: i64,
    /// Number of worker threads used to process requests
    #[arg(long, default_value_t = 10)]
    pub worker_count: i64,
    /// Maximum number of worker processes
    #[arg(long, default_value_t = 0)]
    pub max_jobs: i64,
    /// Maximum time for worker processes (in minutes, 0 = no limit)
    #[arg(long, default_value_t = 60)]
    pub max_job_time: i64,
    /// The port on which to listen for connections
    #[arg(long, default_value_t = 8080)]
    pub port: u16,
    /// A prefix to prepend to all URLs
    #[arg(long)]
    pub url_prefix: Option<String>,
    /// Number of books to show in a single page
    #[arg(long, default_value_t = 50)]
    pub num_per_page: i64,
    /// Advertise OPDS feeds via BonJour
    #[arg(long, default_value_t = true)]
    pub use_bonjour: bool,
    /// Maximum number of books in OPDS feeds
    #[arg(long, default_value_t = 30)]
    pub max_opds_items: i64,
    /// Maximum number of ungrouped items in OPDS feeds
    #[arg(long, default_value_t = 100)]
    pub max_opds_ungrouped_items: i64,
    /// The interface on which to listen for connections
    #[arg(long)]
    pub listen_on: Option<String>,
    /// Fallback to auto-detected interface
    #[arg(long, default_value_t = true)]
    pub fallback_to_detected_interface: bool,
    /// Zero copy file transfers for increased performance
    #[arg(long, default_value_t = true)]
    pub use_sendfile: bool,
    /// Max. log file size (in MB, 0 = no rotation)
    #[arg(long, default_value_t = 20)]
    pub max_log_size: i64,
    /// Log HTTP 404 (Not Found) requests
    #[arg(long, default_value_t = true)]
    pub log_not_found: bool,
    /// Password based authentication to access the server
    #[arg(long, default_value_t = false)]
    pub auth: bool,
    /// Allow un-authenticated local connections to make changes
    #[arg(long, default_value_t = false)]
    pub local_write: bool,
    /// Allow un-authenticated connections from specific IP addresses to make changes
    #[arg(long)]
    pub trusted_ips: Option<String>,
    /// Path to user database
    #[arg(long)]
    pub userdb: Option<String>,
    /// HTTP authentication mode: "auto", "basic", or "digest"
    #[arg(long, default_value = "auto")]
    pub auth_mode: String,
    /// Temporarily ban an IP after repeated login failures, for this many minutes (0 = no banning)
    #[arg(long, default_value_t = 0)]
    pub ban_for: i64,
    /// Number of login failures after which an IP address is banned
    #[arg(long, default_value_t = 5)]
    pub ban_after: i64,
    /// Comma separated list of user-defined metadata fields to hide from OPDS/mobile views
    #[arg(long)]
    pub ignored_fields: Option<String>,
    /// Comma separated list of user-defined metadata fields to exclusively show in OPDS/mobile views
    #[arg(long)]
    pub displayed_fields: Option<String>,
    /// Default book list mode for new users: "cover_grid", "details_list", or "custom_list"
    #[arg(long, default_value = "cover_grid")]
    pub book_list_mode: String,
    /// Path to the built browser UI's static files (issue #432/#498)
    /// -- not a port of any upstream option (upstream bundles its own
    /// compiled JS into the binary's own resources at build time; this
    /// port instead serves a separately-built `web/dist` at runtime,
    /// see `router`'s own doc for how). `None` (the default) serves
    /// no UI at all -- every existing JSON/OPDS/download route still
    /// works unaffected, matching this crate's behavior before #498.
    #[arg(long)]
    pub static_dir: Option<String>,
}

impl Default for ServerOptions {
    fn default() -> ServerOptions {
        // clap::Parser::parse_from with no arguments applies every
        // #[arg(default_value...)] above -- the single source of truth
        // for defaults, rather than a second hand-maintained copy.
        ServerOptions::parse_from(["calibre_srv"])
    }
}

/// Port of `parse_config_file`.
pub fn parse_config_file(path: &Path) -> Result<ServerOptions> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("reading {path:?}"))?;
    let opts: ServerOptions = serde_json::from_str(&raw).with_context(|| format!("parsing {path:?}"))?;
    Ok(opts)
}

/// Port of `write_config_file`.
pub fn write_config_file(opts: &ServerOptions, path: &Path) -> Result<()> {
    let raw = serde_json::to_string_pretty(opts)?;
    std::fs::write(path, raw).with_context(|| format!("writing {path:?}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_upstream() {
        let opts = ServerOptions::default();
        assert_eq!(opts.port, 8080);
        assert_eq!(opts.timeout, 120.0);
        assert_eq!(opts.worker_count, 10);
        assert_eq!(opts.max_opds_items, 30);
        assert_eq!(opts.max_opds_ungrouped_items, 100);
        assert_eq!(opts.num_per_page, 50);
        assert!(opts.use_bonjour);
        assert!(!opts.auth);
        assert_eq!(opts.auth_mode, "auto");
        assert_eq!(opts.book_list_mode, "cover_grid");
    }

    #[test]
    fn config_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server-config.json");
        let mut opts = ServerOptions::default();
        opts.port = 9999;
        opts.auth = true;
        write_config_file(&opts, &path).unwrap();
        let loaded = parse_config_file(&path).unwrap();
        assert_eq!(loaded.port, 9999);
        assert!(loaded.auth);
    }
}
