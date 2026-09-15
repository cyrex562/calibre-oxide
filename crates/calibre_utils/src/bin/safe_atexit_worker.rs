//! The worker process spawned by `calibre_utils::safe_atexit`. Port of
//! `safe_atexit.py`'s `main()`: reads newline-delimited JSON commands
//! from stdin (`{"action": "rmtree"|"unlink"|"run_program", "payload": ...}`);
//! `rmtree`/`unlink` are queued and only acted on once stdin hits EOF
//! (that queue-until-EOF behavior, not this file, is what makes the
//! whole mechanism crash-safe -- see `calibre_utils::safe_atexit`'s
//! module doc); `run_program` launches immediately.
//!
//! Not this binary's own concern to invoke directly -- spawned only by
//! `calibre_utils::safe_atexit::ensure_worker`.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};

enum PendingAction {
    Rmtree(PathBuf),
    Unlink(PathBuf),
}

impl PendingAction {
    /// Port of the Unix `remove_dir` (best-effort, ignore every
    /// error) and `unlink` (best-effort, ignore every error). The
    /// Windows retry-with-sleep loop upstream's own `remove_dir` uses
    /// isn't ported -- see `calibre_utils::safe_atexit`'s module doc.
    fn run(self) {
        match self {
            PendingAction::Rmtree(path) => {
                let _ = std::fs::remove_dir_all(path);
            }
            PendingAction::Unlink(path) => {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Port of `run_program`: launch `cmdline` detached, with a reaper
/// thread so the worker doesn't accumulate zombie children while it
/// stays alive (matching upstream's own `Thread(target=process.wait,
/// daemon=True)`).
fn run_program(cmdline: &[String]) {
    let Some((program, args)) = cmdline.split_first() else {
        return;
    };
    if let Ok(mut child) = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

fn main() {
    // Port of `signal.signal(signal.SIGINT, signal.SIG_IGN)`: a
    // foreground Ctrl-C meant for the caller shouldn't also kill this
    // worker before it's had a chance to run its queued cleanup.
    // Windows has no SIGINT to ignore this way -- not ported there,
    // same reasoning as this module's other Windows gaps.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }

    let mut pending: Vec<PendingAction> = Vec::new();
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    while let Some(Ok(line)) = lines.next() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(cmd) = serde_json::from_str::<serde_json::Value>(&line) else {
            eprintln!("safe_atexit_worker: malformed command: {line}");
            continue;
        };
        match cmd["action"].as_str() {
            Some("rmtree") => {
                if let Some(p) = cmd["payload"].as_str() {
                    pending.push(PendingAction::Rmtree(PathBuf::from(p)));
                }
            }
            Some("unlink") => {
                if let Some(p) = cmd["payload"].as_str() {
                    pending.push(PendingAction::Unlink(PathBuf::from(p)));
                }
            }
            Some("run_program") => {
                if let Some(args) = cmd["payload"].as_array() {
                    let cmdline: Vec<String> = args.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
                    run_program(&cmdline);
                }
            }
            _ => eprintln!("safe_atexit_worker: unknown command: {line}"),
        }
    }

    // EOF: our stdin's write end is gone, whether the caller closed it
    // deliberately (`shutdown_worker`) or the OS reclaimed it because
    // the caller exited some other way, including a crash or a kill.
    // Port of `atexit`'s LIFO-order callback execution.
    for action in pending.into_iter().rev() {
        action.run();
    }
}
