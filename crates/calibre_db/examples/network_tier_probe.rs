//! Manual validation probe for issues #262/#263/#264/#265 (`docs/
//! FAULT_TOLERANCE.md` §6, split off from #257/PR #261's logic-only
//! disclosure): drives real `LibraryHandle` operations against a real
//! network mount passed on the command line, so its behavior can be
//! checked against real NFS/SMB/S3/etc. instead of the local-fs
//! stand-in the unit tests use. Not a `#[test]` -- there is no real
//! network mount in CI/this sandbox to point it at automatically; a
//! human (or a driving shell script) runs it against a real mount and
//! reads the output.
//!
//! ```text
//! network_tier_probe tier <path>
//! network_tier_probe basic <path>
//! network_tier_probe lock-hold <path> <secs>
//! network_tier_probe lock-try <path>
//! network_tier_probe write-loop <path> <count> <delay-ms>
//! network_tier_probe flock-raw-hold <path> <secs>
//! network_tier_probe flock-raw-try <path>
//! ```

use calibre_db::library_handle::LibraryHandle;
use std::env;
use std::fs::OpenOptions;
use std::path::Path;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let Some(mode) = args.get(1) else {
        eprintln!("usage: network_tier_probe <mode> <path> [args...]");
        return ExitCode::FAILURE;
    };
    let result = match mode.as_str() {
        "hash-file" => {
            println!("{}", blake3::hash(&std::fs::read(&args[2]).unwrap()).to_hex());
            Ok(())
        }
        "tier" => tier(&args[2]),
        "basic" => basic(&args[2]),
        "lock-hold" => lock_hold(&args[2], args[3].parse().unwrap()),
        "lock-try" => lock_try(&args[2]),
        "write-loop" => write_loop(&args[2], args[3].parse().unwrap(), args[4].parse().unwrap()),
        "flock-raw-hold" => flock_raw_hold(&args[2], args[3].parse().unwrap()),
        "flock-raw-try" => flock_raw_try(&args[2]),
        other => {
            eprintln!("unknown mode {other:?}");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("FAIL: {e}");
            ExitCode::FAILURE
        }
    }
}

fn tier(path: &str) -> Result<(), String> {
    let handle = LibraryHandle::open(Path::new(path)).map_err(|e| e.to_string())?;
    println!("tier={:?}", handle.tier());
    Ok(())
}

fn basic(path: &str) -> Result<(), String> {
    let lib = Path::new(path);
    let handle = LibraryHandle::open(lib).map_err(|e| e.to_string())?;
    println!("tier={:?}", handle.tier());

    // write_atomic + read-back.
    let target = lib.join("probe_write.txt");
    let payload = b"network tier probe payload\n".to_vec();
    handle
        .write_atomic(&target, &payload)
        .map_err(|e| format!("write_atomic: {e}"))?;
    let read_back = std::fs::read(&target).map_err(|e| format!("read-back: {e}"))?;
    if read_back != payload {
        return Err("write_atomic: read-back content mismatch".to_string());
    }
    println!("write_atomic: OK ({} bytes)", payload.len());

    // copy_atomic + read-back.
    let source = lib.join("probe_source.txt");
    std::fs::write(&source, b"copy source content\n").map_err(|e| e.to_string())?;
    let copy_target = lib.join("probe_copy.txt");
    let hash = handle
        .copy_atomic(&source, &copy_target)
        .map_err(|e| format!("copy_atomic: {e}"))?;
    let copied = std::fs::read(&copy_target).map_err(|e| e.to_string())?;
    if copied != b"copy source content\n" {
        return Err("copy_atomic: read-back content mismatch".to_string());
    }
    println!("copy_atomic: OK (hash={hash})");

    // rename_atomic + existence checks.
    let renamed = lib.join("probe_renamed.txt");
    handle
        .rename_atomic(&copy_target, &renamed)
        .map_err(|e| format!("rename_atomic: {e}"))?;
    if copy_target.exists() {
        return Err("rename_atomic: source still exists after rename".to_string());
    }
    if !renamed.exists() {
        return Err("rename_atomic: target missing after rename".to_string());
    }
    println!("rename_atomic: OK");

    // remove_atomic.
    handle
        .remove_atomic(&renamed)
        .map_err(|e| format!("remove_atomic: {e}"))?;
    if renamed.exists() {
        return Err("remove_atomic: target still exists after remove".to_string());
    }
    println!("remove_atomic: OK");

    let _ = std::fs::remove_file(&target);
    let _ = std::fs::remove_file(&source);
    println!("basic: ALL OK");
    Ok(())
}

/// Opens the handle (acquiring §7's writer lock) and holds it for
/// `secs`, printing a line the moment the lock is acquired so a
/// driving script can synchronize a concurrent `lock-try`/disruption
/// against it.
fn lock_hold(path: &str, secs: u64) -> Result<(), String> {
    let handle = LibraryHandle::open(Path::new(path)).map_err(|e| e.to_string())?;
    println!("lock-hold: acquired tier={:?}", handle.tier());
    thread::sleep(Duration::from_secs(secs));
    println!("lock-hold: releasing");
    Ok(())
}

fn lock_try(path: &str) -> Result<(), String> {
    match LibraryHandle::open(Path::new(path)) {
        Ok(_) => {
            println!("lock-try: acquired (no contention)");
            Ok(())
        }
        Err(calibre_db::library_handle::LibraryHandleError::AlreadyLocked) => {
            println!("lock-try: AlreadyLocked (correctly rejected)");
            Ok(())
        }
        Err(e) => Err(format!("unexpected error: {e}")),
    }
}

/// Repeatedly `write_atomic`s a unique payload every `delay_ms`, for
/// `count` iterations, printing per-iteration success/failure and
/// elapsed time -- for characterizing real §6 retry/backoff behavior
/// while a driving script disrupts the mount concurrently (container
/// restart, `tc netem`, forced unmount) instead of the unit tests'
/// injected-failure stand-in.
fn write_loop(path: &str, count: u32, delay_ms: u64) -> Result<(), String> {
    let lib = Path::new(path);
    let handle = LibraryHandle::open(lib).map_err(|e| e.to_string())?;
    println!("write-loop: tier={:?}", handle.tier());
    let target = lib.join("probe_loop.txt");
    for i in 0..count {
        let payload = format!("iteration {i}\n");
        let start = Instant::now();
        match handle.write_atomic(&target, payload.as_bytes()) {
            Ok(()) => println!("iter {i}: OK in {:?}", start.elapsed()),
            Err(e) => println!("iter {i}: FAIL in {:?}: {e}", start.elapsed()),
        }
        thread::sleep(Duration::from_millis(delay_ms));
    }
    let _ = std::fs::remove_file(&target);
    Ok(())
}

/// Characterizes the mount's *raw* `flock` semantics directly (not
/// through `LibraryHandle`), since §7's writer lock is exactly a
/// `flock`-style exclusive lock and NFS/SMB both have their own
/// documented history of surprises here depending on server/client
/// config -- worth confirming independently of whatever
/// `LibraryHandle::open`'s own `AlreadyLocked` behavior reports.
fn flock_raw_hold(path: &str, secs: u64) -> Result<(), String> {
    let lock_path = Path::new(path).join("probe.flock");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| e.to_string())?;
    file.try_lock().map_err(|e| format!("try_lock: {e:?}"))?;
    println!("flock-raw-hold: acquired");
    thread::sleep(Duration::from_secs(secs));
    println!("flock-raw-hold: releasing");
    Ok(())
}

fn flock_raw_try(path: &str) -> Result<(), String> {
    let lock_path = Path::new(path).join("probe.flock");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| e.to_string())?;
    match file.try_lock() {
        Ok(()) => {
            println!("flock-raw-try: acquired (no contention)");
            Ok(())
        }
        Err(e) => {
            println!("flock-raw-try: rejected ({e:?})");
            Ok(())
        }
    }
}
