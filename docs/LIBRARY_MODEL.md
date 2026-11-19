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

As built (`calibre_db::scan_pipeline`, run as a cancellable job by
`calibre_srv::library_jobs`), the split sits in a different place, because the
hash turned out to be the cheap half to do *once* and the embedded metadata the
expensive half to do lazily:

1. **Scan** — each new file is streamed **once** through BLAKE3 (1 MiB reads,
   size counted in the same pass, never a whole-file read). That one read
   serves the ignore list, the checksum record and the book entry. The book
   appears titled from its filename, in batches of one `BEGIN IMMEDIATE`
   transaction each, as soon as a batch commits. Files already indexed are
   found with one query and are not opened at all.
2. **Enrichment** — embedded title, authors and the cover, for the books the
   scan queued (`checksums_db.enrichment_queue`). A PDF is read by PDFium from
   disk, not loaded into memory. It fills in only what is still a placeholder,
   so a title the user changed in between is kept. Resumable by construction:
   a book leaves the queue only when done.
   A book whose metadata cannot be written (or whose batch of books cannot be
   registered) is a **per-file error** in the job's report (`failed`, plus the
   job's error count) and the scan carries on; a failed batch is retried book by
   book. Author, tag and language lists are de-duplicated (trimmed,
   case-insensitive, empty dropped, first occurrence kept) before they are linked,
   because the link tables are `UNIQUE(book, item)` over `NOCASE` names, and a
   book listing one author twice used to abort the whole scan.
3. **Hash backfill** — every format file with no recorded hash, which is every
   file of a library real calibre made. Same streaming read; resumable because
   the work list is "files with no hash".

Reads run on a small bounded pool (default 3; `CALIBRE_OXIDE_SCAN_WORKERS`, or
`?workers=` on the route); every database write stays on one thread in sorted
order, so the result does not depend on the thread count. Cancel is checked
between read chunks. A batch is committed whole or not at all, so a cancelled or
crashed scan leaves no half-registered book, and a rerun skips whatever already
has a book.

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

## Finding the same book recorded differently

Content hashes find the same *file*. The same *book* is often recorded
differently: `The Hobbit` / `Hobbit, The (z-lib.org)`, `J.R.R. Tolkien` /
`Tolkien, J. R. R.`, an ISBN-10 and its ISBN-13. `calibre_db::dedupe` finds
those, and `POST /duplicates/scan/{library_id}` reports each group with its
strongest `reason`, a `confidence`, and the per-pair `evidence`:

| Reason | Evidence | Confidence |
| --- | --- | --- |
| `content` | identical bytes | 1.00 |
| `identifier` | an equal, *valid* ISBN (10 and 13 compared as 13) or equal `asin`/`amazon*`/`goodreads`/`google`/`doi` | 0.95-0.97 |
| `title_author_exact` | equal normalised title and an equal normalised author | 0.90 |
| `title_author_fuzzy` | equal once edition/bracket noise or a subtitle is removed, or a small edit apart, with an initials-compatible author | 0.55-0.80 |

(PDFs matched by their pages, between exact and fuzzy, are the next section.)

Rules that keep this from flooding the user:

- **An unknown author is not evidence.** `Unknown`, `Unknown Author`,
  `Anonymous`, `Various`, calibre's translated `Unknown`, or nothing. A book
  with no specific author can only be grouped by content or identifier, never by
  title. `grouping.rs` uses the same rule.
- **Junk embedded metadata is not evidence either**, and is not trusted at
  extraction: a title like `ACDSee 32 print job`, `Microsoft Word - foo.doc`
  (the prefix is stripped when what remains means something), `Untitled`,
  `Document1`, or a bare filename such as `F:\On Hold\1-45.tif`, and an author
  such as `Admin`, `Administrator`, `Owner` or `User`, is treated as absent, so
  the filename title applies. One short documented list
  (`calibre_db::junk_metadata`) serves both extraction and dedupe, so libraries
  imported before it benefit too. PDF Info strings in UTF-16 (BOM `FE FF` /
  `FF FE`) are decoded, not rendered as `?? G U R P S`.
- **Zero-byte files are never content duplicates** (they all share the hash of
  nothing) and are never in the bulk `merge-exact` plan. The scan response lists
  them separately as `empty_files` / `empty_file_count`, and the duplicates
  panel mentions them.
- **A title alone is not evidence, and the same title by a different author is
  a different book.**
- **Titles that differ by a designator are different books.** Underscores and
  runs of separators are spaces (`Elven_Names_-_Male` = `Elven Names - Male`),
  and an edit-distance match is vetoed when a word on one side and not the
  other is a number or version, a Roman numeral, a single letter
  (`Templates A to H` / `P to Z`), a volume/part/chapter word, or one of a tiny
  set of opposites (`male`/`female`, `men`/`women`).
- **Volumes are never normalised away.** `Vol. 1`/`Vol. 2`, `Part I`/`Part II`,
  `Book One`/`Book Two` stay distinct: numbers survive in every key, a subtitle
  that names a volume is never dropped, and an edit-distance match needs equal
  numeric signatures.
- **Two subtitled titles do not match on their shared prefix**
  (`Discworld: Mort` is not `Discworld: Eric`); only "subtitle on one side only"
  does.
- Junk ISBNs (bad checksum, `0000000000`) match nothing, and an identifier shared
  by more than 50 books is treated as a bulk-edit accident.
- Candidates are blocked by author surname, so a 50k-book library is a sum of
  small comparisons, not a pairwise scan.
- `?min=` drops weaker evidence *before* merging, so a fuzzy edge never bridges
  two groups in an `exact` scan. The bulk `/duplicates/merge-exact` never reads
  this scan: it plans from byte-identical files only, so a fuzzy group is merged
  only by a person confirming it.
- The add-time check, `copy_one_book`'s duplicate check and the import policy
  `skip_title_author` use the *exact* keys only (skipping a file is irreversible
  enough to want strong evidence), and match on *any* shared author rather than
  calibre's all-authors intersection.

The normalisation (what differs from calibre's `fuzzy_title` and
`find_identical_books`, and why Jaro-Winkler is not used) is documented in
`crates/calibre_db/src/dedupe.rs`.

## Finding the same PDF saved differently

Neither hashes nor titles see two kinds of duplicate: a **scan and its OCR copy**
(the same page images once with and once without a text layer, usually
re-encoded too) and an **optimizer re-save** (downsampled images, re-encoded
streams, stripped metadata). They share no bytes and often no title or author
(or have only a filename-derived title), so they are found from the *pages*.
`calibre_db::pdf_similarity` does it; calibre has no such feature, so nothing
is ported.

**Fingerprint.** One per distinct file content, computed once from the file on
disk through PDFium and cached in `.calibre-oxide/checksums.db` (table
`pdf_fingerprints`, keyed by the BLAKE3 the scan recorded, with a `version`
column and a recorded `failed` status for PDFs PDFium cannot open). It is a
sidecar cache like the hashes themselves: not library metadata, not in the
change log, recomputed after a rebuild. It holds the page count; a 256-bit
signature of up to 8 sampled pages (first three, last three, two in the
middle; rendered with a long side of 640 px, cropped to its ink (found by
binarising against the local background, scanner borders dropped), then a
difference hash of the gray means of a 17 x 16 grid, with a mask of the bits
too close to call); whether
there is a text layer; and a 128-slot MinHash of the word 3-shingles of the
first 48 pages' text.

**Job.** `POST /pdf-fingerprint/{library_id}` is a cancellable, resumable
library job. It fingerprints every PDF that has a content hash and no
current-version fingerprint (the hash backfill gives the rest a hash), reads
each distinct content once, records PDFs it cannot read as failed (never
retried until the algorithm's version changes), and fails without recording
anything if PDFium is not installed.

**Matching.** Never all-pairs: candidates come from bit-sampling LSH (16 tables of
14 signature bits; two pages collide only on the same page number, give or
take two, among documents whose page counts are within two), and a text MinHash LSH for the text-only tier; then
the sampled pages are aligned (a cover added or dropped shifts the front, not
the end) and compared at a distance of at most 48 of 256 bits (disagreements among the
bits both pages trust). Measured on real PDFium renders (the ignored
benchmark `crates/calibre_db/tests/pdf_signature_benchmark.rs`), a page
and its scan, OCR copy or optimizer re-encode are at most 17 apart and
different pages at least 60. Two *independent* scans of one book with
different skew do not match by pixels; their OCR text can (`pdf_text_match`). A pair matches when
at least 3 non-blank pages were compared and 75% match (a one- or two-page
PDF needs equal counts and every page). Blank and near-blank pages are never
compared.

| Reason | Evidence | Confidence |
| --- | --- | --- |
| `pdf_resave` | same pages; same text (MinHash similarity at least 0.8) or no text on either | 0.85 |
| `pdf_ocr_variant` | same pages; exactly one has a text layer, or both do with *distinctive* similarity 0.5-0.8 over at least 64 distinctive slots (two OCR runs) | 0.80 / 0.75 |
| `pdf_text_match` | different pages, but both have text, *distinctive* similarity at least 0.9 over at least 64 distinctive slots, page counts within 2 | 0.60 |

*Distinctive* means the text boilerplate is left out: a MinHash slot value held
by at least `max(5, 0.5%)` of the library's documents is a shingle many
documents share (Google Books' "This is a digital copy of a book that was
preserved for generations..." notice, which is all the text an old Google scan
has in its first 48 pages; a licence; a publisher's template), and says nothing
about two particular books. Without this, unrelated Google scans matched at
similarity 1.00, and documents built from one template (the Intel instruction
pages `MOVHPS`/`MOVLPS`, a publisher's whole line) were taken for two OCR runs.

Page-content pairs make groups by **complete linkage**, not transitively: two
clusters merge only when every cross pair has an edge (a cluster of up to three
may chain), so a run of weak pairs cannot chain 49 unrelated books into one
group. Books holding identical files count as one node.

These rank **between `exact` and `fuzzy`**; `?min=pdf` includes them and
everything stronger. Same pages with *different* words (similarity under 0.5,
or too little distinctive text) is not a match: that is two books from one
template. The fingerprint job runs **one** file at a time (two PDFium workers
killed the server process on a real library), and every PDFium call in the
process is serialised behind one lock. Only fingerprinted PDFs
take part and the scan reports how many are not (`pdf_unfingerprinted`), so the
UI can offer to run the job. They never reach the bulk `merge-exact`.
`suggested_dest` for these groups is the copy with a text layer, then more
pages, then the larger file (an optimizer shrinks images, so the larger of two
re-saves is the original).

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

## The trash

Delete used to mean "send to the OS wastebasket". Windows has no Recycle Bin
for a file on an SMB share (nor do most NAS mounts), so on exactly the library
this app is for, delete failed and the file stayed. calibre never had that
problem: since 6.0 a library delete moves the book into **`.caltrash` inside
the library** (`db/backend.py`: `move_book_to_trash`, `move_book_files_to_trash`,
`expire_old_trash`, `list_trash_entries`; `gui2/trash.py`). This is that.

* `.caltrash/b/<book id>/` holds a deleted book; `.caltrash/f/<book id>/<ext>`
  plus `metadata.json` a format removed from a book that remains.
* Entries expire after the `expire_old_trash_after` library preference (seconds;
  default 14 days, never under a day). Expiry runs when the trash is listed and
  is throttled to once an hour per library, like calibre's.
* They can be listed, restored, deleted for good, or emptied: `GET /trash`,
  `POST /trash/restore|delete|empty|expire`, and **Library → Trash…**.
* A move is a same-volume `rename` through `LibraryHandle::rename_atomic`
  (journaled; retried on a network tier) — never copy-then-delete. If
  `.caltrash` is on another volume the move fails and the file stays.

**How it differs from calibre, because the folder may be shared.** calibre moves
a book's *folder*; here a folder can hold many books and the user's own files,
so only the book's **own files** move (its `data` rows' files, plus `cover.jpg`
and `metadata.opf` when its folder is its alone, plus the book's `data/` extra
files when the folder is its alone or the book lives in the library root under
`data/<uuid>/`), as `delete_book_with` has always done. Merge deletes its
sources the same way, so a merged-away book's extra files travel in its trash
entry (restorable with it) and its emptied folder goes; the OS recycle bin is
never involved. The entry holds them flat, with an `oxide-trash.json` manifest:
the original relative path of each file, the uuid, the metadata a restore
writes back. A trashed in-place file is restorable to its **original relative
path**; if something has taken the name since it comes back as
`name (restored).ext`, never over it.

* Trashing records **no** ignore entry — the file is no longer in its folder,
  so a rescan cannot find it — and the scanner does not walk `.caltrash`.
* Restoring clears any ignore entry for the file, so a book is never both
  present and hidden.
* The change log: a trash is `BookRemoved` / `FormatRemoved`; a restore is the
  ordinary add (`BookAdded`, its fields, path, a `FormatSet` per file) under the
  **same uuid**. A rebuild from the log lands on the right state for any
  sequence of trash and restore without reading `.caltrash`; replay also drops
  an ignore entry for a file the rebuilt library tracks.
* A restored book keeps its id if that id is free, else gets a new one.

## Importing a folder tree

`POST /import-folder` (`calibre_db::import`) copies a folder tree *into* the
library — the counterpart of scanning, which indexes files already there.

* Modes (calibre's): *several books per folder* (files sharing a stem are one
  book's formats) or *one book per folder*.
* Duplicate policies: skip identical content (default), also skip same
  title+author, add new formats to the matching book (calibre's "automerge"),
  or add everything.
* One read per source file: copied into `.calibre-oxide/import-staging` while
  being hashed, the copy re-read and compared (size + BLAKE3), then **renamed**
  into `Author/Title (id)/Title - Author.ext` in one transaction per batch.
* Metadata is carried over, not just title/author: from the embedded metadata
  the title, authors, series (and index), publisher, tags, identifiers (ISBN
  etc.), languages, comments, rating, pubdate and cover; then calibre-style
  **sidecars in the source folder** override it — `<stem>.opf` beside a book
  file, or `metadata.opf` when the book is the only one in its folder — and a
  `cover.jpg`/`cover.png` beside the used `metadata.opf` is the cover when the
  book has none. The title/author duplicate checks use these final values.
  (Placeholders — title/author `Unknown`, language `und` — never override a
  real value; tags/identifiers containing a comma are skipped, since the
  field writers join on commas.)
* Resumable: a ledger in the checksum sidecar remembers every source file's
  outcome, so a rerun skips what was done without reading it. The source is
  never modified.

## Behaviour decided

| | |
| --- | --- |
| Adding single files from outside the folder | the file is copied to the **folder root**, keeping its own filename; ` (1)` on collision. An import is then indistinguishable from a file copied in by hand. |
| Adding a *folder* (Add folder / `POST /import-folder`) | a job: the tree is copied in, recursively, into calibre's `Author/Title (id)/Title - Author.ext` layout, with real embedded metadata. See "Importing a folder tree" below. |
| Deleting a book | **asks**, with "remove from library" preselected and "delete the file as well" the explicit second choice. In a folder the user owns, erasing their file on a keypress is not a default. File deletion moves the book's own files into the library's `.caltrash` (see "The trash"), as calibre does — the OS bin is not used, because a network share has none. |
| Auto-add drop folder (outside the library) | a file that was *added* is **moved** into `<drop folder>/.added/` (renamed `name (1).ext` on a collision, never overwritten) — never deleted, and not the OS bin, which an SMB share lacks. Which files were added is decided per exact path from the add result, not by matching names in messages. Duplicates and failures stay in the drop folder. |
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

### Batched entries: one fsync per hundred books

An entry is a file, and a durable entry is a file that has been fsynced. A book
registered by the scan is nine ops, so a library of 300,000 files was 2.7
million fsyncs — days on a network share, and 18,000 of them (about 40 seconds)
for a 2,000-file smoke test on a local disk.

`ChangeOp::Batch { ops }` records several ops as **one entry**: one file, one
fsync, one link in the hash chain. The scan writes one per batch of books.

- **Replay is unchanged.** The inner ops are applied in order, exactly as if
  they were separate entries (`apply::apply_op`). The audit
  `every_write_survives_a_rebuild_from_the_log` covers a batch, and the scan
  tests rebuild a batched scan from its log and compare.
- **A batch is atomic in the log.** It is one file, parsed whole or rejected by
  its hash, so a crash cannot leave half of one — stricter than the per-op
  entries it replaces, where a crash could leave a book with an add and no
  fields.
- **The gap that remains is the one every write here already had**: the database
  commit comes first and the log append second, so a crash in between leaves a
  batch of books the log does not know about. It is now a hundred books wide
  instead of one. `ChangeLog::verify` and a rebuild are where it would surface,
  and a rerun of the scan does not repair it (the books exist, so they are
  skipped); a rebuild would drop them and the next scan would re-add them from
  the files — a loss of nothing but their ids.
- **Compaction keeps a batch whole.** It has no single supersede key, so a later
  edit of one of its fields is not collapsed into it. That costs disk, not
  correctness: the later entry replays after the batch and wins.
- **Single adds are unchanged**: one entry per op.

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
| every `Cache` write method | append to the change log — the same crate-wide retrofit shape as #93's "every durable write goes through `LibraryHandle`". **Partly done:** the location primitives (`set_book_path`, `record_format_row`, `forget_format_row`, `set_format_name`) now log, which closed a real bug — `apply_relocations` wrote `data.name` with raw SQL, so a moved file was re-attached in the database and never recorded, and the next rebuild undid it. Appends happen with the connection lock released. Custom columns, prefs and covers still need new `ChangeOp` variants; see #901. |
| `filenames::file_identity` | make public |
| `adding.rs` | `find_books_in_directory`'s stem grouping is no longer the grouping rule |
| auto-add watcher | **done:** a watched folder *inside* the library is scanned in place; only one outside it is still drained after import. The old unconditional delete-after-import would have deleted the book's own file, and watching the library root would have deleted every book it indexed. Re-adding is prevented by the records plus the ignore list (E12), which is what made the delete trick unnecessary. New: `scan::rescan`, `POST /scan-library/{library_id}`. |
| `duplicates.rs` | **done:** content hash (#892), then identifiers and normalised/fuzzy title+author (`calibre_db::dedupe`, with `reason`, `confidence`, `evidence` and `?min=`) |

### Rescan ordering

`scan::rescan` re-attaches moved files **before** indexing new ones, and the order is
load-bearing. A file that turns up somewhere new is usually a file that left somewhere old —
the same book, moved. Indexing first would mint a second book for it and leave the first
pointing at nothing, so a single drag in the user's file manager would turn one book into a
duplicate plus an orphan.

Re-attachment is safe on an incomplete scan because it concludes from a file that *was*
found, not from one that was not. Only `missing` needs the whole library to have been seen,
and nothing in a rescan acts on it (E17).
