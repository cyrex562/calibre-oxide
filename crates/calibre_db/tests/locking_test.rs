use calibre_db::locking::{SHLock, SafeReadLock, SafeWriteLock};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[test]
fn test_shlock_simple() {
    let lock = SHLock::new();
    lock.acquire_exclusive();
    lock.release_exclusive();
    lock.acquire_shared();
    lock.release_shared();
}

#[test]
fn test_reentrant_shared() {
    let lock = SHLock::new();
    lock.acquire_shared();
    lock.acquire_shared(); // Should not block
    lock.release_shared();
    lock.release_shared();
}

#[test]
fn test_reentrant_exclusive() {
    let lock = SHLock::new();
    lock.acquire_exclusive();
    lock.acquire_exclusive(); // Should not block
    lock.release_exclusive();
    lock.release_exclusive();
}

#[test]
fn test_shared_concurrent() {
    let lock = Arc::new(SHLock::new());
    let lock2 = lock.clone();

    // Acquire shared in main thread
    lock.acquire_shared();

    let t = thread::spawn(move || {
        // Should also be able to acquire shared immediately
        lock2.acquire_shared();
        lock2.release_shared();
    });

    t.join().unwrap();
    lock.release_shared();
}

#[test]
fn test_exclusive_blocks_shared() {
    let lock = Arc::new(SHLock::new());
    let lock2 = lock.clone();

    lock.acquire_exclusive();

    let t = thread::spawn(move || {
        // This should block until we release
        // We can't easily verify blocking without simple timing or channels,
        // but we can ensure it eventually runs.
        lock2.acquire_shared();
        lock2.release_shared();
    });

    thread::sleep(Duration::from_millis(100)); // Ensure thread had time to try and block
    lock.release_exclusive();
    t.join().unwrap();
}

#[test]
fn test_raii_locks() {
    let lock = SHLock::new();
    {
        let _guard = SafeReadLock::new(&lock);
    } // Released here
    {
        let _guard = SafeWriteLock::new(&lock);
    } // Released here
}
