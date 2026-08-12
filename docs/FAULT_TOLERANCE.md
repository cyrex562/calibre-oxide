# Fault-Tolerance Design Contract

Every port and every new feature in calibre-oxide **must** respect the rules
in this document. The harness's judge agent uses this as the review rubric
for any code that touches disk, device, or network state. A PR that violates
these rules is rejected regardless of whether tests pass.

## Motivating incident

August 2026 — user was working with Calibre against a library on an external
SSD at an airport. Laptop lid closed with the SSD attached. On wake, the
Calibre library folder was corrupted and unreadable. Calibre-oxide must not
have this failure mode.

The rest of this document is the specific engineering that prevents it.

## 1. Storage tiers

Every write path must classify its target as one of:

- **Local-internal**: fixed disk on the machine. Failure mode: process crash,
  power loss, OS crash.
- **Local-external**: USB/Thunderbolt/SD, mounted as a local filesystem.
  Failure mode: everything internal plus surprise removal, lid-close sleep
  mid-flush, bus reset, filesystem freeze.
- **Network**: SMB/NFS/WebDAV/cloud mount. Failure mode: everything external
  plus latency spikes, half-committed writes, disconnect mid-transaction,
  server-side rename semantics that differ from POSIX.

The classification is cheap: `GetDriveTypeW` on Windows, `/proc/mounts` +
`statfs` on Linux. Store it on the library handle, not per operation.

## 2. The write discipline

**Every** durable mutation to a library folder or to `metadata.db` follows
this sequence. No exceptions.

1. Compute the operation. Do not touch the target yet.
2. Serialize the operation to a **journal entry** in
   `<library>/.calibre-oxide/journal/<uuid>.op` — the entry contains a
   monotonic sequence number, the previous head, an operation descriptor,
   and a BLAKE3 of the descriptor. `fsync` the journal file. `fsync` the
   journal directory.
3. Perform the operation using **write-temp / fsync / rename**:
   - Write payload to `<target>.tmp-<uuid>`.
   - `fsync` the temp file.
   - `rename` temp over target. On Windows, use `MoveFileExW` with
     `MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH`. On POSIX,
     `rename(2)` is already atomic.
   - `fsync` the parent directory.
4. Mark the journal entry `committed` — a separate write of a single-byte
   status file next to the entry, `fsync`ed.
5. Only after commit, `fsync`ed, do we ack the operation to the caller.

Recovery on startup: scan the journal, replay any committed-but-unacked
entries (the ack file was lost — safe to re-apply), roll back any
started-but-not-committed entries (the temp files still exist and can be
identified by uuid).

## 3. SQLite discipline

- `metadata.db` and all sidecar databases MUST open in **WAL** mode with
  `synchronous=FULL`.
- All schema-altering statements run inside `BEGIN IMMEDIATE ... COMMIT`.
- Every write path checkpoints (`PRAGMA wal_checkpoint(TRUNCATE)`) after
  the operation is journaled and before it is acked. On external/network
  storage, checkpoint every write. On local-internal, checkpoint every 32
  writes or 5 s.
- Never open the SQLite file with `journal_mode=MEMORY` or
  `synchronous=OFF` on any tier.

## 4. Device-disappearance handling

- All I/O to a library folder goes through a `LibraryHandle` that owns an
  OS-level notification subscription for its mount:
  - Windows: `RegisterDeviceNotificationW` for `DBT_DEVTYP_DEVICEINTERFACE`
    plus `WM_DEVICECHANGE` in a dedicated message pump thread.
  - Linux: `libudev` monitor filtered to the block device.
- On `DEVICE_REMOVED` for the mount, the handle:
  1. Cancels in-flight I/O (best-effort — the FS call may already be stuck
     in the kernel).
  2. Marks itself `Detached`.
  3. Any subsequent call returns `Error::DeviceDetached` — no retry loop,
     no silent corruption path.
- On reattach, the caller must explicitly re-open the library. No implicit
  re-attach. The recovery scan (§2) runs on re-open.

## 5. Sleep / lid-close handling

- Register for OS power-state notifications:
  - Windows: `PowerRegisterSuspendResumeNotification` +
    `WM_POWERBROADCAST` (`PBT_APMSUSPEND` / `PBT_APMRESUMEAUTOMATIC`).
  - Linux: `org.freedesktop.login1` `PrepareForSleep` signal via zbus.
- On imminent suspend:
  1. Every open `LibraryHandle` flushes pending writes, checkpoints WAL,
     `fsync`s parent directories, releases exclusive file locks, and moves
     to `Suspended`.
  2. Any operation attempted while `Suspended` blocks with a timeout of
     30 s waiting for `PBT_APMRESUMEAUTOMATIC`, then errors.
- On resume: revalidate every `LibraryHandle` by re-`statfs`ing the mount
  and reading the journal head. If the mount fingerprint (device id +
  filesystem uuid + top-level `.calibre-oxide/library.id`) does not match
  what we recorded pre-suspend, the handle transitions to `Detached`.
  This is the codified answer to the airport-SSD incident.

## 6. Network storage

Everything in §2-§5 still applies, plus:

- **No** partial writes across a network. Assemble the full payload
  locally in a scratch dir, then upload in one operation, then verify by
  reading back and comparing BLAKE3.
- Operations that mutate multiple network files (e.g., "move book" =
  rename directory + update metadata.db) run through a **two-phase**
  variant of the journal: prepare all changes as staged uploads, only
  then flip references in metadata.db.
- Retry policy: exponential backoff up to 60 s, then bubble up. Never
  retry silently for more than 5 minutes — surface the failure.

## 7. Concurrency

- One writer per library. Enforced by `flock`-style exclusive lock on
  `<library>/.calibre-oxide/writer.lock` acquired at handle open.
- Read handles are unlimited but see snapshots — no dirty reads across
  the write boundary. SQLite WAL gives us this naturally; for filesystem
  reads, materialize path lists through the read side of the journal.

## 8. Checksums everywhere

- Every book file's BLAKE3 is stored in metadata.db at add time and
  re-verified on any operation that touches the file. Mismatch is
  surfaced as `Error::Corruption` and logged with the file path and both
  hashes; the operation aborts before mutating anything.
- Cover images and sidecar files: same rule.
- The journal itself is BLAKE3-chained (§2 step 2 references previous
  head).

## 9. What is *not* allowed

- `fs::write`, `fs::rename` directly against a library path. Wrap through
  `LibraryHandle::write_atomic`.
- `File::create` followed by any writes without the temp-rename-fsync
  dance.
- Any `unwrap()` / `expect()` on I/O operations against a library. Errors
  bubble to the caller — the caller decides whether to retry or surface.
- Silent fallbacks. If storage classification says "network" and a
  network-only guarantee can't be met, error out, don't downgrade.

## 10. Testable invariants

The judge agent enforces these by grep and by test:

- `grep -R "std::fs::rename" crates/` on a library path → reject unless
  inside `LibraryHandle::rename_atomic`.
- `grep -R "\.unwrap()" crates/calibre_db crates/calibre_ebooks/**/library/` → reject.
- Every write-path test must include a "kill process at random point"
  variant — we use `fail::cfg` (the `fail` crate) to inject panics between
  every step of §2, and assert recovery is clean.
- Every device driver test must include a "device removed mid-op" variant
  and assert `Error::DeviceDetached`.

## References

- Dan Luu, "Files are hard": https://danluu.com/file-consistency/
- SQLite, "How to corrupt an SQLite database": https://www.sqlite.org/howtocorrupt.html
- LWN, "Ensuring data reaches disk": https://lwn.net/Articles/457667/
- Windows, `MoveFileExW`: https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw
