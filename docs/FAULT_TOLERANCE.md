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

### Validated against real mounts (issues #262/#263, PR #696)

§6 was originally shipped (#257/PR #261) verified logic-only, against a
plain local filesystem standing in for "network" — no real network
filesystem was available at the time. Since validated against a real
loopback NFSv4 mount (`itsthenetwork/nfs-server-alpine` in a container)
and a real loopback SMB3 mount (Samba in a container, mounted via the
Linux `cifs.ko` client):

- Tier classification, and the real write/copy/rename/remove path, both
  confirmed correct against real NFSv4 and SMB3.
- **Rename onto an already-existing target** — SMB's most-cited real
  divergence from POSIX — confirmed atomic and correct on both real
  mounts: the source's content lands at the target, the source is
  gone, nothing partial. (Scope limit: this is a modern Samba server
  and a modern Linux `cifs.ko` client; the historically-flaky
  combinations are older SMB1/non-Samba servers, which weren't
  available to test against here.)
- A real, general bug was found and fixed while setting this up
  (issue #694): journal recovery falsely reported corruption after the
  same path was written more than once. Unrelated to network storage —
  reproduces locally too — but only surfaced because a real validation
  script happened to reopen a handle after several writes to the same
  target, which the original logic-only tests never did.
- **`LibraryHandle`'s own writer lock (§7) needed a real fix for SMB**
  (this PR): the Linux `cifs.ko` client reports a losing `flock`
  conflict as `EACCES`, not `EWOULDBLOCK` — confirmed live with two
  real processes racing for the same lock file over the SMB3 mount.
  Before this fix, a second `LibraryHandle::open` on an SMB-hosted
  library that lost the lock race got a generic I/O error instead of
  `AlreadyLocked`, defeating any caller logic that specifically checks
  for that variant. Fixed by also treating `PermissionDenied`
  immediately after a successful lock-file `open()` as contention.
  Raw `flock` (independent of `LibraryHandle`) also confirmed correctly
  rejecting the second locker on both mounts, just with this
  differing error kind on SMB.
- **A real NFS "hard" mount (the default) blocks the underlying
  syscall itself during a server outage**, rather than returning an
  error for `retry_with_backoff` to retry. A real NFS server restart
  mid-`write_atomic` left the operation blocked (not failed) for the
  outage's full duration; it resumed and completed correctly, with no
  corruption, once the server came back. This means the 5-minute
  `total_budget` is not a hard ceiling on how long an operation can
  actually take under a real outage on a hard mount — the clock inside
  `retry_with_backoff` only starts once a syscall *returns* an error,
  and a hard mount may not return one at all until the outage ends.
  Not fixed: this is standard, intentional NFS hard-mount behavior
  (favoring "block until it's safe" over "fail fast, maybe corrupt"),
  and changing it is a mount-option decision for whoever administers
  the mount, not something `LibraryHandle` controls. A real SMB server
  restart under the same test, by contrast, surfaced as one slow
  (~2.5 s) but successful operation — the default "soft" CIFS mount
  recovered on its own well inside a single retry attempt.
- NFSv3's specifically-weaker `flock` support (the thing this section
  most wanted checked) was **not** tested — the readily available
  loopback server image only serves NFSv4.x. NFSv4's locking, which is
  what a new real-world deployment would actually use, tested clean.

### Validated against a real S3-backed FUSE mount (issue #264, PR #697)

Object stores are architecturally different enough from NFS/SMB that
the results above don't transfer — validated separately, against a
real MinIO bucket (loopback container) mounted two ways: `rclone
mount` and `s3fs-fuse`.

- **Real bug fixed**: `s3fs-fuse` reports its fstype as exactly
  `fuse.s3fs`, which wasn't in `NETWORK_FSTYPES` — a library on an
  s3fs mount silently got `LocalInternal` treatment, the opposite of
  the caution an object-store mount needs (no local-staging, no
  read-back-verify, no retry/backoff). `fuse.rclone` was already
  correctly recognized. Added `fuse.s3fs`.
- Tier classification (once fixed), the write/copy/rename/remove path,
  and rename onto an already-existing target all confirmed correct
  against both real mounts.
- **`fsync`-durability genuinely depends on the mount's own
  configuration, confirmed by direct measurement — this is real, not
  theoretical, and `LibraryHandle` cannot fix it from its own code.**
  With `rclone mount`'s default `--vfs-cache-mode writes`,
  `write_atomic`'s own explicit `fsync()` on the scratch/temp file
  returns success while the object is still, measurably, not yet
  present in MinIO — confirmed by querying MinIO directly (bypassing
  the FUSE mount entirely) immediately after `write_atomic` returned
  `Ok`. `LibraryHandle`'s own read-back-verify step doesn't catch this
  either, since it reads back through the *same* FUSE cache that's
  lying about durability. Remounting with `--vfs-cache-mode off` closed
  the gap completely — the same immediate query confirmed every file
  durably present in MinIO the instant `write_atomic` returned.
  **Recommendation for anyone hosting a library on an S3-backed FUSE
  mount: use a cache mode with genuinely synchronous writes (e.g.
  `rclone mount --vfs-cache-mode off`), not a default tuned for read
  performance** — this is a real, disclosed limitation of what any
  application-level `fsync()` call can guarantee against a FUSE daemon
  that chooses not to honor it synchronously, not something a future
  `LibraryHandle` change could close on its own.
- Not tested: the "rename is actually copy-then-delete" failure mode
  the issue specifically flagged (a crash mid-rename leaving a
  copied-and-partially-deleted source) — both FUSE tools' own VFS
  layers handled every rename tested here as a single opaque
  operation, and reliably interrupting one specifically *inside* the
  backend copy+delete (as opposed to interrupting the calling process,
  which doesn't touch an already-dispatched backend call) needs a
  fault-injection point inside the FUSE daemon itself that neither
  tool exposes from the outside. Flagged as a real, still-open
  question rather than assumed covered.

Not yet validated against a real mount: Google Drive desktop sync
(#265, deferred).

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
