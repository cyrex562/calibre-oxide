//! Storage-tier classification for Windows paths, journal-mode policy
//! per tier, and library-in-use detection (issue #258's classification
//! half; the device-removal and sleep/resume monitors of #258 are out of
//! scope here).
//!
//! The pure decision logic (`classify_windows_path`, `subst_target`,
//! `JournalMode::for_tier`) compiles and is unit-tested on every
//! platform; only the thin FFI lookups (`GetDriveTypeW`,
//! `QueryDosDeviceW`, `OpenMutexW`) are `cfg(windows)`.
//!
//! # How real calibre signals "library in use"
//!
//! It does not mark the library at all. Reading `old_src`:
//! `calibre/utils/lock.py::singleinstance('db')` takes a per-user,
//! per-machine named mutex (`calibre-singleinstance-<user>-db` on
//! Windows, an abstract unix socket on Linux, a lock file on macOS).
//! The GUI (`gui2/main.py`), `calibre-server` (`srv/standalone.py`) and
//! `calibredb` (`db/cli/main.py`) all take it, so they exclude each
//! other, but only *on one machine and for one user*, and it is not tied
//! to any particular library. Nothing is written into the library
//! directory, and nothing coordinates across machines. SQLite's own
//! locks (calibre uses the default rollback journal) are only held for
//! the duration of a transaction. So the detectable signals are:
//!
//! 1. calibre's named mutex (Windows only here; advisory, because it
//!    says "calibre is running", not "calibre has *this* library open");
//! 2. an exclusive-share-mode open of `metadata.db` failing (Windows;
//!    calibre keeps its SQLite connection open for its whole session, so
//!    this fires for the whole time it has the library open, and SMB
//!    servers enforce share modes across clients);
//! 3. `BEGIN EXCLUSIVE` returning busy (any OS; only fires mid-write);
//! 4. another Lexmechanic instance's writer lock (see `library_handle`).

use crate::library_handle::StorageTier;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// ---------------------------------------------------------------------
// Journal mode
// ---------------------------------------------------------------------

/// SQLite journal mode a tier must use. WAL needs shared memory and is
/// documented as unsupported on network filesystems, so `Network` is
/// always `Delete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalMode {
    Wal,
    Delete,
}

impl JournalMode {
    pub fn for_tier(tier: StorageTier) -> Self {
        match tier {
            StorageTier::Network => JournalMode::Delete,
            StorageTier::LocalInternal | StorageTier::LocalExternal => JournalMode::Wal,
        }
    }

    pub fn as_pragma_value(self) -> &'static str {
        match self {
            JournalMode::Wal => "WAL",
            JournalMode::Delete => "DELETE",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "wal" => Some(JournalMode::Wal),
            "delete" => Some(JournalMode::Delete),
            _ => None,
        }
    }
}

pub fn tier_name(tier: StorageTier) -> &'static str {
    match tier {
        StorageTier::LocalInternal => "local_internal",
        StorageTier::LocalExternal => "local_external",
        StorageTier::Network => "network",
    }
}

/// `PRAGMA <schema>.journal_mode=...; synchronous=FULL` for an attached
/// sidecar database, following whatever mode `main` ended up in so a
/// network library never gets a WAL sidecar.
pub(crate) fn sidecar_journal_sql(conn: &rusqlite::Connection, schema: &str) -> String {
    let main: String = conn
        .query_row("PRAGMA main.journal_mode", [], |r| r.get(0))
        .unwrap_or_else(|_| "wal".to_string());
    let mode = JournalMode::parse(&main).unwrap_or(JournalMode::Wal);
    format!(
        "PRAGMA {schema}.journal_mode={}; PRAGMA {schema}.synchronous=FULL;",
        mode.as_pragma_value()
    )
}

// ---------------------------------------------------------------------
// Windows path classification (pure)
// ---------------------------------------------------------------------

/// `GetDriveTypeW`'s result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DriveKind {
    Unknown,
    NoRoot,
    Removable,
    Fixed,
    Remote,
    CdRom,
    RamDisk,
}

impl DriveKind {
    #[allow(dead_code)] // only called by the Windows FFI wrapper
    pub(crate) fn from_raw(raw: u32) -> Self {
        match raw {
            1 => DriveKind::NoRoot,
            2 => DriveKind::Removable,
            3 => DriveKind::Fixed,
            4 => DriveKind::Remote,
            5 => DriveKind::CdRom,
            6 => DriveKind::RamDisk,
            _ => DriveKind::Unknown,
        }
    }

    pub(crate) fn tier(self) -> StorageTier {
        match self {
            DriveKind::Remote => StorageTier::Network,
            // Optical media is slow, removable and read-only: treat it
            // like any other removable device.
            DriveKind::Removable | DriveKind::CdRom => StorageTier::LocalExternal,
            DriveKind::Fixed | DriveKind::RamDisk | DriveKind::Unknown | DriveKind::NoRoot => {
                StorageTier::LocalInternal
            }
        }
    }
}

enum WinPath {
    Unc,
    Drive(char),
    Other,
}

/// Strips `\\?\` / `\\.\` prefixes and recognises UNC and drive-letter
/// paths. `\\?\UNC\server\share` is UNC; `\\?\C:\x` is drive `C`.
fn parse_windows_path(path: &str) -> WinPath {
    let p = path.replace('/', "\\");
    let rest = if let Some(r) = p
        .strip_prefix("\\\\?\\")
        .or_else(|| p.strip_prefix("\\\\.\\"))
    {
        if r.len() >= 4 && r[..4].eq_ignore_ascii_case("UNC\\") {
            return WinPath::Unc;
        }
        r.to_string()
    } else if p.starts_with("\\\\") {
        return WinPath::Unc;
    } else {
        p
    };
    let b = rest.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        WinPath::Drive((b[0] as char).to_ascii_uppercase())
    } else {
        WinPath::Other
    }
}

/// Maps a `QueryDosDeviceW` result for a drive letter to the path it
/// points at, when the drive is a `subst`/network redirection. `None`
/// for a real volume (`\Device\HarddiskVolumeN`).
pub(crate) fn subst_target(dos_device: &str) -> Option<String> {
    if let Some(r) = dos_device.strip_prefix("\\??\\") {
        if r.len() >= 4 && r[..4].eq_ignore_ascii_case("UNC\\") {
            return Some(format!("\\\\{}", &r[4..]));
        }
        return Some(r.to_string());
    }
    let lower = dos_device.to_ascii_lowercase();
    const REDIRECTORS: &[&str] = &[
        "\\device\\lanmanredirector",
        "\\device\\mup",
        "\\device\\webdavredirector",
        "\\device\\mrxdav",
    ];
    if REDIRECTORS.iter().any(|r| lower.starts_with(r)) {
        return Some("\\\\redirected-network-device".to_string());
    }
    None
}

/// Pure classification of a Windows path. `drive_kind` is
/// `GetDriveTypeW`, `dos_device` is `QueryDosDeviceW` (to see through
/// `subst`); both are injected so this runs without a real share.
pub(crate) fn classify_windows_path(
    path: &str,
    drive_kind: &dyn Fn(char) -> DriveKind,
    dos_device: &dyn Fn(char) -> Option<String>,
) -> StorageTier {
    let mut current = path.to_string();
    // A subst chain is at most a few links; the bound only guards
    // against a pathological self-reference.
    for _ in 0..8 {
        match parse_windows_path(&current) {
            WinPath::Unc => return StorageTier::Network,
            WinPath::Other => return StorageTier::LocalInternal,
            WinPath::Drive(letter) => {
                let kind = drive_kind(letter);
                if kind == DriveKind::Remote {
                    return StorageTier::Network;
                }
                match dos_device(letter).and_then(|d| subst_target(&d)) {
                    Some(next) => current = next,
                    None => return kind.tier(),
                }
            }
        }
    }
    StorageTier::LocalInternal
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn os_drive_kind(letter: char) -> DriveKind {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    let root = wide(&format!("{letter}:\\"));
    // SAFETY: `root` is a NUL-terminated UTF-16 string that outlives the call.
    DriveKind::from_raw(unsafe { GetDriveTypeW(root.as_ptr()) })
}

#[cfg(windows)]
fn os_dos_device(letter: char) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::QueryDosDeviceW;
    let name = wide(&format!("{letter}:"));
    let mut buf = [0u16; 1024];
    // SAFETY: `name` is NUL-terminated; `buf` is writable for its length.
    let n = unsafe { QueryDosDeviceW(name.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if n == 0 {
        return None;
    }
    // A REG_MULTI_SZ; the first string is the current mapping.
    let end = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buf[..end]))
}

/// Real Windows classification (issue #258): canonicalize (resolves
/// symlinks and junctions, yields `\\?\...`), then the pure logic over
/// the live OS lookups. Falls back to the absolute raw path when the
/// path does not exist yet.
#[cfg(windows)]
pub(crate) fn classify_windows(path: &Path) -> StorageTier {
    let resolved = path
        .canonicalize()
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf());
    classify_windows_path(&resolved.to_string_lossy(), &os_drive_kind, &os_dos_device)
}

// ---------------------------------------------------------------------
// Environment override
// ---------------------------------------------------------------------

/// `LEXMECHANIC_STORAGE_TIER=network|external|local` forces the tier
/// the library is treated as. A user escape hatch for a share the OS
/// reports as local (a vendor NAS driver, a FUSE bridge) and a way to
/// exercise network behaviour without a real share.
pub(crate) fn tier_override() -> Option<StorageTier> {
    let v = lexmechanic_utils::env::var("LEXMECHANIC_STORAGE_TIER").ok()?;
    parse_tier_override(&v)
}

pub(crate) fn parse_tier_override(v: &str) -> Option<StorageTier> {
    match v.trim().to_ascii_lowercase().as_str() {
        "network" | "nas" | "smb" => Some(StorageTier::Network),
        "external" | "removable" | "local_external" => Some(StorageTier::LocalExternal),
        "local" | "internal" | "local_internal" => Some(StorageTier::LocalInternal),
        _ => None,
    }
}

// ---------------------------------------------------------------------
// Host identity
// ---------------------------------------------------------------------

pub fn local_hostname() -> String {
    for var in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(v) = std::env::var(var) {
            if !v.trim().is_empty() {
                return v.trim().to_string();
            }
        }
    }
    for f in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(v) = std::fs::read_to_string(f) {
            if !v.trim().is_empty() {
                return v.trim().to_string();
            }
        }
    }
    "unknown-host".to_string()
}

// ---------------------------------------------------------------------
// Library-in-use detection
// ---------------------------------------------------------------------

/// One reason to believe another program has the library open.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InUseSignal {
    /// Stable machine-readable name of the signal that fired:
    /// `exclusive_open_denied`, `sqlite_busy`, `calibre_running`.
    pub signal: &'static str,
    /// Hard signals refuse writes; soft ones only warn.
    pub blocking: bool,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct InUseReport {
    pub signals: Vec<InUseSignal>,
}

impl InUseReport {
    /// The user-facing reason writes are refused, if any hard signal fired.
    pub fn blocking_message(&self) -> Option<String> {
        let hard: Vec<&str> = self
            .signals
            .iter()
            .filter(|s| s.blocking)
            .map(|s| s.message.as_str())
            .collect();
        if hard.is_empty() {
            None
        } else {
            Some(hard.join(" "))
        }
    }
}

/// Paths this process already has a `Backend` open on. A second `Backend`
/// in the same process must not mistake the first one's open handle for
/// another program.
static OPEN_IN_PROCESS: Mutex<Option<HashMap<PathBuf, usize>>> = Mutex::new(None);

fn registry_key(library_path: &Path) -> PathBuf {
    library_path
        .canonicalize()
        .unwrap_or_else(|_| library_path.to_path_buf())
}

/// Whether this process has a `Backend` open on `library_path` right now
/// (the server's libraries, a library window). The mirror uses it to refuse a
/// target that is a live library: copying over files a running library holds
/// open would corrupt it.
pub fn is_open_in_process(library_path: &Path) -> bool {
    let key = registry_key(library_path);
    OPEN_IN_PROCESS.lock().ok().and_then(|g| g.as_ref().map(|m| m.contains_key(&key))).unwrap_or(false)
}

/// While alive, marks `library_path` as open in this process.
#[derive(Debug)]
pub(crate) struct ProcessOpenGuard {
    key: PathBuf,
}

impl ProcessOpenGuard {
    /// Registers the path; returns the guard and whether another
    /// `Backend` in this process already had it open.
    pub(crate) fn register(library_path: &Path) -> (Self, bool) {
        let key = registry_key(library_path);
        let mut g = OPEN_IN_PROCESS.lock().unwrap();
        let map = g.get_or_insert_with(HashMap::new);
        let n = map.entry(key.clone()).or_insert(0);
        let already = *n > 0;
        *n += 1;
        (ProcessOpenGuard { key }, already)
    }
}

impl Drop for ProcessOpenGuard {
    fn drop(&mut self) {
        if let Ok(mut g) = OPEN_IN_PROCESS.lock() {
            if let Some(map) = g.as_mut() {
                if let Some(n) = map.get_mut(&self.key) {
                    *n -= 1;
                    if *n == 0 {
                        map.remove(&self.key);
                    }
                }
            }
        }
    }
}

/// `LEXMECHANIC_ALLOW_SHARED_LIBRARY=1` skips the in-use probes, for a
/// user who knows another program is harmless (e.g. a backup tool that
/// holds the file open).
fn probes_disabled() -> bool {
    matches!(
        lexmechanic_utils::env::var("LEXMECHANIC_ALLOW_SHARED_LIBRARY").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// Windows: tries to open `metadata.db` with share mode 0. Any other
/// open handle (calibre's SQLite connection, another Lexmechanic, an SMB
/// client) makes this fail with a sharing violation. Retried briefly so a
/// transient antivirus/indexer scan is not reported as a second program.
#[cfg(windows)]
fn probe_exclusive_open(db_path: &Path) -> Option<InUseSignal> {
    use std::os::windows::fs::OpenOptionsExt;
    if !db_path.exists() {
        return None;
    }
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    for attempt in 0..4 {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(db_path)
        {
            Ok(_) => return None,
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(ERROR_SHARING_VIOLATION) | Some(ERROR_LOCK_VIOLATION)
                ) =>
            {
                if attempt < 3 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
            // Access denied, read-only share, etc.: cannot tell, so do
            // not claim the library is in use.
            Err(_) => return None,
        }
    }
    Some(InUseSignal {
        signal: "exclusive_open_denied",
        blocking: true,
        message: "Another program (such as calibre, or Lexmechanic on another computer) has this library's metadata.db open, so writing now could corrupt it. Close the other program and reopen this library.".to_string(),
    })
}

#[cfg(not(windows))]
fn probe_exclusive_open(_db_path: &Path) -> Option<InUseSignal> {
    None
}

/// Windows: is calibre's own single-instance mutex held on this machine?
/// Advisory only (see module doc): it is not tied to a library.
#[cfg(windows)]
fn probe_calibre_running() -> Option<InUseSignal> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::OpenMutexW;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    let user = std::env::var("USERNAME").ok()?;
    let name = wide(&format!("calibre-singleinstance-{user}-db"));
    // SAFETY: `name` is NUL-terminated and outlives the call; the handle
    // is closed immediately.
    unsafe {
        let h = OpenMutexW(SYNCHRONIZE, 0, name.as_ptr());
        if h.is_null() {
            return None;
        }
        CloseHandle(h);
    }
    Some(InUseSignal {
        signal: "calibre_running",
        blocking: false,
        message: "calibre (or calibredb / calibre-server) is running on this computer. If it has this library open, do not edit it from here at the same time.".to_string(),
    })
}

#[cfg(not(windows))]
fn probe_calibre_running() -> Option<InUseSignal> {
    None
}

/// Pre-open probes (run before this process opens its own connection).
pub(crate) fn probe_before_open(
    db_path: &Path,
    other_backend_in_process: bool,
    report: &mut InUseReport,
) {
    if probes_disabled() {
        return;
    }
    if !other_backend_in_process {
        if let Some(s) = probe_exclusive_open(db_path) {
            report.signals.push(s);
        }
    }
    if let Some(s) = probe_calibre_running() {
        report.signals.push(s);
    }
}

/// `BEGIN EXCLUSIVE` with a short timeout: busy means another connection
/// is mid-transaction right now.
pub(crate) fn probe_sqlite_busy(
    conn: &rusqlite::Connection,
    other_backend_in_process: bool,
    report: &mut InUseReport,
) {
    if probes_disabled() || other_backend_in_process {
        return;
    }
    let _ = conn.busy_timeout(std::time::Duration::from_millis(300));
    let busy = match conn.execute_batch("BEGIN EXCLUSIVE; ROLLBACK;") {
        Ok(()) => false,
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::DatabaseBusy
                || e.code == rusqlite::ErrorCode::DatabaseLocked =>
        {
            let _ = conn.execute_batch("ROLLBACK;");
            true
        }
        Err(_) => false,
    };
    let _ = conn.busy_timeout(std::time::Duration::from_millis(5000));
    if busy {
        report.signals.push(InUseSignal {
            signal: "sqlite_busy",
            blocking: true,
            message: "Another program is writing to this library's database right now.".to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind_for(map: &'static [(char, DriveKind)]) -> impl Fn(char) -> DriveKind {
        move |c| {
            map.iter()
                .find(|(l, _)| *l == c)
                .map(|(_, k)| *k)
                .unwrap_or(DriveKind::Unknown)
        }
    }

    fn no_subst(_: char) -> Option<String> {
        None
    }

    #[test]
    fn unc_paths_are_network_in_every_spelling() {
        let k = kind_for(&[]);
        for p in [
            r"\\nas\books\lib",
            r"\\?\UNC\nas\books\lib",
            r"\\?\unc\nas\books",
            "//nas/books/lib",
            r"\\.\UNC\nas\books",
        ] {
            assert_eq!(classify_windows_path(p, &k, &no_subst), StorageTier::Network, "{p}");
        }
    }

    #[test]
    fn drive_types_map_to_tiers() {
        let k = kind_for(&[
            ('C', DriveKind::Fixed),
            ('E', DriveKind::Removable),
            ('F', DriveKind::CdRom),
            ('R', DriveKind::RamDisk),
            ('Z', DriveKind::Remote),
        ]);
        let t = |p| classify_windows_path(p, &k, &no_subst);
        assert_eq!(t(r"C:\Books"), StorageTier::LocalInternal);
        assert_eq!(t(r"\\?\C:\Books"), StorageTier::LocalInternal);
        assert_eq!(t(r"c:\books"), StorageTier::LocalInternal);
        assert_eq!(t(r"E:\Books"), StorageTier::LocalExternal);
        assert_eq!(t(r"F:\"), StorageTier::LocalExternal);
        assert_eq!(t(r"R:\"), StorageTier::LocalInternal);
        assert_eq!(t(r"Z:\Books"), StorageTier::Network);
        assert_eq!(t(r"Q:\unknown"), StorageTier::LocalInternal);
    }

    #[test]
    fn subst_drive_is_resolved_to_its_target() {
        let k = kind_for(&[
            ('C', DriveKind::Fixed),
            ('S', DriveKind::Fixed),
            ('T', DriveKind::Fixed),
            ('U', DriveKind::Fixed),
        ]);
        let dev = |c: char| match c {
            'S' => Some(r"\??\UNC\nas\books".to_string()),
            'T' => Some(r"\??\C:\Books".to_string()),
            'U' => Some(r"\??\U:\loop".to_string()),
            _ => None,
        };
        assert_eq!(classify_windows_path(r"S:\lib", &k, &dev), StorageTier::Network);
        assert_eq!(classify_windows_path(r"T:\lib", &k, &dev), StorageTier::LocalInternal);
        // A self-referencing subst terminates.
        assert_eq!(classify_windows_path(r"U:\x", &k, &dev), StorageTier::LocalInternal);
    }

    #[test]
    fn subst_target_parses_dos_device_names() {
        assert_eq!(subst_target(r"\??\C:\Books").as_deref(), Some(r"C:\Books"));
        assert_eq!(subst_target(r"\??\UNC\nas\books").as_deref(), Some(r"\\nas\books"));
        assert!(subst_target(r"\Device\LanmanRedirector\;Z:0\nas\books").is_some());
        assert_eq!(subst_target(r"\Device\HarddiskVolume3"), None);
    }

    #[test]
    fn journal_mode_follows_tier() {
        assert_eq!(JournalMode::for_tier(StorageTier::Network), JournalMode::Delete);
        assert_eq!(JournalMode::for_tier(StorageTier::LocalInternal), JournalMode::Wal);
        assert_eq!(JournalMode::for_tier(StorageTier::LocalExternal), JournalMode::Wal);
    }

    #[test]
    fn tier_override_parsing() {
        assert_eq!(parse_tier_override("Network"), Some(StorageTier::Network));
        assert_eq!(parse_tier_override("local"), Some(StorageTier::LocalInternal));
        assert_eq!(parse_tier_override("removable"), Some(StorageTier::LocalExternal));
        assert_eq!(parse_tier_override("bogus"), None);
    }

    #[test]
    fn blocking_message_only_for_hard_signals() {
        let mut r = InUseReport::default();
        r.signals.push(InUseSignal { signal: "calibre_running", blocking: false, message: "soft".into() });
        assert!(r.blocking_message().is_none());
        r.signals.push(InUseSignal { signal: "sqlite_busy", blocking: true, message: "hard".into() });
        assert_eq!(r.blocking_message().as_deref(), Some("hard"));
    }

    #[test]
    fn process_registry_distinguishes_first_and_second_open() {
        let dir = tempfile::tempdir().unwrap();
        let (g1, already1) = ProcessOpenGuard::register(dir.path());
        assert!(!already1);
        let (g2, already2) = ProcessOpenGuard::register(dir.path());
        assert!(already2);
        drop(g1);
        drop(g2);
        let (_g3, already3) = ProcessOpenGuard::register(dir.path());
        assert!(!already3);
    }

    #[cfg(windows)]
    #[test]
    fn real_windows_lookups_classify_the_temp_dir_as_local() {
        let dir = tempfile::tempdir().unwrap();
        assert_ne!(classify_windows(dir.path()), StorageTier::Network);
    }

    #[cfg(windows)]
    #[test]
    fn exclusive_open_probe_fires_when_another_handle_is_open() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("metadata.db");
        std::fs::write(&db, b"x").unwrap();
        assert!(probe_exclusive_open(&db).is_none());
        let _held = std::fs::File::open(&db).unwrap();
        let s = probe_exclusive_open(&db).expect("should detect the open handle");
        assert_eq!(s.signal, "exclusive_open_denied");
        assert!(s.blocking);
    }
}
