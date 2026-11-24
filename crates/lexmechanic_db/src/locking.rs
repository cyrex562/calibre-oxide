use std::collections::HashMap;
use std::sync::{Condvar, Mutex};
use std::thread::{self, ThreadId};

/// A reentrant Read-Write lock, similar to Calibre's SHLock.
/// Supports multiple readers (reentrant) and a single writer (reentrant).
pub struct SHLock {
    state: Mutex<LockState>,
    cond: Condvar,
}

struct LockState {
    shared_owners: HashMap<ThreadId, usize>,
    exclusive_owner: Option<ThreadId>,
    exclusive_count: usize,
}

impl LockState {
    fn new() -> Self {
        Self {
            shared_owners: HashMap::new(),
            exclusive_owner: None,
            exclusive_count: 0,
        }
    }
}

impl SHLock {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(LockState::new()),
            cond: Condvar::new(),
        }
    }

    pub fn acquire_shared(&self) {
        let tid = thread::current().id();
        let mut state = self.state.lock().unwrap();

        loop {
            // If we already own the exclusive lock, we can't downgrade to shared (or simply acquire shared as well).
            // Calibre python says: "WARNING: Be very careful to not try to acquire a read lock while the same thread holds a write lock"
            // But also: "The read_lock ... can also be acquired multiple times by the same thread."
            // If we own exclusive, and ask for shared, it's effectively allowed in many implementations, but Python implementation says:
            // "If the lock is already spoken for by an exclusive, add us to the shared queue... unless exclusive owner is me -> raise DowngradeLockError"
            if let Some(owner) = state.exclusive_owner {
                if owner == tid {
                    panic!("DowngradeLockError: Cannot acquire shared lock while holding exclusive lock");
                }
                // Must wait
                state = self.cond.wait(state).unwrap();
            } else {
                // No exclusive owner.
                // We can acquire shared.
                *state.shared_owners.entry(tid).or_insert(0) += 1;
                break;
            }
        }
    }

    pub fn release_shared(&self) {
        let tid = thread::current().id();
        let mut state = self.state.lock().unwrap();

        if let Some(count) = state.shared_owners.get_mut(&tid) {
            *count -= 1;
            if *count == 0 {
                state.shared_owners.remove(&tid);
                // If no more readers, wake up potential writers
                if state.shared_owners.is_empty() {
                    self.cond.notify_all();
                }
            }
        } else {
            panic!("release() called on unheld shared lock");
        }
    }

    pub fn acquire_exclusive(&self) {
        let tid = thread::current().id();
        let mut state = self.state.lock().unwrap();

        loop {
            // Case 1: We already own it (Reentrant)
            if let Some(owner) = state.exclusive_owner {
                if owner == tid {
                    state.exclusive_count += 1;
                    break;
                }
                // Someone else owns it, wait.
                state = self.cond.wait(state).unwrap();
            } else {
                // Case 2: No exclusive owner.
                // Check if there are readers.
                if state.shared_owners.is_empty() {
                    // Start exclusive
                    state.exclusive_owner = Some(tid);
                    state.exclusive_count = 1;
                    break;
                } else {
                    // There are readers. Check if we are one of them?
                    // "Do not allow upgrade of lock"
                    if state.shared_owners.contains_key(&tid) {
                        panic!("LockingError: Can't upgrade SHLock object");
                    }
                    // Wait for readers to finish
                    state = self.cond.wait(state).unwrap();
                }
            }
        }
    }

    pub fn release_exclusive(&self) {
        let tid = thread::current().id();
        let mut state = self.state.lock().unwrap();

        if state.exclusive_owner == Some(tid) {
            state.exclusive_count -= 1;
            if state.exclusive_count == 0 {
                state.exclusive_owner = None;
                // Wake up waiters (readers or other writers)
                self.cond.notify_all();
            }
        } else {
            panic!("release() called on unheld exclusive lock");
        }
    }
}

/// Helper struct for RAII shared lock
pub struct SafeReadLock<'a> {
    lock: &'a SHLock,
}

impl<'a> SafeReadLock<'a> {
    pub fn new(lock: &'a SHLock) -> Self {
        lock.acquire_shared();
        Self { lock }
    }
}

impl<'a> Drop for SafeReadLock<'a> {
    fn drop(&mut self) {
        self.lock.release_shared();
    }
}

/// Helper struct for RAII exclusive lock
pub struct SafeWriteLock<'a> {
    lock: &'a SHLock,
}

impl<'a> SafeWriteLock<'a> {
    pub fn new(lock: &'a SHLock) -> Self {
        lock.acquire_exclusive();
        Self { lock }
    }
}

impl<'a> Drop for SafeWriteLock<'a> {
    fn drop(&mut self) {
        self.lock.release_exclusive();
    }
}
