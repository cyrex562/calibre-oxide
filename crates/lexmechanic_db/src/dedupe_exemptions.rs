//! "Not a duplicate": pairs of books the user has said are different, so the
//! duplicate scan stops proposing them (tranche 12).
//!
//! # What is stored, and where
//!
//! An **exemption** is a *set* of books that are pairwise "not duplicates of
//! each other" -- marking a group of 50 is one entry, not 1,225 pairs. Each
//! member is recorded by
//!
//! * its **uuid** (the identity that survives a rebuild or a restore from the
//!   trash, see `change_log::change`), and
//! * the **content hashes** of its format files at that time, so a book that
//!   is deleted and re-imported (new uuid, same bytes) is still exempt. Empty
//!   files are never used as a hash key (they all hash alike).
//!
//! All exemptions are one JSON document in the library preference
//! [`PREF_KEY`]. Preferences are written through
//! [`Cache::set_preference`], which appends a `PrefSet` to the change log, so
//! the exemptions are durable the same way every other library setting is:
//! they survive restarts, rescans and a database rebuild from the log
//! (`rebuild_from_change_log` replays the `PrefSet`). No new table or log op
//! was needed.
//!
//! # How the scan uses them
//!
//! [`cannot_link`] turns the stored sets into "these two books may never share
//! a group" edges between book ids; `dedupe::find_groups_full` applies them
//! before grouping (as veto on the title tiers and in splitting the content
//! and identifier buckets) and again while merging evidence into groups, so
//! even a chain `A~B`, `B~C` cannot put an exempt `A`,`C` in one group. A
//! group whose members are all mutually exempt disappears; a partly exempt one
//! splits.
//!
//! Two books are exempt when, for one exemption, one matches member `m1` and
//! the other a *different* member `m2` (a book "matches" a member by uuid or
//! by a shared file hash). So a book is never exempt from its own re-import.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Mutex;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::cache::Cache;
use crate::dedupe::BookRecord;

/// The library preference holding every exemption.
pub const PREF_KEY: &str = "lexmechanic.duplicate_exemptions";

/// A hash shared by more books than this does not exempt them all (a stock
/// file imported hundreds of times would otherwise make a quadratic edge set).
const MAX_HASH_BUCKET: usize = 20;

/// For each book id, the books it may never be grouped with.
pub type CannotLink = HashMap<i32, HashSet<i32>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExemptMember {
    pub uuid: String,
    /// BLAKE3 hex of each non-empty format file when it was marked.
    #[serde(default)]
    pub hashes: Vec<String>,
    /// For the "Dismissed" list: the title when it was marked.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exemption {
    /// Hash of the sorted member uuids: marking the same books twice is one entry.
    pub id: String,
    pub members: Vec<ExemptMember>,
    /// Unix seconds.
    pub created_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Document {
    #[serde(default)]
    exemptions: Vec<Exemption>,
}

/// Serializes read-modify-write of the preference within this process.
static WRITE: Mutex<()> = Mutex::new(());

/// The id of an exemption (and of a duplicate group) over `uuids`.
pub fn set_id(uuids: &[String]) -> String {
    let mut sorted: Vec<&str> = uuids.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let mut h = blake3::Hasher::new();
    for u in sorted {
        h.update(u.as_bytes());
        h.update(b"\n");
    }
    h.finalize().to_hex().to_string()[..16].to_string()
}

fn load(cache: &Cache) -> Result<Document> {
    match cache.get_preference_compat(PREF_KEY)? {
        Some(raw) if !raw.trim().is_empty() => Ok(serde_json::from_str(&raw).unwrap_or_default()),
        _ => Ok(Document::default()),
    }
}

fn save(cache: &Cache, doc: &Document) -> Result<()> {
    if doc.exemptions.is_empty() {
        cache.clear_preference_compat(PREF_KEY)
    } else {
        cache.set_preference_compat(PREF_KEY, &serde_json::to_string(doc)?)
    }
}

/// Every exemption, oldest first.
pub fn list(cache: &Cache) -> Result<Vec<Exemption>> {
    let mut all = load(cache)?.exemptions;
    all.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    Ok(all)
}

/// Non-empty format hashes by book id.
fn hashes_by_book(cache: &Cache) -> Result<HashMap<i32, Vec<String>>> {
    let mut out: HashMap<i32, Vec<String>> = HashMap::new();
    for ((book, _format), (hash, size, _)) in cache.checksums().all_format_records()? {
        if size > 0 {
            out.entry(book).or_default().push(hash);
        }
    }
    for v in out.values_mut() {
        v.sort();
        v.dedup();
    }
    Ok(out)
}

/// Marks every pair among `book_ids` as "not a duplicate". At least two
/// distinct books are needed. Marking the same set again returns the existing
/// entry. Unknown ids are an error.
pub fn add(cache: &Cache, book_ids: &[i32]) -> Result<Exemption> {
    let mut ids: Vec<i32> = book_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() < 2 {
        bail!("an exemption needs at least two different books");
    }
    let hashes = hashes_by_book(cache)?;
    let mut members = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(uuid) = cache.book_uuid(id)?.filter(|u| !u.is_empty()) else { bail!("no book with id {id}") };
        let title = cache.field_for(id, "title")?.unwrap_or_default();
        let authors = cache.author_names_for_book(id).unwrap_or_default();
        members.push(ExemptMember { uuid, hashes: hashes.get(&id).cloned().unwrap_or_default(), title, authors });
    }
    let uuids: Vec<String> = members.iter().map(|m| m.uuid.clone()).collect();
    let id = set_id(&uuids);
    let _guard = WRITE.lock().unwrap();
    let mut doc = load(cache)?;
    if let Some(existing) = doc.exemptions.iter().find(|e| e.id == id) {
        return Ok(existing.clone());
    }
    let exemption = Exemption { id, members, created_at: crate::trash::now_secs() };
    doc.exemptions.push(exemption.clone());
    save(cache, &doc)?;
    Ok(exemption)
}

/// Removes one exemption; whether it existed.
pub fn remove(cache: &Cache, id: &str) -> Result<bool> {
    let _guard = WRITE.lock().unwrap();
    let mut doc = load(cache)?;
    let before = doc.exemptions.len();
    doc.exemptions.retain(|e| e.id != id);
    let removed = doc.exemptions.len() != before;
    if removed {
        save(cache, &doc)?;
    }
    Ok(removed)
}

/// Removes the exemptions that a user-made group has overruled: those whose
/// members are **all** among `group_uuids` and of which at least one is in
/// `touching` (the books the user just put in the group). An exemption that
/// also covers books outside the group is left alone (it says something about
/// those too); a manual group is never split by an exemption in any case (see
/// [`crate::dedupe_manual_groups`]). Returns how many were removed.
pub fn remove_within(cache: &Cache, group_uuids: &[String], touching: &[String]) -> Result<usize> {
    let group: HashSet<&str> = group_uuids.iter().map(String::as_str).collect();
    let touched: HashSet<&str> = touching.iter().map(String::as_str).collect();
    let _guard = WRITE.lock().unwrap();
    let mut doc = load(cache)?;
    let before = doc.exemptions.len();
    doc.exemptions.retain(|e| !(e.members.iter().all(|m| group.contains(m.uuid.as_str())) && e.members.iter().any(|m| touched.contains(m.uuid.as_str()))));
    let removed = before - doc.exemptions.len();
    if removed > 0 {
        save(cache, &doc)?;
    }
    Ok(removed)
}

/// Removes every exemption; how many there were.
pub fn clear(cache: &Cache) -> Result<usize> {
    let _guard = WRITE.lock().unwrap();
    let doc = load(cache)?;
    let n = doc.exemptions.len();
    if n > 0 {
        save(cache, &Document::default())?;
    }
    Ok(n)
}

/// The "may never share a group" relation for `records` (which must carry
/// their `uuid`). Empty when nothing is exempt, which is the common case and
/// costs one preference read.
pub fn cannot_link(cache: &Cache, records: &[BookRecord]) -> Result<CannotLink> {
    let exemptions = load(cache)?.exemptions;
    if exemptions.is_empty() {
        return Ok(CannotLink::new());
    }
    let hashes = hashes_by_book(cache)?;
    Ok(cannot_link_from(&exemptions, records, &hashes))
}

/// [`cannot_link`] over already-loaded inputs.
pub fn cannot_link_from(exemptions: &[Exemption], records: &[BookRecord], hashes: &HashMap<i32, Vec<String>>) -> CannotLink {
    let by_uuid: HashMap<&str, i32> = records.iter().filter(|r| !r.uuid.is_empty()).map(|r| (r.uuid.as_str(), r.id)).collect();
    let in_library: HashSet<i32> = records.iter().map(|r| r.id).collect();
    let mut by_hash: HashMap<&str, Vec<i32>> = HashMap::new();
    for (book, list) in hashes {
        if in_library.contains(book) {
            for h in list {
                by_hash.entry(h.as_str()).or_default().push(*book);
            }
        }
    }
    let mut out = CannotLink::new();
    for ex in exemptions {
        let matched: Vec<BTreeSet<i32>> = ex
            .members
            .iter()
            .map(|m| {
                let mut set = BTreeSet::new();
                if let Some(&id) = by_uuid.get(m.uuid.as_str()) {
                    set.insert(id);
                }
                for h in &m.hashes {
                    if let Some(ids) = by_hash.get(h.as_str()).filter(|ids| ids.len() <= MAX_HASH_BUCKET) {
                        set.extend(ids.iter().copied());
                    }
                }
                set
            })
            .collect();
        for i in 0..matched.len() {
            for j in (i + 1)..matched.len() {
                for &a in &matched[i] {
                    for &b in &matched[j] {
                        if a != b {
                            out.entry(a).or_default().insert(b);
                            out.entry(b).or_default().insert(a);
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dedupe::{scan_library_full, Strength};

    fn add_book(dir: &std::path::Path, cache: &Cache, name: &str, title: &str, author: &str, bytes: &[u8]) -> i32 {
        let source = dir.join(name);
        std::fs::write(&source, bytes).unwrap();
        let mut meta = lexmechanic_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec![author.to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    fn groups(cache: &Cache) -> Vec<Vec<i32>> {
        scan_library_full(cache, Strength::Fuzzy).unwrap().groups.into_iter().map(|g| g.book_ids).collect()
    }

    /// Three books, one title and author, three different files.
    fn three(dir: &std::path::Path, cache: &Cache) -> [i32; 3] {
        [
            add_book(dir, cache, "a.txt", "The Great Test", "Ada Lovelace", b"alpha"),
            add_book(dir, cache, "b.txt", "The Great Test", "Ada Lovelace", b"bravo"),
            add_book(dir, cache, "c.txt", "The Great Test", "Ada Lovelace", b"charlie"),
        ]
    }

    #[test]
    fn marking_a_whole_group_makes_it_disappear_and_removing_brings_it_back() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cache = Cache::new(lib.path()).unwrap();
        let [a, b, c] = three(src.path(), &cache);
        assert_eq!(groups(&cache), vec![vec![a, b, c]]);

        let ex = add(&cache, &[a, b, c]).unwrap();
        assert!(groups(&cache).is_empty());
        assert_eq!(list(&cache).unwrap().len(), 1);
        // Marking the same books again is the same entry.
        assert_eq!(add(&cache, &[c, b, a]).unwrap().id, ex.id);
        assert_eq!(list(&cache).unwrap().len(), 1);

        assert!(remove(&cache, &ex.id).unwrap());
        assert!(!remove(&cache, &ex.id).unwrap());
        assert_eq!(groups(&cache), vec![vec![a, b, c]]);
    }

    #[test]
    fn a_partial_exemption_splits_the_group() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cache = Cache::new(lib.path()).unwrap();
        let [a, b, c] = three(src.path(), &cache);
        add(&cache, &[a, b]).unwrap();
        assert_eq!(groups(&cache), vec![vec![a, c]]);
        assert_eq!(clear(&cache).unwrap(), 1);
        assert_eq!(groups(&cache), vec![vec![a, b, c]]);
    }

    #[test]
    fn one_book_is_not_enough() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cache = Cache::new(lib.path()).unwrap();
        let [a, ..] = three(src.path(), &cache);
        assert!(add(&cache, &[a]).is_err());
        assert!(add(&cache, &[a, a]).is_err());
        assert!(add(&cache, &[a, 9999]).is_err());
    }

    #[test]
    fn exemptions_survive_reopening_the_library() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        {
            let cache = Cache::new(lib.path()).unwrap();
            let ids = three(src.path(), &cache);
            add(&cache, &ids).unwrap();
        }
        let reopened = Cache::new(lib.path()).unwrap();
        assert!(groups(&reopened).is_empty());
        assert_eq!(list(&reopened).unwrap().len(), 1);
    }

    #[test]
    fn exemptions_survive_a_rebuild_from_the_change_log() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cache = Cache::new(lib.path()).unwrap();
        let ids = three(src.path(), &cache);
        let made = add(&cache, &ids).unwrap();

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
        let after = list(&rebuilt).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, made.id);
        assert_eq!(after[0].members.len(), 3);
    }

    /// The same file imported again (a new uuid) is still exempt from the book
    /// it was marked against, because the exemption also holds its hash.
    #[test]
    fn a_reimported_identical_file_stays_exempt() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cache = Cache::new(lib.path()).unwrap();
        let a = add_book(src.path(), &cache, "a.txt", "The Great Test", "Ada Lovelace", b"alpha");
        let b = add_book(src.path(), &cache, "b.txt", "The Great Test", "Ada Lovelace", b"bravo");
        add(&cache, &[a, b]).unwrap();
        assert!(groups(&cache).is_empty());

        crate::removal::delete_with_file(&cache, b).unwrap();
        let b2 = add_book(src.path(), &cache, "b2.txt", "The Great Test", "Ada Lovelace", b"bravo");
        assert_ne!(cache.book_uuid(b2).unwrap(), None);
        assert!(groups(&cache).is_empty(), "the re-import must stay exempt");

        // A different file with the same title is *not* covered.
        let d = add_book(src.path(), &cache, "d.txt", "The Great Test", "Ada Lovelace", b"delta");
        let g = groups(&cache);
        assert!(g.iter().any(|set| set.contains(&d)), "{g:?}");
    }

    #[test]
    fn empty_files_never_key_an_exemption() {
        let lib = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cache = Cache::new(lib.path()).unwrap();
        let a = add_book(src.path(), &cache, "a.txt", "Zero A", "Writer A", b"");
        let b = add_book(src.path(), &cache, "b.txt", "Zero B", "Writer B", b"");
        let ex = add(&cache, &[a, b]).unwrap();
        assert!(ex.members.iter().all(|m| m.hashes.is_empty()), "{ex:?}");
    }
}
