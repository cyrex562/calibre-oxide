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
- [x] annotations.py
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
- [ ] css_transform_rules.py (deferred — 472-LOC rules engine, its own PR)
- [x] html_entities.c (ported to `html_entities.rs` — `html-escape` crate supersedes the C lookup table)
- [x] html_entities.h (ported — the 5000-line table is now `html-escape`'s embedded WHATWG data)
- [x] html_entities.py (ported to `html_entities.rs`)
- [ ] html_transform_rules.py (deferred — 669-LOC rules engine, its own PR)
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
- [x] cli.py (ported into `calibre_conversion::cli_helpers` — input/output validation, USAGE banner, HEURISTIC_OPTIONS. Plugin-driven option assembly deferred to #126)
- [x] config.py (ported into `calibre_conversion::config` — GuiRecommendations JSON roundtrip, OPTIONS registry, format-preference helpers, atomic save/load. DB-integrated `*_specifics` deferred to #93 LibraryHandle)
- [x] plumber.py
- [x] preprocess.py
- [x] search_replace.py
- [x] utils.py

##### plugins

- [ ] __init__.py
- [x] azw4_input.py (Wrapper)
- [x] chm_input.py (Regex/Placeholder)
- [x] comic_input.py
- [x] djvu_input.py (Placeholder)
- [x] docx_input.py
- [x] docx_output.py (Stub)
- [x] epub_input.py
- [x] epub_output.py
- [x] fb2_input.py
- [x] fb2_output.py
- [x] html_input.py
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
- [x] txt_input.py
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
- [x] `container.py` -> `docx/container.rs`
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
- [x] `periodical.py` -> `epub/periodical.rs`

##### cfi

- [x] `__init__.py` (empty upstream; the module root is `epub/cfi/mod.rs`)
- [x] `epubcfi.ebnf` -> reproduced as the grammar in `epub/cfi/mod.rs` docs (issue #25 spells it `epublfi.ebnf`)
- [x] `parse.py` -> `epub/cfi/parse.rs`
- [x] `tests.py` -> unit tests in `epub/cfi/parse.rs`, plus `tests/epub_cfi_test.rs` cross-validation

#### fb2

- [x] `__init__.py` (`base64_decode`) -> `fb2/mod.rs`
- [x] `fb2ml.py` -> `fb2/fb2ml.rs` (the `Stylizer` becomes a trait — see #132/#130 for the missing CSS cascade; `--pretty-print` re-serialization and image transcoding are noted in #145)

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
- [x] main.py
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
- [ ] jacket.py
- [ ] kepubify.py
- [ ] main.py
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
- [ ] field_metadata.py
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

- [ ] ajax.py
- [ ] auth.py
- [ ] auto_reload.py
- [ ] bonjour.py
- [ ] books.py
- [ ] cdb.py
- [ ] changes.py
- [ ] code.py
- [ ] content.py (partial -- `cover`/`thumb`/format downloads only, no etag caching, no thumbnail resizing, no `opf`/`json`; see `calibre_srv::content`'s module doc)
- [ ] convert.py
- [ ] embedded.py
- [x] errors.py
- [ ] fast_css_transform.cpp
- [ ] fts.py
- [ ] handler.py
- [ ] html_as_json.cpp
- [x] http_request.py (subsumed by `axum`/`hyper`'s own HTTP parsing -- see `calibre_srv`'s crate root doc)
- [x] http_response.py (subsumed by `axum`/`hyper`)
- [ ] jobs.py
- [ ] last_read.py
- [ ] legacy.py
- [ ] library_broker.py
- [x] loop.py (subsumed by `tokio`'s async runtime)
- [ ] manage_users_cli.py
- [ ] metadata.py
- [ ] opds.py (partial -- root nav feed + title/newest acquisition feeds only, no categories/search/multi-library; see `calibre_srv::opds`'s module doc)
- [x] opts.py
- [x] pool.py (subsumed by `tokio`'s task scheduler)
- [ ] pre_activated.py
- [ ] render_book.py
- [x] routes.py (subsumed by `axum::Router`)
- [ ] standalone.py (`calibre_srv`'s own minimal `main.rs` covers the same "run the server over TCP" role, not a port of this file's process-management machinery)
- [ ] TODO.rst
- [ ] users.py
- [ ] users_api.py
- [x] utils.py (the still-relevant pure-logic slice -- `Offsets`/`http_date` -- is ported as `calibre_srv::utils`; the rest is socket/logging plumbing subsumed by `axum`/`tokio`)
- [ ] web_socket.py
- [ ] __init__.py

### translations

- [ ] dynamic.py
- [ ] msgfmt.py
- [ ] __init__.py

### utils

- [ ] bibtex.py
- [ ] browser.py
- [ ] certgen.c
- [ ] certgen.py
- [ ] cleantext.py
- [ ] cocoa.m
- [ ] complete.py
- [x] config.py
- [x] config_base.py
- [ ] copy_files.py
- [ ] copy_files_test.py
- [ ] cpp_binding.h
- [x] date.py
- [ ] exim.py
- [ ] ffml_processor.py
- [ ] ffmpeg.c
- [x] filenames.py
- [ ] file_type_icons.py
- [ ] forked_map.py
- [ ] formatter.py
- [ ] formatter_functions.py
- [x] html2text.py
- [ ] https.py
- [ ] icu.c
- [ ] icu.py
- [ ] icu_calibre_utils.h
- [ ] icu_test.py
- [ ] img.py
- [x] imghdr.py
- [ ] inotify.py
- [ ] iphlpapi.py
- [ ] ipython.py
- [ ] ip_routing.py
- [x] iso8601.py
- [ ] linux_trash.py
- [ ] localization.py
- [ ] localunzip.py
- [ ] lock.py
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
- [ ] smtp.py
- [ ] smtplib.py
- [x] socket_inheritance.py
- [ ] speedup.c
- [x] speedups.py
- [ ] tdir_in_cache.py
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

- [ ] freetype.cpp
- [ ] free_type.py
- [ ] metadata.py
- [ ] scanner.py
- [ ] subset.py
- [ ] utils.py
- [ ] winfonts.cpp
- [ ] win_fonts.py
- [ ] __init__.py

##### sfnt

- [ ] cmap.py
- [ ] common.py
- [ ] container.py
- [ ] errors.py
- [ ] glyf.py
- [ ] gsub.py
- [ ] head.py
- [ ] kern.py
- [ ] loca.py
- [ ] maxp.py
- [ ] merge.py
- [ ] metrics.py
- [ ] subset.py
- [ ] __init__.py

###### cff

- [ ] constants.py
- [ ] dict_data.py
- [ ] table.py
- [ ] writer.py
- [ ] __init__.py

#### hyphenation

- [ ] dictionaries.py
- [ ] hyphen.c
- [ ] hyphenate.py
- [ ] test_hyphenation.py
- [ ] __init__.py

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

- [ ] linux.py
- [ ] osx.py
- [ ] windows.py
- [ ] __init__.py

#### opensearch

- [ ] description.py
- [ ] query.py
- [ ] url.py
- [ ] __init__.py

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

- [ ] rcc.cpp
- [ ] rcc.h
- [ ] rcc.sip
- [ ] __init__.py

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

- [ ] emf.py
- [ ] parse.py
- [ ] __init__.py

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

- [ ] simple.py
- [ ] utils.py
- [ ] __init__.py

#### site_parsers

- [ ] natgeo.py
- [ ] nytimes.py
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

- [ ] binary.py
- [ ] builtins.py
- [ ] functools.py
- [ ] html_entities.py
- [ ] http_client.py
- [ ] http_cookie.py
- [ ] http_server.py
- [ ] io.py
- [ ] plistlib.py
- [ ] queue.py
- [ ] reprlib.py
- [ ] smtplib.py
- [ ] socketserver.py
- [ ] urllib.py
- [ ] __init__.py

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

- [ ] add.pyj
- [ ] book_details.pyj
- [ ] comments_editor.pyj
- [ ] constants.pyj
- [ ] conversion_widgets.pyj
- [ ] convert_book.pyj
- [ ] cover_grid.pyj
- [ ] custom_list.pyj
- [ ] delete_book.pyj
- [ ] details_list.pyj
- [ ] edit_metadata.pyj
- [ ] fts.pyj
- [ ] globals.pyj
- [ ] home.pyj
- [ ] item_list.pyj
- [ ] library_data.pyj
- [ ] local_books.pyj
- [ ] main.pyj
- [ ] prefs.pyj
- [ ] router.pyj
- [ ] search.pyj
- [ ] show_note.pyj
- [ ] theme.pyj
- [ ] top_bar.pyj
- [ ] ui.pyj
- [ ] views.pyj
- [ ] __init__.pyj

### read_book

- [ ] anchor_visibility.pyj
- [ ] annotations.pyj
- [ ] bookmarks.pyj
- [ ] cfi.pyj
- [ ] content_popup.pyj
- [ ] db.pyj
- [ ] extract.pyj
- [ ] find.pyj
- [ ] flow_mode.pyj
- [ ] footnotes.pyj
- [ ] gestures.pyj
- [ ] globals.pyj
- [ ] goto.pyj
- [ ] highlights.pyj
- [ ] hints.pyj
- [ ] iframe.pyj
- [ ] mathjax.pyj
- [ ] open_book.pyj
- [ ] overlay.pyj
- [ ] paged_mode.pyj
- [ ] profiles.pyj
- [ ] read_aloud.pyj
- [ ] read_audio_ebook.pyj
- [ ] referencing.pyj
- [ ] resources.pyj
- [ ] scrollbar.pyj
- [ ] search.pyj
- [ ] search_worker.pyj
- [ ] selection_bar.pyj
- [ ] settings.pyj
- [ ] shortcuts.pyj
- [ ] smil.pyj
- [ ] test_cfi.pyj
- [ ] timers.pyj
- [ ] toc.pyj
- [ ] touch.pyj
- [ ] tts.pyj
- [ ] ui.pyj
- [ ] view.pyj
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

- [ ] data-types.h
- [ ] names.h
- [ ] unicode_names.c

