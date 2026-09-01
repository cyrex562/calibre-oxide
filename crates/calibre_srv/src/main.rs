//! A minimal standalone entry point for this increment's content
//! server. Port of neither `standalone.py` nor `embedded.py` exactly --
//! upstream's process-management machinery there (multi-worker process
//! pools, GUI-embedded server lifecycle) is out of scope for now; this
//! is just enough to run [`calibre_srv::router`] over TCP.
//!
//! `--add-user`/`--remove-user`/`--list-users`/`--set-readonly`/
//! `--change-password` are convenience flags for this binary, a
//! narrow subset of `manage_users_cli.py`'s real subcommands
//! (`add`/`remove`/`list`/`readonly`/`chpass`) -- backed by the same
//! real `UserManager` methods those subcommands use. Not ported:
//! `change_set_password` (`is_allowed_to_change_password_via_http`/
//! `set_allowed_to_change_password_via_http` need the `misc_data`
//! column `users.rs` doesn't expose yet) and `libraries` (per-library
//! restriction, no multi-library support anywhere in this crate).

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
    /// Modifier for --add-user: give the new user only read access
    #[arg(long)]
    readonly: bool,
    /// Remove a user from the user database and exit
    #[arg(long)]
    remove_user: Option<String>,
    /// List all usernames in the user database and exit
    #[arg(long)]
    list_users: bool,
    /// Set/reset/toggle/show whether a user is readonly (NAME:set|reset|toggle|show) and exit
    #[arg(long)]
    set_readonly: Option<String>,
    /// Change a user's password (NAME:PASSWORD) and exit
    #[arg(long)]
    change_password: Option<String>,
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

    if cli.list_users {
        let users = UserManager::new(&userdb_path(&cli))?;
        for name in users.all_user_names() {
            println!("{name}");
        }
        return Ok(());
    }

    if let Some(name) = &cli.remove_user {
        let users = UserManager::new(&userdb_path(&cli))?;
        if !users.remove_user(name) {
            anyhow::bail!("No such user: {name:?}");
        }
        println!("Removed user {name:?}");
        return Ok(());
    }

    if let Some(spec) = &cli.set_readonly {
        let (name, op) = spec.split_once(':').ok_or_else(|| anyhow::anyhow!("--set-readonly expects NAME:set|reset|toggle|show"))?;
        let users = UserManager::new(&userdb_path(&cli))?;
        if !users.has_user(name) {
            anyhow::bail!("No such user: {name:?}");
        }
        let val = match op {
            "toggle" => !users.is_readonly(name),
            "set" => true,
            "reset" => false,
            "show" => {
                println!("{}", if users.is_readonly(name) { "set" } else { "reset" });
                return Ok(());
            }
            other => anyhow::bail!("{other} is an unknown operation (expected set, reset, toggle, or show)"),
        };
        users.set_readonly(name, val);
        println!("Readonly for {name:?} is now {}", if val { "set" } else { "reset" });
        return Ok(());
    }

    if let Some(spec) = &cli.change_password {
        let (name, pw) = spec.split_once(':').ok_or_else(|| anyhow::anyhow!("--change-password expects NAME:PASSWORD"))?;
        let users = UserManager::new(&userdb_path(&cli))?;
        users.change_password(name, pw).map_err(|e| anyhow::anyhow!(e))?;
        println!("Changed password for {name:?}");
        return Ok(());
    }

    if let Some(spec) = &cli.add_user {
        let (name, pw) = spec.split_once(':').ok_or_else(|| anyhow::anyhow!("--add-user expects NAME:PASSWORD"))?;
        let users = UserManager::new(&userdb_path(&cli))?;
        users.add_user(name, pw, cli.readonly).map_err(|e| anyhow::anyhow!(e))?;
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
    let reader_profiles = calibre_srv::reader_profiles::ProfileStore::new(&cli.library_path.join("reader-profiles.sqlite"))?;
    let state = AppState { cache: Arc::new(cache), opts: Arc::new(cli.opts), auth, changes: calibre_srv::web_socket::new_change_broadcaster(), reader_profiles: Arc::new(reader_profiles) };

    let use_bonjour = state.opts.use_bonjour;
    let library_name = cli.library_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "calibre-oxide Library".to_string());
    let app = router(state);
    let addr: SocketAddr = format!("{listen_on}:{port}").parse()?;
    println!("calibre-oxide content server listening on http://{addr}");

    // Kept alive for the life of the process (see calibre_srv::bonjour's
    // own doc) -- unregisters on Drop, i.e. when main() returns.
    let _bonjour = if use_bonjour {
        match calibre_srv::bonjour::Bonjour::start(&library_name, port, "/opds") {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("Warning: could not advertise via mDNS/Bonjour: {e}");
                None
            }
        }
    } else {
        None
    };

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}
