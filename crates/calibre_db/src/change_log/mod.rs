//! The library's authoritative change log (issue #900, part of #899).
//!
//! `metadata.db` is a **derived query cache**. The authority is an
//! append-only log of metadata changes under
//! `<library>/.calibre-oxide/changes/`, from which the database can be
//! rebuilt exactly. Deleting `metadata.db` is a recovery step, not a
//! loss.
//!
//! # Why the authority is not the database
//!
//! A library is a folder the user owns, so it ends up inside OneDrive,
//! Dropbox or iCloud. SQLite written from two machines through a
//! file-sync service corrupts: there is no locking, no coordination,
//! just two versions of one binary file delivered over each other. A
//! directory of one-small-file-per-change *merges* under exactly that
//! treatment — both peers' files arrive, and replaying them in order
//! produces one consistent library.
//!
//! The same structure buys crash safety (an entry is fsynced before the
//! database is touched), history, and the ability to throw away a
//! corrupt database with no data loss.
//!
//! See `docs/LIBRARY_MODEL.md` for the full reasoning, including why
//! this was chosen over writing a `metadata.opf` sidecar next to every
//! book file.
//!
//! # The three pieces
//!
//! | | |
//! | --- | --- |
//! | [`hlc`] | ordering changes made on machines whose clocks disagree |
//! | [`change`] | what a change is: the op, its stamp, its chain link |
//! | [`store`] | the on-disk log: append, replay, verify, compact |
//!
//! # Scope of this pass
//!
//! The store and nothing else. No `Cache` write path appends to it yet
//! — that is #901, a crate-wide retrofit of the same shape as #93's
//! "every durable write goes through `LibraryHandle`" — and no merge
//! runs on open, which is #902. Landing the store alone keeps the tree
//! working and makes the retrofit reviewable in slices.
//!
//! # One correction to the filed design
//!
//! #899 described entries as "BLAKE3-chained like the existing
//! journal". That is wrong for this log, and only shows up on trying to
//! write it: a single chain requires a single writer, and this log has
//! one writer per machine with no way for them to agree on who comes
//! next. Two peers extending one chain both claim the same predecessor
//! and the chain breaks under normal use.
//!
//! Entries are chained **per origin** instead. Each install has its own
//! sequence and its own chain; the merged log is several chains side by
//! side. That keeps what chaining is for — a deleted or edited entry
//! leaves a detectable gap or mismatch in *that* origin's sequence —
//! without requiring coordination that cannot exist. Same reason git
//! branches rather than demanding a global commit order.

pub mod change;
pub mod hlc;
pub mod store;

pub use change::{Change, ChangeOp, ChangeParseError};
pub use hlc::{Hlc, HlcClock};
pub use store::{ChangeLog, CompactionReport, InstallId, Replay, SnapshotHeader, VerifyReport, Watermark, DEFAULT_RETENTION};
