//! A minimal standalone entry point for this increment's content
//! server. Port of neither `standalone.py` nor `embedded.py` exactly --
//! upstream's process-management machinery there (multi-worker process
//! pools, GUI-embedded server lifecycle) is out of scope for now; this
//! is just enough to run [`calibre_srv::router`] over TCP.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;

use calibre_db::cache::Cache;
use calibre_srv::{opts::ServerOptions, router, AppState};

#[derive(Parser)]
#[command(about = "calibre-oxide content server")]
struct Cli {
    /// Path to the calibre library to serve
    library_path: PathBuf,
    #[command(flatten)]
    opts: ServerOptions,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cache = Cache::new(&cli.library_path)?;
    let state = AppState { cache: Arc::new(cache), opts: Arc::new(cli.opts) };
    let port = state.opts.port;
    let listen_on = state.opts.listen_on.clone().unwrap_or_else(|| "0.0.0.0".to_string());

    let app = router(state);
    let addr = format!("{listen_on}:{port}");
    println!("calibre-oxide content server listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
