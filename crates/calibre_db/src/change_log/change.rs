//! What a change *is*: the record appended to the log for every
//! durable metadata mutation.
//!
//! # Books are identified by uuid, never by id
//!
//! `books.id` is a local autoincrement. Two machines that each add a
//! book independently both get id 1, so an id cannot survive a merge --
//! replaying a peer's `FieldSet { book: 1, .. }` would edit whichever
//! unrelated book happens to be first locally. The schema already
//! generates a uuid per book (and its insert trigger overwrites any
//! caller-supplied one, which `add_book_db_entry` works around with an
//! explicit UPDATE); that uuid is the only identifier that means the
//! same thing on both machines.
//!
//! # Cover images are not in the log
//!
//! A change is a small JSON document, and a cover is a megabyte of
//! JPEG. Blobs live content-addressed under `.calibre-oxide/covers/`
//! and the log records only the hash — the same split git makes between
//! blobs and the commits that reference them. It also means re-setting
//! the same cover twice costs one log entry and no bytes.

use serde::{Deserialize, Serialize};

use super::hlc::Hlc;

/// One durable metadata mutation.
///
/// Every variant names the book by uuid. Adding a variant is a
/// compatibility event for peers on older versions — see
/// [`Change::from_json`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ChangeOp {
    /// Brings a book into existence. Everything beyond identity
    /// arrives as separate [`ChangeOp::FieldSet`] entries, so there is
    /// one code path for "set a field" rather than two.
    BookAdded { book: String },

    /// Removes a book and everything hanging off it.
    BookRemoved { book: String },

    /// Sets one field. `None` clears it.
    ///
    /// Fields are named as `Cache::set_field` names them, so replay
    /// does not need a translation table.
    FieldSet { book: String, field: String, value: Option<String> },

    /// Records a format file: which format, what it is called on disk
    /// (`data.name`), its size, and its content hash.
    ///
    /// Covers adding a format and renaming one — both are "this format
    /// is now this file", and treating them as one operation is what
    /// keeps `data.name` and the file on disk from drifting apart the
    /// way they did before #885.
    FormatSet { book: String, format: String, name: String, size: i64, hash: Option<String> },

    FormatRemoved { book: String, format: String },

    /// Points the book at a cover blob under `covers/`, or clears it.
    CoverSet { book: String, blob: Option<String> },
}

impl ChangeOp {
    /// The uuid of the book this change is about. Every variant has
    /// one; replay needs it without matching on the op.
    pub fn book(&self) -> &str {
        match self {
            ChangeOp::BookAdded { book }
            | ChangeOp::BookRemoved { book }
            | ChangeOp::FieldSet { book, .. }
            | ChangeOp::FormatSet { book, .. }
            | ChangeOp::FormatRemoved { book, .. }
            | ChangeOp::CoverSet { book, .. } => book,
        }
    }

    /// What this change supersedes, for compaction.
    ///
    /// Two changes with the same key produce the same end state from
    /// the later one alone, so the earlier can be dropped. `None`
    /// means "never supersedes anything" — `BookRemoved` has to stay,
    /// because dropping it would resurrect the book on the next replay.
    pub fn supersede_key(&self) -> Option<(&str, &str, &str)> {
        match self {
            ChangeOp::FieldSet { book, field, .. } => Some((book, "field", field)),
            ChangeOp::FormatSet { book, format, .. } => Some((book, "format", format)),
            ChangeOp::CoverSet { book, .. } => Some((book, "cover", "")),
            // `BookAdded` is not superseded by a later `BookAdded`
            // (there is never a second one) and must not be dropped:
            // replay needs it to create the row every other change
            // targets.
            ChangeOp::BookAdded { .. } | ChangeOp::BookRemoved { .. } | ChangeOp::FormatRemoved { .. } => None,
        }
    }
}

/// A log entry: one change, stamped and chained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub hlc: Hlc,
    /// Which install made this change.
    pub origin: String,
    /// This origin's own sequence number, starting at 1.
    ///
    /// Per-origin, *not* global: a global sequence would need the two
    /// machines to agree on the next number, which is exactly what they
    /// cannot do. A gap in one origin's sequence is a missing or
    /// deleted entry, and is detectable because of this.
    pub seq: u64,
    /// The hash of this origin's previous entry, or `None` for its
    /// first. Chains *within* an origin; see the module docs on
    /// [`super::store`] for why there is no single chain across all of
    /// them.
    pub prev: Option<String>,
    pub op: ChangeOp,
    /// BLAKE3 of everything above, so a truncated or edited entry file
    /// is detectable on its own, without reference to its neighbours.
    pub hash: String,
}

/// The fields a [`Change`]'s hash covers — everything but the hash.
///
/// A separate struct rather than hashing a hand-built string: serde
/// emits struct fields in declaration order, so this is deterministic,
/// and adding a field to `Change` cannot silently fall outside the
/// hash the way appending to a format string would.
#[derive(Serialize)]
struct Hashed<'a> {
    hlc: &'a Hlc,
    origin: &'a str,
    seq: u64,
    prev: &'a Option<String>,
    op: &'a ChangeOp,
}

impl Change {
    /// Builds an entry and computes its hash.
    pub fn new(hlc: Hlc, origin: String, seq: u64, prev: Option<String>, op: ChangeOp) -> Self {
        let hash = Self::compute_hash(&hlc, &origin, seq, &prev, &op);
        Change { hlc, origin, seq, prev, op, hash }
    }

    fn compute_hash(hlc: &Hlc, origin: &str, seq: u64, prev: &Option<String>, op: &ChangeOp) -> String {
        let payload = Hashed { hlc, origin, seq, prev, op };
        // Infallible for these types; a panic here would mean a
        // non-serialisable `ChangeOp` variant, which the compiler
        // prevents.
        let bytes = serde_json::to_vec(&payload).expect("a change is always serialisable");
        blake3::hash(&bytes).to_hex().to_string()
    }

    /// Whether the entry's own hash matches its contents.
    pub fn hash_is_valid(&self) -> bool {
        Self::compute_hash(&self.hlc, &self.origin, self.seq, &self.prev, &self.op) == self.hash
    }

    /// The filename this entry is stored under.
    ///
    /// Three properties, all load-bearing:
    ///
    /// - **Sorts in clock order**, so reading the log in directory
    ///   order is reading it in causal order.
    /// - **Cannot collide across machines.** The origin is in the name,
    ///   so two peers minting the same stamp in the same millisecond
    ///   still write different files. The file-operation journal's
    ///   plain monotonic sequence number would have had both peers
    ///   write `000042.op` and one silently overwrite the other after a
    ///   sync.
    /// - **Cannot collide with the same origin's own history**, via the
    ///   nonce — insurance against an install whose clock state was
    ///   lost and restarted.
    pub fn file_name(&self, nonce: &str) -> String {
        format!("{}-{}-{}.json", self.hlc.file_prefix(), self.origin, nonce)
    }

    /// Reads an entry, rejecting one whose hash does not match.
    ///
    /// An unknown `op` is a deserialisation error, which is the honest
    /// outcome: a peer running a newer version has written something
    /// this build cannot apply, and guessing would corrupt the library.
    /// Callers that must survive it (merge, which should skip rather
    /// than abort) distinguish it from corruption by the error kind.
    pub fn from_json(bytes: &[u8]) -> Result<Change, ChangeParseError> {
        let change: Change = serde_json::from_slice(bytes).map_err(|e| ChangeParseError::Malformed(e.to_string()))?;
        if !change.hash_is_valid() {
            return Err(ChangeParseError::HashMismatch);
        }
        Ok(change)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ChangeParseError {
    /// Not a change at all, or one this build does not understand.
    #[error("not a readable change entry: {0}")]
    Malformed(String),
    /// A change whose contents do not match its own hash: truncated
    /// mid-write, or edited.
    #[error("the entry's contents do not match its hash")]
    HashMismatch,
}

/// Total order over merged changes.
///
/// `(hlc, origin, seq)`. The HLC does the real work; the origin breaks
/// ties between peers that minted the same stamp, and `seq` breaks ties
/// within one origin. Arbitrary but *identical on every machine*, which
/// is the only property that matters — two peers replaying the same set
/// of changes must reach the same state.
pub fn total_order(a: &Change, b: &Change) -> std::cmp::Ordering {
    a.hlc.cmp(&b.hlc).then_with(|| a.origin.cmp(&b.origin)).then_with(|| a.seq.cmp(&b.seq))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op() -> ChangeOp {
        ChangeOp::FieldSet { book: "uuid-1".into(), field: "title".into(), value: Some("Dune".into()) }
    }

    fn change(wall: u64, origin: &str, seq: u64) -> Change {
        Change::new(Hlc { wall_ms: wall, counter: 0 }, origin.into(), seq, None, op())
    }

    #[test]
    fn a_change_hashes_its_own_contents() {
        let c = change(1, "aaa", 1);
        assert!(c.hash_is_valid());
    }

    #[test]
    fn editing_a_change_invalidates_its_hash() {
        let mut c = change(1, "aaa", 1);
        c.op = ChangeOp::FieldSet { book: "uuid-1".into(), field: "title".into(), value: Some("Not Dune".into()) };
        assert!(!c.hash_is_valid());
    }

    #[test]
    fn a_change_round_trips_through_json() {
        let c = change(7, "bbb", 3);
        let bytes = serde_json::to_vec(&c).unwrap();
        assert_eq!(Change::from_json(&bytes).unwrap(), c);
    }

    #[test]
    fn a_tampered_entry_is_rejected_on_read() {
        let c = change(7, "bbb", 3);
        let text = String::from_utf8(serde_json::to_vec(&c).unwrap()).unwrap().replace("Dune", "Duno");
        assert_eq!(Change::from_json(text.as_bytes()), Err(ChangeParseError::HashMismatch));
    }

    #[test]
    fn a_truncated_entry_is_malformed_not_a_hash_mismatch() {
        let c = change(7, "bbb", 3);
        let bytes = serde_json::to_vec(&c).unwrap();
        let err = Change::from_json(&bytes[..bytes.len() / 2]).unwrap_err();
        assert!(matches!(err, ChangeParseError::Malformed(_)), "got {err:?}");
    }

    #[test]
    fn filenames_include_the_origin_so_two_peers_cannot_collide() {
        // Same millisecond, same counter, different machines -- the
        // case a plain sequence number gets wrong.
        let a = change(1_700_000_000_000, "aaaa", 1);
        let b = change(1_700_000_000_000, "bbbb", 1);
        assert_ne!(a.file_name("00000000"), b.file_name("00000000"));
        assert!(a.file_name("00000000").starts_with(&a.hlc.file_prefix()));
    }

    #[test]
    fn the_total_order_is_the_same_on_every_machine() {
        let mut one = vec![change(2, "bbb", 1), change(1, "aaa", 1), change(2, "aaa", 1)];
        let mut other = vec![change(2, "aaa", 1), change(2, "bbb", 1), change(1, "aaa", 1)];
        one.sort_by(total_order);
        other.sort_by(total_order);
        assert_eq!(one, other);
        // And the order is the expected one: clock first, origin second.
        assert_eq!(one.iter().map(|c| (c.hlc.wall_ms, c.origin.as_str())).collect::<Vec<_>>(), vec![(1, "aaa"), (2, "aaa"), (2, "bbb")]);
    }

    #[test]
    fn every_op_names_its_book() {
        let ops = [
            ChangeOp::BookAdded { book: "u".into() },
            ChangeOp::BookRemoved { book: "u".into() },
            ChangeOp::FieldSet { book: "u".into(), field: "t".into(), value: None },
            ChangeOp::FormatSet { book: "u".into(), format: "PDF".into(), name: "n".into(), size: 1, hash: None },
            ChangeOp::FormatRemoved { book: "u".into(), format: "PDF".into() },
            ChangeOp::CoverSet { book: "u".into(), blob: None },
        ];
        for op in ops {
            assert_eq!(op.book(), "u");
        }
    }

    /// Compaction drops a change only when a later one makes it
    /// redundant. Getting this wrong either loses edits or resurrects
    /// deleted books.
    #[test]
    fn only_last_write_wins_operations_supersede() {
        assert!(ChangeOp::FieldSet { book: "u".into(), field: "title".into(), value: None }.supersede_key().is_some());
        assert!(ChangeOp::CoverSet { book: "u".into(), blob: None }.supersede_key().is_some());
        // A removal that got dropped would bring the book back.
        assert!(ChangeOp::BookRemoved { book: "u".into() }.supersede_key().is_none());
        // And an add that got dropped would leave every later change
        // pointing at a book that was never created.
        assert!(ChangeOp::BookAdded { book: "u".into() }.supersede_key().is_none());
    }

    #[test]
    fn different_fields_of_one_book_do_not_supersede_each_other() {
        let title = ChangeOp::FieldSet { book: "u".into(), field: "title".into(), value: None };
        let rating = ChangeOp::FieldSet { book: "u".into(), field: "rating".into(), value: None };
        assert_ne!(title.supersede_key(), rating.supersede_key());
    }

    #[test]
    fn the_same_field_on_different_books_does_not_supersede() {
        let a = ChangeOp::FieldSet { book: "a".into(), field: "title".into(), value: None };
        let b = ChangeOp::FieldSet { book: "b".into(), field: "title".into(), value: None };
        assert_ne!(a.supersede_key(), b.supersede_key());
    }
}
