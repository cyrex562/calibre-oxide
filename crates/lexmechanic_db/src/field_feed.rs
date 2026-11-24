//! Keeps [`crate::cache::Cache`]'s in-memory field snapshot current without
//! reloading all of it after every write.
//!
//! The snapshot used to be thrown away whenever SQLite's `total_changes()`
//! moved, and the next read reloaded every table (every comment, plus a
//! `MAX(size)` subquery per book) while holding the connection lock. A bulk
//! loop that writes one book and then reads one book -- organize, retitle,
//! resolve -- therefore did a full reload per book: quadratic in the library
//! size, which is the difference between seconds and hours at 100k books.
//!
//! Instead, SQLite's update hook feeds every row change this connection
//! makes into a bounded [`FieldFeed`]. A reader remembers how far into the
//! feed it has read; on its next read it takes the new events, works out
//! which books (or which item tables, such as authors or tags) they touch,
//! and re-reads just those. The hook sees every write on the connection
//! whatever code path made it, so this keeps the property the old check had:
//! no write path has to remember to invalidate anything.
//!
//! Writes made by *another* connection (another process, or a second
//! `Backend` over the same file) never reach the hook. `PRAGMA data_version`
//! changes when another connection commits, and any change in it is a full
//! reload. A reader that falls further behind than the feed's capacity also
//! does a full reload, as does any event that cannot be attributed to a book.

use crate::fields::FieldStore;
use crate::tables::ItemTable;
use rusqlite::hooks::Action;
use rusqlite::{Connection, OptionalExtension, Result};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

/// More than this many events unread and the oldest are dropped; a reader
/// that missed them reloads in full.
const MAX_EVENTS: usize = 100_000;

/// More dirty books than this in one catch-up and a full reload is cheaper.
const MAX_PATCHED_BOOKS: usize = 5_000;

/// The tables whose rows the field snapshot is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FeedTable {
    Books,
    Comments,
    Data,
    Identifiers,
    AuthorsLink,
    TagsLink,
    SeriesLink,
    PublishersLink,
    RatingsLink,
    LanguagesLink,
    Item(ItemTable),
}

impl FeedTable {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "books" => Self::Books,
            "comments" => Self::Comments,
            "data" => Self::Data,
            "identifiers" => Self::Identifiers,
            "books_authors_link" => Self::AuthorsLink,
            "books_tags_link" => Self::TagsLink,
            "books_series_link" => Self::SeriesLink,
            "books_publishers_link" => Self::PublishersLink,
            "books_ratings_link" => Self::RatingsLink,
            "books_languages_link" => Self::LanguagesLink,
            "authors" => Self::Item(ItemTable::Authors),
            "tags" => Self::Item(ItemTable::Tags),
            "series" => Self::Item(ItemTable::Series),
            "publishers" => Self::Item(ItemTable::Publishers),
            "ratings" => Self::Item(ItemTable::Ratings),
            "languages" => Self::Item(ItemTable::Languages),
            _ => return None,
        })
    }

    /// The table name, for the tables with a `book` column.
    fn keyed_name(self) -> Option<&'static str> {
        Some(match self {
            Self::Comments => "comments",
            Self::Data => "data",
            Self::Identifiers => "identifiers",
            Self::AuthorsLink => "books_authors_link",
            Self::TagsLink => "books_tags_link",
            Self::SeriesLink => "books_series_link",
            Self::PublishersLink => "books_publishers_link",
            Self::RatingsLink => "books_ratings_link",
            Self::LanguagesLink => "books_languages_link",
            Self::Books | Self::Item(_) => return None,
        })
    }
}

const KEYED: [FeedTable; 9] = [
    FeedTable::Comments,
    FeedTable::Data,
    FeedTable::Identifiers,
    FeedTable::AuthorsLink,
    FeedTable::TagsLink,
    FeedTable::SeriesLink,
    FeedTable::PublishersLink,
    FeedTable::RatingsLink,
    FeedTable::LanguagesLink,
];

#[derive(Debug, Clone, Copy)]
pub(crate) struct RowEvent {
    table: FeedTable,
    deleted: bool,
    rowid: i64,
}

struct FeedInner {
    /// Sequence number of `events[0]`.
    base: u64,
    events: VecDeque<RowEvent>,
}

/// Row changes made through one connection, shared by every clone of its
/// `Backend`.
pub(crate) struct FieldFeed {
    inner: Mutex<FeedInner>,
}

impl FieldFeed {
    pub(crate) fn new() -> Self {
        FieldFeed { inner: Mutex::new(FeedInner { base: 0, events: VecDeque::new() }) }
    }

    fn push(&self, ev: RowEvent) {
        let mut g = self.inner.lock().unwrap();
        if g.events.len() >= MAX_EVENTS {
            g.events.pop_front();
            g.base += 1;
        }
        g.events.push_back(ev);
    }

    fn position(&self) -> u64 {
        let g = self.inner.lock().unwrap();
        g.base + g.events.len() as u64
    }

    /// The events since `pos`, or `None` if some of them have been dropped.
    fn since(&self, pos: u64) -> Option<Vec<RowEvent>> {
        let g = self.inner.lock().unwrap();
        if pos < g.base {
            return None;
        }
        Some(g.events.iter().skip((pos - g.base) as usize).copied().collect())
    }

    /// Starts feeding this feed from `conn`'s row changes.
    pub(crate) fn install(self: &std::sync::Arc<Self>, conn: &Connection) {
        let feed = std::sync::Arc::clone(self);
        conn.update_hook(Some(move |action: Action, _db: &str, table: &str, rowid: i64| {
            if let Some(table) = FeedTable::from_name(table) {
                feed.push(RowEvent { table, deleted: matches!(action, Action::SQLITE_DELETE), rowid });
            }
        }));
    }
}

/// A snapshot of the standard fields and what is needed to keep it current.
pub(crate) struct Loaded {
    data_version: i64,
    pos: u64,
    pub(crate) store: Arc<FieldStore>,
    /// Row id -> book, for the tables with a `book` column. A delete event
    /// carries only the row id, and by then the row is gone.
    reverse: HashMap<(FeedTable, i64), i32>,
}

fn data_version(conn: &Connection) -> i64 {
    // Checked on every snapshot request, so the statement is kept prepared.
    match conn.prepare_cached("PRAGMA data_version") {
        Ok(mut stmt) => stmt.query_row([], |r| r.get(0)).unwrap_or(0),
        Err(_) => conn.query_row("PRAGMA data_version", [], |r| r.get(0)).unwrap_or(0),
    }
}

fn full_load(conn: &Connection, feed: &FieldFeed) -> Result<Loaded> {
    // The position is taken first: an event raised while loading is then
    // read again later, which costs a redundant patch, never a missed one.
    let pos = feed.position();
    let data_version = data_version(conn);
    let store = Arc::new(FieldStore::load(conn)?);
    let mut reverse = HashMap::new();
    for t in KEYED {
        let name = t.keyed_name().unwrap();
        let mut stmt = conn.prepare(&format!("SELECT id, book FROM {name}"))?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i32>(1)?)))?;
        for row in rows {
            let (id, book) = row?;
            reverse.insert((t, id), book);
        }
    }
    Ok(Loaded { data_version, pos, store, reverse })
}

/// Brings `slot` up to date with `conn` and returns the snapshot. `full_loads`
/// counts the full reloads. The snapshot is shared: a reader keeps using it
/// after the lock is released, and a later patch copies it first rather than
/// changing what that reader sees.
pub(crate) fn refresh(
    slot: &mut Option<Loaded>,
    conn: &Connection,
    feed: &FieldFeed,
    full_loads: &std::sync::atomic::AtomicUsize,
) -> Result<Arc<FieldStore>> {
    let mut need_full = true;
    if let Some(loaded) = slot.as_mut() {
        if loaded.data_version == data_version(conn) {
            let pos = feed.position();
            if pos == loaded.pos {
                need_full = false;
            } else if let Some(events) = feed.since(loaded.pos) {
                if apply(loaded, conn, events)? {
                    loaded.pos = pos;
                    need_full = false;
                }
            }
        }
    }
    if need_full {
        *slot = Some(full_load(conn, feed)?);
        full_loads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(Arc::clone(&slot.as_ref().unwrap().store))
}

/// Patches the snapshot for `events`. `Ok(false)` means they cannot be
/// attributed and the caller must reload in full.
fn apply(loaded: &mut Loaded, conn: &Connection, events: Vec<RowEvent>) -> Result<bool> {
    let mut books: BTreeSet<i32> = BTreeSet::new();
    let mut items: HashSet<ItemTable> = HashSet::new();
    // Rows inserted and deleted again inside this batch were never in the
    // snapshot, so their delete event names nothing to patch.
    let mut transient: HashSet<(FeedTable, i64)> = HashSet::new();

    for ev in events {
        match ev.table {
            FeedTable::Books => {
                books.insert(ev.rowid as i32);
            }
            FeedTable::Item(item) => {
                items.insert(item);
            }
            keyed => {
                let key = (keyed, ev.rowid);
                if ev.deleted {
                    match loaded.reverse.remove(&key) {
                        Some(book) => {
                            books.insert(book);
                        }
                        None => {
                            if !transient.remove(&key) {
                                return Ok(false);
                            }
                        }
                    }
                } else {
                    // A row that moved from one book to another dirties both.
                    if let Some(old) = loaded.reverse.get(&key) {
                        books.insert(*old);
                    }
                    let sql = format!("SELECT book FROM {} WHERE id = ?1", keyed.keyed_name().unwrap());
                    let book: Option<i32> = conn.prepare_cached(&sql)?.query_row([ev.rowid], |r| r.get(0)).optional()?;
                    match book {
                        Some(book) => {
                            loaded.reverse.insert(key, book);
                            books.insert(book);
                        }
                        None => {
                            transient.insert(key);
                        }
                    }
                }
            }
        }
        if books.len() > MAX_PATCHED_BOOKS {
            return Ok(false);
        }
    }

    if items.is_empty() && books.is_empty() {
        return Ok(true);
    }
    // Copies the snapshot only if a reader still holds it.
    let store = Arc::make_mut(&mut loaded.store);
    for item in items {
        store.reload_item_table(conn, item)?;
    }
    for book in books {
        store.reload_book(conn, book)?;
    }
    Ok(true)
}
