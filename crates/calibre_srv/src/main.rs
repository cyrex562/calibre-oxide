//! A minimal standalone entry point for this increment's content
//! server. Port of neither `standalone.py` nor `embedded.py` exactly --
//! upstream's process-management machinery there (multi-worker process
//! pools, GUI-embedded server lifecycle) is out of scope for now; this
//! is just enough to run [`calibre_srv::router`] over TCP.
//!
//! `--add-user NAME:PASSWORD` is a convenience flag for this binary,
//! not a port of `manage_users_cli.py` (a much larger CLI this
//! increment doesn't cover -- see `lib.rs`'s doc) -- just enough to
//! actually exercise `--auth` without a separate tool.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;

use calibre_db::cache::Cache;
use calibre_srv::{auth::AuthGate, opts::ServerOptions, router, users::UserManager, AppState};

#[derive(Parser)]
#[command(about = "calibre-oxide content server")]
struct Cli {
    /// Path to the calibre library to serve
    library_path: PathBuf,
    /// Add a user (NAME:PASSWORD) to the user database and exit, rather than starting the server
    #[arg(long)]
    add_user: Option<String>,
    #[command(flatten)]
    opts: ServerOptions,
}

fn userdb_path(cli: &Cli) -> PathBuf {
    match &cli.opts.userdb {
        Some(p) => PathBuf::from(p),
        None => cli.library_path.join("server-users.sqlite"),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(spec) = &cli.add_user {
        let (name, pw) = spec.split_once(':').ok_or_else(|| anyhow::anyhow!("--add-user expects NAME:PASSWORD"))?;
        let users = UserManager::new(&userdb_path(&cli))?;
        users.add_user(name, pw, false).map_err(|e| anyhow::anyhow!(e))?;
        println!("Added user {name:?}");
        return Ok(());
    }

    let cache = Cache::new(&cli.library_path)?;
    let auth = if cli.opts.auth {
        let users = UserManager::new(&userdb_path(&cli))?;
        Some(Arc::new(AuthGate::new(users, "calibre-oxide".to_string(), cli.opts.ban_for, cli.opts.ban_after)))
    } else {
        None
    };
    let port = cli.opts.port;
    let listen_on = cli.opts.listen_on.clone().unwrap_or_else(|| "0.0.0.0".to_string());
    let state = AppState { cache: Arc::new(cache), opts: Arc::new(cli.opts), auth, changes: calibre_srv::web_socket::new_change_broadcaster() };

    let app = router(state);
    let addr: SocketAddr = format!("{listen_on}:{port}").parse()?;
    println!("calibre-oxide content server listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}
