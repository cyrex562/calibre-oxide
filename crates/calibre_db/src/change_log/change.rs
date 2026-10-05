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

/// What a change applies to (#901).
///
/// Not every change is about a book: a custom column's *definition* and
/// a preference belong to the library itself. Replay and compaction both
/// need to know what an op addresses without matching on the op, so
/// every op answers [`ChangeOp::target`].
///
/// Derived from the variant rather than carried as a field, so a
/// nonsensical pairing — a `BookAdded` claiming library scope — cannot be
/// constructed at all. The scope is still explicit, just statically so.
///
/// One log and one total order across all of it, deliberately: #902 has
/// to resolve conflicts *between* book-level and library-level changes,
/// and two streams would mean two orderings to reconcile.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChangeTarget {
    /// One book, by uuid.
    Book(String),
    /// The library itself: schema and preferences.
    Library,
}

/// One durable metadata mutation.
///
/// Book-scoped variants name the book by uuid; the rest apply to the
/// library. Adding a variant is a compatibility event for peers on older
/// versions — see [`Change::from_json`].
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

    /// Sets one library preference. `None` clears it.
    ///
    /// Library-scoped: a preference is not owned by any book.
    PrefSet { key: String, value: Option<String> },

    /// Creates a custom column — its *definition*, not a value in it.
    ///
    /// The log carries schema as well as contents (#901), because
    /// `metadata.db` is only genuinely a disposable derived cache if a
    /// replay can rebuild the columns the values live in. A log that
    /// restored a book's `#rating` without creating `#rating` first
    /// would have nowhere to put it.
    ///
    /// Ordering therefore matters: this has to be replayed before any
    /// `FieldSet` naming the column. The single total order gives that
    /// for free, since the column is always created before a value can
    /// be written to it.
    CustomColumnAdded { label: String, name: String, datatype: String, is_multiple: bool },

    /// Drops a custom column and its values.
    CustomColumnRemoved { label: String },

    /// Renames an author, tag or publisher everywhere it is used.
    ///
    /// One op for what the database does in one statement, rather than a
    /// `FieldSet` per affected book: renaming a tag shared by ten thousand
    /// books would otherwise be ten thousand fsynced log files. It also
    /// expresses the *merge* case exactly -- renaming into a name that
    /// already exists folds the two together, which a per-book snapshot of
    /// the final value could only approximate.
    ///
    /// Library-scoped: it is about the item, not about any one book.
    /// `kind` is `authors`, `tags` or `publishers`, the table renamed.
    ItemRenamed { kind: String, from: String, to: String },
}

/// The unit a last-writer-wins comparison is about: one value that two
/// peers might both have written (#902).
///
/// A cell is **not** the same thing as a compaction key
/// ([`ChangeOp::supersede_key`]), though they overlap. Compaction asks "can
/// the earlier of two changes be dropped?"; a cell asks "if two changes
/// arrive out of order, which one's value is the library's?". They differ
/// for [`ChangeOp::FormatRemoved`]: it must never be *dropped* by
/// compaction, but it has to be *compared* against [`ChangeOp::FormatSet`]
/// for the same format -- otherwise a peer's older "format added" arriving
/// after a newer "format removed" would bring a deleted format back.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Cell {
    /// The book's uuid, or empty for the library itself.
    pub scope: String,
    /// What within that scope: `field:title`, `format:EPUB`, `cover`,
    /// `pref:<key>`, `column:<label>`.
    pub key: String,
}

impl ChangeOp {
    /// The value this change writes, if it writes one that two peers could
    /// contend over. `None` for changes that are not last-writer-wins:
    /// creating or removing a book, renaming an item -- those compose
    /// rather than overwrite, and are applied unconditionally.
    pub fn cell(&self) -> Option<Cell> {
        let book = |b: &str, key: String| Some(Cell { scope: b.to_string(), key });
        let library = |key: String| Some(Cell { scope: String::new(), key });
        match self {
            ChangeOp::FieldSet { book: b, field, .. } => book(b, format!("field:{field}")),
            // One cell for both, deliberately: see the type's docs.
            ChangeOp::FormatSet { book: b, format, .. } | ChangeOp::FormatRemoved { book: b, format } => book(b, format!("format:{format}")),
            ChangeOp::CoverSet { book: b, .. } => book(b, "cover".to_string()),
            ChangeOp::PrefSet { key, .. } => library(format!("pref:{key}")),
            // A column that is added, removed and added again is one
            // contested value, not three independent ones.
            ChangeOp::CustomColumnAdded { label, .. } | ChangeOp::CustomColumnRemoved { label } => library(format!("column:{label}")),
            ChangeOp::BookAdded { .. } | ChangeOp::BookRemoved { .. } | ChangeOp::ItemRenamed { .. } => None,
        }
    }

    /// What this change applies to. Replay and compaction need it
    /// without matching on the op.
    pub fn target(&self) -> ChangeTarget {
        match self {
            ChangeOp::BookAdded { book }
            | ChangeOp::BookRemoved { book }
            | ChangeOp::FieldSet { book, .. }
            | ChangeOp::FormatSet { book, .. }
            | ChangeOp::FormatRemoved { book, .. }
            | ChangeOp::CoverSet { book, .. } => ChangeTarget::Book(book.clone()),
            ChangeOp::PrefSet { .. } | ChangeOp::CustomColumnAdded { .. } | ChangeOp::CustomColumnRemoved { .. } | ChangeOp::ItemRenamed { .. } => ChangeTarget::Library,
        }
    }

    /// The book this change is about, if it is about one.
    pub fn book(&self) -> Option<&str> {
        match self {
            ChangeOp::BookAdded { book }
            | ChangeOp::BookRemoved { book }
            | ChangeOp::FieldSet { book, .. }
            | ChangeOp::FormatSet { book, .. }
            | ChangeOp::FormatRemoved { book, .. }
            | ChangeOp::CoverSet { book, .. } => Some(book),
            ChangeOp::PrefSet { .. } | ChangeOp::CustomColumnAdded { .. } | ChangeOp::CustomColumnRemoved { .. } | ChangeOp::ItemRenamed { .. } => None,
        }
    }

    /// What this change supersedes, for compaction.
    ///
    /// Two changes with the same key produce the same end state from
    /// the later one alone, so the earlier can be dropped. `None`
    /// means "never supersedes anything" — `BookRemoved` has to stay,
    /// because dropping it would resurrect the book on the next replay.
    pub fn supersede_key(&self) -> Option<(ChangeTarget, &str, &str)> {
        match self {
            ChangeOp::FieldSet { book, field, .. } => Some((ChangeTarget::Book(book.clone()), "field", field)),
            ChangeOp::FormatSet { book, format, .. } => Some((ChangeTarget::Book(book.clone()), "format", format)),
            ChangeOp::CoverSet { book, .. } => Some((ChangeTarget::Book(book.clone()), "cover", "")),
            // Per key, so two preferences never supersede each other.
            ChangeOp::PrefSet { key, .. } => Some((ChangeTarget::Library, "pref", key)),
            // `BookAdded` is not superseded by a later `BookAdded`
            // (there is never a second one) and must not be dropped:
            // replay needs it to create the row every other change
            // targets. The custom-column pair is the same case one level
            // up -- dropping the `Added` would leave a replay with values
            // and no column to put them in, and dropping the `Removed`
            // would resurrect the column.
            ChangeOp::BookAdded { .. }
            | ChangeOp::BookRemoved { .. }
            | ChangeOp::FormatRemoved { .. }
            | ChangeOp::CustomColumnAdded { .. }
            | ChangeOp::CustomColumnRemoved { .. }
            // Never superseded: a rename composes with whatever else was
            // done to the item, and dropping one would leave later ops
            // naming an item that, on replay, still has its old name.
            | ChangeOp::ItemRenamed { .. } => None,
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
            assert_eq!(op.book(), Some("u"));
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

    /// #901: two preferences must not supersede each other, and a schema
    /// change must never be compacted away.
    #[test]
    fn library_scoped_ops_supersede_per_key_and_schema_is_never_dropped() {
        let sort = ChangeOp::PrefSet { key: "sort".into(), value: Some("title".into()) };
        let sort_again = ChangeOp::PrefSet { key: "sort".into(), value: Some("author".into()) };
        let other = ChangeOp::PrefSet { key: "columns".into(), value: None };

        assert_eq!(sort.supersede_key(), sort_again.supersede_key(), "the later write wins for the same key");
        assert_ne!(sort.supersede_key(), other.supersede_key(), "different preferences are independent");

        // Dropping either of these would leave a replay with values and
        // no column to put them in, or resurrect a deleted column.
        assert!(ChangeOp::CustomColumnAdded { label: "shelf".into(), name: "Shelf".into(), datatype: "text".into(), is_multiple: false }.supersede_key().is_none());
        assert!(ChangeOp::CustomColumnRemoved { label: "shelf".into() }.supersede_key().is_none());
    }

    /// A preference and a book field with the same name are different
    /// things, and compaction keys must not conflate them.
    #[test]
    fn a_library_op_never_shares_a_supersede_key_with_a_book_op() {
        let pref = ChangeOp::PrefSet { key: "title".into(), value: None };
        let field = ChangeOp::FieldSet { book: "title".into(), field: "title".into(), value: None };
        assert_ne!(pref.supersede_key(), field.supersede_key());
    }

    #[test]
    fn target_says_what_each_op_applies_to() {
        assert_eq!(ChangeOp::BookAdded { book: "u".into() }.target(), ChangeTarget::Book("u".into()));
        assert_eq!(ChangeOp::PrefSet { key: "sort".into(), value: None }.target(), ChangeTarget::Library);
        assert_eq!(ChangeOp::CustomColumnRemoved { label: "shelf".into() }.target(), ChangeTarget::Library);
        // A library-scoped op has no book, which is how replay knows not
        // to look one up.
        assert_eq!(ChangeOp::PrefSet { key: "sort".into(), value: None }.book(), None);
    }

    /// A rename is about the item, not any book, and must never be
    /// compacted away: dropping one would leave later ops naming an item
    /// that, on replay, still has its old name.
    #[test]
    fn an_item_rename_is_library_scoped_and_never_superseded() {
        let rename = ChangeOp::ItemRenamed { kind: "tags".into(), from: "old".into(), to: "new".into() };
        assert_eq!(rename.target(), ChangeTarget::Library);
        assert_eq!(rename.book(), None);
        assert!(rename.supersede_key().is_none());
    }

    /// Format add and remove contend over the same value.
    #[test]
    fn a_format_removal_shares_a_cell_with_a_format_add() {
        let set = ChangeOp::FormatSet { book: "u".into(), format: "EPUB".into(), name: "n".into(), size: 1, hash: None };
        let removed = ChangeOp::FormatRemoved { book: "u".into(), format: "EPUB".into() };
        assert_eq!(set.cell(), removed.cell());
        assert!(set.cell().is_some());
        // ...while compaction still must not drop the removal.
        assert!(removed.supersede_key().is_none());
    }

    #[test]
    fn different_fields_and_different_books_are_different_cells() {
        let title = ChangeOp::FieldSet { book: "a".into(), field: "title".into(), value: None };
        let other_field = ChangeOp::FieldSet { book: "a".into(), field: "rating".into(), value: None };
        let other_book = ChangeOp::FieldSet { book: "b".into(), field: "title".into(), value: None };
        assert_ne!(title.cell(), other_field.cell());
        assert_ne!(title.cell(), other_book.cell());
    }

    /// Creating or removing a book, and renaming an item, compose rather
    /// than overwrite -- there is no "older value" to lose.
    #[test]
    fn changes_that_compose_have_no_cell() {
        assert_eq!(ChangeOp::BookAdded { book: "u".into() }.cell(), None);
        assert_eq!(ChangeOp::BookRemoved { book: "u".into() }.cell(), None);
        assert_eq!(ChangeOp::ItemRenamed { kind: "tags".into(), from: "a".into(), to: "b".into() }.cell(), None);
    }

    #[test]
    fn a_column_added_then_removed_is_one_contested_value() {
        let added = ChangeOp::CustomColumnAdded { label: "shelf".into(), name: "Shelf".into(), datatype: "text".into(), is_multiple: false };
        let removed = ChangeOp::CustomColumnRemoved { label: "shelf".into() };
        assert_eq!(added.cell(), removed.cell());
    }
}
