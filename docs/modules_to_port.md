# Modules to port

## src/calibre

### ai

- [x] __init__.py
- [x] config.py
- [x] prefs.py
- [x] utils.py

#### github

- [x] __init__.py
- [x] backend.py
- [x] config.py

#### google

- [x] __init__.py
- [x] backend.py
- [x] config.py

#### lm_studio

- [x] __init__.py
- [x] backend.py
- [x] config.py

#### ollama

- [x] __init__.py
- [x] backend.py
- [x] config.py

#### open_router

- [x] __init__.py
- [x] backend.py
- [x] config.py

#### openai

- [x] __init__.py
- [x] backend.py
- [x] config.py

### customize

- [x] __init__.py
- [x] builtins.py
- [x] conversion.py
- [x] profiles.py
- [x] ui.py
- [x] zipplugin.py

### db

**See #201**: an audit found most of the `[x]` marks below were false —
`crates/calibre_db`'s core files are ~1-15% of their real Python
counterpart's size and, on inspection, silently stub out core behavior
(schema upgrades, search, sort, cache invalidation) rather than
implementing it. Corrected here; do not trust the remaining `[x]` marks
in this section without spot-checking against #201's methodology
either -- this pass wasn't exhaustive.

- [x] __init__.py (#218: `FTSQueryError` -> `calibre_db::errors::FtsQueryError` (real type, not wired into anything yet -- no FTS subsystem exists to raise it from, see #226). `_get_next_series_num_for_list`/`_get_series_values` -> `calibre_utils::series::get_next_series_num_for_list`/`get_series_values`, reading a new `series_index_auto_increment` field on `GlobalPrefs` (this crate conflates upstream's separate `tweaks`/`prefs` systems into one). `get_data_as_dict` -> `Cache::get_data_as_dict`, a JSON bulk export including custom columns and resolved cover/format paths. NOT ported: the `{label}_index` companion field for `series`-datatype custom columns (this crate's custom-column model has no per-datatype index sub-column at all); format-path resolution re-derives the filename `add_format` would use rather than reading it back from the `data` table's own `name` column, so a format added some other way with a different on-disk name wouldn't be found. Also fixed a real bug found while testing this: `Cache::add_book_db_entry` only ever linked a multi-author book's *first* author to `books_authors_link`, silently dropping the rest -- now links all of them.)
- [x] adding.py (#219: real bulk directory-import scanning -- filename filtering (`splitext`/`compile_glob`/`compile_rule`/`filter_filename`/`metadata_extensions`/`allow_path`), directory grouping into candidate books (`find_books_in_directory`, one-book-per-directory or grouped-by-filename-stem), and real orchestration on top (`import_book_directory`/`import_book_directory_multiple`/`recursive_import`, using `Cache::add_book`/`add_format` from #216). NOT ported: real per-format embedded-metadata extraction (`metadata_from_formats` -- title is derived from the filename instead, a disclosed simplification; this crate has per-format metadata *readers* scattered across `calibre_ebooks::metadata::*` but no unified dispatcher yet), duplicate detection (`find_identical_books` is real since #221 but not wired into this import path), `add_catalog`/`add_news` (need `Cache`-internal `_search`/`_create_book_entry`-style methods this crate doesn't have), import-plugin hooks (no plugin system to hook into). The pre-existing narrow `adding::add_book(cache, title, authors)` free function (DB-row-only, used by `copy_to_library.rs`) is unrelated and untouched.)
- [x] annotations.py (issue #485: `annotations_map_for_book`/`merge_annotations_for_book` -- real storage + the server-side merge algorithm (`merge_annotations`/`merge_annots_with_identical_field`, plain timestamp-string comparison, no CFI parsing needed -- see `calibre_db::annotations`'s module doc for why an earlier plan assuming `cfi_sort_key` was wrong). Backs `calibre_srv::books`'s `get-annotations`/`update-annotations` endpoints. NOT ported: `search_annotations` (FTS), `all_annotations`/`all_annotations_for_book`, `delete_annotations`/`update_annotations` (by-id write API), `restore_annotations`, `save_annotations_list` -- real upstream APIs with no caller in this crate yet, each a separate follow-up. Replaces a previous unused/untested `Bookmark`/`Highlight`/`Annotation` scaffold that had no real callers and couldn't represent `last-read`-type annotations anyway.)
- [x] backend.py (#203: real connection setup, custom SQL functions/collations/aggregates, brand-new-library schema creation via the bundled DDL, author-sort trigger fixup, `library_id`, and JSON-typed `preferences` get/set/delete -- all verified against upstream. NOT ported: default-pref population/migration, dynamic custom-column tables, the in-memory field/table model, notes/FTS bootstrap, trash dir, file/cover storage, library move/backup. See #204 for the field/table model, cache.py's territory. #93 follow-up: `Backend::write_handle` -- start of the crate-wide write-path retrofit onto `LibraryHandle` (issue #93). Lazily opens and caches a `LibraryHandle` on first write, shared across every clone of that `Backend`; deliberately NOT opened on every `Backend::new`/`Cache::new`, so opening a library read-only stays safe to do any number of times. `covers::set_cover`/`backup::backup_metadata` converted so far; `LibraryHandle::remove_atomic` (a real, journaled, recovery-aware delete primitive for a file or directory, recursively -- the journal previously only had `WriteFile`/`RenameFile`) is now wired into `cache.rs`'s `remove_format`/`delete_book` and `restore.rs`'s stale-`metadata_pre_restore.db` cleanup. `LibraryHandle::copy_atomic` (a large-file-safe write for a caller with an already-formed source file, e.g. a book format being added -- streams both its hashing and copying passes instead of buffering the whole file, unlike `write_atomic`) is now wired into `cache.rs`'s `add_format`; journal recovery's own write verification was switched to the same streaming hash so a large recovered file doesn't get buffered into memory either. `cache.rs`'s `rename_book_files` is now converted too -- every individual directory/file rename and the empty-old-directory cleanup go through `rename_atomic`/`remove_atomic`, though the directory-move-then-file-renames-then-DB-update sequence as a whole still isn't one atomic transaction (this crate's journal has no multi-operation batch concept spanning several `LibraryHandle` calls; each call is independently crash-safe, same as every other conversion here). `notes/connection.rs`'s `add_resource`/`remove_unreferenced_resources` are converted too, which also fixed a real pre-existing bug: a failed resource write used to be silently swallowed while the DB still recorded the resource as present (`NotesConnection` gained a `Backend` field to reach the same lazily-opened handle rather than risk a second, colliding `LibraryHandle::open`). The crate-wide write-path retrofit is now done: `restore.rs`'s `restore_database` holds the writer lock for its entire run (not just around the `metadata.db` backup rename), since the long book-rescanning loop after the rename is not one SQL transaction and needs real isolation from a concurrent writer through a different `Backend`/`Cache` -- a concurrent write attempt during a restore now fails fast with `AlreadyLocked` instead of racing the rebuild, the same tradeoff a real database's `VACUUM`/`REINDEX` makes. See `library_handle.rs`'s module doc for the full retrofit summary. #93 is now closed; remaining fault-tolerance work split into #257-#260. #257 (§6 network-storage writes): per-operation safety is done -- `write_atomic`/`copy_atomic`/`rename_atomic`/`remove_atomic` all stage payloads in a local scratch dir, upload/rename, read back and verify via BLAKE3, and retry with exponential backoff (60s cap, 5min total budget) when the handle's tier is `Network`; the multi-file two-phase batch case is now done too: `LibraryHandle::begin_network_batch`/`NetworkBatch` journal a whole set of related renames/removals (§6's own example: a book move) as one entry before any of them run, then drive them forward to completion in order -- `cache.rs`'s `rename_book_files` uses it when the handle's tier is `Network`. Still explicitly out of scope: folding the `metadata.db` update into that same atomic unit (would need `LibraryHandle` to gain a SQLite connection it doesn't have, issue #260's territory) and real validation against an actual network mount (#262-#265). #260 (§3 SQLite WAL discipline): `Backend::new` and every sidecar's `ATTACH` (`checksums.db`/`notes.db`/`full-text-search.db`) now set `journal_mode=WAL`/`synchronous=FULL` for real, and `LibraryHandle::prepare_for_suspend` really checkpoints the WAL on suspend (§5 step 1's other half) by opening its own short-lived connection to each database file -- no connection between `LibraryHandle` and any live `Backend` needed, since SQLite lets any connection checkpoint a database's WAL. The tier-aware per-write checkpoint cadence (previously deferred) is now done too, via a real SQLite `commit_hook` registered once on the shared connection (covers all 66+ write call sites without touching any of them -- it only increments a counter) plus a background thread that polls the counter and performs the actual checkpoint once due (every write on `Network`, every 32 writes/5s on local-internal). Real design constraint worth recording: an earlier attempt that checkpointed synchronously *from inside* the commit hook deadlocked (confirmed with a standalone reproduction) since SQLite's writer lock isn't released until after the hook returns -- so this is asynchronous (up to `poll_interval`, 200ms in production, after the triggering write) rather than strictly "before the write is acked" as §3's literal wording asks, a real, disclosed, user-approved relaxation. #259 (§5 step 2 blocking-with-timeout): `Shared::check_open` now really blocks up to 30s waiting for `resume()` while `Suspended` instead of failing immediately, via a `Condvar` paired with the state lock -- `set_state` notifies it on every transition (so a `Detached` transition while blocked wakes it immediately too, with the right error, rather than waiting out the timeout); `Detached` itself still fails fast with no blocking when already in that state.)
- [x] backup.py
- [x] cache.py (#204: real `all_book_ids`, real `field_for` for every standard field -- scalar `books` columns, `comments`, `series`/`publisher`/`rating` many-to-one, `authors`/`tags`/`languages`/`formats` many-to-many, `identifiers`, `size` -- and real `pref`/`set_pref` including namespaced keys, all verified against the real schema. Multi-value fields return a joined string, not upstream's tuple/dict shape -- a disclosed simplification, see `cache.rs`'s module docs. NOT ported: writes beyond `write.rs`, notes, FTS, composite fields, virtual libraries, saved searches, categories, trash, dump/restore, `move_library_to` -- each its own follow-up. Also fixed real bugs in `search.rs`/`view.rs` that depended on this: `search.rs` had misleading stale comments describing a hardcoded id-range fallback that was never actually implemented (the real query was already correct); `view.rs::sort()` previously ignored its `field` argument entirely and always sorted by internal id. #214 (#201/#212 follow-up) later added real custom-column support (`Cache::add_custom_column`/`get_custom_column_value`/`set_custom_column_value`/`remove_custom_column`/`custom_column_label_map`) -- a `custom_columns` metadata row plus a dynamic `custom_column_N` value table per column, the same shape `library.rs` originally hand-rolled, just hosted on `Cache`'s connection now so `Library` can delegate instead of duplicating the SQL. This is NOT a port of upstream's real `tables.py`/`fields.py` custom-column architecture (see those entries below) -- it's a narrower, same-shape extension of this file's existing per-call-SQL strategy, not the in-memory bulk-loaded `Table` object model. #216 (same follow-up chain) then added real filesystem book/format/cover/rename/clone management (`Cache::add_book`/`add_format`/`remove_format`/`delete_book`/`update_book_metadata`/`clone_to`), again moved from `library.rs`'s original duplicate implementation -- with two real gaps fixed along the way: `add_format` (and `add_book`'s own initial file copy, which used to bypass it) never inserted a row into the `data` table, so `field_for(id, "formats"/"size")` never saw a book's real formats; `covers.rs::set_cover` never flipped `has_cover` in the `books` table. With #212/#214/#216 landed, `Library` and `Cache` share one schema and one connection, and every method they both had is now `Library` delegating to `Cache` via `as_cache()` rather than duplicating SQL/file logic -- `Library` itself is now mostly a thin CLI-facing wrapper (argument/error-type translation) plus the `Book` struct-returning read methods (`get_book`/`list_books`) `Cache` doesn't have an equivalent of yet. #222 phase 3 (later) cut `field_for` itself over from per-call SQL to reading a lazily-loaded, auto-invalidated in-memory snapshot (`tables.rs`/`fields.rs`) -- see those entries and `cache.rs`'s own module doc for that story; the value shape/field set described above is unchanged.)
- [x] categories.py (#220: real `authors`/`tags`/`series`/`publisher`/`languages` categories -- real per-item book count and average rating (nonzero ratings, star scale), computed via one join query per category against the real schema. All 3 real sort modes (`name`/`popularity`/`rating`) supported, matching upstream's tie-breaking. `Library::get_categories` (previously its own separate authors-only implementation) now delegates to this instead of duplicating. NOT ported: field-metadata-driven category discovery (custom columns, composite categories -- no `field_metadata` system exists, same recurring gap as #210/#214/#216/#218), user categories, grouped-search-term consolidation, hierarchical categories, the `ratings` category itself (folded into every other category's `avg_rating` instead), the `search` category from saved searches. `sort_key_for_name`'s real ICU collation is approximated as case-insensitive string comparison, same as elsewhere in this crate.)
- [x] constants.py
- [x] copy_to_library.py (#221: real duplicate detection -- `check_duplicates: true` now builds real author/title maps from the destination library and skips (`Ok(None)`) on a same-author/near-same-title match via `crate::utils::find_identical_books` (was real and tested, just never wired in). Also fixed the source book's author list to query the real `authors`/`books_authors_link` tables instead of treating the whole `author_sort` string as one author name. NOT ported: format-file copying (the `add_book` helper this calls still doesn't set a book's `path`, so there's nowhere to copy files to yet), the real `duplicate_action` enum's `add_formats_to_existing` automerge path, conversion-options/annotations copying.)
- [x] covers.py
- [x] errors.py
- [x] fields.py (#222 phase 2: `FieldStore`, a real in-memory typed field-access layer wrapping `tables.rs`'s `StandardTables`, reproducing every `Cache::field_for` match arm against bulk-loaded data instead of per-call SQL. This is what `Cache::field_for` (phase 3) actually delegates to now. Replaces the previous `Field` trait/`BasicField` skeleton, which had no real callers anywhere. Custom-column field wrapping still isn't ported -- `cache.rs`'s own `get_custom_column_value`/`set_custom_column_value` (#214) are unaffected/untouched by this.)
- [x] lazy.py
- [x] legacy.py (#223: `LegacyDb` now really delegates to `Cache`/`View` (upstream's `new_api`/`data`) for id/index resolution, the full metaprogrammed getter API, a setter API backed by the new `Cache::set_field`, format/cover/identifier accessors, `get_categories`/`find_identical_books`/`get_data_as_dict`, item-management (`all_*_names`/`get_*_with_ids`/`*_name`/`delete_*_using_id`/`rename_*`), directory import (delegates to `adding.rs`, #219), and a few standalone helpers. NOT ported: saved searches (no such subsystem exists at all), `field_metadata`/composite fields (`standard_field_keys` etc. return a hardcoded list, the same recurring gap as elsewhere), `allow_case_change`/dirtying/notification/conversion-options/custom-book-data/on-device state, and the real `create_book_entry`/`add_books`/`import_book` (uses this crate's already-narrower `adding.rs`/`copy_to_library.rs` building blocks as-is). This crate's own pre-existing, non-upstream `LegacyDB::check_compatibility`/`migrate` stub is unrelated and untouched. See `legacy.rs`'s module doc.)
- [x] listeners.py
- [x] locking.py
- [x] restore.py (#224: real book-id preservation -- `Cache::add_book_db_entry_with_id` re-inserts each restored book with the original id recovered from its OPF's embedded `calibre` identifier instead of a fresh autoincrement id; every field `parse_opf` recovers is now applied via `Cache::set_field` (#223), not just title/author_sort/uuid; format files sitting in each book's directory are rediscovered and re-registered (upstream's `is_ebook_file`/first-by-mtime-per-extension dedup, ported faithfully); `cover.jpg` sets `has_cover`; a bad OPF in one directory is recorded and skipped rather than aborting the whole restore (`RestoreReport`). Also fixed `backup.rs::backup_metadata`, which the round-trip test exercises: it queried the real `authors` table instead of treating the whole `author_sort` string as one author name. NOT ported: preferences/`field_metadata`/custom-column-definition restore (no such subsystem or backup sidecar exists), the staged temp-directory + atomic-swap strategy (restores in place, same as before this pass), notes/`link_maps`/annotations restore, and the background-thread execution model. See `restore.rs`'s module doc.)
- [x] schema_upgrades.py (all 25 migration steps (versions 1-26) ported and verified end-to-end against a hand-derived version-1 starting schema. NOT ported: `upgrade_version_19`'s custom-recipe-to-file export (no recipes/feeds subsystem exists) and `upgrade_version_24`'s FTS reindex trigger (no FTS pipeline exists) -- neither makes schema/data changes, so skipping their bodies doesn't desync later steps. `upgrade_version_10`/`_11`'s field_metadata-driven tag-browser-view creation is ported for the fixed set of built-in categorized fields only, not custom columns (not supported yet). See schema_upgrades.rs's module docs.)
- [x] search.py (#210, #201 follow-up: real `DateSearch`/`NumericSearch`/`BooleanSearch`/`KeyPairSearch` matchers plus a real `And`/`Or`/`Not`/`Token` tree evaluator matching upstream's exact candidate-narrowing semantics, wired to `Cache::field_for`'s fixed field set via a hardcoded location-alias table (no `field_metadata` system exists in this crate). NOT ported: `template:`/`@usercategory`/`vl:`/`search:savedname` search, the `#=N` count operator, grouped-search-terms expansion, the `LRUCache` query cache, and real ICU primary-collation matching (`ACCENT_MATCH`/primary `CONTAINS_MATCH` are NFD-diacritic-stripping approximations). Follow-up #212 unified `crates/calibre_db/src/library.rs`'s schema with `Backend`'s real bundled one and wired `Library::search()` to this engine for real -- the CLI `search` subcommand now uses it, see `library.rs`'s module docs.)
- [x] sqlite_extension.cpp (semantic port: tokenizer + stemmer; FTS5 registration deferred to #93)
- [x] tables.py (#222: real `read()` bulk-loading for every standard field -- `OneToOneTable`/`SizeTable`/`UuidTable`/`ManyToOneTable`/`RatingTable`(incl. its real rating=0 cleanup)/`ManyToManyTable`/`AuthorsTable`/`FormatsTable`/`IdentifiersTable`, matching `Cache::field_for`'s exact field set (including fixing `IdentifiersTable`/`ManyToManyTable` to preserve real join order -- `IndexMap` + explicit `ORDER BY`/a real `languages` `item_order` param -- once something actually consumed that order). Phase 3 wires this into `Cache::field_for` for real: a lazily-loaded, per-`Cache`-instance snapshot, auto-invalidated via SQLite's own `total_changes()` counter rather than manually-instrumented call sites (robust against every write path in the crate, including raw-SQL test fixtures, with zero missed-invalidation risk). `crate::view::View::sort` and every field access in `search.rs` already called `Cache::field_for` themselves, so both benefit automatically -- no separate rearchitecture of either was needed. The mutation side (`remove_books`/`remove_items`/`rename_item`/`set_links`/`fix_link_table`/`fix_case_duplicates`) still isn't ported -- writes go through `Cache`'s existing real per-call-SQL write methods, which invalidate this read-side snapshot rather than updating it incrementally in place. See `tables.rs`/`cache.rs`'s module docs for the full story.)
- [x] utils.py
- [x] view.py (`sort()`'s `field` argument was fixed to be used for real as part of #204 -- this line was stale, corrected 2026-08-19; previously wrongly left unchecked describing a bug that was already gone)
- [x] write.py (#225: real field-name-dispatched value adapters (`adapt_single_text`/`adapt_multiple_text`/`adapt_bool`/`adapt_rating`/`adapt_series_index`/`adapt_languages`/`clean_identifier`, ported from `get_adapter`/the `Writer` classes) in front of `Cache::set_field` (#223), including `title`/`author_sort` empty-value fallbacks and `uuid`/`timestamp`/`sort`/`pubdate`'s "reject empty rather than clear" rule (`Writer.accept_vals`). `set_field`/`set_field_many` skip a write entirely when the adapted value already matches what's stored, matching every upstream writer function's "ignore items whose value is the same as the current value" check (a real gap before this pass), and return the real dirtied-book-id set. `series` now supports the combined `"Name [3]"` syntax via `calibre_utils::series::get_series_values` (#218, previously unused). NOT ported: real per-field `datatype`/`is_multiple` dispatch (needs #222's later phases; dispatches by hardcoded field name instead, same simplification `Cache::set_field` already made), custom-column datatype adapters, enum-field allowed-value filtering, real locale-database language canonicalization. See `write.rs`'s module doc.)

#### cli

- [ ] __init__.py
- [x] `cmd_add_custom_column.py` -> `crates/calibre_db/src/cli/cmd_add_custom_column.rs`
- [x] cmd_add_format.py
- [x] cmd_add.py
- [x] `cmd_backup_metadata.py` -> `crates/calibre_db/src/cli/cmd_backup_metadata.rs`
- [x] cmd_catalog.py (Partial/Skeleton -- CSV only, `title`/`author_sort`/`pubdate`/`isbn`/`path`-the-string; anything else is a hard "unsupported format" error. #93 §8 follow-up: checked whether it needed checksum re-verification wiring, same as `cmd_export.rs`'s -- it doesn't. `cmd_catalog.rs` never opens a format file, cover, or any other on-disk book file; it only writes DB metadata field values, so there's no book-file byte read here to verify against. Disclosed in `checksums.rs`'s module doc, which had previously *assumed* otherwise without checking -- corrected. Real upstream `catalog.py` (unported, see below) generates richer formats that do embed cover thumbnails; that would be a real checksum-verification site if ever ported.)
- [x] `cmd_check_library.py` (Partial/Skeleton) -> `crates/calibre_db/src/cli/cmd_check_library.rs` (#93 §8 follow-up: `check_library.rs`'s per-book scan now also verifies each present format/cover file's content against a real BLAKE3 checksum recorded at add time, reporting real mismatches as `corrupted_formats`/`corrupted_covers` -- printed as new "Corrupted book formats"/"Corrupted cover files" sections. Fixed a real pre-existing bug found while wiring this in: `check_library.rs`'s own `IGNORE_AT_TOP_LEVEL` set held placeholder `".trash"`/`".notes"` strings that never matched the real `crate::constants::TRASH_DIR_NAME`/`NOTES_DIR_NAME` (`.caltrash`/`.calnotes`), so `scan_library` was spuriously flagging every real trash/notes directory as an invalid top-level entry; also added `.calibre-oxide` itself, missing since #93 phase 1 introduced it. See `checksums.rs`'s module doc for the full checksum-store design.)
- [x] cmd_clone.py
- [x] cmd_custom_columns.py
- [x] cmd_embed_metadata.py
- [x] cmd_export.py (#93 §8 follow-up: `CmdExport::run` now re-verifies each book's format file against its recorded BLAKE3 checksum before copying it out -- a real mismatch skips exporting that one book, printed to stderr, rather than copying corrupted bytes; a book with no recorded checksum exports unchanged, same as before this pass. No "convert" command exists anywhere in this repo that resolves a book_id/format through `Library`/`Cache` -- `calibre_conversion`'s `ebook_convert` binary is a standalone file-to-file converter with no library concept -- so there was nothing to wire re-verification into on that side; see `checksums.rs`'s module doc.)
- [x] cmd_fts_index.py (#226: real `status`/`enable`/`disable`/`reindex`, backed by `Library::is_fts_enabled`/`set_fts_enabled`/`fts`. Renamed from `cli/cmd_fits_index.rs` -- "fits" was a typo for "fts". NOT ported: `--wait-for-completion`/the `wait` action and indexing-rate reporting, since this crate has no background indexing pipeline to report progress on -- see `fts/connection.rs`'s module doc)
- [x] cmd_fts_search.py (#226: real `--restrict-to ids:/search:` (the latter via `Library::search`, #210), snippet/highlight markers, stemming toggle, `text`/`json` output, indexing-threshold gate. Renamed from `cli/cmd_fits_search.rs`. NOT ported: upstream's cross-format snippet-dedup grouping in text output)
- [x] cmd_list_categories.py
- [x] cmd_list.py
- [x] `cmd_remove_custom_column.py` -> `crates/calibre_db/src/cli/cmd_remove_custom_column.rs`
- [x] cmd_remove_format.py
- [x] cmd_restore_database.py
- [x] cmd_saved_searches.py
- [x] cmd_search.py (#212: now backed by the real query-syntax engine via `Library::search` -- see the `search.py`/`search.rs` entry above)
- [x] cmd_set_custom.py
- [x] cmd_set_metadata.py
- [x] cmd_show_metadata.py
- [x] cmd_switch.py
- [x] main.py
- [x] tests.py (ported as unit tests in `cli/cmd_check_library.rs`; extracted `write_check_library_results` for testability)
- [x] utils.py

#### fts

- [x] __init__.py
- [x] connect.py (#226: real schema creation (`dirtied_formats`/`books_text`/two FTS5 virtual tables) and sync triggers, dirty-set management, `add_text`/`unindex`/`vacuum`, and `search` (real dynamic SQL: single-/multi-id `restrict_to_book_ids` via a temp table, `snippet()`/`highlight()`, real `FtsQueryError` from #218 finally wired up). NOT ported: the custom `calibre`/`porter` FTS5 tokenizers -- both virtual tables use the built-in `unicode61` tokenizer instead, since registering them requires #93's still-pending rusqlite FTS5 extension wiring; `unicode_normalize`. See `fts/connection.rs`'s module doc.)
- [x] pool.py (#226: deliberately NOT ported -- `Pool`/`Worker` spawn a subprocess per book/format to extract text out-of-process via `calibre.db.fts.text.main`; this crate has no ebook-text-extraction pipeline to spawn in the first place. `FtsConnection::add_text` -- the real DB write once text is available -- is ported and is exactly what a future extraction pipeline would call.)
- [x] schema_upgrade.py
- [x] text.py

#### notes

- [x] __init__.py
- [x] connect.py (#227: real schema (`notes`/`resources`/`notes_resources_link`/two FTS5 virtual tables) and sync/cascade-delete triggers; core CRUD (`set_note`/`get_note`/`get_note_data`/`rename_note`/`delete_field`/`items_with_notes_for_field`/`all_items_with_notes`); resource attachment storage with real content-addressed dedup and orphaned-file cleanup (`add_resource`/`get_resource_data`/`remove_unreferenced_resources`); real search (`all_notes`/`search`, the latter raising the real `FtsQueryError` from #218/#226). Wired up as `Library::notes()`, same pattern as `Library::fts()` (#226). NOT ported: the retire/backup/undo-trail (a note delete is just a delete, no recovery history), `.metadata` JSON sidecars, `field_metadata`/`supports_notes` gating (no such subsystem, the recurring #201 gap), export/restore integration, and the custom `calibre`/`porter` FTS5 tokenizers (same #93 dependency as #226 -- `unicode61` fallback). `export_note`/`import_note` stay out of scope here, tracked as #228 below. See `notes/connection.rs`'s module doc.)
- [x] exim.py (#228: real `export_note`/`import_note` via `calibre_ebooks::mobi::dom::Dom` (html5ever-backed, same pattern `oeb::transforms::data_url` already uses for near-identical img-src rewriting). `export_note` expands `resource://` placeholders into real `data:` URLs; `import_note` stores base64 `data:` URLs and local files (relative to `basedir` or absolute) as real resources via `NotesConnection::add_resource`, rewriting `src` to a `resource://` placeholder -- including upstream's real path-traversal containment check (a resolved local-file path must stay inside `basedir`). NOT ported: upstream's `del img.attrib['src']` pass in `import_note`, which serializes before it runs and so has no effect on either return value -- skipped as genuine dead code, same observable behavior. See `notes/exim.rs`'s module doc for the rest.)
- [x] schema_upgrade.py

### devices

- [x] __init__.py
- [x] cli.py
- [x] errors.py
- [x] interface.py
- [x] mime.py
- [x] misc.py
- [x] scanner.py
- [x] udisks.py
- [x] utils.py
- [x] winusb.py

#### android

- [x] __init__.py
- [x] driver.py

#### folder_device

- [x] __init__.py
- [x] driver.py

#### kindle

- [x] __init__.py
- [x] driver.py
- [x] apnx.py
- [x] bookmark.py

##### apnx_page_generator

- [x] __init__.py
- [x] i_page_generator.py
- [x] page_group.py
- [x] page_number_type.py
- [x] pages.py

###### generators

- [x] __init__.py
- [x] accurate_page_generator.py
- [x] exact_page_generator.py
- [x] fast_page_generator.py
- [x] pagebreak_page_generator.py

#### kobo

- [x] __init__.py
- [x] driver.py (Stub)
- [x] bookmark.py (Stub)
- [x] books.py (Stub)
- [x] db.py (Stub)
- [x] kobotouch_config.py (Stub)

#### libusb

- [x] libusb.c (ported to `calibre_devices::libusb` using pure-Rust `nusb`)

#### mtp

- [ ] __init__.py
- [ ] base.py
- [ ] books.py
- [ ] defaults.py
- [ ] driver.py
- [ ] filesystem_cache.py
- [ ] test.py

##### unix

- [ ] __init__.py
- [ ] devices.c
- [ ] devices.h
- [ ] driver.py
- [ ] libmtp.c
- [ ] sysfs.py

###### upstream

- [ ] device-flags.h
- [ ] music-players.h
- [ ] update.py

##### windows

- [ ] __init__.py
- [ ] content_enumeration.cpp
- [ ] device_enumeration.cpp
- [ ] device.cpp
- [ ] driver.py
- [ ] global.h
- [ ] wpd.cpp

#### nook

- [x] __init__.py
- [x] driver.py

#### smart_device_app

- [x] __init__.py
- [x] driver.py

#### usbms

- [x] __init__.py
- [x] books.py
- [x] cli.py
- [x] device.py
- [x] deviceconfig.py
- [x] driver.py

#### usbobserver

- [x] usbobserver.c (Deferred: macOS-only IOKit/CoreFoundation extension. Out of scope per user's Windows/Linux/Android platform decision. `get_usb_devices` is superseded by cross-platform `calibre_devices::libusb`.)

#### userdefined

- [x] __init__.py
- [x] driver.py

### ebooks

- [x] __init__.py
- [x] BeautifulSoup.py (ported to `beautiful_soup.rs` — preprocess pipeline; html5ever parser wiring is a follow-up)
- [x] chardet.py (ported to `chardet.rs` — chardetng + encoding_rs)
- [x] constants.py (already ported to `constants.rs`)
- [ ] covers.py (deferred — Qt image generation; needs Tauri wiring, will land with the app cover generator)
- [x] css_transform_rules.py (issue #117 CLOSED -- `css_transform_rules.rs`: a real match-a-CSS-property's-value (exact/negated/regex/numeric-with-unit-conversion), then remove/change/append/arithmetic-transform rules engine over `crate::css::model::StyleDeclarationBlock`. `transform_container` wired directly against the already-real `crate::oeb::polish::css::transform_css`, needing no new container-walking logic at all. Shorthand-property expansion (matching e.g. `margin-top` against a compact `margin: 0`) reuses `crate::oeb::normalize_css::normalize_edge`, covering the same five shorthands `normalize_filter_css` already does; other shorthands (`font`/`background`/`list-style`/non-edge `border`) pass through unexpanded, the same disclosed narrowing `oeb::polish::css::filter_css` already has. Real, non-obvious upstream behavior confirmed by reading the source and locked in with a test: a normalizable shorthand's own compact name (e.g. `property: "margin"`) is NEVER directly matchable by a rule -- real upstream's own declaration iterator yields ONLY the expanded longhand names for a normalizable property, never the raw shorthand. `\N`-style regex backreferences in a `change` action's replacement text are translated from Python `regex.sub` syntax to Rust `regex` syntax. `export_rules`/`import_rules`' real plain-text rule-file format ported and round-trip tested, matching `html_transform_rules.rs`'s (#118) own precedent)
- [x] html_entities.c (ported to `html_entities.rs` — `html-escape` crate supersedes the C lookup table)
- [x] html_entities.h (ported — the 5000-line table is now `html-escape`'s embedded WHATWG data)
- [x] html_entities.py (ported to `html_entities.rs`)
- [x] html_transform_rules.py (issue #118 CLOSED -- `html_transform_rules.rs`: a real match-by-tag/class/CSS-selector/text-content, then rename/remove/unwrap/re-class/re-attr/wrap/insert rules engine over `crate::dom::Dom`, reusing the real `crate::css` selector matcher (issue #164) rather than needing the separate top-level `css_selectors` module port. Added a new, reusable `Dom::clone_from` (splices a parsed HTML snippet's subtree into a different `Dom`) for the `insert`/`insert_end`/`prepend`/`append` actions; every mutation action turned out simpler against this crate's own standard-DOM tree (text as ordinary child nodes) than lxml's `.text`/`.tail` mixed-content model the real functions were written against -- `unwrap_tag` in particular is exactly the pre-existing `Dom::remove_promoting_children`, needing no new code at all. Real `transform_container` integration point wired against `oeb::polish::container::Container`. Disclosed narrowings: `match_type: "xpath"` (arbitrary user XPath) has no engine to run it and is a real reported error; `match_type: "*"`/`"contains_text"` are ported via the real CSS universal selector and a direct own-text tree predicate respectively, matching real semantics without needing an XPath engine; GUI display strings (`ACTION_MAP`/`MATCH_TYPE_MAP`'s tooltip text) are dropped except a short label kept for `export_rules`' human-readable comment lines; `transform_conversion_book` (OEBBook/conversion-pipeline integration) is out of scope, no rules-based conversion input plugin exists in this port yet. `export_rules`/`import_rules` real plain-text (not JSON) rule-file format ported and round-trip tested)
- [x] hyphenate.py (ported to `hyphenate.rs` — `hyphenation` crate with Knuth-Liang patterns)
- [ ] render_html.py (deferred — Qt WebEngine; new viewer is a Tauri panel)
- [ ] tweak.py (deferred — CLI tool needing libunzip + TempDir wiring, its own PR)
- [x] uchardet.c (superseded by `chardet.rs` via chardetng — pure Rust replaces C wrapper)

#### azw4

- [ ] __init__.py
- [ ] reader.py

#### chm

- [x] __init__.py
- [x] reader.py (ported to `chm/reader.rs` using pure-Rust `libchm`; TOC/HHC parsing deferred to #121 html5ever wiring)
- [x] metadata.py (ported into `metadata/chm.rs::get_metadata` — now uses `ChmReader` instead of returning default)

#### comic

- [x] __init__.py
- [x] input.py (archive extraction + page enumeration; Qt-based PageProcessor rendering pipeline deferred to follow-up issue)


#### conversion

- [x] __init__.py
- [x] archives.py
- [x] cli.py (ported into `calibre_conversion::cli_helpers` — input/output validation, USAGE banner, HEURISTIC_OPTIONS. Plugin-driven option assembly deferred to #126; issue #476 wired `check_command_line_options` into `calibre_conversion`'s `ebook_convert` binary for real, replacing its previous ad hoc clap-only arg handling)
- [x] config.py (ported into `calibre_conversion::config` — GuiRecommendations JSON roundtrip, OPTIONS registry, format-preference helpers, atomic save/load. DB-integrated `*_specifics` deferred to #93 LibraryHandle. Registry not yet wired into `Plumber` — issue #476's own disclosed follow-up)
- [x] issue #476 (foundational, blocked #429): found and removed `calibre_conversion`'s own second, thinner, untested conversion architecture (`OebBook`/`InputPlugin`/`OutputPlugin`/`ConversionPipeline`/`plugins::{epub_input,epub_output}`) — it duplicated, with a strictly worse content model (a leaked temp dir, no NCX/TOC generation), what `calibre_ebooks::conversion::plumber::Plumber` already did, tested, and reused elsewhere (`oeb::iterator::book::extract_book`, issue #38). `calibre_conversion::bin::ebook_convert` now calls `Plumber` directly (real dynamic format dispatch across all ~19/~15 already-ported input/output formats, verified end to end with a real EPUB→EPUB and EPUB→TXT conversion via the built binary — previously hardcoded to EPUB→EPUB only). AZW3 *output* remains genuinely unimplemented anywhere in the crate (a real, pre-existing gap, not introduced or masked by this issue)
- [x] plumber.py
- [x] preprocess.py
- [x] search_replace.py
- [x] utils.py

##### plugins

- [ ] __init__.py
- [x] azw4_input.py (Wrapper)
- [x] chm_input.py (Regex/Placeholder)
- [x] comic_input.py
- [x] djvu_input.py (issue #129: wired for real -- extracts the DjVu file's OCR text via `djvu::file::DjvuFile::text()`, HTML-izes it via `txt::processor::convert_basic` (same as the plain-text pipeline), then feeds the result through the real `HTMLInput` to build the OEB, matching upstream's own 4-step `convert`. Errors for a text-less/pure-page-scan file, matching upstream's own `ValueError`. Disclosed narrowing: real upstream's own "set metadata from file" step is a practical no-op for DjVu even in real calibre (no registered `metadata/djvu.py` reader plugin) -- this port still sets a real filename-derived title after the `HTMLInput` call purely to replace `HTMLInput::convert`'s own unrelated hardcoded placeholder title, a separate pre-existing gap in that file, not fixed here. No longer the "DJVU Content Not Supported Yet" placeholder page)
- [x] docx_input.py
- [x] docx_output.py (Stub)
- [x] epub_input.py
- [x] epub_output.py
- [x] fb2_input.py
- [x] fb2_output.py (issue #457: `output::fb2_output::FB2Output::convert` was previously an unwired, ad-hoc regex-based converter separate from the real `fb2::fb2ml::Fb2Mlizer` port -- a "marked done but silently wrong" gap of the same class #201 flagged for `calibre_db`. Now rewired to call `Fb2Mlizer::extract_content` for real, matching `rtf_output.rs`'s own #50 rewiring pattern (real `TagStylizer`, real `DefaultImageConverter`). Also found and fixed that `conversion::plumber.rs` never actually dispatched `.fb2` output at all -- added the missing `output_ext == "fb2"` branch alongside `"rtf"`'s own. Disclosed narrowing shared with every other output plugin in this crate: `SVGRasterizer`/`linearize_jacket` transforms aren't wired at the output-plugin level anywhere yet, not new to this fix)
- [x] html_input.py (issue #536 fixed a real bug: `HTMLInput::convert` hardcoded the book title to the unrelated literal `"Converted Log"` regardless of the real HTML's own `<title>` -- now uses the root file's own title, which `html::input::get_filelist`'s `HtmlFile::title` already extracts (falling back to the file's stem, matching real upstream's own fallback chain narrowed to just this piece, not a separate filename-metadata-pattern parser). Real, still-open gap, NOT fixed here (separate, larger piece of work): the file's own inline `TODO` for real link rewriting between crawled pages -- files are copied as-is, so relative links between pages can still break)
- [x] html_output.py
- [x] htmlz_input.py
- [x] htmlz_output.py
- [x] lit_input.py
- [x] lit_output.py
- [x] lrf_input.py
- [x] mobi_input.py
- [x] mobi_output.py
- [x] `odt_input.py` -> `input/odt_input.rs`
- [x] `odt_output.py` -> `output/odt_output.rs`
- [x] oeb_output.py
- [x] pdb_input.py
- [x] pdb_output.py
- [x] `pdf_input.py` -> `input/pdf_input.rs`
- [x] `pdf_output.py` -> `output/pdf_output.rs`
- [x] pml_input.py
- [x] pml_output.py
- [x] rb_input.py
- [x] rb_output.py
- [x] recipe_input.py (Placeholder)
- [x] rtf_input.py
- [x] snb_input.py
- [x] snb_output.py
- [x] tcr_input.py
- [x] `tcr_output.py` -> `output/tcr_output.rs`
- [x] txt_input.py (issue #537 fixed: `TXTInput::convert` was ad-hoc -- a crude "is this markdown" content sniff, `pulldown_cmark` regardless of real formatting, and a hand-rolled HTML escaper -- instead of this crate's own real, already-ported `txt::processor` pipeline. Now real: BOM/`chardet`-based encoding detection (with the same gb2312-family-to-gbk override real upstream applies inline), `xml_replace_entities`, `normalize_line_endings`, real `detect_paragraph_type`/`detect_formatting_type` auto-detection dispatching to the real `separate_paragraphs_*`/`block_to_single_line` transforms and to `convert_markdown_with_metadata`/`convert_textile`/`convert_basic`, then delegates through the real `HTMLInput`, matching `djvu_input.rs`'s (#129) established "HTML-ize then delegate" shape. `.md`/`.markdown`/`.textile` extensions force their formatting type and skip paragraph reformatting, matching upstream exactly. Real Markdown front-matter metadata (title/authors) is used when present. Disclosed narrowings: `.txtz` zip-archive bundles are not supported (no driving need yet); `formatting_type == 'heuristic'` needs no special handling here since real upstream's own `txt_input.py` ALSO just calls `convert_basic` for it, only flagging a separate later `HeuristicProcessor` pipeline stage this crate doesn't have wired at all; `paragraph_type == 'unformatted'` falls back to the same handling as `'single'` since `HeuristicProcessor`/`DocAnalysis`'s real punctuation-based line-unwrap preprocessing doesn't exist in this port; `fix_resources` (local image relocation for Markdown/Textile output) isn't implemented; no CLI-options threading exists yet (issue #126) so `formatting_type`/`paragraph_type` are always auto-detected, never user-overridden)
- [x] txt_output.py
- [x] zip_input.py
- [x] rar_input.py (Placeholder)

#### djvu

- [x] `__init__.py` -> `djvu/mod.rs`
- [x] `bzzdecoder.c` -> `djvu/bzz.rs`
- [x] `djvu.py` -> `djvu/file.rs`
- [x] `djvubzzdec.py` -> `djvu/bzz.rs`

#### docx

- [x] `__init__.py` (`InvalidDOCX`) -> `docx/error.rs`
- [x] `block_styles.py` -> `docx/block_styles.rs`
- [x] `char_styles.py` -> `docx/char_styles.rs`
- [x] cleanup.py (fully ported to `docx/cleanup.rs` — vertical-align wrapping, noteref spacing, hr-relocation, span merging/lifting/dir-normalization, bold/italic simplification, page-break conversion, and cover-image detection (`detect_cover`, using the already-ported `calibre_utils::imghdr::identify`) — incl. a reproduced bug (the final mergeable-span run is never flushed) and reuse of `Dom::remove_promoting_children` for `lift`/sole-span unwrapping — not yet wired into an orchestrator, see #130/#291)
- [x] `container.py` -> `docx/container.rs` (issue #139/#3 fixed a real bug: `get_relationships` stripped an absolute (`/`-prefixed) `Target`'s leading slash but still joined it onto the part's own directory anyway, so `/word/styles.xml` inside `word/` resolved to `word/word/styles.xml`. Now resolves absolute targets from the package root directly. `read_styles.rs`'s own independent `word/word`-prefix compatibility fallback, matching real upstream's `get_name`, is untouched -- it's a separate, still-valid defensive layer for malformed documents in general, not specific to this bug)
- [x] `dump.py` -> `docx/dump.rs`
- [x] fields.py (fully ported — pure field-instruction parsing half in `docx/fields.rs` (`Field`/scanner/`parse_hyperlink`/`parse_xe`/`parse_index`/`parse_ref`/`parse_noteref`, cross-validated against Python's own `test_parse_fields` cases), plus `FieldsCollector` (the source-tree-only half of the `Fields` orchestrator: the stack-based `w:fldChar`/`w:fldSimple` walk collecting fields, dispatching each by name to the parsers above, producing `hyperlink_fields`/`xe_fields`/`index_fields`), plus three functions in `to_html.rs` completing the orchestrator once the HTML tree exists: `assign_xe_anchors` (the deferred half of `parse_xe`'s synthetic bookmark — stamps a real id directly, no `state.anchor_map` entry needed since nothing else ever looks up an `XE` field's internal-only anchor name), `resolve_links` extended with the `hyperlink_fields` block it always had a documented gap for, and `apply_index_fields` (calls `docx/index.rs`'s `process_index`/`polish_index_markup` for each `INDEX` field once every anchor exists, near the end of `convert_document` matching where Python's own `Fields.polish_markup` runs). The open architectural question this was blocked on (real `crate::xmltree` migration vs. a side-table for `parse_xe`'s synthetic bookmark) resolved the same way `index.py`'s `process_index` (#293) did: no real tree mutation needed anywhere in the entire docx port, in the end. One disclosed, deliberate divergence from Python survives: since `process_index` only runs after the whole document is walked rather than inline during Python's own single pass, an `INDEX` field here sees every `XE` field in the document, not just ones dispatched earlier in Python's own linear pass. Closes #290 — the last remaining piece of the entire #22/#130 effort. See #130/#288/#290)
- [x] fonts.py (fully ported to `docx/fonts.rs` — `is_symbol_font`/`map_symbol_text`/`SYMBOL_MAPS`, plus `Family`/`Embed`/`panose_to_css_generic_family`/`get_variant` and the `Fonts` struct itself (`call`, reading `fontTable.xml`; `family_for`, resolving a run's CSS `font-family`; `embed_fonts`/`write`, extracting+writing whichever embedded fonts got used) — and fully wired: `read_styles.rs` reads `fontTable.xml` (`RawParts::fonts_table`/`fonts_table_rels`, `wire_parts`'s new `fonts`/`fonts_table_root` params), `Styles::resolve_run` calls `family_for` as its own last step (matching Python exactly), and `Styles::generate_css` (`styles.py`'s `generate_css`) assembles the full `docx.css` document, written by `write_document` before the manifest scan so it's picked up by it. The believed system-font-scanner blocker turned out to gate only one narrow helper, `has_system_fonts` (an `altName`-preference check inside `Family`) — stubbed to always return `false`, a real Python code path (`except NoFonts: return False`), not a hypothetical. See #22)
- [x] `footnotes.py` -> `docx/footnotes.rs`
- [x] images.py (fully ported to `docx/images.rs` — geometry/CSS, the `Images` struct (real embedded-image extraction from the DOCX zip/linked files, deduplicated naming, resizing via `oeb::transforms::rescale::fit_image`), and `pic_to_img`/`drawing_to_html`/`pict_to_html`/`to_html` (the `w:drawing`/`w:pict` → `<img>` markup generators) — plus `resolve_links`'s `images.links` block and `Images::to_html` wired directly into `convert_run`'s `w:drawing`/`w:pict` handling, both in `to_html.rs` — two upstream bugs reproduced faithfully, an EMF-to-raster conversion gap disclosed (no Rust EMF parser) — see #130/#289)
- [x] index.py (fully ported to `docx/index.rs` — `get_applicable_xe_fields`/`split_up_block`/`find_match`/`add_link`/`merge_blocks`/`polish_index_markup`, incl. a real reproduced `split_up_block` bug (wrong `ldict` depth from a shadowed loop variable), cross-validated by hand-tracing the real algorithm; plus `process_index` (with private `make_block`/`add_xe` helpers) — turned out NOT to need the mutable *source* tree after all: since this port builds HTML directly rather than re-walking a mutated source tree the way Python's own pipeline does, `process_index` just builds the equivalent `<p>`/`<a>` HTML straight into `crate::dom::Dom`, no `crate::xmltree` needed. `fields.py`'s `parse_xe` reframes the same way — see #130/#290, now closed. `process_index` takes already-resolved `XeField`s as input, wired into the real `Fields` orchestrator by `to_html.rs`'s `apply_index_fields`)
- [x] `lcid.py` -> `docx/lcid.rs`
- [x] `names.py` -> `docx/names.rs`
- [x] numbering.py (`Level`/`NumberingDefinition`/`Numbering` reading, plus `Level::css`/`char_css`, in `docx/numbering.rs`; `apply_markup` ported as `docx/to_html.rs`'s `apply_numbering_markup` — `Level::css` omits `list-style-image` for picture bullets, deferred to `images.py` (#289) — see #130/#286)
- [x] `settings.py` -> `docx/settings.rs`
- [x] styles.py (`PageProperties`/`Style` and the full `Styles` paragraph/run cascade orchestrator ported to `docx/styles.rs`; `Styles.cascade` ported as `docx/to_html.rs`'s `cascade`; `generate_css` ported and wired — see #22/#130)
- [x] tables.py (`RowStyle`/`CellStyle`/`TableStyle` and full `Table`/`Tables` row/cell/paragraph style resolution in `docx/tables.rs`; `apply_markup` (HTML `<table>`/`<tr>`/`<td>` construction) ported as `docx/to_html.rs`'s `apply_table_markup`/`apply_tables_markup` — `handle_merged_cells`'s tracked exclusion set (`removed_cells`) is honoured there — see #130/#286)
- [x] `theme.py` -> `docx/theme.rs`
- [x] to_html.py (`docx/to_html.rs`'s real port -- `convert_run`/`convert_p`/`convert_body`/`read_block_anchors`/`mark_block_runs`/`convert_footnotes`/`apply_tab_indentation`/`resolve_links`/`cascade`/`apply_tables_markup`/`apply_numbering_markup`/`apply_block_run_frames`/`apply_paragraph_frames`/`assign_style_classes`/`cleanup_markup`/`create_toc`, a `convert_document` orchestrator sequencing all of that in `Convert.__call__`'s real order, `Convert.read_styles`'s package-loading step (`docx/read_styles.rs`'s `read_raw_parts`/`wire_parts`), `Convert.write`'s OPF/NCX/index.html/docx.css output (`build_html_document`/`write_document`, built on a crate-wide `opf_writer.rs` relocated out of `mobi/` plus `Toc::to_ncx_toc`; `write_document` now also calls `Styles::generate_css` and writes `docx.css` before the manifest scan, matching Python's real write order), images wired into the whole body/paragraph/run/footnote walk via `Images::to_html`, and `convert_docx_document` -- a real top-level entry point (`Convert.__call__`'s full equivalent, including its own `self.write(doc)` call at the end) that opens a package, resolves/parses every part, runs `convert_document`, and returns `write_document`'s `metadata.opf` path. **Wired into `input/docx_input.rs`, replacing the old provisional `DOCXToHTML` sketch (deleted) and its `HTMLInput`-delegation path** -- `DOCXInput::convert` now reads the real OPF back via `oeb::reader::OEBReader::read_opf` + `oeb::container::DirContainer`, the same general OPF-based book loader `EPUBInput` already used (no docx-specific adapter needed after all -- the earlier assumption that one was needed turned out wrong once this was actually investigated). `resolve_alternate_content` is now ported too (`AlternateContent`/`resolve_alternate_content`, called up front by `convert_document`) -- as a tracked side-table (`excluded_choices`/`substitutions`) rather than real tree mutation, the same established precedent as `tables.rs`'s `Table::removed_cells`, since this crate's `roxmltree` pipeline is read-only: every `mc:Choice` next to a `mc:Fallback` is excluded from `convert_p`'s main body/note walk (avoiding duplicate content), and the common single-fallback-drawing/pict-inside-a-run case unwraps directly in `convert_run`. `fields.py`'s `Fields` orchestrator is ported too (#290, closed, also #288's own last remaining piece) -- `assign_xe_anchors`/`apply_index_fields` complete it once the HTML tree exists, extending `resolve_links` with the `hyperlink_fields` block it always had a documented gap for. Turned out NOT to need the source-tree mutation this was blocked on either, the same reframing `index.py`'s `process_index` demonstrated. **With this, the entire #22/#130 docx port effort is complete** except one disclosed, narrow-scope gap: a document relying on an exotic (non-drawing/pict) `mc:AlternateContent` structure converts without full fidelity there (`resolve_alternate_content` only special-cases the common case). See #130/#288.)
- [x] toc.py (`from_headings`/`from_toc`/`structure_toc`/`link_to_txt`/`create_toc` and a `Toc`/`TocNode` arena port of `calibre.ebooks.metadata.toc.TOC` in `docx/toc.rs`, plus a `Toc::to_ncx_toc` adapter into the crate-wide `metadata::toc::TOC` shape consumed by `opf_writer::write_ncx` — see #130/#288)

##### writer

- [x] `__init__.py` -> `docx/writer/mod.rs`
- [x] `container.py` -> `docx/writer/container.rs` (plus `writer/xml.rs`, the element builder replacing `lxml.builder`)
- [x] `fonts.py` -> `docx/writer/fonts.rs`
- [x] from_html.py (see #132 — its "needs a real OEB stylizer" premise was stale: `oeb/polish/cascade.rs`'s real CSS cascade already existed, just for a different consumer (issue #164); `oeb/polish/style.rs` now ports the accessor half of `stylizer.py`'s `Style` class against it — `get`/`own`/`item`/`color`/`background_color`/`font_size`/`line_height`/`effective_text_decoration`/`first_vertical_align`/`is_hidden`/`width`/`height`/margins/paddings, plus `dom`/`resolved`/`profile` accessors added to let a caller holding only a `Style` reconstruct/pass through its underlying context without three extra parameters. `TextRun`, `Block`, `Blocks`, `lang_for_tag`, and the core `Convert` element walker (`create_block_from_parent`/`add_block_tag`/`add_inline_tag`/`process_tag`/`process_item`) all ported to `docx/writer/from_html.rs`, resolving `cascade::resolve_styles` (already real, `oeb/polish/stats.rs` already uses it) per element via `oeb::polish::pretty`'s existing `leading_text`/`dom_tail` helpers for lxml's text/tail model. `Blocks` now has real table support too — its own `tables_arena`/`open_tables` stack (delegating into `tables.rs`'s now-real `Tables::table_start_new_row`/`table_start_new_cell`/`table_finish_tag`/`table_add_table`), `finish_tag`'s table-closing branch (nesting a finished table into a still-open outer table, or appending it to `Blocks`' own top-level `items: Vec<ItemId>` — a real `Block(BlockId)`/`Table(TableId)` union now, not just `Vec<BlockId>`), and a real `ItemContainer` field on `Block` (`enum ItemContainer { Top, Cell(CellId) }`, Python's `parent_items`, dropped when `Block` was first ported since nothing needed it without real per-cell item lists) so `delete_block_at`/`apply_page_break_after` can tell which items-list a block actually lives in (page-break-after propagation is a top-level-only concept in Python — two blocks sharing a cell's items list do NOT count, now correctly enforced instead of the earlier "always true" placeholder). **`process_tag`'s table-`display` dispatch is now wired too** (`table`/`table-row`/`table-cell`, calling `Blocks::start_new_table`/`::start_new_row`/`::start_new_cell` directly, matching Python's own three-way `if` inside the `display.startswith('table')` branch) — real table content now flows through the whole walker end to end, verified with an integration test walking a real `<table><tr><td>` structure and checking the resulting `Cell`'s own `items` holds the right `Block`. **`tagname == 'img'` is now wired up too** — `add_block_tag`/`add_inline_tag` call `ImagesManager::add_image` for real, via a new `images_manager: &mut ImagesManager`/`names: &DocxNamespace` pair of fields on `ProcessCtx` (which grew a second lifetime parameter, `ProcessCtx<'a, 'b>`, since `ImagesManager<'b>`'s own `&'b OEBBook` borrow is unrelated to `ProcessCtx`'s other `'a` borrows — `names` is needed because, unlike every other `DocxNamespace` use in this file, `<img>` markup is built immediately during the walk, not deferred to serialize time) — closing the LAST documented `todo!()` in this file (the table one closed earlier). `<hr>`'s border-suppression override is a disclosed, narrower gap (no `Style` mutation capability exists). **`Blocks::serialize` is now ported too** (`Blocks.serialize`, walking `self.items` and, for each `ItemId::Block`/`ItemId::Table`, calling `Block::serialize` directly or delegating into `tables.rs`'s `Tables::serialize_table` via a `serialize_block` closure — the same closure-based seam `tables.rs`'s own serialize methods already established, see that entry below); `own_style_id` is resolved per block via the new `StylesManager::pure_block_style_label` helper (mirroring `combined_style_label`'s already-established position-based-label pattern), and `is_first_float_block`/`is_last_float_block` are a disclosed, narrow simplification (always `false`) pending the still-open float-group-membership design question below — they only affect floated blocks, the uncommon case. **`Convert.write` (the final assembly step) is now ported too**, as a free function `write(writer: &mut DocxWriter, blocks: &Blocks, styles_manager: &StylesManager, links_manager: &mut LinksManager, lists_manager: &ListsManager, images_manager: &ImagesManager)` — `DocxWriter::new`'s own `create_skeleton` call already builds `document`/`styles`, so `write` only fills them in: `blocks.serialize` into the body, moving the skeleton's placeholder `w:sectPr` to the end via a new `Element::move_child_to_end` primitive (port of lxml's `body.append(body[0])`, which re-parents an existing child rather than duplicating it — `xml.rs`'s own `append` always adds a NEW child, so this needed its own method), `links_manager.serialize_toc` when `links_manager.has_toc()` (a new accessor — `serialize_toc` unconditionally adds a "Table of Contents" heading even with zero entries, so the caller must gate the call, matching Python's own `if self.links_manager.toc:` guard) with `styles_manager.primary_heading_style_label()` (a new accessor mirroring `pure_block_style_label`'s position-based-label pattern, since `primary_heading_style()` only returns the `CombinedStyle` value, not its final `w:styleId` string), then `styles_manager.serialize`/`images_manager.serialize`/`lists_manager.serialize` into their respective parts. Two things `Convert.write` also does are deliberately NOT ported here, each a real, separate blocker: `images_manager.write_cover_block` (needs a real resolved cover image, which needs `Convert.__call__`'s cover-resolution flow) and `fonts_manager.serialize` (needs the set of font families actually used, extracted from `styles_manager`'s interned styles, plus embedded-font-face discovery from the manifest — neither exists yet; omitting embedded fonts is still valid OOXML, Word just falls back to whatever `font-family` name is already written into each style). **`Convert.__call__` itself is NOW ported too**, as `convert(oeb: &OEBBook, opts: &PageOptions, mi: &MetaInformation, add_toc: bool) -> DocxWriter` — the real multi-item spine walk. Per-item CSS cascade resolution reuses `oeb::transforms::flatcss::resolve_document_styles` (not `oeb::polish::cascade::resolve_styles`, which needs a `polish::Container` — a completely different, filesystem-backed object model from the plain `OEBBook` this whole module is built against, and not cheaply constructible from one; `flatcss.rs` already solved exactly this problem for the identical reason, confirmed by reading ITS OWN module docs before reusing it rather than reinventing a third cascade path). The pre-`write()` cleanup pass (`resolve_skipped`/`delete_block_at`/`apply_page_break_after`/`resolve_language`/`styles_manager.finalize`/`lists_manager.finalize`) runs for real; `lists_manager.finalize` and a NEW `Blocks::resolve_skipped_range` method are BOTH called once per spine item (not once globally across the whole book, unlike Python's literal structure) since a `NodeId` is only meaningful within the specific `Dom` it came from — `resolve_skipped_range` is `resolve_skipped`'s own pairwise walk, scoped to a `Range<usize>` of ONE item's own `Blocks::all_blocks()` positions, for the identical reason `ListsManager::finalize`'s signature already needed fixing. **A real correctness question resolved, not just wired around**: Python shares ONE `document_relationships` object by reference across `LinksManager`/`ImagesManager`/the package writer, but this port's managers each own a `Clone` (no `Rc<RefCell<...>>`); since `DocumentRelationships::add`'s ids are assigned sequentially by insertion order, two independent clones would silently collide on the SAME id for different targets (an image and an external hyperlink both getting `rId5`) — avoided because `ImagesManager`'s registrations all happen eagerly DURING the walk while `LinksManager`'s only happen LATER at `write()` time (`Blocks::serialize` → `Block::serialize` → `TextRun::serialize` → `serialize_hyperlink`, confirmed by grepping every real call site, not assumed), so `convert` re-seeds `links_manager` from `images_manager`'s final state (a new `LinksManager::set_document_relationships` setter) right before calling `write`, then feeds the combined result into `DocxWriter.document_relationships` for the real zip write. A real bug was caught by `convert`'s own end-to-end tests, not by inspection: an `ItemContext`'s block-position `Range<usize>` becomes STALE once `delete_block_at` shifts later positions, so `lists_manager.finalize` (which runs AFTER the delete pass) needs each item's actual `Vec<BlockId>` captured up front instead, immune to later shifts (`BlockId`s stay valid forever, unlike positions in `all_blocks`). **Cover-image embedding is NOW ported too** (`images.py`'s own last piece, see that entry above) — `convert` resolves `oeb.metadata`'s `"cover"` term to a manifest id/href, reads it via the already-real `ImagesManager::read_image`, and builds real markup via `ImagesManager::create_cover_markup` after the cleanup pass (matching Python's own ordering exactly), passed into `write` as a new `cover_image: Option<Element>` parameter that `write_cover_block`s in LAST (after any TOC), so the cover ends up as the document's very first content. A new `PageOptions::preserve_cover_aspect_ratio: bool` field carries that conversion option through. Deliberately NOT ported (see `convert`'s own doc comment for exactly why each is disclosed, not silently dropped): SVG rasterization (this crate's `SvgRasterizer`, issue #162, only has the rasterization-cache half) and font embedding (`FontsManager::serialize` is fully ported but has no used-family-extraction/manifest-font-face-discovery glue yet — a document with no embedded fonts is still valid OOXML). Neither blocks producing a real, valid `.docx`; proven end to end by tests that run `convert` on a real multi-item `OEBBook` with headings/a list/an image/a cover, write the result to a real zip, and read it back through this crate's OWN, independently-built `docx` reader (issue #22/#130). The `Style`/`Stylizer` subclasses (subsumed by `oeb/polish/style.rs`/`cascade.rs`) are the only remaining unported piece of this file, and they're moot (nothing needs them). **`from_html.py` is now FULLY CLOSED OUT.**)
- [x] images.py (see #132; also needs `pt_to_emu` from the reader's `images.py`, already ported — #130. The self-contained utility layer — `get_image_margins`, `create_filename`, `create_docx_image_markup` — is ported to `docx/writer/images.rs`, plus `Style.img_size`/`img_dimension` ported to `oeb/polish/style.rs`. `ImagesManager`'s data-source half is ported too: `Image` (narrowed to an `href: String` instead of holding a whole `ManifestItem` — matches this effort's established "narrow to what's read" pattern) and `read_image`/`read_svg` (manifest lookup with a urlquote-fallback, real byte content via `OEBBook.container.read(href)`, format/dimension detection via the already-ported `calibre_utils::imghdr::identify`, a corrupted-image fallback to calibre's bundled `images/blank.png` via `calibre_utils::resources::get_image_path`, and relationship registration via the already-ported `DocumentRelationships::add_image`) — **resolving the real image-content-source design question**: confirmed by grepping every other writer in this crate (`mobi::writer2`/`writer8`, `oeb::transforms::{guide,data_url,htmltoc,metadata}`, `input::{rb,snb}_input`) that they all read manifest item bytes via `OEBBook.container: Box<dyn oeb::container::Container>`'s `.read(href)` — the SAME abstraction `oeb::reader.rs` uses to load the OPF itself. This is a DIFFERENT `Container` than `oeb::polish::container::Container` (the filesystem-backed "Polish Book" editor, issue #163) despite the identical name — confirmed by reading both definitions, not assumed. **`create_image_markup`/`add_image` are NOW ported too**: the floating/margin decision logic (CSS `float`, `margin:auto`-on-both-sides centering for a block image, and the "inline image alone inside a block" promotion via the parent's `display`/`text-align`, all properties with no dedicated `Style` `@property` so read via `Style::get` — port of `Style._get` — matching this effort's domName-dispatch discipline), a new `Floating` enum (`Left`/`Right`/`Center`) standing in for Python's `'left' | 'right' | 'center' | None` string-or-`None`, and the real `wp:inline`/`wp:anchor` OOXML assembly (real vs. faked margins depending on whether the image ends up floated at all). `stylizer`/`html_img.getparent()`'s style is rebuilt via `Style::new(style.dom(), style.resolved(), style.profile(), parent)`, the same "reconstruct via `Style`'s own accessors" technique `Blocks::end_current_block` established (PR #345). `ImagesManager` gained real `count`/`page_width`/`page_height`/`svg_originals` fields (all previously deferred as "no real caller yet" — this PR is that caller); `svg_originals` itself stays permanently empty until `Convert.__call__`'s SVG-rasterization pass (unported) populates it. **Wiring into `from_html.rs`'s own `<img>`-tag `todo!()`s is NOW done too** (a separate integration PR, as planned — threaded a new `images_manager: &mut ImagesManager`/`names: &DocxNamespace` pair through `ProcessCtx`, touching every `process_tag`/`process_item` call site including every test's; matched this effort's own established "arena+algorithms PR, then integration PR" split, `tables.rs`'s `Cell`/`Row`/`Table` vs. `Blocks`' own wiring; `tables.rs`'s serialize methods vs. `Blocks::serialize`). **`serialize` is NOW ported too**: writes every embedded raster/SVG image's real bytes (re-read from `self.oeb.container` at serialize time rather than cached, matching Python's lazy `partial(self.get_data, ...)` callback's "don't hold every image in memory" property by a different means) into a `part name -> bytes` map — turned out this needed NO new capability at all, contrary to this entry's own earlier note: `docx/writer/container.rs`'s `DocxWriter.parts: BTreeMap<String, Vec<u8>>` (this crate's `DOCX` equivalent, "extra parts to write verbatim") already has exactly this shape and was already used for embedded fonts; confirmed by grepping every `.parts` call site before assuming a blocker. **`create_cover_markup`/`write_cover_block` are NOW ported too — the LAST piece of `images.py`, closing this file out completely.** Unlike `create_image_markup`, a cover has no floating/margin decision at all — always centered, always page-relative-anchored — so the only real logic is the aspect-ratio-preserving width/height rescale (`img.width >= img.height` picks which dimension gets recomputed from the other, matching Python's own branch exactly). `write_cover_block` is a free function (Python's own version reads nothing from `self` except the namespace, which every caller already has) needing no new `Element` primitive (`insert`-at-front/`find_descendant_mut`, both already real since `links.py`'s TOC half). The real cover-image RESOLUTION (`oeb.metadata.cover[0]` -> manifest id -> `ImagesManager::read_image`) lives in `from_html::convert`, not `images.rs` — a new `PageOptions::preserve_cover_aspect_ratio: bool` field (Python's own `docx_output.py` conversion option, `recommended_value=False`) was added to carry that option through. `SvgRasterizer`'s `svg_originals` tracking itself (not part of the existing `oeb::transforms::rasterize::SvgRasterizer` port, issue #162) remains the only unported piece anywhere near this file, and it's not this file's own scope. **`images.py` is now FULLY CLOSED OUT — all six files issue #132 originally tracked are complete.**)
- [x] links.py (see #132 — fully ported to `docx/writer/links.rs`: `start_text`, `sanitize_bookmark_name`, `TOCItem`/its `serialize`, and all of `LinksManager` (`__init__`, `bookmark_for_anchor`, `bookmark_id`, `serialize_hyperlink`, `process_toc_node`, `process_toc_links`, `serialize_toc`). `serialize_toc` needed `<w:body>` to support prepending a child, so `docx/writer/xml.rs`'s `Element` gained `insert`/`find_descendant_mut`. Not yet wired up: `LinksManager`'s only caller, `Convert.write` (`from_html.py`), isn't ported either)
- [x] lists.py (see #132 — `find_list_containers`, `Level`, `NumberingDefinition`, and `ListsManager` (`finalize`/`serialize`, `numbering.xml`'s `w:abstractNum`/`w:num`) all ported to `docx/writer/lists.rs`, against `Block.list_tag`/`.numbering_id` (already ported, PR #333) and `Style::own`/`.get` (already ported). One deliberate divergence: Python's `NumberingDefinition` dedup pass never actually fires (defines `__hash__` but not `__eq__`, so `dict`-based lookup falls back to object identity) — and would be a real bug if it ever did fire (the surviving definition's `link_blocks` would run instead of the discarded duplicate's, orphaning its blocks). This port matches Python's actual shipped (never-deduping) behavior, not its broken intent — see the module's own doc comment. `finalize`'s signature was later refined (issue #132's `Convert.__call__` orchestration work) to accept an explicit `block_ids: &[BlockId]` slice instead of scanning `Blocks::all_blocks()` itself, and to APPEND each call's `NumberingDefinition`s — continuing `num_id` from wherever a previous call left off — instead of overwriting `self.definitions`: since this port's `NodeId`s are only meaningful within their own `Dom` (unlike Python's globally-comparable lxml elements), a real multi-item document must call `finalize` once per spine item, each with that item's own `(dom, resolved, profile)` and just the block ids created while processing it, or two items' lists would misresolve against the wrong document or silently collide on `num_id`)
- [x] styles.py (see #132 — `TextStyle`, `BlockStyle`, `FloatSpec` (CSS -> `w:rPr`/`w:pPr`/`w:framePr`, incl. their own `serialize`/`serialize_properties`), and `DescendantTextStyle` (a run's `w:rPr` overrides relative to its containing block) all ported to `docx/writer/styles.rs`, against the `oeb/polish/style.rs` seam, plus `read_css_block_borders`/`parse_css_length`/`css_font_family_to_docx`/`convert_underline`/`LINE_STYLES`/`bmap`/`is_dropcaps`. `StylesManager`'s `create_text_style`/`create_block_style` dedup cache, `.finalize` (block/run-style pairing by weighted use, per-heading-tag outline-level promotion, descendant-style dedup — writing back onto `from_html.rs`'s `Block::linked_style`/`TextRun::parent_style`/`.descendant_style_id`), and now `.serialize` (writing every combined/descendant/pure-block style into a real `<w:styles>` element, plus overwriting the document-default `<w:lang>`'s attributes) are all ported, as an arena (`TextStyleId`/`BlockStyleId` handles into `Vec<TextStyle>`/`Vec<BlockStyle>`) standing in for Python's shared-object intern cache. `CombinedStyle` is a new, minimal type (just `(block_style, text_style, outline_level)` — Python's version also carries `id`/`name`/`.blocks`, neither needed since this port derives final `id`/`name` from a style's position in `StylesManager`'s `pure_block_styles`/`combined_styles`/`descendant_text_styles` lists rather than mutating them onto the (hash-key-bearing) style objects in place, the way Python does; `.serialize` hand-builds combined styles' `<w:style>` element directly since Python's `CombinedStyle.serialize` is its own standalone method, not a `DOCXStyle` subclass, while pure block styles really do reuse the already-ported `BlockStyle::serialize`). Not ported, moot: `DOCXStyle`'s hash/dedup base class (`TextStyle`/`BlockStyle`/`DescendantTextStyle` derive `Eq`/`Hash` directly) and `BlockStyle`'s `next_style` field (Python sets it to `None` in `__init__` and never assigns it anywhere else in the whole `docx/` tree — genuinely dead in practice, confirmed by grep, so every `serialize`/`serialize_properties` call site here passes `None` literally rather than threading a field that's never anything else)
- [x] tables.py (see #132; `Border`, `border_style_weight`, `as_percent`, `convert_width`, and a `Border`-repackaging `read_css_block_borders` (thin wrapper over `styles.rs`'s already-ported one) all ported to `docx/writer/tables.rs`. `SpannedCell`/`Cell`/`Row`/`Table` are ported, held in a `Tables` arena (`TableId`/`RowId`/`CellId` handles into `Vec<Table>`/`Vec<Row>`/`Vec<CellSlot>`, `CellSlot` a real `Cell(Cell)`/`Spanned(SpannedCell)` enum standing in for Python's duck-typed `isinstance` checks) — one arena per level, cross-referenced by id, not one shared tagged arena. `Tables::neighbor`/`::applicable_borders`/`::resolve_border`/`::resolve_cell_borders` (CSS table border-conflict resolution) and `::expand_spanned_cells` (merge-continuation placeholder insertion for `rowspan`/`colspan`'d cells) are ported and tested in isolation. `Tables` also ports the full stateful mutation half every real HTML-walking caller needs — `table_start_new_row`/`table_start_new_cell`/`row_start_new_cell` (port of `Table`/`Row.start_new_row`/`.start_new_cell`), `table_finish_tag` (port of `Table.finish_tag` including the row-level `Row.finish_tag` layer), and `table_add_block`/`table_add_table`/`row_add_block`/`row_add_table`/`cell_add_block`/`cell_add_table` (port of `Table`/`Row`/`Cell.add_block`/`.add_table`, including their auto-create-a-cell/row fallbacks). `Cell` has a real `items: Vec<ItemId>` field. `Blocks`' OWN half of the integration (`from_html.py`) is ported too: its own `tables_arena`/`open_tables` stack, `finish_tag`'s table-closing branch, and a real `Block::container: Option<ItemContainer>` field so `delete_block_at`/`apply_page_break_after` correctly tell which items-list a block lives in. **Every `.serialize` method is NOW ported too**: `SpannedCell::serialize` (a bare merge-continuation `<w:tc>`) and `Tables::serialize_cell`/`::serialize_row`/`::serialize_table` (the real `<w:tc>`/`<w:tr>`/`<w:tbl>` OOXML, including border/background/margin/valign/rowspan/colspan handling, float-direction `tblpPr`, and the "last item was a table → still needs a trailing paragraph" Word 2007 requirement). The one real design point: rendering a `Block`'s own content needs `StylesManager`/`LinksManager` (owned by `Blocks`, in `from_html.rs`) — rather than importing those types into `tables.rs` (inverting the established module-dependency direction), the three `Tables::serialize_*` methods take a `serialize_block: &mut dyn FnMut(&mut Element, BlockId)` callback for exactly that one case; a nested `Table` item needs no such callback, since it recurses straight back into `Tables::serialize_table` itself. `Blocks::serialize` (`from_html.py`) now builds that real closure — see that entry above)
- [ ] TODO (upstream notes file, nothing to port)
- [x] `utils.py` -> `docx/writer/utils.rs` (with the `tinycss.color3` colour grammar it depends on)

#### epub

- [x] `__init__.py` -> `epub.rs` (`simple_container_xml`, `initialize_container`; `rules()` needs a CSS object model, not ported)
- [x] `pages.py` -> `epub/pages.rs` (element selection is the caller's — no XPath engine; `add_page_map`'s unreachable writer call is not reproduced)
- [x] `periodical.py` -> `epub/periodical.rs` (issue #139/#5 fixed: article `<summary>` now reads the article's own `description`, not its section's, which real calibre does and which silently discarded every article's real description. See the module's own doc for the still-open #142 question on the escaping divergence)

##### cfi

- [x] `__init__.py` (empty upstream; the module root is `epub/cfi/mod.rs`)
- [x] `epubcfi.ebnf` -> reproduced as the grammar in `epub/cfi/mod.rs` docs (issue #25 spells it `epublfi.ebnf`)
- [x] `parse.py` -> `epub/cfi/parse.rs`
- [x] `tests.py` -> unit tests in `epub/cfi/parse.rs`, plus `tests/epub_cfi_test.rs` cross-validation

#### fb2

- [x] `__init__.py` (`base64_decode`) -> `fb2/mod.rs`
- [x] `fb2ml.py` -> `fb2/fb2ml.rs` (the `Stylizer` becomes a trait — see #132/#130 for the missing CSS cascade; `--pretty-print` re-serialization and real image transcoding, issue #145, both closed — `DefaultImageConverter`, `Fb2Options::pretty_print`)

#### html

- [x] `__init__.py` (docstring only) -> `html/mod.rs`
- [x] `input.py` -> `html/input.rs`
- [x] `meta.py` -> `html/meta.rs`
- [x] `to_zip.py` -> `html/to_zip.rs` (settings + packaging; the `gui_convert` input dump `run()` needs is #147, and `do_user_config` is Qt)

#### htmlz

- [x] `__init__.py` (empty upstream) -> `htmlz/mod.rs`
- [x] `oeb2html.py` -> `htmlz/oeb2html.rs` (the three subclasses become `CssMode`; resolved CSS comes from `oeb::stylizer::StyleProvider`)

#### iterator

- [ ] __init__.py

#### lit

- [x] __init__.py
- [x] lzx.py
- [x] mssha1.py
- [x] reader.py
- [x] writer.py

##### maps

- [x] __init__.py
- [x] html.py
- [x] opf.py

#### lrf

- [ ] __init__.py
- [ ] fonts.py
- [ ] input.py
- [ ] lrfparser.py
- [ ] meta.py
- [ ] objects.py
- [ ] tags.py

##### html

- [ ] __init__.py
- [ ] color_map.py
- [ ] convert_from.py
- [ ] table.py

###### demo

- [ ] a.png
- [ ] demo.html
- [ ] large.jpg
- [ ] medium.jpg
- [ ] small.jpg

##### lrs

- [ ] __init__.py
- [ ] convert_from.py

##### pylrs

- [ ] __init__.py
- [ ] elements.py
- [ ] pylrf.py
- [ ] pylrfopt.py
- [ ] pylrs.py

#### metadata

- [x] __init__.py
- [x] archive.py
- [x] author_mapper.py
- [x] cli.py (Replaced by ebook-meta.rs)
- [x] docx.py
- [x] epub.py
- [x] ereader.py
- [x] extz.py
- [x] fb2.py
- [x] haodoo.py
- [x] html.py
- [x] imp.py
- [x] kdl.py (Skipped - specific scraper)
- [x] kfx.py
- [x] lit.py
- [x] lrx.py
- [x] mobi.py
- [x] odt.py
- [x] opf_2_to_3.py (Merged into opf.rs/cli)
- [x] opf.py
- [x] opf2.py (Merged into opf.rs)
- [x] opf3_test.py (Covered by opf_refinement_test.rs)
- [x] opf3.py (Merged into opf.rs)
- [x] pdb.py
- [x] pdf.py
- [x] plucker.py
- [x] pml.py
- [x] rar.py
- [x] rb.rs
- [x] rtf.rs
- [x] search_internet.py
- [x] snb.py
- [x] tag_mapper.py
- [x] test_author_sort.py
- [x] toc.py
- [x] topaz.py
- [x] txt.py
- [x] utils.py
- [x] worker.py
- [x] xmp.py (Unverified - Test Ignored)
- [x] zip.py

#### ebooks/metadata
- [x] chm.py (Regex-based port)
- [x] azw4.py (PDF wrapper port)

#### mobi

- [ ] __init__.py
- [x] huffcdic.py
- [x] langcodes.py
- [x] mobiml.py (Stub)

- [ ] tbs_periodicals.rst
- [x] tweak.py (Stub)

- [x] utils.py

##### debug

- [x] __init__.py
- [x] containers.py
- [x] headers.py
- [x] index.py
- [x] main.py
- [x] mobi6.py (TBS periodical section/article-transition narration not
      reproduced — narrow, try/except-guarded even in the Python)
- [x] mobi8.py (`read_tbs`'s cross-check against the unported
      `mobi/writer8/tbs.py` deferred; decoding the TBS bytes actually
      present is not)

##### reader

- [ ] __init__.py
- [x] containers.py
- [x] headers.py
- [x] index.py
- [x] markup.py
- [x] mobi6.py
- [x] mobi8.py
- [x] ncx.py

##### writer2

- [x] __init__.py
- [x] indexer.py
- [x] main.py (issue #157 closed the last real gap: `MobiWriter::write_joint`/`generate_joint_record0` now really interleave a joint MOBI6+KF8 `.azw3` file -- shared `Resources` block keyed off the union of both writers' `used_images`, a literal `BOUNDARY` marker, the embedded `KF8Book::record0_for_joint` plus its own record list, and a MOBI6-view `record0` built via `writer8::mobi::build_joint_mobi6_record0` (the same 264-byte KF8-style header shape, stamped `file_version = 6`) with an EXTH `kf8_header_index` marker for the reader's `kf8_type == "joint"` detection. Round-trip tested: both the MOBI6 and the embedded KF8 payload recover their real text through this crate's own readers. No output plugin drives this path yet -- `output::mobi_output::MOBIOutput` only calls the standalone `write()`, and there is no `AZW3Output` plugin in this crate at all; that dispatch is separate, unstarted scope)
- [x] resources.py
- [x] serializer.py

##### writer8

- [x] __init__.py
- [x] cleanup.py
- [x] exth.py
- [x] header.py
- [x] index.py
- [x] main.py
- [x] mobi.py
- [x] skeleton.py
- [x] tbs.py
- [x] toc.py

#### odt

- [x] __init__.py
- [x] input.py

#### oeb

- [x] __init__.py
- [x] base.py (Constants, Metadata, Container ported)
- [x] normalize_css.py (DEFAULTS and edge normalization ported)
- [x] parse_utils.py (XML Namespace helpers, iterlinks, xml2text ported)
- [x] reader.py (OEBReader and OEBBook structure ported)
- [x] stylizer.py (Basic Stylizer/Style struct with inheritance ported)
- [x] writer.py (OEBWriter for OPF generation ported)

##### display

- [ ] __init__.py
- [x] webview.py

##### iterator

- [ ] __init__.py
- [x] book.py (EbookIterator: extraction via Plumber's refactored input dispatch, paginated/indexed spine, search)
- [x] bookmarks.py (BookmarksMixin trait, parse/serialize_bookmarks, safe_replace-backed in-file copy)
- [x] spine.py (SpineItem, IndexEntry, create_indexing_data, character_count/anchor_map/all_links)

##### polish

- [ ] __init__.py
- [x] cascade.py
- [x] container.py
- [x] cover.py
- [x] create.py
- [x] css.py
- [x] download.py
- [x] embed.py
- [x] errors.py
- [x] fonts.py
- [x] hyphenation.py
- [x] images.py
- [x] import_book.py
- [x] jacket.py (issue #168's jacket.py half CLOSED -- `oeb::polish::jacket`: find/replace/remove an existing jacket page in an already-built book via `Container`, real end-to-end tests exercising metadata rendering, spine insertion at the right index, idempotent replace, and removal. Real `container.mi`-equivalent metadata via `crate::opf::parse_opf` on the container's own serialized OPF; the GUI `page_setup` output-profile preference lookup is collapsed into `transforms::jacket::JacketOptions::default()`, matching that already-ported file's own documented narrowing. `kepubify.py` split out to its own issue since it also depends on the unported `tts.py`)
- [ ] kepubify.py (split into its own issue, blocked on tts.py)
- [x] main.py (issue #169 CLOSED -- `oeb::polish::main::polish_one`, the real "Polish Book" orchestrator, ported against `AnyContainer` (dispatching `upgrade_book`'s `EpubContainer`-specific signature across `Epub`/`Kepub`/`Azw3` variants, matching real upstream's own `book_type` check). Every dependency (#162-166, #168) is closed. Disclosed narrowings: `opts.opf` (replace metadata from an external OPF file) needs an in-place OPF `<metadata>` rewriter this port doesn't have (only a full reader and a from-scratch builder) -- returns a real error; `opts.embed`/`opts.subset` call straight through to `embed_all_fonts`/`subset_all_fonts`'s own real, documented `todo!()` font-subsetting-library gaps rather than intercepting them; CLI argument parsing (`option_parser`/`main`, needs #126) and GUI-integration wrappers (`gui_polish`/`tweak_polish`) and the multi-file batch driver (`polish()`, a few lines of boilerplate any real caller can write directly) are out of scope -- `polish_one` itself is the real, reusable value)
- [x] opf.py
- [x] parsing.py
- [x] pretty.py
- [x] replace.py
- [ ] report.py
- [x] spell.py
- [x] split.py
- [x] stats.py
- [x] subset.py
- [x] toc.py
- [ ] tts.py
- [x] upgrade.py
- [x] utils.py

###### check

- [x] __init__.py
- [x] base.py
- [x] css.py
- [x] fonts.py
- [x] images.py
- [x] links.py
- [x] main.py
- [x] opf.py
- [x] parsing.py

##### transforms

- [ ] __init__.py
- [x] alt_text.py
- [x] cover.py
- [x] data_url.py
- [x] embed_fonts.py
- [x] filenames.py
- [x] flatcss.py
- [x] guide.py
- [x] htmltoc.py
- [x] jacket.py
- [x] linearize_tables.py
- [x] manglecase.py
- [x] metadata.py
- [x] page_margin.py
- [x] rasterize.py
- [x] rescale.py
- [x] split.py
- [x] structure.py
- [x] subset.py
- [x] trimmanifest.py
- [x] unsmarten.py

#### pdb

- [ ] __init__.py
- [x] formatreader.py
- [x] formatwriter.py
- [x] header.py

##### ereader

- [x] __init__.py
- [x] inspector.py
- [x] reader.py
- [x] reader132.py
- [x] reader202.py
- [x] writer.py

##### pdf

- [ ] __init__.py
- [x] reader.py

##### plucker

- [ ] __init__.py
- [ ] reader.py

##### ztxt

- [ ] __init__.py
- [ ] formatreader.py
- [ ] formatwriter.py
- [ ] header.py

#### pdf

- [x] __init__.py
- [x] develop.py
- [x] html_writer.py
- [x] image_writer.py
- [x] pdftohtml.py
- [x] reflow.py
- [x] utils.h

##### render

- [x] __init__.py
- [x] common.py
- [x] fonts.py
- [x] gradients.py
- [x] graphics.py
- [x] links.py
- [x] serialize.py

#### pml

- [x] __init__.py
- [x] pmlconverter.py
- [x] pmlml.py

#### rb

- [x] __init__.py
- [x] rbml.py
- [x] reader.py
- [x] writer.py

#### readability

- [x] __init__.py
- [x] cleaners.py
- [x] debug.py
- [x] htmls.py
- [x] readability.py

#### rtf

- [ ] __init__.py
- [x] input.py
- [x] preprocess.py
- [x] rtfml.py

#### rtf2xml

- [ ] __init__.py
- [x] add_brackets.py
- [x] body_styles.py
- [x] border_parse.py
- [x] char_set.py
- [x] check_brackets.py
- [x] check_encoding.py
- [x] colors.py
- [x] combine_borders.py
- [x] configure_txt.py
- [x] convert_to_tags.py
- [x] copy.py
- [x] default_encoding.py
- [x] delete_info.py
- [x] field_strings.py
- [x] fields_large.py
- [x] fields_small.py
- [x] fonts.py
- [x] footnote.py
- [x] get_char_map.py
- [x] get_options.py
- [x] group_borders.py
- [x] group_styles.py
- [x] header.py
- [x] headings_to_sections.py
- [x] hex_2_utf8.py
- [x] info.py
- [x] inline.py
- [x] line_endings.py
- [x] list_numbers.py
- [x] list_table.py
- [x] make_lists.py
- [x] old_rtf.py
- [x] options_trem.py
- [x] output.py
- [x] override_table.py
- [x] paragraph_def.py
- [x] paragraphs.py
- [x] ParseRtf.py
- [x] pict.py
- [x] preamble_div.py
- [x] preamble_rest.py
- [x] process_tokens.py
- [x] replace_illegals.py
- [x] sections.py
- [x] styles.py
- [x] table_info.py
- [x] table.py
- [x] tokenize.py

#### snb

- [x] __init__.py
- [x] snbfile.py
- [x] snbml.py

#### tcr

- [ ] __init__.py

#### textile

- [x] __init__.py
- [x] functions.py
- [x] unsmarten.py

#### txt

- [x] __init__.py
- [x] markdownml.py
- [x] newlines.py
- [x] processor.py
- [x] textileml.py
- [x] txtml.py

#### unihandecode

- [x] __init__.py
- [x] jacodepoints.py
- [x] jadecoder.py (codepoint-table fallback only; upstream's pykakasi-based
      Japanese analysis pass is not ported -- no Rust equivalent, see
      `jadecoder.rs`'s docs)
- [x] krcodepoints.py
- [x] krdecoder.py
- [x] unicodepoints.py
- [x] unidecoder.py
- [x] vncodepoints.py
- [x] vndecoder.py
- [x] zhcodepoints.py

### gui2

- [ ] add.py
- [ ] add_filters.py
- [ ] author_mapper.py
- [ ] auto_add.py
- [ ] bars.py
- [ ] book_details.py
- [ ] central.py
- [ ] changes.py
- [ ] chat_widget.py
- [ ] comments_editor.py
- [ ] complete2.py
- [ ] covers.py
- [ ] cover_flow.py
- [ ] css_transform_rules.py
- [ ] custom_column_widgets.py
- [ ] device.py
- [ ] dnd.py
- [ ] ebook_download.py
- [ ] email.py
- [ ] extra_files_watcher.py
- [ ] filename_pattern.ui
- [ ] flow_toolbar.py
- [ ] font_family_chooser.py
- [ ] geometry.py
- [ ] gestures.py
- [ ] html_transform_rules.py
- [ ] icon_theme.py
- [ ] image_popup.py
- [ ] init.py
- [ ] jobs.py
- [ ] job_indicator.py
- [ ] keyboard.py
- [ ] languages.py
- [ ] layout.py
- [ ] layout_menu.py
- [ ] linux_file_dialogs.py
- [ ] listener.py
- [ ] llm.py
- [ ] main.py
- [ ] main_window.py
- [ ] markdown_editor.py
- [ ] markdown_syntax_highlighter.py
- [ ] momentum_scroll.py
- [ ] notify.py
- [ ] open_with.py
- [ ] palette.py
- [ ] pin_columns.py
- [ ] proceed.py
- [ ] publisher_mapper.py
- [ ] pyqt6_compat.py
- [ ] qt_file_dialogs.py
- [ ] save.py
- [ ] search_box.py
- [ ] search_restriction_mixin.py
- [ ] series_mapper.py
- [ ] splash_screen.py
- [ ] tag_mapper.py
- [ ] threaded_jobs.py
- [ ] throbber.py
- [ ] tools.py
- [ ] trash.py
- [ ] ui.py
- [ ] update.py
- [ ] webengine.py
- [ ] widgets.py
- [ ] widgets2.py
- [ ] win_file_dialogs.py
- [ ] __init__.py

#### actions

- [ ] add.py
- [ ] add_to_library.py
- [ ] all_actions.py
- [ ] annotate.py
- [ ] author_mapper.py
- [ ] auto_scroll.py
- [ ] booklist_context_menu.py
- [ ] browse_annots.py
- [ ] browse_notes.py
- [ ] catalog.py
- [ ] choose_library.py
- [ ] column_tooltips.py
- [ ] convert.py
- [ ] copy_to_library.py
- [ ] delete.py
- [ ] device.py
- [ ] edit_collections.py
- [ ] edit_metadata.py
- [ ] embed.py
- [ ] fetch_news.py
- [ ] fts.py
- [ ] help.py
- [ ] layout_actions.py
- [ ] llm_book.py
- [ ] manage_categories.py
- [ ] mark_books.py
- [ ] match_books.py
- [ ] next_match.py
- [ ] open.py
- [ ] plugin_updates.py
- [ ] polish.py
- [ ] preferences.py
- [ ] random.py
- [ ] restart.py
- [ ] saved_searches.py
- [ ] save_to_disk.py
- [ ] show_book_details.py
- [ ] show_quickview.py
- [ ] show_stored_templates.py
- [ ] show_template_tester.py
- [ ] similar_books.py
- [ ] sort.py
- [ ] store.py
- [ ] tag_mapper.py
- [ ] toc_edit.py
- [ ] tweak_epub.py
- [ ] unpack_book.py
- [ ] view.py
- [ ] virtual_library.py
- [ ] __init__.py

#### catalog

- [ ] catalog_bibtex.py
- [ ] catalog_bibtex.ui
- [ ] catalog_csv_xml.py
- [ ] catalog_epub_mobi.py
- [ ] catalog_epub_mobi.ui
- [ ] catalog_tab_template.ui
- [ ] __init__.py

#### convert

- [ ] azw3_output.py
- [ ] azw3_output.ui
- [ ] bulk.py
- [ ] comic_input.py
- [ ] comic_input.ui
- [ ] debug.py
- [ ] debug.ui
- [ ] docx_input.py
- [ ] docx_input.ui
- [ ] docx_output.py
- [ ] epub_output.py
- [ ] epub_output.ui
- [ ] fb2_input.py
- [ ] fb2_input.ui
- [ ] fb2_output.py
- [ ] fb2_output.ui
- [ ] font_key.py
- [ ] font_key.ui
- [ ] gui_conversion.py
- [ ] heuristics.py
- [ ] heuristics.ui
- [ ] htmlz_output.py
- [ ] htmlz_output.ui
- [ ] kepub_output.py
- [ ] kepub_output.ui
- [ ] look_and_feel.py
- [ ] look_and_feel.ui
- [ ] lrf_output.py
- [ ] lrf_output.ui
- [ ] metadata.py
- [ ] metadata.ui
- [ ] mobi_output.py
- [ ] mobi_output.ui
- [ ] page_setup.py
- [ ] page_setup.ui
- [ ] pdb_output.py
- [ ] pdb_output.ui
- [ ] pdf_input.py
- [ ] pdf_input.ui
- [ ] pdf_output.py
- [ ] pdf_output.ui
- [ ] pmlz_output.ui
- [ ] pml_output.py
- [ ] rb_output.py
- [ ] rb_output.ui
- [ ] regex_builder.py
- [ ] regex_builder.ui
- [ ] rtf_input.py
- [ ] rtf_input.ui
- [ ] search_and_replace.py
- [ ] search_and_replace.ui
- [ ] single.py
- [ ] snb_output.py
- [ ] snb_output.ui
- [ ] structure_detection.py
- [ ] structure_detection.ui
- [ ] toc.py
- [ ] toc.ui
- [ ] txtz_output.py
- [ ] txt_input.py
- [ ] txt_input.ui
- [ ] txt_output.py
- [ ] txt_output.ui
- [ ] xpath_wizard.py
- [ ] xpath_wizard.ui
- [ ] __init__.py

#### device_drivers

- [ ] configwidget.py
- [ ] configwidget.ui
- [ ] mtp_config.py
- [ ] mtp_folder_browser.py
- [ ] tabbed_device_config.py
- [ ] __init__.py

#### dialogs

- [ ] add_empty_book.py
- [ ] add_from_isbn.py
- [ ] authors_edit.py
- [ ] book_info.py
- [ ] catalog.py
- [ ] catalog.ui
- [ ] check_library.py
- [ ] choose_format.py
- [ ] choose_format_device.py
- [ ] choose_format_device.ui
- [ ] choose_library.py
- [ ] choose_library.ui
- [ ] choose_plugin_toolbars.py
- [ ] comments_dialog.py
- [ ] confirm_delete.py
- [ ] confirm_delete_location.py
- [ ] confirm_merge.py
- [ ] connect_to_folder.py
- [ ] conversion_error.py
- [ ] conversion_error.ui
- [ ] custom_recipes.py
- [ ] data_files_manager.py
- [ ] delete_matching_from_device.py
- [ ] delete_matching_from_device.ui
- [ ] device_category_editor.py
- [ ] device_category_editor.ui
- [ ] drm_error.py
- [ ] drm_error.ui
- [ ] duplicates.py
- [ ] edit_authors_dialog.py
- [ ] edit_authors_dialog.ui
- [ ] edit_category_notes.py
- [ ] enum_values_edit.py
- [ ] exim.py
- [ ] ff_doc_editor.py
- [ ] jobs.ui
- [ ] llm_book.py
- [ ] match_books.py
- [ ] match_books.ui
- [ ] message_box.py
- [ ] metadata_bulk.py
- [ ] metadata_bulk.ui
- [ ] multisort.py
- [ ] opml.py
- [ ] palette.py
- [ ] password.py
- [ ] password.ui
- [ ] plugin_updater.py
- [ ] progress.py
- [ ] quickview.py
- [ ] quickview.ui
- [ ] restore_library.py
- [ ] saved_search_editor.py
- [ ] scheduler.py
- [ ] search.py
- [ ] select_formats.py
- [ ] show_category_note.py
- [ ] smartdevice.py
- [ ] smartdevice.ui
- [ ] tag_categories.py
- [ ] tag_categories.ui
- [ ] tag_editor.py
- [ ] tag_editor.ui
- [ ] tag_list_editor.py
- [ ] tag_list_editor.ui
- [ ] tag_list_editor_table_widget.py
- [ ] template_dialog.py
- [ ] template_dialog.ui
- [ ] template_dialog_box_layout.py
- [ ] template_dialog_code_widget.py
- [ ] template_general_info.py
- [ ] template_line_editor.py
- [ ] trim_image.py
- [ ] __init__.py

#### fts

- [ ] dialog.py
- [ ] scan.py
- [ ] search.py
- [ ] utils.py
- [ ] __init__.py

#### library

- [ ] alternate_views.py
- [ ] annotations.py
- [ ] bookshelf_view.py
- [ ] caches.py
- [ ] delegates.py
- [ ] models.py
- [ ] notes.py
- [ ] views.py
- [ ] __init__.py

#### lrf_renderer

- [ ] bookview.py
- [ ] config.ui
- [ ] document.py
- [ ] main.py
- [ ] main.ui
- [ ] text.py
- [ ] __init__.py

#### metadata

- [ ] basic_widgets.py
- [ ] bulk_download.py
- [ ] config.py
- [ ] diff.py
- [ ] pdf_covers.py
- [ ] single.py
- [ ] single_download.py
- [ ] __init__.py

#### pictureflow

- [ ] pictureflow.cpp
- [ ] pictureflow.h
- [ ] pictureflow.sip

#### preferences

- [ ] device_debug.py
- [ ] device_user_defined.py
- [ ] email.ui
- [ ] emailp.py
- [ ] history.py
- [ ] ignored_devices.py
- [ ] keyboard.py
- [ ] look_feel.py
- [ ] look_feel.ui
- [ ] main.py
- [ ] metadata_sources.py
- [ ] metadata_sources.ui
- [ ] misc.py
- [ ] misc.ui
- [ ] plugboard.py
- [ ] plugboard.ui
- [ ] plugins.py
- [ ] plugins.ui
- [ ] save_template.py
- [ ] save_template.ui
- [ ] saving.py
- [ ] saving.ui
- [ ] search.py
- [ ] search.ui
- [ ] sending.py
- [ ] sending.ui
- [ ] server.py
- [ ] template_functions.py
- [ ] template_functions.ui
- [ ] texture_chooser.py
- [ ] toolbar.py
- [ ] toolbar.ui
- [ ] tweaks.py
- [ ] __init__.py

##### look_feel_tabs

- [ ] bookshelf_view.py
- [ ] bookshelf_view.ui
- [ ] book_details.py
- [ ] book_details.ui
- [ ] cover_grid.py
- [ ] cover_grid.ui
- [ ] cover_view.py
- [ ] cover_view.ui
- [ ] edit_metadata.py
- [ ] edit_metadata.ui
- [ ] main_interface.py
- [ ] main_interface.ui
- [ ] quickview.py
- [ ] quickview.ui
- [ ] tb_display.py
- [ ] tb_display.ui
- [ ] tb_hierarchy.py
- [ ] tb_hierarchy.ui
- [ ] tb_icon_rules.py
- [ ] tb_icon_rules.ui
- [ ] tb_partitioning.py
- [ ] tb_partitioning.ui
- [ ] __init__.py

#### progress_indicator

- [ ] CalibreStyle.cpp
- [ ] QProgressIndicator.cpp
- [ ] QProgressIndicator.h
- [ ] QProgressIndicator.sip
- [ ] __init__.py

#### store

- [ ] amazon_base.py
- [ ] amazon_live.py
- [ ] basic_config.py
- [ ] basic_config_widget.ui
- [ ] declined.txt
- [ ] loader.py
- [ ] opensearch_store.py
- [ ] search_result.py
- [ ] web_store.py
- [ ] web_store_dialog.py
- [ ] __init__.py

##### config

- [ ] store.py
- [ ] __init__.py

###### chooser

- [ ] chooser_dialog.py
- [ ] chooser_widget.py
- [ ] chooser_widget.ui
- [ ] models.py
- [ ] results_view.py
- [ ] __init__.py

###### search

- [ ] search_widget.py
- [ ] search_widget.ui
- [ ] __init__.py

##### search

- [ ] adv_search_builder.py
- [ ] adv_search_builder.ui
- [ ] download_thread.py
- [ ] models.py
- [ ] results_view.py
- [ ] search.py
- [ ] search.ui
- [ ] __init__.py

##### stores

- [ ] baen_webscription_plugin.py
- [ ] beam_ebooks_de_plugin.py
- [ ] biblio_plugin.py
- [ ] bn_plugin.py
- [ ] bubok_portugal_plugin.py
- [ ] bubok_publishing_plugin.py
- [ ] chitanka_plugin.py
- [ ] ebookpoint_plugin.py
- [ ] ebooksgratuits_plugin.py
- [ ] ebookshoppe_uk_plugin.py
- [ ] ebooks_com_plugin.py
- [ ] ebook_nl_plugin.py
- [ ] empik_plugin.py
- [ ] feedbooks_plugin.py
- [ ] google_books_plugin.py
- [ ] gutenberg_plugin.py
- [ ] kobo_plugin.py
- [ ] legimi_plugin.py
- [ ] libri_de_plugin.py
- [ ] litres_plugin.py
- [ ] manybooks_plugin.py
- [ ] mills_boon_uk_plugin.py
- [ ] nexto_plugin.py
- [ ] ozon_ru_plugin.py
- [ ] pragmatic_bookshelf_plugin.py
- [ ] publio_plugin.py
- [ ] rw2010_plugin.py
- [ ] smashwords_plugin.py
- [ ] swiatebookow_plugin.py
- [ ] virtualo_plugin.py
- [ ] weightless_books_plugin.py
- [ ] woblink_plugin.py
- [ ] wolnelektury_plugin.py
- [ ] __init__.py

###### mobileread

- [ ] adv_search_builder.py
- [ ] adv_search_builder.ui
- [ ] cache_progress_dialog.py
- [ ] cache_progress_dialog.ui
- [ ] cache_update_thread.py
- [ ] mobileread_plugin.py
- [ ] models.py
- [ ] store_dialog.py
- [ ] store_dialog.ui
- [ ] __init__.py

#### tag_browser

- [ ] model.py
- [ ] ui.py
- [ ] view.py
- [ ] __init__.py

#### toc

- [ ] location.py
- [ ] main.py
- [ ] __init__.py

#### tts

- [ ] config.py
- [ ] develop.py
- [ ] download.py
- [ ] manager.py
- [ ] piper.py
- [ ] qt.py
- [ ] speechd.py
- [ ] types.py
- [ ] __init__.py

#### tweak_book

- [ ] boss.py
- [ ] char_select.py
- [ ] check.py
- [ ] check_links.py
- [ ] download.py
- [ ] file_list.py
- [ ] function_replace.py
- [ ] job.py
- [ ] jump_to_class.py
- [ ] live_css.py
- [ ] main.py
- [ ] manage_fonts.py
- [ ] plugin.py
- [ ] polish.py
- [ ] preferences.py
- [ ] preview.py
- [ ] reports.py
- [ ] save.py
- [ ] search.py
- [ ] spell.py
- [ ] templates.py
- [ ] text_search.py
- [ ] toc.py
- [ ] tts.py
- [ ] ui.py
- [ ] undo.py
- [ ] widgets.py
- [ ] __init__.py

##### completion

- [ ] basic.py
- [ ] popup.py
- [ ] utils.py
- [ ] worker.py
- [ ] __init__.py

##### diff

- [ ] highlight.py
- [ ] main.py
- [ ] view.py
- [ ] _patiencediff_c.c
- [ ] __init__.py

##### editor

- [ ] canvas.py
- [ ] comments.py
- [ ] help.py
- [ ] image.py
- [ ] insert_resource.py
- [ ] snippets.py
- [ ] text.py
- [ ] themes.py
- [ ] widget.py
- [ ] __init__.py

###### smarts

- [ ] css.py
- [ ] html.py
- [ ] python.py
- [ ] utils.py
- [ ] __init__.py

###### syntax

- [ ] base.py
- [ ] css.py
- [ ] html.c
- [ ] html.py
- [ ] javascript.py
- [ ] pygments_highlighter.py
- [ ] python.py
- [ ] utils.py
- [ ] xml.py
- [ ] __init__.py

#### viewer

- [ ] annotations.py
- [ ] bookmarks.py
- [ ] config.py
- [ ] control_sleep.py
- [ ] convert_book.py
- [ ] highlights.py
- [ ] integration.py
- [ ] llm.py
- [ ] lookup.py
- [ ] main.py
- [ ] overlay.py
- [ ] printing.py
- [ ] search.py
- [ ] shortcuts.py
- [ ] toc.py
- [ ] toolbars.py
- [ ] tts.py
- [ ] ui.py
- [ ] web_view.py
- [ ] widgets.py
- [ ] __init__.py

#### wizard

- [ ] device.ui
- [ ] finish.ui
- [ ] kindle.ui
- [ ] library.ui
- [ ] send_email.py
- [ ] send_email.ui
- [ ] stanza.ui
- [ ] __init__.py

### headless

- [ ] CMakeLists.txt
- [ ] headless.json
- [ ] headless_backingstore.cpp
- [ ] headless_backingstore.h
- [ ] headless_integration.cpp
- [ ] headless_integration.h
- [ ] main.cpp

### library

- [ ] add_to_library.py
- [ ] caches.py
- [ ] check_library.py
- [ ] coloring.py
- [ ] comments.py
- [ ] custom_columns.py
- [ ] database.py
- [ ] database2.py
- [x] field_metadata.py (issue #421, first slice -- `calibre_db::field_metadata::FieldMetadata`, a real per-field descriptor registry: every standard column ported verbatim from `_builtin_field_metadata()`, plus one entry per custom column via `Cache::custom_column_label_map`, with accurate `is_category`/`datatype`/`display`/`is_multiple` separator dicts. Issue #500 widened it with `sortable_field_keys`/`ui_sortable_field_keys`/`all_metadata` + real `Serialize` support, and wired it into `GET /ajax/field-metadata` -- the first real HTTP consumer. Still read-only -- mutation methods (`add_user_category`/`add_search_category`/`add_grouped_search_terms`) remain unported, and `categories`/`notes`/`cdb::set_fields` still don't consume it for their own hierarchical/is_csp logic)
- [ ] prefs.py
- [ ] restore.py
- [ ] save_to_disk.py
- [ ] schema_upgrades.py
- [ ] sqlite.py
- [ ] sqlite_custom.c
- [ ] __init__.py

#### catalogs

- [x] bibtex.py
- [x] csv_xml.py
- [x] epub_mobi.py
- [x] epub_mobi_builder.py
- [x] utils.py
- [x] __init__.py

### scraper

- [x] qt.py
- [x] qt_backend.py
- [x] simple.py
- [x] test_fetch_backend.py
- [x] webengine_backend.py (real JS rendering via a `wry`/`tao`-based worker, `crates/calibre_scraper_worker` — spawned by `calibre_ebooks::scraper::WebEngineBrowser` over a JSON-lines protocol; the same webview stack `app/src-tauri` already depends on. Disclosed gaps: no request bodies, no real response status/headers — see `webengine_browser.rs`'s module doc)
- [x] __init__.py

### spell

- [x] break_iterator.py (`unicode-segmentation`, not ICU — locale-independent; upstream's hyphen-recombination not replicated, disclosed in `spell/break_iterator.rs`'s module doc)
- [x] dictionary.py (real spell-checking via `spellbook`, a pure-Rust Hunspell-compatible engine — reads the same vendored `.dic`/`.aff` files; no persisted-prefs store, caller supplies lookup closures instead)
- [x] import_from.py (`import_from_libreoffice_source_tree`, a maintainer-only build tool, intentionally not ported — disclosed in `spell/import_from.rs`'s module doc; the runtime `.oxt`-import and online-fetch paths are)
- [x] __init__.py

### srv

**Large, multi-increment port (issue #60, 36 files, ~12,300 lines) --
calibre's entire hand-rolled async HTTP/WebSocket content server.
Architecture decision: `axum`/`tokio` replace the custom event loop
wholesale rather than a file-by-file port -- see `calibre_srv`'s crate
root doc for the full explanation. Phase 1 (this pass) ships a
single-library, unauthenticated OPDS catalog + book/cover downloads.**

- [ ] ajax.py (partial -- read-only JSON REST API: book/books, categories/category (flat, no hierarchy), books_in, search (now with real `vl=` virtual-library restriction, issue #500), library-info, field-metadata, virtual-libraries, session-data (issue #500, new endpoints extending this API for the #432 browser-UI epic rather than porting upstream's separate `interface-data/*` family -- see `calibre_srv::ajax`'s module doc for the full rationale); see that module's doc for what's narrowed)
- [x] auth.py (Basic auth only, no Digest; see `calibre_srv::auth`'s module doc)
- [ ] auto_reload.py (issue #431, low priority)
- [x] bonjour.py (mDNS advertisement via the `mdns-sd` crate, real pure-Rust multicast responder — no system Avahi/Bonjour daemon needed; see `calibre_srv::bonjour`'s module doc for what's narrowed; issue #420 closed)
- [x] books.py (issue #483 closes out #427's core scope -- book-get/set-last-read-position, book-get/update-annotations (issue #485, real annotation storage + merge algorithm via `calibre_db::annotations`), the disk-cache layer (issue #482, `calibre_srv::books_cache::BookCache`), and now the real HTTP surface tying it together with the render pipeline (#481) via `calibre_srv::jobs::JobsManager` (#428): `GET /book-manifest/{book_id}/{fmt}` (cache-hit returns the rendered manifest merged with live metadata/read-positions/annotations, cache-miss queues/dedupes a render job and returns a poll-able job-status object) and `GET /book-file/{book_id}/{fmt}/{size}/{mtime}/{*name}` (serves one cached rendered asset, ETag'd with the content hash, real path-traversal guard). Not ported: mathjax static-asset serving (a separate, unrelated endpoint upstream, no real consumer in this crate yet). See `calibre_srv::render_endpoints`'s module doc for the disclosed `queue_job`/`job_done` simplifications)
- [x] cdb.py (delete-books, set-cover, set-fields, add-book (issue #424, real metadata sniffing + duplicate detection), copy-to-library (issue #425, real two-library copy/move + duplicate detection, no automerge), generic cmd dispatcher (issue #426, narrowed to a representative subset -- `search`/`show_metadata`/`remove` -- of `calibre_db::cli` commands; most others need a real `implementation`-equivalent written per command before they can be added, see `calibre_srv::cdb`'s module doc); issue #500 fixed a real path-shape drift on `delete-books`/`set-fields` -- both now accept the trailing `/{library_id}` segment the real client always sends (and stay usable without it too, matching every other library_id-aware route pair in this crate); see that module's doc for what's narrowed in each)
- [ ] changes.py (partial -- event shapes ported as `calibre_srv::web_socket::ChangeEvent` (BooksAdded/BooksDeleted/FormatsAdded/FormatsRemoved/MetadataChanged); no SavedSearchesChanged, tracked by issue #422)
- [ ] code.py (issue #432, epic -- real scope found: two substantial RapydScript SPAs (`book_list`/`read_book`, see `src/pyj` below), not vendorable upstream assets. Reimplemented from scratch in TypeScript+Vue, not a literal port. #498 (static serving) and #499 (reader MVP, `read_book`) closed; #500 (`/ajax/*` extension) and #501 (`book_list` library-browser MVP) not started -- see #432's own issue body)
- [ ] content.py (partial -- `cover`/`thumb`/format downloads (`calibre_srv::content`, no etag caching, no thumbnail resizing, no `opf`/`json`), the Notes feature (`calibre_srv::notes`), data-files endpoints (`calibre_srv::data_files`, issue #418 closed), and reader-profiles (`calibre_srv::reader_profiles`, issue #419 closed); issue #498 added real static-file serving for the new browser UI (`--static-dir`, `calibre_srv::router`'s own `fallback_service`, a real SPA-serving pattern -- not literally content.py's own `/static/{+what}` resource-directory serving, which upstream bundles at build time), but no icon/favicon endpoints specifically yet (issue #432))
- [x] convert.py (issue #429 -- `calibre_srv::convert`: real `POST /conversion/start/{book_id}`, `GET`/`POST /conversion/status/{job_id}`, `GET /conversion/book-data/{book_id}`, backed by `calibre_ebooks::conversion::plumber::Plumber` (#476) via `calibre_srv::jobs::JobsManager` (#428); `conversion_status` is the first real trigger for `web_socket::ChangeEvent::FormatsAdded`. Narrowed: no live conversion options (`Plumber::run` takes none yet), no progress percentage/message (no report-progress hook in `Plumber`), `book-data` omits `profiles`/`conversion_options` (needs the same live per-format option support); see the module's own doc)
- [ ] embedded.py
- [x] errors.py
- [x] fast_css_transform.cpp (issues #479/#488 both closed: `calibre_ebooks::css::url_rewrite::transform_urls` -- real `url()`/`@import`-string rewriting via a token-level `cssparser`-based splicer, disclosed narrower than upstream's own lenient hand-rolled tokenizer for two malformed-CSS edge cases (internal whitespace/mid-token comments in an unquoted url); `calibre_ebooks::css::property_transform::transform_properties` -- the other four rewrite categories (page-break-*→break-*+`-webkit-column-*` fallback duplicate, `-epub-writing-mode`/`-webkit-writing-mode`→`writing-mode`, absolute font-size units/keywords→`rem`, string quote normalization as a collateral effect of an otherwise-changed declaration), same chunk-level splicing approach, wired into `render_book.rs`'s CSS-file and inline-`<style>`/`style=` handling alongside #479's own rewriting. Disclosed narrowings: mid-token comment-splitting not replicated (same class as #479's own), an escaped value-side *unit* (not property name) isn't converted, and one synthetic extreme-magnitude font-size test's scientific-notation formatting isn't replicated (Rust's `Display for f64` matches Python's `repr()` for every other real test value). Also corrected: an earlier draft of #488's own filed scope wrongly claimed `%` converts to rem too -- the real .cpp source's unit map has no `%` entry, confirmed and fixed in the module's own tests)
- [x] fts.py (search/disable/reindex/indexing/snippets, backed by calibre_db's already-ported FTS5 engine; restriction reinterpreted as a search query, no virtual-library concept; see `calibre_srv::fts`'s module doc)
- [ ] handler.py (not planned -- subsumed by axum::Router + AppState, same as routes.py)
- [x] html_as_json.cpp (issue #478, part of #427 -- `calibre_ebooks::reader_json::{serialize_node,serialize_document}`, real serialization of `calibre_ebooks::dom::Dom` into the in-browser reader's own JSON document format. NOT ported: XML namespace tracking (`Dom` is HTML5-only, no namespace URIs to track -- `ns_map` is always empty), and the comment-vs-other-node-type distinction (`Dom` already collapses doctypes/processing instructions into empty-string `Comment` nodes before this module ever sees them); see the module's own doc for both)
- [x] http_request.py (subsumed by `axum`/`hyper`'s own HTTP parsing -- see `calibre_srv`'s crate root doc)
- [x] http_response.py (subsumed by `axum`/`hyper`)
- [x] jobs.py (issue #428; `calibre_srv::jobs::JobsManager` -- real bounded-concurrency task tracking on `tokio` tasks instead of upstream's forked-subprocess model; wired into `AppState`/`main.rs`, driving both the render-book job queue (issue #483, `calibre_srv::render_endpoints`) and the server-side conversion job queue (issue #429, `calibre_srv::convert`) -- see the module's own doc)
- [ ] last_read.py (superseded -- see issue #60's body)
- [ ] legacy.py (issue #430, low priority -- wait for a concrete need)
- [x] library_broker.py (issue #423, first slice -- `calibre_srv::library_broker::LibraryBroker`, the base non-GUI class: opens N libraries, dedups by canonical path/samefile, default-library-is-first-inserted, `get`/`library_map`; `GuiLibraryBroker` out of scope, no GUI. Wired into `AppState::libraries`/`cache_for`, threaded through `content::get` as a real end-to-end `library_id`-switches-libraries demonstration; every other handler and `opds.py`/`ajax.py`'s multi-library gaps are separate follow-ups -- see the module's own doc)
- [x] loop.py (subsumed by `tokio`'s async runtime)
- [ ] manage_users_cli.py (partial -- `calibre_srv`'s own `--add-user`/`--remove-user`/`--list-users`/`--set-readonly`/`--change-password` flags cover add/remove/list/readonly/chpass; no change_set_password (misc_data column) or libraries (multi-library) subcommands)
- [ ] metadata.py (blocked on issue #421 field_metadata)
- [ ] opds.py (partial -- root nav feed, title/newest acquisition feeds, category/categorygroup browsing, and query-language search; no multi-library yet (issue #423); see `calibre_srv::opds`'s module doc)
- [x] opts.py
- [x] pool.py (subsumed by `tokio`'s task scheduler)
- [ ] pre_activated.py (not planned -- N/A, tokio's own `TcpListener::bind` covers "run over TCP")
- [x] render_book.py (issues #478/#479/#480/#481 all closed -- `calibre_ebooks::render_book`: real `extract_book`/`process_exploded_book`/`render` orchestration, `create_link_replacer` (the real container-backed virtualization decision logic #480 deferred), cover-page synthesis, TOC (with `from_xpaths` heading-derived fallback)/landmarks/spine/page-progression-direction, per-file HTML/CSS transforms wired end to end, real `calibre-book-manifest.json` written to disk. Scope: EPUB only (AZW3/MOBI explosion is a separate, pre-existing `oeb::polish::container::Azw3Container::open`/`commit` `todo!()` gap, not fixed here -- `extract_book` returns a real `Err` rather than risking a panic), HTML/CSS files only (standalone SVG parses into a different tree type than `calibre_ebooks::dom::Dom`, needing its own virtualizer -- not ported; SMIL not ported), sequential (no worker-pool parallelism). Not ported: `serialize_metadata`/`extract_annotations` (`render_for_viewer`'s extras), `quicklook`/CLI dev tools; see the module's own doc)
- [x] routes.py (subsumed by `axum::Router`)
- [ ] standalone.py (`calibre_srv`'s own minimal `main.rs` covers the same "run the server over TCP" role, not a port of this file's process-management machinery)
- [ ] TODO.rst
- [x] users.py (`UserManager`: CRUD + password check ported; `session_data` (issue #500) now has real `get_session_data`/`set_session_data` accessors, backing `GET`/`POST /ajax/session-data`; `restriction`/`misc_data` still kept as schema columns but not yet exposed through any accessor -- see `calibre_srv::users`'s module doc)
- [x] users_api.py (`POST /users/change-pw`; `is_allowed_to_change_password_via_http` check skipped, see `calibre_srv::users_api`'s module doc)
- [x] utils.py (the still-relevant pure-logic slice -- `Offsets`/`http_date` -- is ported as `calibre_srv::utils`; the rest is socket/logging plumbing subsumed by `axum`/`tokio`)
- [x] web_socket.py (transport subsumed by `axum`'s native WebSocket support; `GET /web-socket` broadcasts real `ChangeEvent`s on library mutation -- see `calibre_srv::web_socket`'s module doc for why this is wired differently than upstream's own incomplete implementation)
- [ ] __init__.py

### translations

- [x] dynamic.py (`calibre_utils::translations::dynamic`; caching behavior ported, `.mo`-loading strategy narrowed to an injected closure since this project doesn't vendor upstream's bundled `locales.zip` -- see that module's own doc)
- [x] msgfmt.py (`calibre_utils::translations::msgfmt`; `.po`-to-`.mo` compiler, cross-validated byte-for-byte against a real run of the upstream script itself via system `python3`, since it has no calibre-specific imports)
- [x] __init__.py (empty package marker, N/A)

### utils

- [ ] bibtex.py
- [ ] browser.py
- [x] certgen.c / certgen.py (issue #463 -- `calibre_utils::certgen`: real self-signed CA + server certificate generation via the pure-Rust `rcgen` crate, replacing the 460-line OpenSSL `certgen.c` binding upstream's own `certgen.py` wraps. `create_server_cert` is the one entry point (upstream's own only real end-to-end caller). Verified with real X.509 parsing + signature-chain verification via `x509-parser` (an independent implementation from `rcgen`), not just PEM-prefix string checks. Disclosed narrowing/improvement: ECDSA P-256 keys instead of RSA (a strictly better modern default, nothing in this crate needs RSA-specific compatibility); password-encrypted key export not ported (no caller needs it). **Not wired into `calibre_srv`'s HTTP listener** -- real HTTPS support is its own architecture decision (TLS integration, auto-generate-vs-operator-supplied-cert, renewal), out of scope for this port; this module is the building block a future `--https` feature would use)
- [x] cleantext.py (`calibre_utils::cleantext`; real `clean_ascii_chars`/`clean_xml_chars`/`unescape`, entity decoding via the `htmlentity` crate)
- [ ] cocoa.m
- [ ] complete.py
- [x] config.py
- [x] config_base.py
- [x] copy_files.py (issue #464 -- `calibre_utils::copy_files`: hardlink-aware recursive tree copy (`copy_tree`/`copy_files`/`rename_files`), ported as `UnixFileCopier`'s semantics used verbatim on every platform (see module doc for why). Scope decision (the issue's own open question): kept standalone, NOT wired into `calibre_db::copy_to_library.rs`'s existing ad hoc logic -- a separate refactor decision with its own migration-verification risk, out of scope for a port issue. Disclosed narrowing: `WindowsFileCopier` (pre-locks every file via `winutil` handles) not ported, no Windows-specific FFI exists in this crate. A real recursive-path bug (nested files landing directly under the dest root instead of their real subdirectory, from computing `rel` against the wrong recursion-local `src`) was caught by this port's own nested-directory test and fixed before shipping)
- [x] copy_files_test.py (native Rust tests written instead -- see `calibre_utils::copy_files`'s own test module)
- [ ] cpp_binding.h
- [x] date.py
- [x] exim.py (issue #461 -- `calibre_utils::exim`: the container-format core only (`Exporter`/`FileDest`, `Importer`/`Pos`/`FileSource`) -- a multi-part, SHA1-hash-verified, append-only archive format, including random-access reads that span a part boundary. Verified with a tiny 32-byte `part_size` forcing files to span 3+ parts, a seek-mid-file test, and a deliberate single-byte corruption test confirming digest-mismatch detection. Disclosed narrowing: upstream's `export()`/`import_data()` orchestration (calling `calibre_db::Cache::export_library`/`import_library`, GUI prefs, global config export) not ported -- neither exists yet, and no `calibre-debug --export-library` equivalent CLI exists in this crate to wire it into; same "port the real primitive, defer the caller-less glue" pattern as `calibre_utils::smtp`/`icu` this session)
- [x] ffml_processor.py (issue #521 CLOSED, the last piece of the #460 epic -- ported to `calibre_utils::ffml_processor` as a flat `Node` enum (one variant per real `NodeKinds`) plus a byte-offset recursive-descent parser and 3 renderers (`tree_to_html`/`tree_to_rst`/`tree_to_transifex`), cross-validated byte-for-byte against the real Python module (runnable standalone once `calibre.prepare_string_for_xml` is stubbed) across bold/italic/code/list/url/ref/guilabel nodes plus edge cases (a `[B]` right after a literal `?`, multi-byte `\`-escaped characters, `str.lstrip`'s character-set-not-prefix semantics in `document_to_rst`/`document_to_summary_rst`). Disclosed narrowings: full HTML5 named-entity decoding lives in `calibre_ebooks::html_entities`, unreachable from `calibre_utils` (wrong side of the dependency graph), so `escape_xml` only undoes the 5 XML-predefined entities before re-escaping; the `safe=false` `formatted_english` GUI-translation fallback isn't modeled (no GUI translation layer exists in this port). No GUI tooltip surface calls this module anywhere in this port yet, matching the issue's own "low priority" framing -- ported and tested as a pure library)
- [ ] ffmpeg.c
- [x] filenames.py
- [x] file_type_icons.py (`calibre_utils::file_type_icons::icon_for_ext`)
- [ ] forked_map.py
- [x] formatter.py (issue #513, part of the #460 formatter epic -- `calibre_utils::formatter`: tokenizer/parser/AST/tree-walking evaluator for the calibre template language. Field access (`ValueSource`) and function calls (`FunctionRegistry`) are trait-based, decoupling the core from `calibre_db` -- real `calibre_db`-backed implementations + the ~125 built-in functions are #514-520's own scope. Includes a real, usable `DictValueSource` (upstream's own `EvalFormatter`/`eval_formatter` equivalent). 54 tests across lexer/parser/interpreter. Disclosed narrowings: stored GPM/Python templates not ported (no storage/registry system exists yet); `break_reporter` GUI debug hook not ported (no GUI exists); Python-`exec()`-based `python:`-prefixed templates are fundamentally not portable to Rust as-is, out of scope entirely. See the module's own doc comments for several more specific, real upstream quirks preserved rather than "fixed" -- a `with`-statement error message typo, a local function's inability to call itself recursively, `raw_field`'s 2-arg form requiring the not-yet-ported real function registry)
- [ ] formatter_functions.py (issue #460 epic -- ~125 built-ins split by upstream's own `category` constant into #514-520, see #460's own tracking body for the exact batching. **#514 (GET_FROM_METADATA + DB_FUNCS, 34 functions) shipped 25 for real** against `calibre_db::formatter_functions::{CacheValueSource,CacheFunctions,CacheCatalog}`: `raw_field`/`raw_list`/`has_cover`/`booksize`/`series_sort`/`author_sorts`/`get_link`/`author_links`/`language_codes`/`language_strings`/`current_library_name`/`current_library_path`/`has_extra_files`/`extra_file_names`/`extra_file_size`/`extra_file_modtime`/`virtual_libraries`/`book_count`/`book_values`/`get_note`/`has_note`/`approximate_formats`/`ondevice`/`annotation_count`. 4 remaining DB_FUNCS (`formats_modtimes`/`formats_sizes`/`formats_paths`/`formats_path_segments`, need new per-format size/path/mtime plumbing) split into #524, **now CLOSED** (see #524's own entry below). 5 not registered at all -- genuinely absent subsystems, not silent stubs: `is_marked`/`user_categories`/`current_virtual_library_name`/`connected_device_name`/`connected_device_uuid`. See `formatter_functions.rs`'s own module doc for the full disclosed-narrowing list. **#515 (STRING_MANIPULATION + CASE_CHANGES, 20 functions) shipped all 20**: 4 (`strcat`/`test`/`contains`/`character`) were already done as #513's own inlined `ExprKind` shortcuts; 14 pure string functions (`strlen`/`substr`/`strcat_max`/`re`/`re_group`/`swap_around_comma`/`ifempty`/`shorten`/`transliterate`/`swap_around_articles`/`uppercase`/`lowercase`/`titlecase`/`capitalize`) need no `Cache` at all and live in `calibre_utils::formatter::string_functions::{StringFunctions,StringCatalog,call,arg_count}`, reusable by any `ValueSource` context; 2 (`check_yes_no`/`field_exists`) need real book-field access and live in `calibre_db::formatter_functions` alongside #514's functions, falling back to `calibre_utils`'s dispatch for everything else. Disclosed narrowing: `re_group`'s per-group templates support only the real, narrow `{$}` / `{$:func(args)[:func(args)]*}` grammar (chaining functions from `string_functions` itself), not upstream's full separate `{field:func}` shorthand-template compiler (not ported anywhere in this crate). Found and fixed two real, previously-latent bugs while testing: `Cache::set_custom_column_value`'s bool branch only accepted `"true"`/`"false"` (not this crate's own `"0"`/`"1"` round-trip convention); `add_custom_column`'s single-value column tables (`bool`/`int`/`float`/`rating`/`text`/`comments`/`series`) had no `UNIQUE(book)` constraint, so `INSERT OR REPLACE` silently accumulated a new row on every write instead of replacing the old one -- neither was caught by #514's or cache.rs's own prior tests, which never wrote the same custom-column value twice for one book. **#516 (LIST_MANIPULATION + LIST_LOOKUP, 21 functions) shipped all 21**: `list_count_field` was already #513's own inlined `ExprKind::ListCountField`; `list_split` (writes indexed locals -- `id_prefix_N` -- so it can't be a plain registry call) is a new inlined `ExprKind::ListSplit` shortcut added by this issue; the remaining 19 (`list_count`/`count`, `list_count_matching`/`count_matching`, `sublist`, `subitems`, `list_join`, `list_union`/`merge_lists`, `range`, `list_remove_duplicates`, `list_difference`, `list_intersection`, `list_sort`, `list_equals`, `list_re`, `list_re_group`, `list_contains`/`in_list`, `str_in_list`, `identifier_in_list`, `list_item`, `select`) need no `Cache` and live in `calibre_utils::formatter::list_functions::{ListFunctions,ListCatalog,call,arg_count}`, with `calibre_db::formatter_functions::CacheFunctions`/`CacheCatalog` falling back to it (and to #515's `string_functions`) via an arity-lookup dispatch, not string-matching on error text. Disclosed docstring/code discrepancy preserved: `list_join`'s dedup key uses plain `str.lower()` while every other list-dedup function here uses real ICU `icu_lower`/`strcmp` (confirmed in the real source, not a typo). **Correction (found while porting #517): `str_in_list` is in fact case-insensitive, matching its docstring** -- the earlier claim of a docstring/code discrepancy here was wrong; it traced to a real bug in this crate's own `calibre_utils::icu::strcmp` (issue #459), not a real upstream inconsistency, fixed below. **#517 (ARITHMETIC + RELATIONAL + BOOLEAN, 16 functions) shipped all 16** in a new `calibre_utils::formatter::numeric_functions::{NumericFunctions,NumericCatalog,call,arg_count}`: `strcmp`/`strcmpcase`/`cmp`/`first_matching_cmp` (RELATIONAL), `add`/`subtract`/`multiply`/`divide`/`ceiling`/`floor`/`round`/`mod`/`fractional_part` (ARITHMETIC), `and`/`or`/`not` (BOOLEAN) -- `calibre_db::formatter_functions` now chains 3 no-Cache modules (`string_functions`/`list_functions`/`numeric_functions`) via a shared `fallback_call`/`fallback_arg_count` arity-lookup router. **Found and fixed a real, previously-undiscovered bug in `calibre_utils::icu::strcmp`** (from issue #459): it used ICU4X's own default collator strength (`Tertiary`, which distinguishes case), but real upstream `strcmp` is `sort_collator()` = `Strength::Secondary` ("Ignores case differences...", straight from upstream's own docstring) -- confirmed by direct probing (`Collator::compare("apple","Apple")` is `Equal` at `Secondary`, not at the old default). Fixed `strcmp` itself and added a real `case_sensitive_strcmp` (`CaseFirst::UpperFirst`, matching upstream's separate `case_sensitive_collator()`) for `strcmpcase`. This bug affected every existing `strcmp` caller (author/tag/category sorting in `categories.rs`/`opds.rs`/`ajax.rs`, plus #514-516's own sorts) -- the full test suite was re-run after the fix and needed no other changes, confirming no other test had baked in the wrong case-sensitive assumption. Preserved real Python numeric-formatting semantics rather than "fixing" them to match this crate's own `format_number` convention: `add`/`subtract`/`multiply`/`divide` always show a trailing `.0` for whole results (Python's raw `str(float)`); `ceiling`/`floor`/`round` return bare integers (`round` uses round-half-to-even via `f64::round_ties_even`, not round-half-away-from-zero); `mod` follows the sign of the divisor (Python's floored modulo), not Rust's `%` (sign of the dividend)). **#518 (FORMATTING_VALUES + DATE_FUNCTIONS + URL_FUNCTIONS, 17 functions) shipped all 17**: `f_string` was already #513's own inlined `ExprKind::FString`; 13 need no `Cache` and live in a new `calibre_utils::formatter::format_functions::{FormatFunctions,FormatCatalog,call,arg_count}` (`human_readable`/`format_number`/`format_date`/`finish_formatting`/`format_duration`/`today`/`days_between`/`date_arithmetic`/`to_hex`/`make_url`/`make_url_extended`/`query_string`/`encode_for_url`), including a real hand-written Python `str.format()`-mini-language engine (fill/align/sign/`#`/zero-pad/width/`,`-grouping/precision for `s`/`b`/`o`/`d`/`x`/`X`/`f`/`F`/`e`/`E`/`%`/`g`/`G`) shared by `format_number` and `finish_formatting`; 3 (`format_date_field`, `rating_to_stars` -- delegates to the already-real `calibre_ebooks::oeb::transforms::jacket::rating_to_stars`, `urls_from_identifiers` -- needs `calibre_ebooks::xml_util::prepare_string_for_xml`) need `calibre_ebooks`/`Cache` and live in `calibre_db::formatter_functions`, joining a 4-module `fallback_call`/`fallback_arg_count` chain (`string_functions`->`list_functions`->`numeric_functions`->`format_functions`). Disclosed narrowings: `urls_from_identifiers` implements only the hardcoded fallback identifiers (isbn/doi/arxiv/oclc/issn) plus explicit `uri`/`url`-named identifiers -- upstream's own `msprefs['id_link_rules']` (user preferences) and metadata-source-plugin URLs are genuinely absent subsystems in this port; `qquote`'s `use_plus=False` path percent-encodes `/` where Python's `quote()` default leaves it unescaped; `g`/`G` float formatting is a close, tested approximation of CPython's significant-digit algorithm, not verified byte-identical in every edge case; `format_date_field`'s real upstream `from_number` branch calls `float()` on an already-parsed `datetime` object and would raise there too -- this port reports the same real error rather than silently working around it. Real, non-obvious quirk found by testing rather than assumed: `format_duration`'s default (no custom suffix) selector always includes a trailing space (e.g. `"2d 1h 0m 20s "`) -- upstream's own docstring examples just don't visually show it). **#519 (ITERATING_VALUES + RECURSION + OTHER, ~13 functions) shipped all of them**: `assign`/`print`/`switch`/`switch_if`/`first_non_empty` were already #513's own inlined shortcuts; `arguments`/`globals`/`set_globals` were already specially handled by the parser/interpreter for local-function definitions. `eval`/`template`/`lookup` needed direct interpreter access (not a plain registry call) and are 3 new inlined `ExprKind` shortcuts: `eval`/`template` re-parse a `"program:"`-prefixed argument as a fresh sub-program (`eval` against the caller's own locals as its field source, `template` against the SAME real value source as the caller but a fresh locals scope) via a new shared `eval_sub_program` helper and a new `BorrowedValueSource` wrapper (lets a sub-evaluation borrow the caller's own value source without moving/cloning it); `lookup` picks a FIELD NAME by regex-matching its value argument, then resolves that field directly (bare-name only, matching the same old-style-shorthand-compiler narrowing already disclosed for `re_group`). The only real `FunctionRegistry` function left is `is_dark_mode` (no GUI/dark-mode concept exists in this headless port), now living in a new tiny `calibre_utils::formatter::misc_functions` module, extending the fallback chain to 5 no-Cache modules. Disclosed narrowing, corrected from an earlier draft assumption: `eval`/`template`'s argument must be `"program:"`-prefixed to evaluate as Template Program Mode (matching real upstream's own `TemplateFormatter.evaluate` dispatch) -- any other form would go through upstream's *separate* old-style `{field}`/`{field:func}` shorthand-template compiler, which isn't ported anywhere in this crate; this port reports a real error for that case instead of silently misinterpreting the string as GPM. **#520 (GUI_FUNCTIONS, 4 functions) shipped all 4, and corrected a real bug found in #519's `is_dark_mode`**: `selected_books`/`sort_book_ids`/`selected_column`/`show_dialog` all need a real Qt GUI (`calibre.gui2.ui.get_gui()`, `QDialog`) that doesn't exist anywhere in this port; real upstream's own `only_in_gui_error(name)` fallback for exactly this situation RAISES `ValueError('The function {name} can be used only in the GUI')` -- checked directly rather than assumed, which caught that #519's `is_dark_mode` had wrongly returned `""` instead of that same real error (its own doc comment even mis-cited `only_in_gui_error` as "returning" when the function actually raises). All 5 GUI-only functions (`is_dark_mode` + the 4 new ones) now live together in `calibre_utils::formatter::misc_functions`, reporting that same real error unconditionally, driven by one shared `GUI_ONLY_FUNCTIONS` list so the dispatch/arity/error-message can't drift apart again). **#524 (formats_sizes/paths/modtimes/path_segments, the last 4 DB_FUNCS deferred from #514) shipped all 4** in `calibre_db::formatter_functions`, backed by a new `Cache::format_file_info(book_id) -> Vec<FormatFileInfo>` (`fmt`/real stored `size`/derived absolute `path`) built on a new `FieldStore::formats_for_book` exposing `FormatsTable`'s already-bulk-loaded `fname_map`/`size_map` (the real stored `data.name` filename, not a re-derivation from the book's current title, which could drift if the title changed after the format was added -- more faithful to upstream's own `format_abspath` than `get_data_as_dict`'s existing inline path logic). `formats_modtimes` has no DB-stored mtime anywhere in this crate's schema read (confirmed by reading `tables.rs`) -- matches `extra_files.rs`'s own precedent of a real filesystem `stat()` call on the derived path at read time, with per-file `stat` failures silently excluded (matching upstream's own `mi.get('format_metadata', {})`, populated only for formats that resolved to a real file). Disclosed, verified-against-the-real-signature narrowing: `formats_paths`'s real Python signature is `(self, ..., sep=',')` -- a single optional positional argument, NOT `*args`, despite `arg_count = -1` -- so 2+ arguments is a real error here too, not silently accepted. This completes ALL of #514-520 + #524. #521 (`ffml_processor.py`) is now CLOSED too, as its own entry above describes -- the entire #460 epic is complete)
- [x] html2text.py
- [ ] https.py
- [x] icu.c / icu.py (issue #459 -- `calibre_utils::icu`'s `strcmp`/`primary_strcmp`: real Unicode Collation Algorithm string comparison via the pure-Rust `icu` (ICU4X) crate, `compiled_data` feature, chosen over `rust_icu`'s real ICU4C bindings because bindgen fails on this box's headless libclang install (no bundled builtin C headers) and this project already prefers pure-Rust over new C-toolchain deps when a real one exists (see `calibre_ebooks::spell`'s `spellbook` choice, #59). Wired into the real sort call sites that used to do a plain `to_lowercase().cmp()`: `calibre_db::categories::sort_tags` and `calibre_srv::opds::sort_key_for`/`ajax::sort_key_for`(shared). Case folding (`lower`/`upper`/`capitalize`/`title_case`) stays on Rust stdlib, unchanged. Disclosed narrowing: exposes comparators (`strcmp`/`primary_strcmp`) instead of upstream's byte `sort_key`, since ICU4X's `Collator` doesn't expose raw sort keys and Rust's `sort_by` doesn't need them. Disclosed gap: `primary_contains`/`primary_find` (collation-strength substring search, ICU4C's `usearch.h`) has no ICU4X equivalent -- `calibre_db::search`'s own pre-existing NFD+lowercase approximation is unchanged)
- [ ] icu_calibre_utils.h (C-only glue for the ICU4C binding path not taken, see above)
- [ ] icu_test.py (native Rust tests written instead -- see `calibre_utils::icu`'s own test module)
- [ ] img.py
- [x] imghdr.py
- [ ] inotify.py
- [ ] iphlpapi.py
- [ ] ipython.py
- [ ] ip_routing.py
- [x] iso8601.py
- [ ] linux_trash.py
- [x] localization.py (language-code half only, issue #140: `calibre_utils::localization::canonicalize_lang`/`lang_as_iso639_1`, backed by the `isolang` crate's real ISO 639 tables instead of upstream's bundled `iso639.calibre_msgpack` resource. The translation-catalog machinery -- `get_lang`, `set_translators`, UI language listing -- is out of scope, matching the issue)
- [x] localunzip.py (issue #469 -- `calibre_utils::localunzip`: real fallback ZIP reader for missing/damaged central directories, local-header-only scanning with resync-past-corruption, stored+raw-deflate entries, streaming/data-descriptor entries, `LocalZipFile` (read/getinfo/extract_all). Disclosed narrowings: `decode_arcname`'s chardet fallback narrowed to UTF-8-or-lossy; `safe_replace` (in-place damaged-ZIP repair) not ported, no caller needs it yet; Windows-reserved-filename check not ported. Zip-bomb guard reshaped from upstream's per-chunk decompress-call-count heuristic to a total-output-size cap, verified by a real test compressing 200MB of zeros into <1MB and confirming the cap rejects it)
- [x] lock.py (`calibre_utils::lock`; Linux subset only -- `flock`-based `lock_file` + abstract-Unix-socket `create_single_instance_mutex`. Windows/macOS branches N/A, see the module's own doc)
- [x] logging.py
- [ ] matcher.c
- [ ] matcher.py
- [ ] mdns.py
- [x] network.py
- [x] ordered_dict.py
- [x] random_ua.py
- [ ] rapydscript.py
- [x] recycle_bin.py
- [x] resources.py
- [ ] run_tests.py
- [ ] safe_atexit.py
- [x] search_query_parser.py
- [x] search_query_parser_test.py
- [ ] serialize.py
- [x] seven_zip.py
- [ ] shared_file.py
- [ ] shm.py
- [x] short_uuid.py
- [x] smartypants.py
- [x] smtp.py (issue #468 -- `calibre_utils::smtp`: real SMTP relay sending via the `lettre` crate, redesigned around it rather than porting `smtp.py`'s own MIME/smtplib wrapping line for line. `create_mail` (plain text or `multipart/mixed` with one attachment) + `send_via_relay` (TLS/SSL/none, matching upstream's `encryption` choices onto `lettre`'s `starttls_relay`/`relay`/`builder_dangerous`). Verified with a real TCP round trip against a hand-written fake SMTP server in-test, not a mocked transport. Disclosed narrowings: direct-to-MX delivery (`sendmail_direct`, needs a DNS resolver dependency, no caller uses it), the maildir-backed retry queue and `--fork` background-delivery CLI orchestration (no send-to-device/email feature exists yet to drive that design) not ported)
- [x] smtplib.py (N/A -- upstream's own lightly-patched copy of Python stdlib `smtplib`, not calibre-specific logic; see #468's own filed scope. Not to be confused with the *different* `polyglot/smtplib.py`, also N/A, below)
- [x] socket_inheritance.py
- [ ] speedup.c
- [x] speedups.py
- [x] tdir_in_cache.py (`calibre_utils::tdir_in_cache`; `fcntl`-record-lock-based crash-robust temp dirs, narrowed to skip the `atexit` eager-cleanup registration -- see the module's own doc)
- [x] terminal.py
- [ ] test_lock.py
- [x] text2int.py
- [ ] threadpool.py
- [x] titlecase.py
- [ ] unicode-test.opf
- [x] unicode_names.py
- [x] unrar.py
- [x] unsmarten.py
- [ ] webengine.py
- [x] wordcount.py
- [x] xml_parse.py
- [ ] zipfile.py
- [ ] __init__.py

#### fonts

- [ ] freetype.cpp (part of #556, low priority)
- [ ] free_type.py (part of #556, low priority)
- [ ] metadata.py (part of #556, low priority)
- [ ] scanner.py (part of #556, low priority)
- [ ] subset.py (issue #553, depends on the sfnt sub-cluster below)
- [x] utils.py (issue #548 CLOSED -- `calibre_utils::fonts::utils`: standalone sfnt table-directory reader (`get_tables`/`get_table`), OS/2 characteristics (`get_font_characteristics`), name-table parsing (`get_font_names`/`get_font_names2`/`get_all_font_names`, with the real Windows/Mac/Unicode-platform fallback chain), checksum verification/fixup (`verify_checksums`/`remove_embed_restriction`/`is_font_embeddable`), and format-4 cmap glyph lookup (`get_glyph_ids`/`supports_text`) -- no dependency on the fuller `sfnt/` object model needed. `panose_to_css_generic_family` is now canonically here; `docx::fonts::panose_to_css_generic_family` (ported earlier, before this module existed) delegates to it instead of duplicating. Real payoff: this is exactly what issue #169's `embed_all_fonts` cites as a missing dependency (font names/characteristics/embeddability for whole-font embedding, no subsetting needed). Disclosed narrowings: the `ttLib`-object calling convention (unreachable, nothing in this port constructs such an object) and `get_printable_characters`'s stdlib-only (no Unicode general-category) filtering, both documented in the module's own doc comment)
- [ ] winfonts.cpp (part of #556, low priority)
- [ ] win_fonts.py (part of #556, low priority)
- [ ] __init__.py

##### sfnt

- [ ] cmap.py (issue #551)
- [ ] common.py (issue #555 -- its only real importer is gsub.py, not container.py)
- [x] container.py (issue #549 CLOSED -- `calibre_utils::fonts::sfnt::container::Sfnt`: real table-tag -> bytes map, sorted-tag iteration, real re-serialization with correct per-table/whole-file checksums and the `head` table's checksum-adjustment field. Every table (known or not) round-trips as opaque bytes for now -- the specialized `HeadTable`/`GlyfTable`/`CmapTable`/etc dispatch is separate, dependency-ordered follow-up scope (#550-555); this container doesn't need it to parse/rebuild a real font. Disclosed narrowings in the module's own doc comment: the `ttLib`-object constructor path is unreachable; `Sfnt.get_all_font_names` (a real upstream method importing a function, `metadata.get_font_names2`, that doesn't exist anywhere in `metadata.py`) is real, unreachable dead code in upstream itself -- confirmed by grep that every real caller of "get all font names" calls `utils.py`'s own version directly -- so it's not ported; the real sfnt-version signature set's dead 5-byte `type1` entry (compared against an always-4-byte slice, so it can never match) is reproduced by omission rather than "fixed")
- [x] errors.py (issue #549 CLOSED -- `calibre_utils::fonts::sfnt::errors::{UnsupportedFont,NoGlyphs}`)
- [ ] glyf.py (issue #550)
- [ ] gsub.py (issue #555, low priority)
- [ ] head.py (issue #550)
- [ ] kern.py (issue #552)
- [ ] loca.py (issue #550)
- [ ] maxp.py (issue #550)
- [ ] merge.py (issue #555, low priority)
- [ ] metrics.py (issue #552)
- [ ] subset.py (issue #553 -- the real payoff, closes oeb::polish::subset.rs's subset_all_fonts todo!())
- [x] __init__.py (issue #549 CLOSED -- `calibre_utils::fonts::sfnt::{align_block,max_power_of_two,load_font}`)

###### cff

- [ ] constants.py
- [ ] dict_data.py
- [ ] table.py
- [ ] writer.py
- [ ] __init__.py

#### hyphenation

- [x] dictionaries.py (`calibre_utils::hyphenation::language_for_locale`/`dictionary_for_language`; locale-alias table narrowed since upstream's `locales.json` isn't vendored -- see the module's own doc)
- [x] hyphen.c (subsumed by the `hyphenation` crate's real Knuth-Liang engine, built from the same TeX/LibreOffice pattern data upstream's own `dictionaries.tar.xz` uses -- embedded via the `embed_all` feature, not a missing-asset gap)
- [x] hyphenate.py (`calibre_utils::hyphenation::add_soft_hyphens`/`add_soft_hyphens_to_words`; the HTML-tree-walking half is not ported here -- this crate has no DOM type, see the module's own doc)
- [x] test_hyphenation.py (covered by this module's own unit tests, incl. the classic Knuth-Liang "hy-phen-a-tion" example)
- [x] __init__.py (empty package marker, N/A)

#### imageops

- [ ] imageops.cpp
- [ ] imageops.h
- [ ] imageops.sip
- [ ] ordered_dither.cpp
- [ ] quantize.cpp

#### ipc

- [ ] job.py
- [ ] launch.py
- [ ] pool.py
- [ ] server.py
- [ ] simple_worker.py
- [ ] worker.py
- [ ] __init__.py

#### lzx

- [x] compressor.c
- [x] lzc.c
- [x] lzc.h
- [x] lzxc.c
- [x] lzxc.h
- [x] lzxd.c
- [x] lzxd.h
- [x] lzxmodule.c
- [x] mspack.h (only the `mspack_system` glue the LZX codec uses; the
      rest of libmspack's CAB/CHM/etc. readers are out of scope)
- [x] system.h (build-time portability shims; no Rust equivalent needed)

#### magick

- [ ] draw.py
- [ ] legacy.py
- [ ] __init__.py

#### msdes

- [x] d3des.h
- [x] des.c
- [x] msdesmodule.c
- [x] spr.h

#### open_with

- [x] linux.py (`calibre_utils::open_with::linux`; real `.desktop` file discovery/parsing, XDG icon-theme lookup, `%f`/`%u`/`%i`/etc field-code substitution -- see that module's own doc for narrowed pieces: no persistent icon cache, plain string sort instead of ICU numeric sort, env-var-based UI language)
- [ ] osx.py (not ported -- native macOS Launch Services/`.app` bundle integration, unverifiable without a macOS host; see `calibre_utils::open_with`'s module doc)
- [ ] windows.py (not ported -- native Win32 registry + Qt integration, unverifiable without a Windows host; same doc)
- [x] __init__.py (empty package marker, N/A)

#### opensearch

- [x] description.py (`calibre_utils::opensearch::Description`; real HTTP fetch via `reqwest::blocking` + roxmltree parsing, verified against a local in-process HTTP server)
- [x] query.py (`calibre_utils::opensearch::Query`; macro substitution via `url::form_urlencoded`)
- [x] url.py (`calibre_utils::opensearch::Url`)
- [x] __init__.py (empty package marker, N/A)

#### podofo

- [ ] doc.cpp
- [ ] fonts.cpp
- [ ] global.h
- [ ] images.cpp
- [ ] impose.cpp
- [ ] outline.cpp
- [ ] outlines.cpp
- [ ] output.cpp
- [ ] podofo.cpp
- [ ] test.cpp
- [ ] utils.cpp
- [ ] __init__.py

#### rcc

N/A, entire cluster (issue #75). This is a SIP/C++ binding for Qt's
`.qrc` resource-compiler format (embedding files into a Qt binary as
a `QResource`). This project's UI is Tauri, not Qt -- Tauri has its
own asset-bundling mechanism (`tauri.conf.json`'s `bundle`/`resources`
config, or `include_bytes!`/`include_dir!` for anything needing to be
compiled in) -- so there is no Qt resource format to compile for.

- [x] rcc.cpp (N/A -- Qt-specific, no Qt in this project)
- [x] rcc.h (N/A -- Qt-specific, no Qt in this project)
- [x] rcc.sip (N/A -- Qt-specific, no Qt in this project)
- [x] __init__.py (N/A -- empty package marker)

#### spell

- [ ] hunspell_wrapper.cpp
- [ ] __init__.py

#### tts

- [ ] piper.cpp
- [ ] piper.py
- [ ] __init__.py

#### windows

- [ ] common.h
- [ ] wintest.py
- [ ] wintoast.cpp
- [ ] wintoastlib.cpp
- [ ] wintoastlib.h
- [ ] winutil.cpp
- [ ] __init__.py

#### winreg

- [ ] dde.py
- [ ] default_programs.py
- [ ] lib.py
- [ ] __init__.py

#### wmf

- [x] emf.py (`calibre_utils::wmf::emf`; real EMR record-stream parser, now wired up in `calibre_ebooks::docx::images::read_image_data` to actually convert embedded EMF images to PNG -- closes a gap that module's own doc previously disclosed)
- [x] parse.py (`calibre_utils::wmf::parse`; real WMF record-stream parser)
- [x] __init__.py (`calibre_utils::wmf::dib`; DIB header parsing + BMP wrapping, `to_png` via the `image` crate instead of Qt)

### web

- [ ] __init__.py

#### feeds

- [ ] news.py
- [ ] templates.py
- [ ] __init__.py

##### recipes

- [ ] collection.py
- [ ] model.py
- [ ] __init__.py

#### fetch

- [ ] simple.py (`RecursiveFetcher` -- large, needs its own scoping pass, tracked as issue #455)
- [x] utils.py (`calibre_ebooks::web::fetch::utils`; real `rescale_image`/`prepare_masthead_image`, backed by the `image` crate + this crate's own already-ported `fit_image`)
- [x] __init__.py (empty package marker, N/A)

#### site_parsers

- [x] natgeo.py (`calibre_ebooks::web::site_parsers::natgeo`; real JSON->HTML extraction)
- [x] nytimes.py (`calibre_ebooks::web::site_parsers::nytimes`; real JSON->HTML extraction, incl. the "live" liveblog JSON shape; `download_url_from_wayback`/`download_url` not ported -- a specific calibre-ebook.com proxy fetch, orthogonal to the parsing logic, see the module's own doc)
- [ ] __init__.py

## src/css_selectors

- [ ] errors.py
- [ ] ordered_set.py
- [ ] parser.py
- [ ] select.py
- [ ] tests.py
- [ ] __init__.py

## src/odf

- [ ] anim.py
- [ ] attrconverters.py
- [ ] chart.py
- [ ] config.py
- [ ] dc.py
- [ ] dr3d.py
- [ ] draw.py
- [ ] easyliststyle.py
- [ ] element.py
- [ ] elementtypes.py
- [ ] form.py
- [ ] grammar.py
- [ ] load.py
- [ ] manifest.py
- [ ] math.py
- [ ] meta.py
- [ ] namespaces.py
- [ ] number.py
- [ ] odf2moinmoin.py
- [ ] odf2xhtml.py
- [ ] odfmanifest.py
- [ ] office.py
- [ ] opendocument.py
- [ ] presentation.py
- [ ] script.py
- [ ] style.py
- [ ] svg.py
- [ ] table.py
- [ ] teletype.py
- [ ] text.py
- [ ] thumbnail.py
- [ ] userfield.py
- [ ] xforms.py
- [ ] __init__.py

## src/perfect-hashing

### frozen

- [ ] algorithm.h
- [ ] CMakeLists.txt
- [ ] map.h
- [ ] random.h
- [ ] set.h
- [ ] string.h
- [ ] unordered_map.h
- [ ] unordered_set.h

#### bits

- [ ] algorithms.h
- [ ] basic_types.h
- [ ] constexpr_assert.h
- [ ] defines.h
- [ ] elsa.h
- [ ] exceptions.h
- [ ] pmh.h
- [ ] version.h

## src/polyglot

N/A, entire cluster (issue #87). Every file here is a Python-2/3
compatibility shim (a thin re-export of a stdlib module, or a trivial
wrapper like `as_base64_bytes`) with zero calibre-specific logic --
verified by reading every file. Rust has no Python-2/3 split to paper
over, and callers use Rust std/crates (`base64`, `std::sync::mpsc`,
`std::io::Cursor`, etc.) directly at the call site instead of through
an indirection layer. Nothing here needs a Rust equivalent.

- [x] binary.py (N/A -- `as_base64_*`/`as_hex_*` trivially map to the `base64`/`hex` crates at each call site)
- [x] builtins.py (N/A -- Python-2/3 compat shim, e.g. `unicode_type = str`)
- [x] functools.py (N/A -- re-exports `functools.lru_cache`)
- [x] html_entities.py (N/A -- re-exports `html.entities.name2codepoint`)
- [x] http_client.py (N/A -- re-exports `http.client`)
- [x] http_cookie.py (N/A -- re-exports `http.cookiejar`)
- [x] http_server.py (N/A -- re-exports `http.server`)
- [x] io.py (N/A -- a `StringIO` subclass that also accepts bytes)
- [x] plistlib.py (N/A -- re-exports `plistlib`)
- [x] queue.py (N/A -- re-exports `queue.Queue`/`Empty`/`Full`/etc.)
- [x] reprlib.py (N/A -- re-exports `reprlib.repr`)
- [x] smtplib.py (N/A -- re-exports `smtplib`)
- [x] socketserver.py (N/A -- re-exports `socketserver`)
- [x] urllib.py (N/A -- re-exports `urllib.parse`/`urllib.request`)
- [x] __init__.py (N/A -- empty package marker)

## src/pyj

- [ ] ajax.pyj
- [ ] autoreload.pyj
- [ ] complete.pyj
- [ ] date.pyj
- [ ] dom.pyj
- [ ] editor.pyj
- [ ] file_uploads.pyj
- [ ] fs_images.pyj
- [ ] iframe_comm.pyj
- [ ] image_popup.pyj
- [ ] initialize.pyj
- [ ] live_css.pyj
- [ ] lru_cache.pyj
- [ ] modals.pyj
- [ ] popups.pyj
- [ ] qt.pyj
- [ ] range_utils.pyj
- [ ] select.pyj
- [ ] session.pyj
- [ ] srv.pyj
- [ ] test.pyj
- [ ] testing.pyj
- [ ] test_annotations.pyj
- [ ] test_date.pyj
- [ ] test_utils.pyj
- [ ] utils.pyj
- [ ] viewer-main.pyj
- [ ] widgets.pyj
- [ ] worker.pyj

### book_list

**Issue #501 (library browser MVP, part of the #432 browser-UI epic)
ported a minimum-viable slice of this SPA to TypeScript from scratch --
`web/src/library/{types,api,query}.ts` +
`web/src/components/{LibraryView,CategoryBrowser,BookDetailsPanel}.vue`
-- not a file-by-file transpile of the `.pyj` sources below, so no
individual file here is a full [x]. Covers: cover-grid view (only one
of upstream's three render modes -- `details_list`/`custom_list`
deferred), pagination/sort/virtual-library switching, a search bar,
a category browser (clicking an item builds an exact-match search
query via `query.ts::categoryItemToQuery`, reusing the same
`/ajax/search` pipeline rather than wiring `/ajax/books_in` as a
second parallel data path -- a disclosed simplification), and a
trimmed book-details panel (metadata, format downloads, a "Read"
button into #499's reader for epub/kepub). Verified end to end against
a live `calibre_srv` + a real multi-book library (see #501's closing
comment for the exact HTTP round trip). Explicitly deferred to their
own issues per #501's own body: edit-metadata UI, add-books
(drag-and-drop upload), conversion UI, FTS UI, notes UI, alternate
view modes, offline/local-books panel.

- [ ] add.pyj (deferred -- add-books UI, own future issue)
- [ ] book_details.pyj (trimmed equivalent: `BookDetailsPanel.vue`)
- [ ] comments_editor.pyj (deferred -- part of edit-metadata UI)
- [ ] constants.pyj
- [ ] conversion_widgets.pyj (deferred -- conversion UI, own future issue)
- [ ] convert_book.pyj (deferred -- conversion UI, own future issue)
- [x] cover_grid.pyj -- `LibraryView.vue`'s grid (cover-grid mode only)
- [ ] custom_list.pyj (deferred alternate view mode)
- [ ] delete_book.pyj (deferred -- not in the MVP's read-only scope)
- [ ] details_list.pyj (deferred alternate view mode)
- [ ] edit_metadata.pyj (deferred, own future issue)
- [ ] fts.pyj (deferred -- FTS UI, own future issue)
- [ ] globals.pyj
- [ ] home.pyj
- [ ] item_list.pyj
- [x] library_data.pyj -- `library/api.ts` (against `/ajax/*`, not upstream's `interface-data/*`)
- [ ] local_books.pyj (deferred -- offline/local-books panel, own future issue)
- [x] main.pyj -- `web/src/main.ts`'s router (#498) + `LibraryView.vue` as its bootstrap-equivalent
- [ ] prefs.pyj (deferred -- session-data endpoint exists (#500) but no settings UI yet)
- [x] router.pyj -- Vue Router (`main.ts`), not a mode/panel-router reimplementation
- [x] search.pyj -- `LibraryView.vue`'s search bar + `/ajax/search`
- [ ] show_note.pyj (deferred -- notes UI, own future issue)
- [ ] theme.pyj
- [x] top_bar.pyj -- `LibraryView.vue`'s toolbar (search/sort/vl)
- [ ] ui.pyj
- [x] views.pyj -- `CategoryBrowser.vue` (category browsing only; no tag-browser tree UI beyond a flat item list)
- [ ] __init__.pyj

### read_book

**Issue #499 (reader MVP, part of the #432 browser-UI epic) ported a
minimum-viable slice of this SPA to TypeScript from scratch --
`web/src/reader/{api,unserialize,virtualLinks,position,toc}.ts` +
`web/src/components/ReaderView.vue` -- not a file-by-file transpile of
the `.pyj` sources below, so no individual file here is a full [x].
Real, verified end to end against a live `calibre_srv` + a real EPUB:
manifest/book-file fetch (`ui.pyj`'s role), JSON-tree-to-DOM +
virtualized-resource resolution (`resources.pyj`'s role, confirmed
byte-for-byte against a live server response), `data-{link_uid}`
anchor click handling, TOC navigation (`toc.pyj`'s role, narrowed --
no bordering-node/anchor-visibility tracking), last-read-position
persistence via a real (non-CFI) position scheme (see `position.ts`'s
own doc for why real EPUB CFI computation, `cfi.pyj`, ~1000 lines, is
deferred). Continuous-scroll only, whole-spine-file navigation
granularity (no real pagination math, `flow_mode.pyj`/`paged_mode.pyj`
not ported). Explicitly NOT ported by this slice: in-book search,
highlights/annotations/bookmarks UI, MathJax, TTS/audiobooks, touch
gestures, settings panel, footnotes, offline/IndexedDB caching
(`db.pyj`'s whole-book-predownload model deliberately not replicated
-- files are fetched on demand instead).**

- [ ] anchor_visibility.pyj
- [ ] annotations.pyj
- [ ] bookmarks.pyj
- [ ] cfi.pyj
- [ ] content_popup.pyj
- [ ] db.pyj (not ported -- on-demand fetch used instead, see banner above)
- [ ] extract.pyj
- [ ] find.pyj
- [ ] flow_mode.pyj (not ported -- native iframe scroll used instead)
- [ ] footnotes.pyj
- [ ] gestures.pyj
- [ ] globals.pyj (partial -- narrow equivalents inline in `ReaderView.vue`, #499)
- [ ] goto.pyj (partial -- narrow equivalent inline in `ReaderView.vue`, #499)
- [ ] highlights.pyj
- [ ] hints.pyj
- [ ] iframe.pyj (partial -- parent-frame-side DOM injection only, no postMessage boundary, see `unserialize.ts`, #499)
- [ ] mathjax.pyj
- [ ] open_book.pyj (not applicable -- standalone-viewer-only)
- [ ] overlay.pyj
- [ ] paged_mode.pyj
- [ ] profiles.pyj
- [ ] read_aloud.pyj
- [ ] read_audio_ebook.pyj
- [ ] referencing.pyj
- [ ] resources.pyj (partial -- `unserialize.ts`/`virtualLinks.ts`, #499)
- [ ] scrollbar.pyj
- [ ] search.pyj
- [ ] search_worker.pyj
- [ ] selection_bar.pyj
- [ ] settings.pyj
- [ ] shortcuts.pyj (partial -- page-turn keys only, `ReaderView.vue`, #499)
- [ ] smil.pyj
- [ ] test_cfi.pyj
- [ ] timers.pyj
- [ ] toc.pyj (partial -- `toc.ts`, #499)
- [ ] touch.pyj
- [ ] tts.pyj
- [ ] ui.pyj (partial -- `api.ts`/`ReaderView.vue`, #499)
- [ ] view.pyj (partial -- `ReaderView.vue`, #499)
- [ ] viewport.pyj
- [ ] word_actions.pyj
- [ ] __init__.pyj

#### prefs

- [ ] colors.pyj
- [ ] fonts.pyj
- [ ] font_size.pyj
- [ ] head_foot.pyj
- [ ] keyboard.pyj
- [ ] layout.pyj
- [ ] main.pyj
- [ ] misc.pyj
- [ ] scrolling.pyj
- [ ] selection.pyj
- [ ] touch.pyj
- [ ] user_stylesheet.pyj
- [ ] utils.pyj
- [ ] __init__.pyj

### viewer

- [ ] constants.pyj
- [ ] router.pyj
- [ ] __init__.pyj

## src/qt

- [ ] core.py
- [ ] core.pyi
- [ ] core_name_map.py
- [ ] dbus.py
- [ ] dbus.pyi
- [ ] dbus_name_map.py
- [ ] loader.py
- [ ] webengine.py
- [ ] webengine.pyi
- [ ] webengine_name_map.py
- [ ] __init__.py
- [ ] __main__.py

## src/templite

- [ ] __init__.py

## src/tinycss

- [ ] color3.py
- [ ] css21.py
- [ ] decoding.py
- [ ] fonts3.py
- [ ] media3.py
- [ ] page3.py
- [ ] parsing.py
- [ ] tokenizer.c
- [ ] tokenizer.py
- [ ] token_data.py
- [ ] version.py
- [ ] __init__.py

## src/unicode_names

N/A, entire cluster (issue #89). Subsumed by the `unicode_names2` crate
(pure-Rust Unicode character name database), already wired up as
`calibre_utils::unicode_names` for `src/calibre/utils/unicode_names.py`
-- this top-level `src/unicode_names/` C library is the exact same
generated character-name database, just as a C extension instead of
the Python module that wraps it.

- [x] data-types.h (N/A -- subsumed by the `unicode_names2` crate)
- [x] names.h (N/A -- subsumed by the `unicode_names2` crate)
- [x] unicode_names.c (N/A -- subsumed by the `unicode_names2` crate)

