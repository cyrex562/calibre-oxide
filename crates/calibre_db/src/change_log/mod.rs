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
//! | [`apply`] | putting a logged change back into `metadata.db` |
//!
//! # Scope of this pass
//!
//! The store (#900) and the first slice of the write-path retrofit
//! (#901). `set_field`, `add_book_db_entry`, `add_format`,
//! `remove_format` and `delete_book` append; the remaining write
//! methods do not yet, and the audit test that would forbid that is
//! the end of the retrofit rather than its beginning. No merge runs on
//! open — that is #902.
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

pub mod apply;
#[cfg(test)]
mod audit;
pub mod change;
pub mod hlc;
pub mod store;

pub use apply::{replay_into, ReplayReport};
pub use change::{Change, ChangeOp, ChangeParseError, ChangeTarget};
pub use hlc::{Hlc, HlcClock};
pub use store::{ChangeLog, CompactionReport, InstallId, Replay, SnapshotHeader, VerifyReport, Watermark, DEFAULT_RETENTION};
