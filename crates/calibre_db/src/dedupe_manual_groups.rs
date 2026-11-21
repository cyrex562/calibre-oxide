//! Duplicate groups the user made themselves (tranche 16).
//!
//! The detection tiers say "these look like the same book"; the user also
//! knows things detection cannot (two scans of one book on different scanners
//! do not match by pixels, a title was retyped). A **manual group** is a set of
//! books the user put together: from a library selection ("Compare as
//! duplicates" then "Save as my group"), or by adding a book to an existing one
//! in the compare view.
//!
//! # What is stored, and where
//!
//! Like [`crate::dedupe_exemptions`]: one JSON document in the library
//! preference [`PREF_KEY`], written through [`Cache::set_preference`] so it is
//! in the change log (it survives restarts, rescans and a rebuild). A member is
//! recorded by its **uuid** (it survives a changed book id, a restore from the
//! trash and a rebuild), with the title and authors it had for the list.
//!
//! A group has its own id (`mg-` and 12 hex digits), because its members
//! change: the id of a *duplicate group* in the review snapshot is a hash of
//! its members and so changes with them. The snapshot's group says which
//! manual group it is (`manual_group_id`).
//!
//! # Operations
//!
//! [`create`] (at least two books; books already in a manual group join *that*
//! group, so one book is in at most one group), [`add`], [`remove_books`],
//! [`delete`]. [`prune`] drops members whose book no longer exists and groups
//! left with fewer than two live books: a resolved group disappears (and does
//! not come back if the trash is restored; the user can make it again).
//!
//! # Manual groups and the other evidence
//!
//! * **Detected groups.** A manual group and a detected group that share a book
//!   are one group in the review snapshot (see `calibre_srv::duplicates_review`):
//!   all the detected evidence is kept and a "grouped by you" note added. A
//!   manual group is **never** produced by the matcher, so exemptions
//!   ([`crate::dedupe_exemptions`]) do not split it: it is the user's word.
//! * **Exemptions: the newest action wins.** Putting books in a group
//!   ([`create`], [`add`]) removes every exemption whose books are all in the
//!   group and which names at least one book just put in it
//!   ([`crate::dedupe_exemptions::remove_within`]): "not duplicates" for exactly
//!   those books was the older decision. An exemption that also covers books
//!   outside the group is kept, and the group is still not split by it.
//!   Conversely, marking books "not duplicates" ([`separate`]) takes them out of
//!   any manual group that holds two or more of them.

use std::collections::HashSet;
use std::sync::Mutex;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::cache::Cache;

/// The library preference holding every manual group.
pub const PREF_KEY: &str = "oxide.duplicate_manual_groups";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualMember {
    pub uuid: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualGroup {
    pub id: String,
    pub members: Vec<ManualMember>,
    /// Unix seconds.
    pub created_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Document {
    #[serde(default)]
    groups: Vec<ManualGroup>,
}

/// A manual group with its members resolved to books that exist now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveGroup {
    pub id: String,
    pub created_at: i64,
    /// Ascending.
    pub book_ids: Vec<i32>,
    pub uuids: Vec<String>,
}

static WRITE: Mutex<()> = Mutex::new(());

fn load(cache: &Cache) -> Result<Document> {
    match cache.get_preference(PREF_KEY)? {
        Some(raw) if !raw.trim().is_empty() => Ok(serde_json::from_str(&raw).unwrap_or_default()),
        _ => Ok(Document::default()),
    }
}

fn save(cache: &Cache, doc: &Document) -> Result<()> {
    if doc.groups.is_empty() {
        cache.clear_preference(PREF_KEY)
    } else {
        cache.set_preference(PREF_KEY, &serde_json::to_string(doc)?)
    }
}

fn new_id() -> String {
    format!("mg-{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

fn member_for(cache: &Cache, id: i32) -> Result<ManualMember> {
    let Some(uuid) = cache.book_uuid(id)?.filter(|u| !u.is_empty()) else { bail!("no book with id {id}") };
    Ok(ManualMember { uuid, title: cache.field_for(id, "title")?.unwrap_or_default(), authors: cache.author_names_for_book(id).unwrap_or_default() })
}

fn distinct(ids: &[i32]) -> Vec<i32> {
    let mut v = ids.to_vec();
    v.sort_unstable();
    v.dedup();
    v
}

/// Every manual group, oldest first (as stored: no liveness check).
pub fn list(cache: &Cache) -> Result<Vec<ManualGroup>> {
    let mut all = load(cache)?.groups;
    all.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    Ok(all)
}

/// One group by id.
pub fn get(cache: &Cache, id: &str) -> Result<Option<ManualGroup>> {
    Ok(load(cache)?.groups.into_iter().find(|g| g.id == id))
}

/// Makes a group of `book_ids` (at least two distinct, existing books). Books
/// that are already in a manual group bring that group along: all such groups
/// and the new books become **one** group (the oldest's id is kept), so a book
/// is in at most one. Removes the exemptions the group overrules.
pub fn create(cache: &Cache, book_ids: &[i32]) -> Result<ManualGroup> {
    let ids = distinct(book_ids);
    if ids.len() < 2 {
        bail!("a group needs at least two different books");
    }
    let members: Vec<ManualMember> = ids.iter().map(|id| member_for(cache, *id)).collect::<Result<_>>()?;
    let uuids: HashSet<String> = members.iter().map(|m| m.uuid.clone()).collect();
    let created = {
        let _guard = WRITE.lock().unwrap();
        let mut doc = load(cache)?;
        let (touched, rest): (Vec<ManualGroup>, Vec<ManualGroup>) = doc.groups.drain(..).partition(|g| g.members.iter().any(|m| uuids.contains(&m.uuid)));
        doc.groups = rest;
        let mut touched = touched;
        touched.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        let (id, created_at) = touched.first().map(|g| (g.id.clone(), g.created_at)).unwrap_or_else(|| (new_id(), crate::trash::now_secs()));
        let mut merged: Vec<ManualMember> = Vec::new();
        for m in touched.into_iter().flat_map(|g| g.members).chain(members) {
            if !merged.iter().any(|x| x.uuid == m.uuid) {
                merged.push(m);
            }
        }
        let group = ManualGroup { id, members: merged, created_at };
        doc.groups.push(group.clone());
        save(cache, &doc)?;
        group
    };
    let all: Vec<String> = created.members.iter().map(|m| m.uuid.clone()).collect();
    let added: Vec<String> = uuids.into_iter().collect();
    crate::dedupe_exemptions::remove_within(cache, &all, &added)?;
    Ok(created)
}

/// Adds books to a group. A book that is in another manual group moves its
/// whole group in (as [`create`]). Errors if the group does not exist.
pub fn add(cache: &Cache, group_id: &str, book_ids: &[i32]) -> Result<ManualGroup> {
    let Some(existing) = get(cache, group_id)? else { bail!("no group {group_id}") };
    let mut ids: Vec<i32> = existing.members.iter().filter_map(|m| cache.book_id_for_uuid(&m.uuid).ok().flatten()).collect();
    ids.extend_from_slice(book_ids);
    // Unknown ids are an error even though the group's own dead members are skipped.
    for id in book_ids {
        member_for(cache, *id)?;
    }
    if distinct(&ids).len() < 2 {
        bail!("a group needs at least two different books");
    }
    let group = create(cache, &ids)?;
    // `create` keeps the oldest id; the group asked for is the oldest unless a
    // newer-numbered older group was swallowed, so say which one it is now.
    Ok(group)
}

/// Takes books out of a group. The group is deleted when fewer than two remain
/// (the returned `None`). Errors if the group does not exist.
pub fn remove_books(cache: &Cache, group_id: &str, book_ids: &[i32]) -> Result<Option<ManualGroup>> {
    let gone: HashSet<String> = book_ids.iter().filter_map(|id| cache.book_uuid(*id).ok().flatten()).collect();
    let _guard = WRITE.lock().unwrap();
    let mut doc = load(cache)?;
    let Some(at) = doc.groups.iter().position(|g| g.id == group_id) else { bail!("no group {group_id}") };
    doc.groups[at].members.retain(|m| !gone.contains(&m.uuid));
    let out = if doc.groups[at].members.len() < 2 {
        doc.groups.remove(at);
        None
    } else {
        Some(doc.groups[at].clone())
    };
    save(cache, &doc)?;
    Ok(out)
}

/// Deletes a group; whether it existed.
pub fn delete(cache: &Cache, group_id: &str) -> Result<bool> {
    let _guard = WRITE.lock().unwrap();
    let mut doc = load(cache)?;
    let before = doc.groups.len();
    doc.groups.retain(|g| g.id != group_id);
    let removed = doc.groups.len() != before;
    if removed {
        save(cache, &doc)?;
    }
    Ok(removed)
}

/// "Not duplicates" for `book_ids`: removes them from every manual group that
/// holds two or more of them (a group left with fewer than two is deleted).
/// Returns the ids of the groups that changed or went.
pub fn separate(cache: &Cache, book_ids: &[i32]) -> Result<Vec<String>> {
    let uuids: HashSet<String> = book_ids.iter().filter_map(|id| cache.book_uuid(*id).ok().flatten()).collect();
    let _guard = WRITE.lock().unwrap();
    let mut doc = load(cache)?;
    let mut changed = Vec::new();
    for g in &mut doc.groups {
        if g.members.iter().filter(|m| uuids.contains(&m.uuid)).count() >= 2 {
            g.members.retain(|m| !uuids.contains(&m.uuid));
            changed.push(g.id.clone());
        }
    }
    if !changed.is_empty() {
        doc.groups.retain(|g| g.members.len() >= 2);
        save(cache, &doc)?;
    }
    Ok(changed)
}

/// Drops members whose book no longer exists, and the groups with fewer than
/// two left. Returns how many groups went.
pub fn prune(cache: &Cache) -> Result<usize> {
    let _guard = WRITE.lock().unwrap();
    let mut doc = load(cache)?;
    if doc.groups.is_empty() {
        return Ok(0);
    }
    let before = doc.groups.len();
    let mut dirty = false;
    for g in &mut doc.groups {
        let n = g.members.len();
        g.members.retain(|m| cache.book_id_for_uuid(&m.uuid).ok().flatten().is_some());
        dirty |= g.members.len() != n;
    }
    doc.groups.retain(|g| g.members.len() >= 2);
    let gone = before - doc.groups.len();
    if dirty || gone > 0 {
        save(cache, &doc)?;
    }
    Ok(gone)
}

/// The groups with at least two books that exist now, members resolved to ids
/// (not written back: see [`prune`]).
pub fn live(cache: &Cache) -> Result<Vec<LiveGroup>> {
    let mut out = Vec::new();
    for g in list(cache)? {
        let mut pairs: Vec<(i32, String)> = g.members.iter().filter_map(|m| cache.book_id_for_uuid(&m.uuid).ok().flatten().map(|id| (id, m.uuid.clone()))).collect();
        pairs.sort_unstable();
        pairs.dedup_by_key(|p| p.0);
        if pairs.len() >= 2 {
            out.push(LiveGroup { id: g.id, created_at: g.created_at, book_ids: pairs.iter().map(|p| p.0).collect(), uuids: pairs.into_iter().map(|p| p.1).collect() });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dedupe::{scan_library_full, Strength};

    fn add_book(dir: &std::path::Path, cache: &Cache, name: &str, title: &str, author: &str, bytes: &[u8]) -> i32 {
        let source = dir.join(name);
        std::fs::write(&source, bytes).unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec![author.to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    fn four(dir: &std::path::Path, cache: &Cache) -> [i32; 4] {
        [
            add_book(dir, cache, "a.txt", "Alpha Primer", "Ada Lovelace", b"alpha"),
            add_book(dir, cache, "b.txt", "Totally Different", "Bob Writer", b"bravo"),
            add_book(dir, cache, "c.txt", "Another Thing", "Cy Author", b"charlie"),
            add_book(dir, cache, "d.txt", "Fourth Book", "Di Person", b"delta"),
        ]
    }

    #[test]
    fn create_add_remove_and_delete() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cache = Cache::new(lib.path()).unwrap();
        let [a, b, c, d] = four(src.path(), &cache);

        assert!(create(&cache, &[a]).is_err());
        assert!(create(&cache, &[a, a]).is_err());
        assert!(create(&cache, &[a, 9999]).is_err());

        let g = create(&cache, &[b, a]).unwrap();
        assert!(g.id.starts_with("mg-"));
        assert_eq!(live(&cache).unwrap()[0].book_ids, vec![a, b]);

        let g2 = add(&cache, &g.id, &[c]).unwrap();
        assert_eq!(g2.id, g.id);
        assert_eq!(live(&cache).unwrap()[0].book_ids, vec![a, b, c]);
        assert!(add(&cache, "mg-nope", &[d]).is_err());
        assert!(add(&cache, &g.id, &[9999]).is_err());

        assert_eq!(remove_books(&cache, &g.id, &[c]).unwrap().unwrap().members.len(), 2);
        // Down to one book: the group goes.
        assert!(remove_books(&cache, &g.id, &[a]).unwrap().is_none());
        assert!(live(&cache).unwrap().is_empty());
        assert!(list(&cache).unwrap().is_empty());
        assert!(remove_books(&cache, &g.id, &[a]).is_err());

        let again = create(&cache, &[c, d]).unwrap();
        assert!(delete(&cache, &again.id).unwrap());
        assert!(!delete(&cache, &again.id).unwrap());
    }

    #[test]
    fn a_book_is_in_one_group_so_creating_over_a_group_joins_it() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cache = Cache::new(lib.path()).unwrap();
        let [a, b, c, d] = four(src.path(), &cache);
        let first = create(&cache, &[a, b]).unwrap();
        let joined = create(&cache, &[b, c]).unwrap();
        assert_eq!(joined.id, first.id);
        assert_eq!(live(&cache).unwrap().len(), 1);
        assert_eq!(live(&cache).unwrap()[0].book_ids, vec![a, b, c]);
        let other = create(&cache, &[d, c]).unwrap();
        assert_eq!(other.id, first.id);
        assert_eq!(live(&cache).unwrap()[0].book_ids, vec![a, b, c, d]);
    }

    #[test]
    fn groups_survive_reopening_and_a_rebuild_from_the_change_log() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let made;
        {
            let cache = Cache::new(lib.path()).unwrap();
            let [a, b, c, _] = four(src.path(), &cache);
            made = create(&cache, &[a, b, c]).unwrap();
        }
        let reopened = Cache::new(lib.path()).unwrap();
        let after = list(&reopened).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, made.id);
        assert_eq!(live(&reopened).unwrap()[0].book_ids.len(), 3);

        let rebuilt_dir = tempfile::tempdir().unwrap();
        let state = crate::constants::LIBRARY_HANDLE_DIR_NAME;
        for sub in ["changes", "snapshots"] {
            let to = rebuilt_dir.path().join(state).join(sub);
            std::fs::create_dir_all(&to).unwrap();
            if let Ok(read) = std::fs::read_dir(lib.path().join(state).join(sub)) {
                for e in read.flatten() {
                    std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
                }
            }
        }
        std::fs::copy(lib.path().join(state).join("install-id"), rebuilt_dir.path().join(state).join("install-id")).ok();
        let rebuilt = Cache::new(rebuilt_dir.path()).unwrap();
        rebuilt.rebuild_from_change_log().unwrap();
        let again = list(&rebuilt).unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].id, made.id);
        assert_eq!(again[0].members.len(), 3);
    }

    #[test]
    fn a_resolved_or_emptied_group_is_pruned() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cache = Cache::new(lib.path()).unwrap();
        let [a, b, c, d] = four(src.path(), &cache);
        create(&cache, &[a, b, c]).unwrap();
        let lone = create(&cache, &[d]).err();
        assert!(lone.is_some());

        // One member deleted: two remain, the group stays.
        crate::removal::delete_with_file(&cache, c).unwrap();
        assert_eq!(live(&cache).unwrap()[0].book_ids, vec![a, b]);
        assert_eq!(prune(&cache).unwrap(), 0);
        assert_eq!(list(&cache).unwrap()[0].members.len(), 2);

        // A second: fewer than two live books, so it disappears.
        crate::removal::delete_with_file(&cache, b).unwrap();
        assert!(live(&cache).unwrap().is_empty());
        assert_eq!(prune(&cache).unwrap(), 1);
        assert!(list(&cache).unwrap().is_empty());
    }

    #[test]
    fn putting_books_in_a_group_overrules_the_exemption_that_covers_exactly_them() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cache = Cache::new(lib.path()).unwrap();
        let [a, b, c, d] = four(src.path(), &cache);
        crate::dedupe_exemptions::add(&cache, &[a, b]).unwrap();
        crate::dedupe_exemptions::add(&cache, &[a, c, d]).unwrap();
        assert_eq!(crate::dedupe_exemptions::list(&cache).unwrap().len(), 2);

        // {a, b}: the pair exemption goes; {a, c, d} also names books outside the group, so it stays.
        create(&cache, &[a, b]).unwrap();
        let left = crate::dedupe_exemptions::list(&cache).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].members.len(), 3);

        // Adding c and d completes {a, c, d}: now it is covered, and goes too.
        let g = create(&cache, &[a, b]).unwrap();
        add(&cache, &g.id, &[c, d]).unwrap();
        assert!(crate::dedupe_exemptions::list(&cache).unwrap().is_empty());
    }

    #[test]
    fn an_exemption_does_not_split_a_manual_group_and_not_duplicates_takes_the_books_out() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cache = Cache::new(lib.path()).unwrap();
        let [a, b, c, d] = four(src.path(), &cache);
        // An exemption that survives creation (it covers a book outside the group).
        crate::dedupe_exemptions::add(&cache, &[a, b, d]).unwrap();
        let g = create(&cache, &[a, b, c]).unwrap();
        assert_eq!(crate::dedupe_exemptions::list(&cache).unwrap().len(), 1);
        // The matcher never sees manual groups, so the exemption cannot split it.
        assert_eq!(live(&cache).unwrap()[0].book_ids, vec![a, b, c]);
        assert!(scan_library_full(&cache, Strength::Fuzzy).unwrap().groups.is_empty());

        // The user then says a and b are not duplicates: the newest action wins.
        let changed = separate(&cache, &[a, b]).unwrap();
        assert_eq!(changed, vec![g.id.clone()]);
        // Only c is left, so the group is gone.
        assert!(list(&cache).unwrap().is_empty());
    }

    #[test]
    fn not_duplicates_for_one_book_of_a_group_leaves_the_group_alone() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cache = Cache::new(lib.path()).unwrap();
        let [a, b, c, d] = four(src.path(), &cache);
        let g = create(&cache, &[a, b, c]).unwrap();
        // Two books of the set are in the group, one is not: the two leave it.
        assert_eq!(separate(&cache, &[a, b, d]).unwrap(), vec![g.id.clone()]);
        // One book of the set only: nothing to say about the pair.
        let g = create(&cache, &[c, d]).unwrap();
        assert!(separate(&cache, &[c, a]).unwrap().is_empty());
        assert_eq!(get(&cache, &g.id).unwrap().unwrap().members.len(), 2);
    }
}
