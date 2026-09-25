# calibre-oxide desktop UI/UX specification

Produced by a design pass against the running application and the
upstream calibre sources. All repo paths are relative to the repository
root; upstream citations are under `old_src/src/calibre/`.

This is a specification, not a record of what is built. Nothing below
is implemented unless a commit says so.

---

## 0. Root causes of "it doesn't read as calibre"

**0.1 The toolbar is upside-down and too thin.** Row 1 is
search/sort/filter chrome; row 2 is actions. calibre is the reverse —
the action toolbar is the top edge of the window, the search bar sits
under it. calibre's default main toolbar is **17 items**, not 6
(`gui2/__init__.py:341-346`):

```
'Add Books', 'Edit Metadata', None, 'Convert Books', 'View', None,
'Store', 'Donate', 'Fetch News', 'Help', None, 'Preferences',
'Remove Books', 'Choose Library', 'Save To Disk', 'Connect Share',
'Tweak ePub'
```

(`None` = separator.) What makes 17 items legible is that each is a
**split button**: icon, label beneath, and a `▾` opening a menu of
related actions. The current registry cannot express this — `primary`
is a flat boolean and `More ▾` is a static dumping ground.

**0.2 The toolbar reflows as you click around.** `visibleToolbarActions`
in `web/src/library/actions.ts` ends with:

```ts
return a.requires === "none" || actionEnabled(a, filter.ctx);
```

so selection-scoped actions are *removed* when nothing is selected and
*inserted* when something is. Buttons physically move under the cursor.
Desktop toolbars never do this. Fix: return every visible action with an
`enabled` flag and render disabled ones greyed.

**0.3 No design system; dark mode is broken.** `web/src/style.css` is
nine lines. There are zero CSS custom properties in `web/src`. Twelve
components carry their own `@media (prefers-color-scheme: dark)` block
with hand-picked hexes — but `body` hardcodes `background: #ffffff`
with no dark override, so on a dark-mode OS the app renders dark panels
on a white page. Twenty distinct font sizes are in use. Not a type
scale; noise, and the reason it reads as a web form.

**0.4 "Select mode" is a mobile pattern.** `select-mode` turns the list
into a checkbox list. Desktop apps use click / Ctrl+click / Shift+click
with a persistent selection.

---

## 1. Information architecture

### 1.1 Registry schema changes (prerequisite for everything else)

In `web/src/library/actions.ts`, `group` currently does double duty as
semantic category and menu placement — which is why the native menu bar
reads `Library | Selection | Book | View`. No desktop app has a
"Selection" menu. Split the concerns:

```ts
export type TopMenu = "file" | "edit" | "library" | "books" | "tools" | "view" | "help";

/** Menubar placement. `section` drives separators; sections render in order. */
export interface MenuPlacement { menu: TopMenu; section: number; order: number }

/** Context-menu placement, replacing `contextMenu?: boolean`. */
export interface ContextPlacement { section: number; order: number }

export interface LibraryAction {
  id: LibraryActionId;
  label: string;
  group: ActionGroup;              // keep — still the semantic bucket
  requires: ActionRequirement;
  desktopOnly?: boolean;
  icon?: IconKey;                  // NEW: basename under /icons, no extension
  accel?: string;                  // NEW: display-only, e.g. "Ctrl+Shift+E"
  tooltip?: string;                // NEW: one-line hover hint
  place?: MenuPlacement;           // NEW: absent ⇒ not in the menu bar
  context?: ContextPlacement;      // NEW: replaces contextMenu?: boolean
  toolbar?: boolean;               // keep (customization eligibility)
  // `primary` is DELETED — superseded by TOOLBAR_LAYOUT in §2.2.
}
```

Keep `contextMenuEntries`, `visibleToolbarActions`, `buildMenuSpec` as
the three surface projections. `MenuActionSpec` gains
`menu`/`section`/`accel`; `app/src-tauri/src/menu.rs` drops
`group_title`/`GROUP_ORDER` for a `TOP_MENU_ORDER` plus section-driven
separators.

### 1.2 Menu bar

Upstream ships **no menu bar** on Linux or Windows
(`gui2/__init__.py:339`, enforced at `gui2/bars.py:814`). Only macOS
gets one, and it is seven toolbar action names with no File/Edit/View
structure.

**Deviate deliberately.** Ship a full menu bar on all platforms:
`menu.rs` already builds one, it is the cheapest discoverability
surface for ~38 actions, and calibre's omission is a Qt-era artifact.

Seven menus; `─` marks a section break.

**File** — Add books… `Ctrl+A` · Add books from folder… `Ctrl+Shift+A` ─
Fetch news… `Ctrl+N` ─ Save to disk… `Ctrl+S` · Send by email… `Ctrl+E`
─ Export catalog… · Export library archive… ─ Close window / Quit

**Edit** — Undo/Redo/Cut/Copy/Paste/Select all (predefined) ─ Find in
library… `Ctrl+F` · Advanced search… `Ctrl+Shift+F` · Select all books ─
Preferences… `Ctrl+,` (moves to the app menu on macOS)

**Library** — Switch/create library… `Ctrl+L` · Switch to previous
library `Ctrl+Alt+P` · Recent libraries ▸ ─ Virtual library ▸ · Saved
searches ▸ · Manage lists… ─ Custom columns… · Manage authors, tags… ─
Browse annotations…

The two dynamic submenus fix `manage-lists` being an opaque label: a
virtual library or saved search should be *applicable* from the menu,
not merely manageable.

**Books** — Read `Enter` · Open externally `Ctrl+Enter` · Recently
viewed ▸ · Quick view `Q` ─ Edit metadata `E` · Download metadata &
covers… `Ctrl+D` · Replace cover… · Bulk edit… `Ctrl+Shift+E` ─
Convert… `C` · Polish… `P` · Tweak book… `T` · Unpack to folder… ·
Repack from folder… ─ Mark / unmark `M` · Similar books ─ Delete…
`Del` · Restore recently deleted

**Tools** — Check library… · Find duplicates… `D` · Restore database…
(no backend yet) ─ Test template… ─ Jobs…

**View** — ◉ Table `Ctrl+1` · ◉ Cover grid `Ctrl+2` · Columns… ─
☑ Categories `Shift+Alt+T` · ☑ Book details `Shift+Alt+D` · ☑ Search bar
`Shift+Alt+F` · ☐ Virtual library tabs ─ Show marked only · Clear
marks ─ Random book

(The three `Shift+Alt+*` accelerators are calibre's own —
`gui2/central.py:375-378`, `gui2/init.py:333`.)

**Help** — Help… `F1` · Keyboard shortcuts… `Ctrl+/` ─ About
calibre-oxide…

**Browser mode.** `web/` must keep working as a plain browser tab, so
build an in-app `MenuBar.vue` from the same `MenuPlacement` data,
rendered only when `!isTauri()`. Each dropdown reuses `ContextMenu.vue`
— one implementation, one registry.

### 1.3 Context menu

calibre's default is short (`gui2/__init__.py:359-364`). Ours, via
`context: { section, order }`:

```
section 0:  Read · Open externally · Quick view
section 1:  Edit metadata · Download metadata & covers… · Replace cover…
section 2:  Convert… · Polish… · Tweak book…
section 3:  Mark / unmark · Similar books · Save to disk… · Send by email…
section 4:  Delete…
```

Thirteen entries, five groups. Drop `test-template`, `unpack-book`,
`repack-book`, `bulk-edit` — a 17-item right-click menu is unusable.

Context-menu entries show their accelerator but **not** their dropdown
— upstream does the same via `menuless_qaction`
(`gui2/actions/__init__.py:232`).

Two behaviours to add:

- With more than one book selected, lead with a disabled title line
  *"3 books selected"* and grey out single-selection actions. Today the
  menu offers `read` for a 3-book selection and silently no-ops.
- **Column-header right-click menu** in `BookTable.vue`: Sort ascending /
  Sort descending / ─ / Hide this column / Add column ▸ / Resize to fit
  / ─ / Columns…. Upstream precedent:
  `gui2/actions/booklist_context_menu.py` exists purely to pop the
  column-header menu.

### 1.4 A fourth surface: "All actions"

calibre puts an **All GUI actions** button in the status bar
(`gui2/init.py:670`, default off). Adopt it: a `⋯` button opening a
searchable flat list of every registry action, grouped by `group`. It
permanently answers "where did X go?" while the IA is in flux.

---

## 2. Toolbar design

### 2.1 Structure

```
ROW 1  ACTION TOOLBAR                                       height 56px
[Add ▾] [Edit metadata ▾] │ [Convert ▾] [Read ▾] │ [Save ▾] [Fetch news]
│ [Delete ▾] ············· [📚 My Library ▾] [⚙ Preferences] [? ▾]
─────────────────────────────────────────────────────────────────────
ROW 2  SEARCH BAR                                           height 34px
[📚 All books ▾] [−] [↕ Sort ▾] [ fts ⚙ │ Search…  │ ✕ ⚠ ]
[🔍 Search] [↓ Highlight] │ [🔖 Saved searches ▾] │ [Table|Grid] [Columns…]
─────────────────────────────────────────────────────────────────────
ROW 3  VIRTUAL LIBRARY TABS   (optional, off by default)    height 28px
```

Swap the two existing rows in `LibraryView.vue`: the actions row moves
above the find row.

**Search bar composition** — modelled on `gui2/layout.py:200-294`:

1. **Virtual library** menu button (instant-popup) showing the active
   VL, default *"All books"*. Replaces the bare `<select v-model="vl">`.
2. **Clear VL** button, hidden unless a VL is active.
3. **Sort** menu button. Replaces the entire `.sort-fields` block —
   two `<select>`s plus removable chips is the most web-form-looking
   control in the window. Menu: each sortable field as a radio, ─,
   Ascending/Descending, ─, "Add secondary sort ▸".
4. **Search field** with icon actions *inside* the input: leading `fts`
   toggle (replacing the `Metadata search / Full-text search` button),
   leading `gear` opening Advanced search (`Ctrl+Shift+F`, a surface
   we lack entirely), trailing `✕` clear, and a trailing error icon
   that appears only on query-parse failure. History dropdown on the
   field.
5. **Search** button.
6. **Highlight** toggle — highlight matches in place vs. filter to
   matches. No current equivalent.
7. **Saved searches** menu button, replacing its `<select>`. Rebuilt on
   open: each saved search, ─, Save current search… / Manage….
8. **Table | Grid** segmented control and **Columns…**, restyled.

The whole search bar is hidden/shown by `Shift+Alt+F`.

### 2.2 What earns a toolbar slot

Slots are earned by **frequency**, not importance, and split buttons let
one slot carry a family. Define the layout as data, not a flag:

```ts
export type ToolbarItem =
  | { kind: "action";    id: LibraryActionId }
  | { kind: "split";     id: LibraryActionId; menu: (LibraryActionId | "-" | DynamicId)[] }
  | { kind: "menu";      id: string; label: string; icon: IconKey; menu: ... }
  | { kind: "separator" }
  | { kind: "spring" };

export const TOOLBAR_LAYOUT: ToolbarItem[] = [
  { kind: "split", id: "add-books",     menu: ["add-folder", "-", "fetch-news"] },
  { kind: "split", id: "edit-metadata", menu: ["fetch-metadata", "replace-cover", "-",
                                               "bulk-edit", "map-metadata", "-",
                                               "test-template"] },
  { kind: "separator" },
  { kind: "split", id: "convert",       menu: ["polish", "tweak-book", "-",
                                               "unpack-book", "repack-book"] },
  { kind: "split", id: "read",          menu: ["open-externally", "quick-view", "-",
                                               "$recently-viewed", "-",
                                               "similar-books"] },
  { kind: "separator" },
  { kind: "split", id: "save-to-disk",  menu: ["send-email", "-",
                                               "export-catalog", "export-library-archive"] },
  { kind: "action", id: "fetch-news" },
  { kind: "separator" },
  { kind: "split", id: "delete-book",   menu: ["-", "$restore-deleted"] },
  { kind: "spring" },
  { kind: "menu",   id: "library",      label: "<current library name>",
                    icon: "lt",         menu: /* §5.1 */ },
  { kind: "action", id: "preferences" },
  { kind: "split",  id: "help",         menu: ["$shortcuts", "$about"] },
];
```

Twelve visible slots reaching ~30 actions. `$`-prefixed ids are dynamic
blocks resolved at open time.

Two upstream patterns worth copying verbatim:

- **Recently viewed** (`gui2/actions/view.py:62-79`): a rebuilt-on-open
  history list between two separators, ending with *Clear recently
  viewed list*.
- **Restore recently deleted** (`gui2/actions/delete.py:96-116`):
  upstream precedent for the delete-undo gap in §5.5. That menu also
  carries *Remove all formats except…* and *Remove covers from selected
  books*, which we have no equivalent for.

Deliberate deviations from calibre's default list: no `Store`, no
`Donate`, no `Connect Share` (no device support), and `Choose Library`
is promoted to a right-aligned **named** button rather than a
mid-toolbar icon — which library you are in is persistent state, not an
action.

### 2.3 Split-button behaviour

- Clicking the **body** runs the primary action.
- Clicking the **`▾` chevron** (14px hit zone, right edge) opens the menu.
- **Right-clicking anywhere** opens the menu — upstream `gui2/bars.py:188`.
- Menus render through `ContextMenu.vue`, anchored bottom-left.
- A split button whose primary is disabled still opens its menu.
- **Adaptive popup**: when the menu has one meaningful entry, drop the
  chevron and let the whole button trigger the primary (upstream
  `gui2/actions/choose_library.py:275-280`). Applies to the library
  button when only one library is known.
- Menus may nest one level.

### 2.4 Icons: yes

Use calibre's own — the project is GPL-3 and 192 PNGs are vendored at
`old_src/resources/images/`. Follow the precedent of
`web/scripts/copy-pdfjs-assets.mjs` (wired as `prebuild`/`predev`): add
`web/scripts/copy-icons.mjs` copying a curated ~45-file subset into
`web/public/icons/`.

| action | file | | action | file |
|---|---|---|---|---|
| add-books | `add_book.png` | | polish | `polish.png` |
| add-folder | `tb_folder.png` | | tweak-book | `tweak.png` |
| fetch-news | `news.png` | | unpack-book | `unpack-book.png` |
| edit-metadata | `edit_input.png` | | repack-book | `sync.png` |
| fetch-metadata | `download-metadata.png` | | quick-view | `quickview.png` |
| replace-cover | `default_cover.png` | | similar-books | `similar.png` |
| bulk-edit | `merge_books.png` | | mark-books | `marked.png` |
| map-metadata | `tags.png` | | delete-book | `remove_books.png` |
| convert | `convert.png` | | restore-deleted | `edit-undo.png` |
| read | `view.png` | | switch-library | `lt.png` |
| open-externally | `external-link.png` | | check-library | `reports.png` |
| save-to-disk | `save.png` | | find-duplicates | `merge.png` |
| send-email | `mail.png` | | custom-columns | `column.png` |
| export-catalog | `catalog.png` | | virtual library | `vl.png` |
| export-library-archive | `bookshelf.png` | | browse-annotations | `highlight.png` |
| test-template | `template_funcs.png` | | pick-random | `random.png` |
| preferences | `config.png` | | help | `help.png` |
| search | `search.png` | | jobs | `jobs.png` |
| advanced search | `gear.png` | | full-text search | `fts.png` |
| saved searches | `folder_saved_search.png` | | sort | `sort.png` |
| clear VL | `minus.png` | | highlight toggle | `arrow-down.png` |
| grid view | `grid.png` | | layout button | `layout.png` |
| search parse error | `dialog_error.png` | | overflow | `h-ellipsis.png` |

Category icons: `user_profile.png`, `tags.png`, `series.png`,
`publisher.png`, `languages.png`, `rating.png`. Format pills:
`old_src/resources/images/mimetypes/`.

**Dark-mode caveat:** these are raster PNGs with baked-in dark line art.
calibre ships `-for-dark-theme` variants for a handful. For the rest,
apply `filter: var(--icon-filter)` — `invert(1) hue-rotate(180deg)
brightness(1.1)` in the dark token block. Verify per icon; where
inversion fails, pull the `-for-dark-theme` file.

### 2.5 Labels and sizing

Icon **above** label — calibre's default `ToolButtonTextUnderIcon`, and
the strongest single "reads as calibre" cue.

Upstream's own numbers are larger: 48px icons, `toolbar_text = 'always'`,
labels wrapped to ~10 chars over two lines
(`gui2/__init__.py:391,393`; `gui2/bars.py:83-117`), giving a ~80px
toolbar. **Deviate deliberately** — that is a 2009 aesthetic and costs
25% more vertical space before a book is visible.

- Icon 24×24, label `--fs-small`, 2px gap, padding `4px 8px`, min-width
  56px, max-width 96px with ellipsis, single line. Row height **56px**.
- **Toolbar style** setting in the existing Toolbar preferences pane:
  `Icon above text` (default) / `Icon beside text` (32px row) /
  `Icon only` (32px row) / `Auto` (beside-text, collapsing to icon-only
  on overflow — upstream `gui2/bars.py:177-186`).
- The right-aligned cluster is always icon-beside-text so the library
  name reads.
- Keep the live-label behaviour ("Adding…") but **drop the `(n)` count
  suffixes**: a label whose width changes with the selection is exactly
  the §0.2 reflow problem. Put the count in the tooltip and status bar.

### 2.6 Overflow

Replace the static `More ▾` with real measurement, in two stages:

1. **Collapse text.** On overflow, switch all buttons to icon-only
   (32px) before removing anything — upstream's `'auto'` text mode.
2. **Overflow groups.** Still too wide: `ResizeObserver` on the row,
   items measured once on mount and cached by id; walk `TOOLBAR_LAYOUT`
   right-to-left from the `spring`, moving whole **separator-delimited
   groups** into overflow, never splitting a group.

The right-aligned cluster never overflows. The overflow button renders
a `ContextMenu` where a split button becomes a block: its primary bold,
its menu items indented beneath. Below ~520px, go icon-only immediately.

---

## 3. Three-panel layout

### 3.1 Proportions

`web/src/library/layout.ts` stores **pixels** (`sidebarWidth: 240`,
`detailsWidth: 360`). Two problems: details being 50% wider than
categories is backwards, and a layout saved on a 2560px monitor
restores at 240px on a 1280px laptop.

calibre stores **fractions of the container**
(`gui2/central.py:276-281`): `tag_browser_width = 0.30`,
`book_details_width = 0.30`.

Adopt fractions, tighter than upstream — 0.30/0.30 suits calibre's
1012px default window but wastes space at 1920:

```
tagBrowserWidth  = 0.22    min 180px   max 480px
bookDetailsWidth = 0.26    min 260px   max 560px
```

At 1440px that is 317 / 838 / 374 — 22% / 58% / 26%. `parseLayout`
already validates per field, so this is a format bump with a
px→fraction fallback for old blobs.

Thresholds worth copying (`gui2/central.py:33-34, 213`): dragging below
**10px** hides a panel; dragging out from hidden requires **50px**; a
collapsed handle renders at **2px**.

Upstream `Visibility` defaults: tag browser ✓, book details ✓, book
list ✓, **cover browser ✗, quick view ✗**. So the missing cover browser
is genuinely optional and Quickview-as-dialog is fine.

### 3.2 Every panel gets a header bar

A 28px strip: `background: var(--bg-bar)`, `border-bottom: 1px solid
var(--border)`, label in `--fs-small` uppercase `letter-spacing: .06em`
`color: var(--fg-muted)`, trailing icon buttons. The cheapest change
that makes three flex columns read as three *panels*.

- **Categories**: `CATEGORIES` · `[⌕ Find]` `[⚙]` `[▾ Collapse all]` `[✕]`
- **Books**: no header — the table header *is* its header, sticky.
- **Details**: `DETAILS` · `[📌 Pin]` `[✕]`. Replaces the
  absolutely-positioned floating `✕` that currently overlaps the title.

Upstream puts the tag-browser bar at the **bottom** and ships **no
collapse-all button** (it is an unbound shortcut only). Deviate on
both: top-placed headers create the panel reading, and the missing
collapse-all is a gap in calibre rather than a feature.

### 3.3 Categories panel — rewrite

The weakest component: an accordion with **one** category open at a
time, refetching on every toggle, rendering two inline text buttons on
*every* item row.

1. **Persistent tree.** Multiple categories open at once; expansion
   state persisted alongside the layout prefs. Items cached per
   category.
2. **Find box** in the panel header, collapsed by default. Port the
   upstream semantics (`tag_browser/ui.py:641-660`): history combo,
   substring by default, `=` prefix exact, `*foo` collapses all first,
   `tags:foo` scopes to one category. Enter finds next, scrolls into
   view, highlights; matching ancestors auto-expand.
3. **Collapse all** button.
4. **Gear menu**, porting `tag_browser/ui.py:782-878`: Show/Hide counts ·
   average rating · notes icon · empty categories ─ Sort by ▸ (Name ✓ /
   Number of books / Average rating) · Search type when selecting
   multiple ▸ (Match any ✓ / Match all) ─ Manage authors, tags… ▸ ·
   Show only books that have visible categories ─ Show all Tag browser
   settings.
5. **Right-click an item**: Rename… / Merge into… / Edit note… / Search
   for this / Exclude this. **Delete the inline buttons** — two extra
   buttons × N items is the loudest visual noise in the app.
6. **Row styling**: 22px rows, 12px indent per level, 10px twisty, 16px
   category icon, name, right-aligned count in `--fs-micro`
   `--fg-faint`. Hover `--bg-hover`, selected `--bg-selected`.
7. Panel background `--bg-sunken`, so the book list reads as foreground.
8. Keep the existing empty-state paragraph, restyled.

### 3.4 Book list

- Row height **24px** compact / **30px** comfortable, switchable. There
  is no declared row height today.
- Zebra striping `--bg-row-alt`. Selection `--bg-selected`; when the
  list loses focus, `--bg-selected-inactive` — a strong desktop cue
  costing one `:focus-within` rule.
- Sticky `<thead>`: `--bg-bar`, `--fs-small`, weight 600, `--fg-muted`,
  `border-bottom: 1px solid var(--border-strong)`.
- Sort indicator `▲`/`▼` at `--fs-micro` in `--accent`, with a
  superscript ordinal for secondary sorts.
- `formats` renders as small uppercase pills, not comma text.
- `rating` renders as stars, not a number.
- Header right-click menu per §1.3.

### 3.5 Details panel

- Header bar per §3.2; title becomes an `<h2>` at `--fs-medium` inside
  the scroll area, no longer fighting an absolutely-positioned `✕`.
- Cover at **120px** (currently 90px), 1px border, soft shadow, click
  for full size.
- All fields as a two-column grid — label `--fs-small`/`--fg-faint`
  right-aligned, value `--fs-body`. Extend the existing custom-columns
  grid to *every* field.
- Formats as pill buttons with the format icon.
- **Pin toggle**: unpinned (default) follows the list selection; pinned
  holds the current book while you browse.

---

## 4. Visual design

### 4.1 Token sheet — replaces `web/src/style.css` entirely

```css
:root {
  /* Spacing — 2px base, 4px rhythm */
  --sp-1: 2px;  --sp-2: 4px;  --sp-3: 6px;  --sp-4: 8px;
  --sp-5: 12px; --sp-6: 16px; --sp-7: 24px; --sp-8: 32px;

  /* Type — desktop density */
  --fs-micro:  10px;   /* counts, badges, format pills          */
  --fs-small:  11px;   /* status bar, column heads, panel heads */
  --fs-body:   13px;   /* DEFAULT — everything unless stated    */
  --fs-medium: 15px;   /* details title, dialog section heads   */
  --fs-large:  18px;   /* dialog titles                         */
  --lh-tight: 1.2;  --lh-body: 1.45;
  --font-ui: -apple-system, BlinkMacSystemFont, "Segoe UI", Cantarell, Roboto, sans-serif;
  --font-mono: ui-monospace, "SF Mono", "Cascadia Mono", Menlo, monospace;

  /* Controls */
  --ctl-h-sm: 22px;   /* status-bar buttons, chips, pills       */
  --ctl-h:    26px;   /* inputs, selects, ordinary buttons      */
  --ctl-h-lg: 30px;   /* primary dialog buttons                 */
  --tb-h:     56px;   /* toolbar, icon-above-label              */
  --tb-icon:  24px;
  --row-h:    24px;   /* table row, compact                     */
  --row-h-lg: 30px;   /* table row, comfortable                 */
  --radius:    3px;
  --radius-lg: 6px;   /* dialogs, menus only                    */

  /* Colour — light */
  --bg:            #ffffff;
  --bg-sunken:     #f2f3f5;   /* side panels                    */
  --bg-bar:        #eceef1;   /* toolbar, status bar, thead     */
  --bg-raised:     #ffffff;   /* menus, dialogs                 */
  --bg-row-alt:    #f8f9fa;
  --bg-hover:      #e6e9ed;
  --bg-active:     #dadee4;
  --bg-selected:   #cfe0ff;
  --bg-selected-inactive: #e2e4e8;
  --fg:            #1a1d21;
  --fg-muted:      #5c6470;
  --fg-faint:      #8b929c;
  --fg-on-accent:  #ffffff;
  --border:        #d3d7dd;
  --border-strong: #b4bac2;
  --accent:        #2a6df4;
  --accent-hover:  #1b58d6;
  --danger:        #b00020;
  --warning:       #a06a00;
  --success:       #2a7f2a;
  --mark:          #d97706;
  --shadow-menu:   0 6px 20px rgb(0 0 0 / 18%);
  --shadow-dialog: 0 16px 48px rgb(0 0 0 / 28%);
  --focus-ring:    0 0 0 2px rgb(42 109 244 / 45%);
  --icon-filter:   none;
}

/* Dark: defined once, applied under BOTH the guarded media query and
   the explicit attribute, so a future theme setting wins either way. */
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) { /* …the block below… */ }
}
:root[data-theme="dark"] {
  --bg:            #1b1e23;
  --bg-sunken:     #16181c;
  --bg-bar:        #22262c;
  --bg-raised:     #24262b;
  --bg-row-alt:    #1f2227;
  --bg-hover:      #2b2f36;
  --bg-active:     #343941;
  --bg-selected:   #2f4463;
  --bg-selected-inactive: #2a2e35;
  --fg:            #e4e6ea;
  --fg-muted:      #a0a7b2;
  --fg-faint:      #6f7783;
  --border:        #3a3d44;
  --border-strong: #4a4e57;
  --accent:        #4d86ff;
  --accent-hover:  #6a9aff;
  --danger:        #ff6b7f;
  --warning:       #e0a44a;
  --success:       #6dcf6d;
  --shadow-menu:   0 6px 20px rgb(0 0 0 / 45%);
  --shadow-dialog: 0 16px 48px rgb(0 0 0 / 60%);
  --icon-filter:   invert(1) hue-rotate(180deg) brightness(1.1);
}

html { font-size: var(--fs-body); }
body {
  font-family: var(--font-ui);
  font-size: var(--fs-body);
  line-height: var(--lh-body);
  color: var(--fg);
  background: var(--bg);          /* fixes the §0.3 white-body bug */
  -webkit-font-smoothing: antialiased;
}
```

### 4.2 Base control rules (global, not scoped)

```css
button, input, select, textarea { font: inherit; }

button {
  height: var(--ctl-h);
  padding: 0 var(--sp-4);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: linear-gradient(var(--bg), var(--bg-sunken));
  color: var(--fg);
  cursor: pointer;
}
button:hover:not(:disabled)  { background: var(--bg-hover); }
button:active:not(:disabled) { background: var(--bg-active); }
button:disabled              { opacity: .45; cursor: default; }
button.primary {
  background: var(--accent); border-color: var(--accent); color: var(--fg-on-accent);
}

input[type="text"], input[type="search"], input[type="number"],
input[type="date"], input:not([type]), select {
  height: var(--ctl-h);
  padding: 0 var(--sp-3);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg);
  color: var(--fg);
}

:focus-visible { outline: none; box-shadow: var(--focus-ring); }
```

The subtle top-to-bottom gradient on buttons is the cheapest "native
desktop, not web form" cue. Flat borderless buttons read as web.

### 4.3 Density and dividers

- **Toolbar** padding `var(--sp-2) var(--sp-3)`; gap `var(--sp-1)`
  within a group, `var(--sp-4)` across a separator. Separator = 1px ×
  60% height `var(--border)`, `margin: 0 var(--sp-2)`.
- **Search bar** height 34px, padding `var(--sp-2) var(--sp-3)`, gap
  `var(--sp-2)`.
- **Status bar** height 24px, `--fs-small`, `background: var(--bg-bar)`,
  `border-top: 1px solid var(--border)`.
- **Splitters**: `flex: 0 0 1px`, `background: var(--border)`, with an
  `::after` widening the *hit* area to 7px. The current 4px visible bar
  is too heavy; keep hover-to-`--accent` on the 1px line.
- **Panel edges**: exactly one 1px `--border` between adjacent surfaces.
  No shadows between panels, no rounded corners inside the window
  chrome. `--radius-lg` only on menus and dialogs.
- Nothing in the main window uses `em` sizing any more — the 20-value
  font-size inventory collapses to the five `--fs-*` tokens.

---

## 5. What's missing entirely

### 5.1 Library management

Add a right-aligned toolbar **menu button** showing the current
library's folder name, modelled on
`gui2/actions/choose_library.py:303-365`:

```
📚 Fiction ▾
   ├ Switch/create library…
   ├ Quick switch ▸   Fiction ✓ / Comics / Research / …   (up to 15 recent)
   ├ Switch to previous library             Ctrl+Alt+P
   ├ ──
   ├ Create new library…
   ├ Open existing library…
   ├ Rename library…
   ├ Remove library from list               (forgets the path, does not delete)
   ├ ──
   ├ Copy selected books to ▸               (upstream Copy To Library)
   ├ Export/import all library data
   ├ ──
   └ Maintenance ▸   Metadata backup status · Check library… · Restore database…
```

**Adaptive popup** (§2.3): with only one library known, drop the
chevron and open the chooser directly.

Also: **library name in the window title** (`calibre-oxide — Fiction`),
and a **first-run empty state** when no library is open — a centred
card with Create / Open / recent list. Today that renders as an empty
book table with no explanation.

### 5.2 Jobs and progress

There is no job or progress UI anywhere in `web/src`. Conversions, news
fetches, metadata downloads and polish runs all happen behind a
`Fetching…` label on one button.

- A status-bar `◴ Jobs: 2 ▾` button, hidden at zero, spinner while
  running (upstream `JobsButton`, `gui2/jobs.py:506`).
- Clicking opens a popover: name, progress bar, elapsed, cancel.
- A **transient status-bar message area** (upstream `show_message`) for
  "Added 3 books" — replacing the paragraphs that push the body down.

### 5.3 Status bar

```
calibre-oxide 0.1.0    [1,284 books, 3 selected]  …  [⬆] [▦ Layout ▾] [⋯] [◴ Jobs: 2]
```

- **Left label** is one string (upstream `_set_label`,
  `gui2/init.py:213-231`): app name + version, then bracketed counts. A
  filtered view reads `247 of 1,284 books`; an active virtual library
  appends `, 1,284 total`. The current sort text moves into the Sort
  button's tooltip.
- **Layout button** — a single `Layout ▾` opening a popup of **64px icon
  tiles**, each with the panel name on top and a bold **Show**/**Hide**
  beneath (upstream `gui2/layout_menu.py:64-86`). This replaces the two
  text toggles. Upstream default is `show_layout_buttons = False`;
  individual toggles become an opt-in setting.
- `⋯` = the All-actions button from §1.4.
- Jobs button per §5.2.

### 5.4 Delete `select-mode`

Replace the modal checkbox list with standard list selection:

- Click = select one. `Ctrl/Cmd+click` = toggle. `Shift+click` = range.
  `Ctrl+A` = all. Arrows move, `Shift+Arrow` extends.
- `selectedIds` becomes the single source of truth; `selectedBookId`
  becomes `lastFocusedId` (range anchor, and what the details panel
  shows).
- Keep a checkbox-column toggle in the View menu for touch and
  accessibility, off by default.
- Retire the `select-mode` entry and its `s` binding.

### 5.5 Other gaps

| Missing | Why it matters |
|---|---|
| **Drag-and-drop add affordance** | The handler exists natively, but nothing on screen invites a drop. |
| **Real keyboard shortcuts** | `shortcuts.ts` refuses `ctrlKey \|\| metaKey \|\| altKey`, so `Ctrl+F`, `Ctrl+L`, `Shift+Alt+T` are impossible. Needs a chord parser and an `accel` rendered right-aligned in every menu item. Only 8 of 38 actions are bindable. |
| **Undo for delete** | `deleteSelectedBooks` uses `window.confirm` and is irreversible. At minimum a 10s "Deleted 3 books · Undo" toast plus the menu entry. |
| **Virtual scrolling** | `◀ Prev / Next ▶` pagination is the most web-page-looking element in the window. calibre scrolls the whole library. |
| **Advanced search dialog** | No surface at all; §2.1 gives it the in-field home. |
| **Search history** | Upstream `SearchBox2` is a history combo. Its context menu also offers *Invert current search* / *Paste and search* / *Clear search history*. |
| **`window.prompt` / `window.confirm`** | Used for renames and deletes. Native browser dialogs are a hard tell. Needs a `Dialog.vue`. |
| **Preferences as a dialog** | A flat 9-section scroll page on its own route. At minimum a left category rail + right pane with per-page Apply/Cancel. |
| **About dialog** | None exists. |
| **Tooltips** | `actionTitle` covers 2 of 38 actions. |
| **Remove formats / remove covers** | Upstream's delete menu has both; we have no equivalent. |
| **Cover browser** | Upstream's third panel — but off by default, so genuinely optional. |

---

## 6. Prioritised plan

Effort in rough dev-days, one person.

### P0 — these four and it already reads as a different application

| # | Item | Files | Days |
|---|---|---|---|
| 1 | **Token sheet + mechanical restyle.** §4.1/§4.2 into `style.css`; replace every hardcoded hex and `em` font-size across the 12 components; delete the 12 ad-hoc dark blocks. Fixes the white-body dark-mode bug. | `style.css` + all `.vue` | 1.5 |
| 2 | **Toolbar + search bar restructure.** Registry schema (§1.1), `TOOLBAR_LAYOUT` (§2.2), `ToolbarButton.vue`, icon vendoring script, row swap, two-stage overflow (§2.6). Plus the search bar rebuild (§2.1) and the one-line §0.2 fix. | `actions.ts`, `LibraryView.vue`, new `ToolbarButton.vue`, `scripts/copy-icons.mjs` | 3 |
| 3 | **Categories panel rewrite** (§3.3). | `CategoryBrowser.vue` | 1.5 |
| 4 | **Library discoverability** (§5.1): named library menu button, create/open/rename, window title, first-run empty state. | `LibraryView.vue`, `app/src-tauri/src/{settings,lib}.rs` | 1.5 |

*P0 ≈ 7.5 days.*

### P1 — structural correctness

| # | Item | Days |
|---|---|---|
| 5 | Menu bar restructure to File/Edit/Library/Books/Tools/View/Help; rewrite `menu.rs`; `MenuBar.vue` for browser mode; dynamic submenus. | 2 |
| 6 | Kill `select-mode`; real multi-select and keyboard nav (§5.4). | 1.5 |
| 7 | Panel headers (§3.2) + splitter restyle + fraction-based `layout.ts` (§3.1). | 1 |
| 8 | Status bar rebuild (§5.3). | 1.5 |
| 9 | Book table density: 24px rows, zebra, sticky header, header context menu, format pills, star ratings. | 1 |

*P1 ≈ 7 days.*

### P2 — depth

| # | Item | Days |
|---|---|---|
| 10 | Modifier-capable shortcut chords + accelerators in every menu; expand the bindable set from 8 to 38. | 1.5 |
| 11 | Context-menu re-sectioning + "N books selected" title line. | 0.5 |
| 12 | Recently-viewed submenu; Restore-recently-deleted + undo toast. | 1 |
| 13 | `Dialog.vue` primitive; retire `window.prompt`/`window.confirm`. | 1.5 |
| 14 | Virtual scrolling replacing pagination. | 2 |
| 15 | Details panel: header bar, pin, uniform field grid, 120px cover, format pills. | 1 |
| 16 | Advanced search dialog + search history. | 1.5 |
| 17 | Preferences as a category-rail dialog. | 1.5 |

### P3 — completeness

Drag-and-drop affordance · virtual-library tab strip · About dialog ·
tooltips for all 38 actions · toolbar style setting · row-density
setting · remove-formats / remove-covers · cover browser (optional).

---

## Two decisions to make before starting

1. **Menu bar on Linux/Windows.** calibre ships none there. The
   recommendation is to ship one — it is already half-built in
   `menu.rs` and is the cheapest home for the ~24 actions that will not
   fit on the toolbar. It is a conscious deviation; the literal-parity
   alternative is a hamburger `≡` at the toolbar's right holding the
   same tree.

2. **Toolbar button style default.** Icon-above-label (calibre's
   default) costs 56px of vertical chrome; upstream actually spends
   ~80px, which this spec deliberately does not copy. Icon-beside costs
   32px and fits more buttons before overflow. The recommendation is
   icon-above at 24px as the default, with the setting shipped
   immediately rather than deferred.
