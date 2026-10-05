# The library model: a folder is the library

This is a deliberate divergence from calibre, decided 2026-09-26. It replaces
the managed-library model rather than sitting beside it.

## The two models

calibre owns its library folder. Adding a book **copies** it into
`<Author>/<Title (id)>/` and names the file after the title; editing the title
moves the files. The database and the directory tree are maintained together,
and the tree encodes author and title.

This project's model: **the folder is yours, and the database is an index of
what is in it.** Files are never moved or renamed unless you ask. App state
lives in a `.calibre-oxide/` subdirectory alongside the index.

```
~/Scans/                          <- the library is just this folder
├── Boiler Manual 1974.pdf        books.path = "",  data.name = "Boiler Manual 1974"
├── scan0042.pdf                  the title can be anything; the file keeps its name
├── Receipts/
│   └── 2019 invoice.pdf          books.path = "Receipts"
├── metadata.db                   a DERIVED query cache -- disposable, see below
└── .calibre-oxide/
    ├── changes/                  the authoritative append-only change log
    ├── snapshots/                periodic compaction of the log
    ├── covers/<uuid>.jpg         covers, keyed by book uuid
    ├── checksums.db              content hashes and file identity
    └── journal/                  the existing file-operation write-ahead journal
```

`metadata.db` stays at the library root, where it already is and where calibre
expects it. That placement is deliberate now rather than incidental: a hidden
dotfolder is exactly what a zip tool skips and a drag-select misses, so the file
a user would recognise as important sits where they will see and copy it.

### Why this is not less portable

The objection to raise here is that calibre's model is "portable and
self-describing", and this gives that up. Half of that is wrong and half of it
was never true in this tree:

- **Portability is unaffected.** A tracked folder is a folder with its index
  inside it and every recorded path relative to the library root. It copies to
  another drive and works. Nothing may ever store an absolute path — that is
  the single invariant the property rests on.
- **Self-describing was already not maintained.** calibre's libraries are
  rebuildable from `metadata.opf` sidecars in each book folder. In this tree
  those sidecars are written *only* by an explicit `calibredb backup_metadata`
  run, never on edit — so `restore.rs` already rebuilds from stale metadata.
  See "Open" below.

### Reading a real calibre library

Still works. Upstream's `format_abspath` resolves
`library_path + books.path + name + ext` — the same formula this project uses
— so a calibre library scans correctly, with `books.path` happening to be
`Author/Title (id)`. We stop *maintaining* that layout; we do not stop reading
it. There is no migration.

One sharp edge, and it is calibre's: when `format_abspath` does not find the
expected filename it falls back to scanning the directory for any file with a
matching extension **and renaming it** to what it expected. In
`<Author>/<Title>/` that is a plausible repair. In a flat folder of 400 PDFs it
would seize an arbitrary one. Real calibre can read a tracked folder; it must
not be allowed to repair one.

## Identity: knowing a file is the same file

Everything downstream — rename detection, move detection, orphans — reduces to
"is the thing at this path the thing I recorded?". Three signals, wildly
different costs:

| Signal | Cost | Catches | Misses |
| --- | --- | --- | --- |
| File identity — `(device, inode)` / `(volume, file index)` | one `stat` | renames and moves within one volume, exactly | anything crossing a volume; copies; identity reuse after delete |
| Size + mtime | same `stat` | cheap "has this changed?" gate; skip rehashing when both match | edits preserving both; coarse timestamps |
| Content hash | reads the whole file | everything, including cross-volume moves | nothing — but it can take hours |

`calibre_utils::filenames::file_identity` already implements the first
(`GetFileInformationByHandle` on Windows, the Unix equivalent elsewhere). It is
currently private and used only for a same-file check; it needs making public.

**The hash is BLAKE3, not SHA256.** The tree already stores BLAKE3 per
`(book_id, kind, key)` in `.calibre-oxide/checksums.db`, and BLAKE3 is several
times faster on large files — which is the difference between minutes and an
hour on a folder of 200 MB scans. Nothing hashes twice. The existing store must
gain a **content → book** lookup, which it does not have today: it can answer
"what is book 12's hash?" but not "which book is this file?", and the second is
what rename recovery needs.

## Scanning is two-phase

Grouping and duplicate detection depend on hashes, and hashing a large library
is minutes to hours of I/O. The scan must not block on it:

1. **Immediately** — walk the tree, and for every file create an entry from its
   `stat` and its own embedded metadata. A folder of 5,000 PDFs is browsable in
   seconds.
2. **In the background** — hash each file, then apply the hash-derived
   conclusions: duplicate pairs and format grouping. These appear as they land.

## When two files are one book

Filename agreement is **not** evidence. Two files called `scan0001` in
different folders have nothing to do with each other, and this is where the
model departs from both calibre and this project's existing
`find_books_in_directory` (which groups by lowercased stem).

| Evidence | Conclusion |
| --- | --- |
| Byte-identical content | the same file twice: track both, flag as a duplicate pair |
| Specific matching metadata, **different** formats | one book, several formats |
| Specific matching metadata, **same** format | probably one book twice: a duplicate |
| Anything else, including matching filenames | separate books |

Two constraints shape this:

- **`data` is `UNIQUE(book, format)`** — one file per format per book. So a hash
  match cannot become "two formats of one book"; the schema has no room. It is a
  duplicate, surfaced through the existing `/duplicates/scan` review panel
  (which today compares author and title, missing identical files with
  different metadata and falsely pairing different books that share a title —
  content hashing fixes both directions).
- **"Specific" is load-bearing.** A scanner-produced PDF usually has no Info
  dictionary at all, or a title like `Microsoft Word - untitled`. Unguarded
  metadata equality would merge every untitled scan in the folder into one
  book — in exactly the case this model exists for. Metadata counts as evidence
  only with a real title plus at least one of author or ISBN, and never a
  known-junk title.

## The life of a tracked file

```
                    discovered by a scan
                            │
                            ▼
   same path, different  ┌──────────┐   not at its recorded path
   content ─────────────▶│ Tracked  │──────────────┐
          ◀── re-hash ───└──────────┘              ▼
                              ▲              ┌──────────┐
       matched by identity    └──────────────│  Absent  │
       or hash elsewhere                     └──────────┘
                                                   │ no match anywhere
                                                   ▼
                                             ┌──────────┐
                              relocated ────▶│  Orphan  │────▶ entry removed
                              by hand        └──────────┘
```

**Orphan is a last resort.** A file renamed or moved inside the library can be
*proved* to be the same file, so it is re-attached silently — `data.name` and
`books.path` updated, no prompt. Only a file that cannot be found anywhere
becomes the user's problem.

`Absent` exists only during a scan, and **the scan must resolve the whole set at
once** rather than file by file. Two books that swap filenames both look absent
individually; matched as a set they are two renames.

## Behaviour decided

| | |
| --- | --- |
| Adding from outside the folder | the file is copied to the **folder root**, keeping its own filename; ` (1)` on collision. An import is then indistinguishable from a file copied in by hand. |
| Deleting a book | **asks**, with "remove from library" preselected and "delete the file as well" the explicit second choice. In a folder the user owns, erasing their file on a keypress is not a default. File deletion goes to the system trash where possible. |
| Change detection | scan on open and on demand. A filesystem watcher is a later addition, for local volumes only — it is a new dependency and unreliable on network shares and at scale. |
| Moving between libraries | ask move-or-copy once per operation. |
| Covers | `.calibre-oxide/covers/<id>.jpg`. `<book dir>/cover.jpg` collides for every book in a flat folder. |
| Retitling | touches nothing on disk. `rename_book_files` must not run. |
| Filenames | never derived from the title. `add_format` currently names by title and must stop. |

## Edge cases

Numbered for reference. **risk** marks the ones where getting it wrong loses or
hides someone's books.

### Discovery

| # | Case | Handling |
| --- | --- | --- |
| E1 | Subfolders | recurse; record the subfolder in `books.path` |
| E2 | A subfolder that is itself a library — **risk** | skip any directory containing `metadata.db` or `.calibre-oxide/`; absorbing a nested library double-tracks every book in it |
| E3 | Grouping | see "When two files are one book" |
| E4 | Non-book files | extension allowlist; a sibling image with a matching stem is a candidate cover |
| E5 | OS junk — `.DS_Store`, `Thumbs.db`, `@eaDir` | ignore list, extending `check_library`'s existing one |
| E6 | A file still being copied in | quiet period before indexing, reusing the auto-add watcher's five-second settle |
| E7 | Symlinks, and loops | do not follow directory symlinks; track file symlinks by their own path |
| E8 | Case-insensitive filesystems | compare paths case-insensitively where the volume is; `Cache::is_case_sensitive` already probes this |
| E9 | 50,000 files | the two-phase scan above |
| E10 | A file deleted from the library that still exists on disk | it will be rediscovered and re-added by the next scan, so "remove from library" needs a durable **ignore list** keyed on content hash or path |

### Drift

| # | Case | Handling |
| --- | --- | --- |
| E11 | Renamed outside the app | match by file identity, then hash; re-attach silently, update `data.name`; never ask |
| E12 | Moved to a subfolder | identical to a rename; update `books.path` too |
| E13 | Two files swap names — **risk** | resolve absences as a set; file-by-file matching mis-assigns both |
| E14 | Edited in place, e.g. an annotated PDF — **risk** | path matches, content does not: this is an *edit*, not corruption. `check_library` currently reports it as a corrupted format; that alarm must become mode-aware or every annotated PDF cries wolf |
| E15 | Two byte-identical files | hash alone cannot order them; tie-break on path distance, track both, flag the pair |
| E16 | The library folder is moved — **risk** | everything looks absent at once. Keeping every stored path relative makes this a non-event, which is why no absolute path may ever be stored |
| E17 | Network share offline, or a permissions error — **risk** | a scan that *failed* must never orphan anything. Distinguish "directory unreadable" from "file absent" and abandon the whole pass on the former |
| E18 | Cloud-sync placeholders (OneDrive, Dropbox, iCloud) — **risk** | a file that exists but is not local: hashing it silently pulls gigabytes down. Detect the offline attribute and defer. Ignore `… (conflicted copy).pdf` churn |
| E19 | Read-only volume | open read-only rather than failing; a library on a USB stick or a NAS export should still be browsable |

### Orphans and moving

| # | Case | Handling |
| --- | --- | --- |
| E20 | Orphan resolution | three actions: point at the real file, put the file back and rescan, or delete the entry. The first warns if the chosen file's hash differs — allowed, but said out loud |
| E21 | Orphans persisting | a durable flag on the book, not a per-scan computation; otherwise the list resets on restart |
| E22 | Deleting an orphan's entry | offer to keep the metadata as a sidecar OPF first. Hand-entered metadata is the expensive part, not the file |
| E23 | The folder is left empty | ask once: keep the index for a folder to be refilled, or remove it. Never delete a database without asking |

## Durability: the change log is authoritative

Decided 2026-09-27. The question was per-file `metadata.opf` sidecars versus a
git-like structure; they turn out to defend against disjoint failures, and only
one of those failures is likely.

| Failure | Per-file OPF | Change log |
| --- | --- | --- |
| `metadata.db` corrupted by a crash | rebuild, lossy | rebuild, exact |
| **Corrupted by two machines syncing the folder** | rebuild, lossy | does not corrupt |
| The index directory is deleted | survives | gone |
| One file copied out to a USB stick | metadata travels | left behind |
| Undo a bulk edit | no | nearly free |
| Two app instances writing | no help | safe by construction |

**`.calibre-oxide/changes/` is the authority; `metadata.db` is a derived query
cache.** Delete the database and it rebuilds exactly from a snapshot plus the
log tail. Corruption stops being data loss and becomes an inconvenience.

Three reasons this beats sidecars:

1. **Cloud sync is the failure that will actually happen.** The library is the
   user's own folder, so it ends up in OneDrive or Dropbox. SQLite written from
   two machines through a file-sync service corrupts — no locking, no
   coordination, two versions of one binary file. A log of one-file-per-change
   merges under naive sync; a database file does not.
2. **The pattern is already in the tree.** `.calibre-oxide/journal/` is an
   append-only, BLAKE3-chained, one-file-per-entry log with sequence numbers,
   commit markers, recovery on open, and a broken chain reported as hard
   corruption. Extending that shape one layer up is consistent, and the hard
   parts (fsync ordering, chain verification, recovery) are solved.
3. **Two sources of truth is a bug generator.** A live-mirrored OPF beside every
   file means the database says one thing and the sidecar another, with nothing
   declaring a winner — the same shape as the three `data.name`-versus-disk bugs
   fixed in #885.

### Where the git analogy breaks

Git's answer to "what if `.git` is deleted" is *you keep your files and lose
history*, and that is acceptable there because **the files are the valuable
thing**. Here it is inverted: a book can be re-downloaded, but 500 hand-entered
ratings cannot be re-typed. So the expensive thing must not live only in a
hidden directory. Two mitigations, neither of which is per-file sidecars:

- `metadata.db` stays **visible** at the library root (above).
- A single compacted `snapshots/` file gives "rebuild from something
  inspectable" at one file rather than thousands.

OPF sidecars survive as a deliberate **export** action, for when a file really
does leave the library — the case they are genuinely good at.

### Considered and rejected: metadata inside the files

Writing XMP into the PDF itself needs no extra files and travels perfectly, and
`metadata/xmp.rs` exists. It is fatal for a non-obvious reason: it changes the
file's content hash on every metadata edit, so every rating change would look
like an external modification to the drift detection above — and it means
writing to files on read-only volumes and to cloud placeholders.

### Design content this implies

- **Two logs, deliberately separate.** The existing `journal/` guards *physical
  file writes* for crash atomicity and is consumed and cleaned during recovery.
  `changes/` is *durable metadata history* and is never discarded except by
  compaction. Merging them would give one of them the wrong lifetime.
- **There is no single hash chain, and cannot be.** The file-operation journal
  chains every entry to the previous one, which works because it has exactly one
  writer. This log has one writer per machine and no way for them to agree on
  who comes next — two peers extending one chain both claim the same
  predecessor, and the chain breaks under normal use. Entries are chained **per
  origin** instead: each install has its own sequence and chain, and the merged
  log is several chains side by side. A deleted or edited entry still leaves a
  detectable gap or mismatch in *that* origin's sequence. Same reason git
  branches rather than demanding a global commit order.
- **Change filenames must not collide across machines.** The file-op journal's
  monotonic sequence number is fine for one writer and wrong for two syncing
  peers, which would both mint the same number. Names need a per-install id and
  a nonce as well.
- **A compacted gap must not read as tampering.** Compaction deletes entries by
  design, so each origin's watermark records the last sequence *and its hash*,
  letting verification bridge the gap the way `JournalCheckpoint::boundary_hash`
  does for the file journal.
- **Ordering needs a hybrid logical clock**, not wall time. Otherwise a machine
  with a fast clock always wins every conflict.
- **Conflicts resolve per field, last writer wins by HLC.** Union-merging
  set-valued fields like tags looks clever and surprises anyone who removed one.
- **Compaction must not outrun sync.** Deleting change files a peer has not
  merged yet loses their edits, so compaction needs a retention horizon rather
  than deleting everything a snapshot covers.
- **Cover blobs do not go in the log.** They live under `covers/`, with the
  log recording only the reference — the same blob/tree split git uses.
  Currently keyed by book uuid rather than content hash; content-addressing
  is a later refinement, and the uuid is already stable across machines.
- **Anything durable in the state directory must be exported.** The library
  export skipped `.calibre-oxide/` wholesale, which was correct only while
  nothing durable lived there. It now holds the change log, so a blanket skip
  produced a backup that could not be rebuilt. The rule is a *denylist* —
  `writer.lock` and `journal/` are excluded, everything else is included —
  so a newly added durable file is exported by default rather than silently
  lost.

## Open

- **Watcher.** Deferred, above. `notify` is not currently a dependency.

## What this changes in existing code

| Where | Change |
| --- | --- |
| `Cache::add_format` | stop naming files after the title; take the name from the source |
| `Cache::add_book` | record in place; copy in only from outside the library |
| `Cache::rename_book_files` | no longer runs on a title change |
| `covers::cover_path` | **done:** `.calibre-oxide/covers/<uuid>.jpg`, keyed by uuid rather than the local autoincrement `id` so two machines cannot mint the same cover filename. Reads fall back to a legacy `<book dir>/cover.jpg` until the next write, which retires it. |
| `check_library` | drop the `Title (id)` folder regex, which this project never produced; make the content-mismatch check an *edit*, not corruption (E14) |
| `checksums.rs` | add a content → book lookup |
| every `Cache` write method | append to the change log — the same crate-wide retrofit shape as #93's "every durable write goes through `LibraryHandle`". **Done, and enforced:** `change_log/audit.rs` performs each public write, rebuilds a second library from the log alone, and compares the two; a second test fails if a function that runs a mutating statement is neither audited nor explained. It found a dozen writes that had been silent (`calibredb set_metadata title`, bulk renames, custom column values, saved searches, covers, timestamps). **Still not in the log:** annotations and reading positions (#967) and the legacy item deleters (#968). |
| `filenames::file_identity` | make public |
| `adding.rs` | `find_books_in_directory`'s stem grouping is no longer the grouping rule |
| auto-add watcher | **done:** a watched folder *inside* the library is scanned in place; only one outside it is still drained after import. The old unconditional delete-after-import would have deleted the book's own file, and watching the library root would have deleted every book it indexed. Re-adding is prevented by the records plus the ignore list (E12), which is what made the delete trick unnecessary. New: `scan::rescan`, `POST /scan-library/{library_id}`. |
| `duplicates.rs` | extend from author/title to content hash |

### Merging another machine's changes

`change_log/merge.rs`. Merging is **not** "replay the log on open". A full replay is last-writer-wins by
construction, but it builds the database from nothing: it would reassign every book's local `id` (which
URLs, notes and open windows refer to) and destroy whatever is not in the log yet (#967). So a merge applies
only what is *new*, on top of what is there, which needs two things a rebuild does not:

- **Which changes have been applied.** A *set* of change keys (`{stamp}-{origin}`, readable from the file
  name), not a high-water mark — file sync delivers in any order, so an origin's seq 7 can arrive before its
  seq 6. A change that cannot be applied yet (its book's `BookAdded` has not arrived) stays unapplied and is
  retried; marking it done would lose it. The common case — nothing new — is one directory listing.
- **How recently each value was written.** Every contested value (a *cell*: one field of one book, one
  format, one cover, one preference) carries the stamp and origin of the change that last wrote it, and an
  incoming change applies only if it sorts after that. **Per field, last writer wins** — union-merging tags
  looks clever and surprises anyone who removed one. `FormatRemoved` shares a cell with `FormatSet`, or a
  peer's older "format added" would resurrect a format removed later.

This state lives **in `metadata.db`**, not a sidecar: the clocks describe the database, so deleting it (the
recovery step this design exists to make safe) must delete them too, or they would claim changes were applied
to a database that no longer has them.

Opening a database that has books but no merge state *adopts* this install's own history — marks it applied and
records its stamps, without re-applying — as a pass finished **before** any peer change is looked at. Done inline
in stamp order, a peer's old change was reached before the local clocks existed and overwrote a newer local value.

Two limitations, stated in the module: `ItemRenamed` composes rather than overwrites, so it is applied
unconditionally and a rename arriving *older* than a later edit naming the old item will rename that edit's
value too; and concurrent edits are indistinguishable from sequential ones, because an HLC stamp cannot say
whether the later writer had seen the earlier — so a peer's edit replacing a local one is always reported as a
conflict. Telling them apart needs each change to name the value it overwrote.

### Rescan ordering

`scan::rescan` re-attaches moved files **before** indexing new ones, and the order is
load-bearing. A file that turns up somewhere new is usually a file that left somewhere old —
the same book, moved. Indexing first would mint a second book for it and leave the first
pointing at nothing, so a single drag in the user's file manager would turn one book into a
duplicate plus an orphan.

Re-attachment is safe on an incomplete scan because it concludes from a file that *was*
found, not from one that was not. Only `missing` needs the whole library to have been seen,
and nothing in a rescan acts on it (E17).
