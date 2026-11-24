//! Writing a batch of enriched books in one transaction and one change-log
//! entry.
//!
//! The scan's second phase used to write each book field by field:
//! `set_field("title")`, `set_field("authors")` and `set_cover` were each an
//! autocommit (an fsync), each a change-log entry (two more fsyncs, the entry
//! file and its directory), and the book then left the queue in one more
//! write. At a hundred thousand books the commits and the field-cache reloads
//! each of them forced cost more than reading the files did.
//!
//! [`Cache::write_enrichment_batch`] decides what each book still needs in one
//! read, writes the cover files, then applies every book's changes (each in a
//! savepoint, so one bad book rolls back alone) in **one** `BEGIN IMMEDIATE`
//! transaction that also removes the books from the enrichment queue, and
//! finally records everything as one [`crate::change_log::ChangeOp::Batch`].
//! The ops in that batch are exactly the ones the per-field writes produced
//! (`FieldSet` for title and authors, `CoverSet` for the cover), so a rebuild
//! from the log gives the same library.
//!
//! Order is the usual one: database commit first, log second. A crash between
//! them leaves enriched books the log has not heard of, the same window the
//! batched registration of phase one has.

use anyhow::Result;
use rusqlite::OptionalExtension;

use crate::cache::Cache;
use crate::change_log::ChangeOp;
use crate::scan_pipeline::JobControl;

/// What a file offered for one book. Only the parts that still are
/// placeholders are written; see [`Cache::write_enrichment_batch`].
#[derive(Debug, Clone, Default)]
pub struct EnrichmentWrite {
    pub book_id: i32,
    /// What to call the book in a report (its folder and file name).
    pub label: String,
    /// The filename the book was titled from, which marks a title as still
    /// being the placeholder.
    pub stem: String,
    /// A real title from the file, already cleaned of junk.
    pub title: Option<String>,
    /// Real authors from the file, already cleaned. Empty for none.
    pub authors: Vec<String>,
    /// A cover image.
    pub cover: Option<Vec<u8>>,
}

struct Plan {
    book_id: i32,
    title: Option<String>,
    authors: Option<String>,
    cover: Option<(Vec<u8>, String)>,
}

impl Cache {
    /// Writes `writes` as described in the module docs and takes every one of
    /// those books off the enrichment queue, including a book with nothing to
    /// write or whose write failed (a rerun must not trip on it forever).
    ///
    /// Returns, per input, whether anything was written for the book. The
    /// time spent on cover files is added to `control`'s `covers` timing and
    /// the rest to `enrich_write`.
    pub fn write_enrichment_batch(&self, writes: &[EnrichmentWrite], control: Option<&JobControl>) -> Vec<Result<bool>> {
        if writes.is_empty() {
            return Vec::new();
        }
        let started = std::time::Instant::now();
        let fail_all = |e: anyhow::Error| -> Vec<Result<bool>> { writes.iter().map(|_| Err(anyhow::anyhow!("{e:#}"))).collect() };
        if let Err(e) = self.checksums().initialize() {
            return fail_all(e.into());
        }

        // 1. Decide, in one read. A title is replaced only while it is still
        //    the filename, authors only while they are still "Unknown", a
        //    cover only while there is none: whatever the user changed since
        //    phase one stays.
        let mut plans: Vec<Plan> = Vec::with_capacity(writes.len());
        {
            let conn = self.backend.conn.lock().unwrap();
            for w in writes {
                let mut plan = Plan { book_id: w.book_id, title: None, authors: None, cover: None };
                let row: Result<Option<(String, i64)>, rusqlite::Error> = conn.query_row("SELECT title, has_cover FROM books WHERE id = ?1", [w.book_id], |r| Ok((r.get(0)?, r.get(1)?))).optional();
                if let Ok(Some((title, has_cover))) = row {
                    if let Some(new) = w.title.as_deref().filter(|t| *t != "Unknown" && *t != w.stem) {
                        if title == w.stem {
                            plan.title = Some(new.to_string());
                        }
                    }
                    if !w.authors.is_empty() {
                        let current: Vec<String> = conn
                            .prepare_cached("SELECT a.name FROM books_authors_link l JOIN authors a ON a.id = l.author WHERE l.book = ?1 ORDER BY l.id")
                            .and_then(|mut stmt| stmt.query_map([w.book_id], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>())
                            .unwrap_or_default();
                        if current.is_empty() || (current.len() == 1 && current[0] == "Unknown") {
                            plan.authors = Some(w.authors.join(" & "));
                        }
                    }
                    if has_cover == 0 {
                        if let Some(bytes) = w.cover.as_ref().filter(|c| !c.is_empty()) {
                            plan.cover = Some((bytes.clone(), blake3::hash(bytes).to_hex().to_string()));
                        }
                    }
                }
                plans.push(plan);
            }
        }

        // 2. Cover files, outside any transaction: each is a journaled atomic
        //    write, and rewriting one after a crash is harmless.
        let cover_started = std::time::Instant::now();
        if plans.iter().any(|p| p.cover.is_some()) {
            match self.backend.write_handle() {
                Ok(handle) => {
                    for plan in plans.iter_mut() {
                        let Some((bytes, _)) = plan.cover.as_ref() else { continue };
                        let stored = (|| -> Result<()> {
                            let path = crate::covers::sidecar_cover_path(self, plan.book_id)?;
                            handle.write_atomic(&path, bytes)?;
                            if let Some(legacy) = crate::covers::legacy_cover_path(self, plan.book_id)?.filter(|p| p.is_file()) {
                                if let Err(e) = handle.remove_atomic(&legacy) {
                                    log::warn!("wrote the new cover for book {} but could not remove {}: {e}", plan.book_id, legacy.display());
                                }
                            }
                            Ok(())
                        })();
                        if let Err(e) = stored {
                            log::warn!("book {} was enriched but its cover could not be stored: {e}", plan.book_id);
                            plan.cover = None;
                        }
                    }
                }
                Err(e) => {
                    log::warn!("enriched books' covers could not be stored: {e}");
                    for plan in plans.iter_mut() {
                        plan.cover = None;
                    }
                }
            }
        }
        let cover_time = cover_started.elapsed();
        if let Some(control) = control {
            control.add_time("covers", cover_time);
        }

        // 3. Everything else, in one transaction.
        let mut results: Vec<Result<bool>> = Vec::with_capacity(plans.len());
        {
            let mut conn = self.backend.conn.lock().unwrap();
            let mut tx = match conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
                Ok(tx) => tx,
                Err(e) => return fail_all(e.into()),
            };
            for plan in &plans {
                let one = (|| -> Result<bool> {
                    let sp = tx.savepoint()?;
                    let mut changed = false;
                    if let Some(title) = &plan.title {
                        sp.execute("UPDATE books SET title = ?1 WHERE id = ?2", (title, plan.book_id))?;
                        changed = true;
                    }
                    if let Some(authors) = &plan.authors {
                        Cache::link_many_to_many_in(&sp, plan.book_id, "authors", "name", "books_authors_link", "author", " & ", authors)?;
                        // The same derivation `set_field("authors")` makes, which
                        // is what replaying the logged `authors` write does.
                        Cache::sync_author_sort_in(&sp, plan.book_id)?;
                        changed = true;
                    }
                    if changed {
                        Cache::touch_last_modified_in(&sp, plan.book_id)?;
                    }
                    if let Some((bytes, hash)) = &plan.cover {
                        crate::checksums::ChecksumStore::record_hash_on(&sp, plan.book_id, "cover", "", hash, bytes.len() as i64, None)?;
                        sp.execute("UPDATE books SET has_cover = 1 WHERE id = ?1", (plan.book_id,))?;
                        changed = true;
                    }
                    sp.commit()?;
                    Ok(changed)
                })();
                results.push(one);
            }
            for plan in &plans {
                if let Err(e) = tx.execute("DELETE FROM checksums_db.enrichment_queue WHERE book_id = ?1", (plan.book_id,)) {
                    // The books are written; only the queue row stays, and a
                    // rerun finds nothing left to add.
                    log::warn!("could not take book {} off the enrichment queue: {e}", plan.book_id);
                }
            }
            if let Err(e) = tx.commit() {
                return fail_all(e.into());
            }
        }

        // 4. One log entry for the lot.
        let mut ops: Vec<ChangeOp> = Vec::new();
        for (plan, result) in plans.iter().zip(&results) {
            if !matches!(result, Ok(true)) {
                continue;
            }
            let uuid = match self.book_uuid(plan.book_id) {
                Ok(Some(uuid)) => uuid,
                _ => continue,
            };
            if let Some(title) = &plan.title {
                ops.push(ChangeOp::FieldSet { book: uuid.clone(), field: "title".to_string(), value: Some(title.clone()) });
            }
            if let Some(authors) = &plan.authors {
                ops.push(ChangeOp::FieldSet { book: uuid.clone(), field: "authors".to_string(), value: Some(authors.clone()) });
            }
            if let Some((_, hash)) = &plan.cover {
                ops.push(ChangeOp::CoverSet { book: uuid, blob: Some(hash.clone()) });
            }
        }
        self.record_many(ops);

        if let Some(control) = control {
            control.add_time("enrich_write", started.elapsed().saturating_sub(cover_time));
        }
        results
    }
}
