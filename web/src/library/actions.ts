// The library action registry (issue #817, Phase 0.1).
//
// # Why this exists
//
// Before this, "what actions exist" was implicit in each component's
// own template, and the only registry -- `TOOLBAR_ACTIONS` in
// settings/api.ts -- covered exactly the ten self-contained header
// buttons, explicitly excluding anything context-dependent (its own
// comment said so).
//
// Three separate features need the same answer to "what can the user
// do, and is it available right now?": the right-click context menu
// (#1.2), keyboard shortcuts (#1.3), and the desktop app's native menu
// bar (#818). Built independently, each would reimplement enablement
// -- how many books are selected, are we in the desktop app, is this
// action meaningful for the current mode -- and the three copies would
// drift. This module is the single answer.
//
// # This module is data only
//
// No handlers live here. A handler needs live component state
// (`selectedIds`, `bulkOpen`, the file input ref), so it belongs to the
// component. Each *surface* supplies a `Record<LibraryActionId, () =>
// void>` covering the actions it implements, and renders only the
// entries it has a handler for. That keeps this file importable from
// `settings/SettingsView.vue` and from the native-menu bridge without
// dragging LibraryView's state along with it.
//
// # Compatibility
//
// Every pre-existing toolbar action id string is preserved exactly, so
// `ToolbarPrefs` blobs already persisted server-side (`hidden`/`order`
// arrays of these ids) keep working untouched.

/** Where an action belongs in a menu. */
export type ActionGroup = "library" | "book" | "selection" | "view";

/**
 * What must be true for an action to be *enabled*. Availability
 * (whether it appears at all) is separate -- see `desktopOnly`.
 */
export type ActionRequirement =
  /** Always enabled. */
  | "none"
  /** At least one book selected. */
  | "selection"
  /** Exactly one book selected. */
  | "single-selection";

export interface LibraryAction {
  id: LibraryActionId;
  /** Menu/toolbar text. Trailing "…" means it opens a further UI. */
  label: string;
  group: ActionGroup;
  requires: ActionRequirement;
  /**
   * Only available inside the Tauri desktop app -- hidden entirely in
   * a plain browser tab rather than shown disabled, because no amount
   * of user action can satisfy it there.
   */
  desktopOnly?: boolean;
  /**
   * Three states, deliberately:
   * - `true`  -- rendered in the toolbar *and* offered in the
   *   toolbar-customization settings panel.
   * - omitted -- rendered in the toolbar, but not customizable
   *   (the selection actions, which appear and vanish with the
   *   selection and so have nothing stable to reorder).
   * - `false` -- never in the toolbar, because a dedicated control
   *   already exists for it.
   *
   * Actions that only make sense against a specific book
   * (`group: "book"`) are never toolbar-eligible.
   */
  toolbar?: boolean;
  /**
   * Show inline on the toolbar rather than behind its overflow menu.
   *
   * Rendering all fourteen library actions inline wrapped the toolbar
   * onto three rows and cost ~160px of vertical space before a single
   * book was visible. calibre keeps one row and puts the rest behind an
   * overflow, so this marks the handful that earn a permanent slot;
   * everything else is one click away and nothing is lost.
   */
  /**
   * Icon basename under `/icons`, without the extension. Copied from
   * calibre's own set by `scripts/copy-icons.mjs`.
   */
  icon?: string;
  /** Display-only accelerator, e.g. `"Ctrl+Shift+E"`. */
  accel?: string;
  /** One-line hover hint. */
  tooltip?: string;
  /**
   * Kept only until `TOOLBAR_LAYOUT` below fully replaces it.
   * @deprecated superseded by TOOLBAR_LAYOUT
   */
  primary?: boolean;
  /** Offered in a book's right-click context menu (#1.2). */
  contextMenu?: boolean;
}

export type LibraryActionId =
  // Library-wide. These ten are the pre-existing `TOOLBAR_ACTIONS`;
  // their ids must not change (see "Compatibility" above).
  | "manage-lists"
  | "custom-columns"
  | "check-library"
  | "find-duplicates"
  | "map-metadata"
  | "browse-annotations"
  | "pick-random"
  | "help"
  | "export-catalog"
  | "export-library-archive"
  | "fetch-news"
  | "add-books"
  | "add-folder"
  | "switch-library"
  | "new-library"
  // Selection-scoped.
  | "select-mode"
  | "mark-books"
  | "show-marked"
  | "clear-marks"
  | "focus-search"
  | "toggle-view"
  | "bulk-edit"
  | "save-to-disk"
  // Book-scoped.
  | "read"
  | "edit-metadata"
  | "fetch-metadata"
  | "convert"
  | "tweak-book"
  | "quick-view"
  | "test-template"
  | "send-email"
  | "replace-cover"
  | "open-externally"
  | "unpack-book"
  | "repack-book"
  | "similar-books"
  | "polish"
  | "delete-book";

/**
 * Every action the app knows about.
 *
 * Listing an action here is a claim that it exists, not that every
 * surface implements it: a surface renders the intersection of this
 * registry with its own handler map. The book-scoped entries below are
 * implemented by `BookDetailsPanel.vue` today and are declared here so
 * the context menu (#1.2) and the native menu can reach them without a
 * second, drifting list.
 */
export const LIBRARY_ACTIONS: LibraryAction[] = [
  { id: "manage-lists", label: "Manage lists…", group: "library", requires: "none", toolbar: true, icon: "vl", tooltip: "Manage virtual libraries and saved searches" },
  { id: "custom-columns", label: "Custom columns…", group: "library", requires: "none", toolbar: true, icon: "column", tooltip: "Add or remove custom columns" },
  { id: "check-library", label: "Check library…", group: "library", requires: "none", toolbar: true, primary: true, icon: "reports", tooltip: "Look for missing or stray files" },
  { id: "find-duplicates", label: "Find duplicates…", group: "library", requires: "none", toolbar: true, primary: true, icon: "merge", accel: "D", tooltip: "Find books that look like duplicates" },
  { id: "map-metadata", label: "Map authors/tags…", group: "library", requires: "none", toolbar: true, icon: "tags", tooltip: "Rewrite authors or tags in bulk" },
  { id: "browse-annotations", label: "Annotations…", group: "library", requires: "none", toolbar: true, icon: "highlight", tooltip: "Browse highlights and notes" },
  { id: "pick-random", label: "Random book", group: "library", requires: "none", toolbar: true, icon: "random", tooltip: "Jump to a random book" },
  { id: "help", label: "Help…", group: "library", requires: "none", toolbar: true, icon: "help", accel: "F1", tooltip: "Open the documentation" },
  { id: "export-catalog", label: "Export catalog…", group: "library", requires: "none", toolbar: true, icon: "catalog", tooltip: "List this library as CSV, EPUB or XML" },
  { id: "export-library-archive", label: "Export library archive…", group: "library", requires: "none", toolbar: true, icon: "bookshelf", tooltip: "Zip the whole library" },
  { id: "fetch-news", label: "Fetch news…", group: "library", requires: "none", toolbar: true, primary: true, icon: "news", accel: "Ctrl+N", tooltip: "Download news as a book" },
  { id: "add-books", label: "Add Books…", group: "library", requires: "none", toolbar: true, primary: true, icon: "add_book", accel: "Ctrl+A", tooltip: "Add book files to this library" },
  { id: "add-folder", label: "Add Folder…", group: "library", requires: "none", toolbar: true, desktopOnly: true, primary: true, icon: "tb_folder", accel: "Ctrl+Shift+A", tooltip: "Add every book in a folder" },
  { id: "switch-library", label: "Switch library…", group: "library", requires: "none", toolbar: true, desktopOnly: true, primary: true, icon: "lt", accel: "Ctrl+L", tooltip: "Open a different library" },
  // Pointing the server at an empty folder has always produced a
  // working library; there was simply no way to say so. "Switch
  // library -> Browse for another" reads as "find an existing one".
  { id: "new-library", label: "New library…", group: "library", requires: "none", toolbar: true, desktopOnly: true, primary: true, icon: "lt", tooltip: "Create an empty library and open it" },

  { id: "select-mode", label: "Select…", group: "view", requires: "none" },
  // Marks are the thing selection is not: they survive a new search,
  // so books can be gathered across several queries and acted on
  // together. Session-only, matching upstream.
  { id: "mark-books", label: "Mark", group: "selection", requires: "selection", icon: "marked", accel: "M", tooltip: "Mark books to gather them across searches" },
  { id: "show-marked", label: "Show marked", group: "view", requires: "none", toolbar: false },
  { id: "clear-marks", label: "Clear marks", group: "view", requires: "none", toolbar: false },
  // `toolbar: false` because both already have dedicated controls --
  // the search box and the Table/Grid switch. They are registry
  // entries so a keyboard shortcut can bind to a real action id
  // rather than needing a catalogue of its own.
  { id: "focus-search", label: "Search", group: "view", requires: "none", toolbar: false, icon: "search", accel: "Ctrl+F" },
  { id: "toggle-view", label: "Switch table/grid", group: "view", requires: "none", toolbar: false, icon: "grid" },
  { id: "bulk-edit", label: "Bulk edit", group: "selection", requires: "selection", icon: "merge_books", accel: "Ctrl+Shift+E", tooltip: "Change a field across every selected book" },
  { id: "save-to-disk", label: "Save to disk", group: "selection", requires: "selection", icon: "save", accel: "Ctrl+S", tooltip: "Write book files to a folder" },

  { id: "read", label: "Read", group: "book", requires: "single-selection", contextMenu: true, icon: "view", accel: "Enter", tooltip: "Open in the built-in reader" },
  { id: "open-externally", label: "Open externally", group: "book", requires: "single-selection", contextMenu: true, desktopOnly: true, icon: "external-link", accel: "Ctrl+Enter", tooltip: "Open in the system's default application" },
  // Unpack/repack hand a book to whatever editor the user prefers.
  // Desktop-only: both need a real folder and real filesystem access.
  { id: "unpack-book", label: "Unpack to folder…", group: "book", requires: "single-selection", contextMenu: true, desktopOnly: true, icon: "unpack-book", tooltip: "Unpack into a folder for external editing" },
  { id: "repack-book", label: "Repack from folder…", group: "book", requires: "single-selection", contextMenu: true, desktopOnly: true, icon: "sync", tooltip: "Repack from its folder" },
  { id: "edit-metadata", label: "Edit metadata", group: "book", requires: "single-selection", contextMenu: true, icon: "edit_input", accel: "E", tooltip: "Edit this book's metadata" },
  { id: "fetch-metadata", label: "Fetch metadata online…", group: "book", requires: "single-selection", contextMenu: true, icon: "download-metadata", accel: "Ctrl+D", tooltip: "Look up metadata and covers online" },
  { id: "convert", label: "Convert…", group: "book", requires: "single-selection", contextMenu: true, icon: "convert", accel: "C", tooltip: "Convert to another format" },
  { id: "tweak-book", label: "Tweak Book…", group: "book", requires: "single-selection", contextMenu: true, icon: "tweak", accel: "T", tooltip: "Edit the book's files directly" },
  { id: "quick-view", label: "Quick View…", group: "book", requires: "single-selection", contextMenu: true, icon: "quickview", accel: "Q", tooltip: "Peek at this book without leaving the list" },
  { id: "test-template", label: "Test template…", group: "book", requires: "single-selection", contextMenu: true, icon: "template_funcs", tooltip: "Try a template against a real book" },
  { id: "send-email", label: "Send…", group: "book", requires: "single-selection", contextMenu: true, icon: "mail", accel: "Ctrl+E", tooltip: "Email a book to a device or address" },
  { id: "replace-cover", label: "Replace cover…", group: "book", requires: "single-selection", contextMenu: true, icon: "default_cover", tooltip: "Replace the cover image" },
  { id: "similar-books", label: "Similar books", group: "book", requires: "single-selection", contextMenu: true, icon: "similar", tooltip: "Find books like this one" },
  // Selection-scoped rather than book-scoped: polishing a batch is
  // the normal case, and the engine handles books independently.
  { id: "polish", label: "Polish…", group: "selection", requires: "selection", icon: "polish", accel: "P", tooltip: "Improve a book without changing its format" },
  { id: "delete-book", label: "Delete", group: "book", requires: "selection", contextMenu: true, icon: "remove_books", accel: "Del", tooltip: "Remove from the library" },
];

const BY_ID = new Map<LibraryActionId, LibraryAction>(LIBRARY_ACTIONS.map((a) => [a.id, a]));

export function findAction(id: LibraryActionId): LibraryAction | undefined {
  return BY_ID.get(id);
}

/** Live state an action's availability and enablement are judged against. */
export interface ActionContext {
  /** How many books are currently selected. */
  selectionCount: number;
  /** Whether we are running inside the Tauri desktop app. */
  isDesktop: boolean;
}

/**
 * Whether the action can appear at all. A desktop-only action in a
 * browser tab is *unavailable*, not merely disabled -- showing it
 * greyed out would promise something the browser can never deliver.
 */
export function actionAvailable(action: LibraryAction, ctx: ActionContext): boolean {
  return !action.desktopOnly || ctx.isDesktop;
}

/** Whether the action can be invoked right now. */
export function actionEnabled(action: LibraryAction, ctx: ActionContext): boolean {
  if (!actionAvailable(action, ctx)) return false;
  switch (action.requires) {
    case "none":
      return true;
    case "selection":
      return ctx.selectionCount >= 1;
    case "single-selection":
      return ctx.selectionCount === 1;
  }
}


/** One entry of a right-click menu. */
export interface ContextMenuEntry {
  id: LibraryActionId;
  label: string;
  enabled: boolean;
  /** Renders a separator above this entry. */
  startsGroup?: boolean;
}

/**
 * The right-click menu for a book, in registry order.
 *
 * Unlike the toolbar, this keeps disabled entries visible: a menu is
 * also how someone discovers what is possible, and an action greyed
 * out because nothing is selected teaches something, where an action
 * that silently vanishes teaches nothing. Actions that are
 * *unavailable* -- desktop-only, in a browser -- still drop out
 * entirely, since no user action can ever satisfy them.
 *
 * Only actions in `handled` appear, so the menu can never offer
 * something with no implementation behind it.
 */
export function contextMenuEntries(handled: Iterable<LibraryActionId>, ctx: ActionContext): ContextMenuEntry[] {
  const handledSet = new Set(handled);
  const eligible = LIBRARY_ACTIONS.filter((a) => (a.contextMenu || a.group === "selection") && handledSet.has(a.id) && actionAvailable(a, ctx));

  // Registry order puts the selection-scoped actions first, which is
  // right for the toolbar and wrong here: a menu opened by
  // right-clicking a book leads with that book's own actions. Sorting
  // by group is stable, so within a group registry order survives.
  const groupRank: Record<ActionGroup, number> = { book: 0, selection: 1, library: 2, view: 3 };
  eligible.sort((a, b) => groupRank[a.group] - groupRank[b.group]);

  let previousGroup: ActionGroup | null = null;
  return eligible.map((a) => {
    const entry: ContextMenuEntry = {
      id: a.id,
      label: a.label,
      enabled: actionEnabled(a, ctx),
      ...(previousGroup !== null && previousGroup !== a.group ? { startsGroup: true } : {}),
    };
    previousGroup = a.group;
    return entry;
  });
}

/** Everything `visibleToolbarActions` needs to decide what renders. */
export interface ToolbarFilter {
  /** Action ids the calling view actually has a handler for. */
  handled: Iterable<LibraryActionId>;
  /** Ids the user has hidden via toolbar customization. */
  hidden: readonly LibraryActionId[];
  /**
   * Ids suppressed by the view's current mode. Full-text search
   * replaces the result grid entirely, so the actions that operate on
   * that grid have never been shown alongside it.
   */
  suppressed?: ReadonlySet<LibraryActionId>;
  ctx: ActionContext;
}

/**
 * The actions a toolbar should render right now, in registry order.
 *
 * Pure on purpose: this is the rule that decides whether the library's
 * primary controls appear at all, and it is far easier to hold to
 * account here than inside a component computed that would need a full
 * mount and a mocked backend to exercise.
 *
 * Note the two different treatments of "cannot be used":
 *
 * - *Unavailable* (a desktop-only action in a browser) removes the
 *   entry. There is no sense in showing a control the platform can
 *   never satisfy.
 * - *Not yet applicable* (needs a selection) keeps the entry and
 *   renders it disabled, via [`toolbarActionEnabled`].
 *
 * That second case used to remove the entry too, which meant the
 * toolbar physically reflowed as the selection changed -- buttons
 * moved out from under the cursor between one click and the next. No
 * desktop toolbar behaves that way, and it was the loudest thing
 * making this one feel like a web page.
 */
/**
 * Whether an action should render on the toolbar *disabled* rather than
 * be dropped from it.
 *
 * Callers render `visibleToolbarActions` and ask this per entry.
 */
export function toolbarActionEnabled(action: LibraryAction, ctx: ActionContext): boolean {
  return action.requires === "none" || actionEnabled(action, ctx);
}

export function visibleToolbarActions(filter: ToolbarFilter): LibraryAction[] {
  const handled = new Set(filter.handled);
  const hidden = new Set(filter.hidden);
  const suppressed = filter.suppressed ?? new Set<LibraryActionId>();

  return LIBRARY_ACTIONS.filter((a) => {
    if (a.group === "book") return false;
    if (!handled.has(a.id)) return false;
    if (hidden.has(a.id)) return false;
    if (suppressed.has(a.id)) return false;
    // Availability is about the *platform* -- a desktop-only action has
    // no business on the web at all -- so it still removes the entry.
    return actionAvailable(a, filter.ctx);
    // Deliberately NOT filtered on `actionEnabled`: see the doc above.
  });
}

/**
 * The toolbar-eligible subset, in registry order. Shape-compatible
 * with the `{ id, label }[]` the settings panel already consumed.
 */
/**
 * A dynamic block, resolved when its menu opens rather than at build
 * time -- a recently-viewed list is different every time it is shown.
 */
export type DynamicMenuId = "$recently-viewed" | "$restore-deleted" | "$recent-libraries" | "$shortcuts" | "$about";

export type ToolbarEntry = LibraryActionId | "-" | DynamicMenuId;

export type ToolbarItem =
  /** A plain button. */
  | { kind: "action"; id: LibraryActionId }
  /** A button whose body runs `id` and whose chevron opens `menu`. */
  | { kind: "split"; id: LibraryActionId; menu: ToolbarEntry[] }
  /** A named menu that is not itself an action -- the library button. */
  | { kind: "menu"; id: string; label: string; icon: string; menu: ToolbarEntry[] }
  | { kind: "separator" }
  /** Pushes everything after it to the far end. */
  | { kind: "spring" };

/**
 * What the toolbar renders, in order.
 *
 * Twelve slots reaching about thirty actions, because each slot is a
 * *family*: the button body runs the one you reach for most, and the
 * chevron holds its relatives. A flat list of thirty buttons is
 * unreadable, and a flat list of six -- which is what the `primary`
 * flag gave -- buries the other twenty-four behind an undifferentiated
 * "More".
 *
 * Slots are earned by frequency rather than importance. "Check
 * library" is important and rare, so it lives in a menu; "Add books"
 * is neither rare nor avoidable, so it gets the first slot.
 *
 * See docs/UI_DESIGN.md §2.2.
 */
export const TOOLBAR_LAYOUT: ToolbarItem[] = [
  // `fetch-news` is deliberately *not* in here: it has its own slot
  // below, as it does in calibre's own toolbar. Listing it twice would
  // make the Add menu look fuller than it is without adding a route.
  { kind: "split", id: "add-books", menu: ["add-folder"] },
  { kind: "split", id: "edit-metadata", menu: ["fetch-metadata", "replace-cover", "-", "bulk-edit", "map-metadata", "-", "test-template"] },
  { kind: "separator" },
  { kind: "split", id: "convert", menu: ["polish", "tweak-book", "-", "unpack-book", "repack-book"] },
  { kind: "split", id: "read", menu: ["open-externally", "quick-view", "-", "$recently-viewed", "-", "similar-books"] },
  { kind: "separator" },
  { kind: "split", id: "save-to-disk", menu: ["send-email", "-", "export-catalog", "export-library-archive"] },
  { kind: "action", id: "fetch-news" },
  { kind: "separator" },
  { kind: "split", id: "delete-book", menu: ["-", "$restore-deleted"] },
  { kind: "spring" },
  // Label is replaced at render time with the open library's name:
  // which library you are in is state, and state belongs on a label.
  { kind: "menu", id: "library", label: "Library", icon: "lt", menu: ["switch-library", "new-library", "-", "$recent-libraries", "-", "check-library", "find-duplicates"] },
  { kind: "split", id: "help", menu: ["$shortcuts", "$about"] },
];

/** Every action id `TOOLBAR_LAYOUT` can reach, on a button or in a menu. */
export function toolbarLayoutActionIds(): LibraryActionId[] {
  const out: LibraryActionId[] = [];
  for (const item of TOOLBAR_LAYOUT) {
    if (item.kind === "action" || item.kind === "split") out.push(item.id);
    if (item.kind === "split" || item.kind === "menu") {
      for (const entry of item.menu) {
        if (entry !== "-" && !entry.startsWith("$")) out.push(entry as LibraryActionId);
      }
    }
  }
  return out;
}

export const TOOLBAR_ACTIONS: LibraryAction[] = LIBRARY_ACTIONS.filter((a) => a.toolbar);

export type ToolbarActionId = LibraryActionId;

/**
 * One entry of the menu spec handed to the desktop app.
 *
 * The page is the authority on what it can do: rather than the Rust
 * side keeping a second copy of this registry (which would drift the
 * moment an action is added here), it receives this list and builds
 * the native menu from it. See `web/src/library/desktopMenu.ts`.
 */
export interface MenuActionSpec {
  id: LibraryActionId;
  label: string;
  group: ActionGroup;
  enabled: boolean;
}

/**
 * Builds the native-menu spec for the actions `handled` covers.
 *
 * Only actions with a real handler are included, so the menu never
 * offers something that would silently do nothing. Unavailable actions
 * (desktop-only in a browser, though that combination cannot arise for
 * a native menu) drop out; merely-disabled ones stay, greyed, because
 * selecting a book is a thing the user can actually go and do.
 */
export function buildMenuSpec(handled: Iterable<LibraryActionId>, ctx: ActionContext): MenuActionSpec[] {
  const handledSet = new Set(handled);
  return LIBRARY_ACTIONS.filter((a) => handledSet.has(a.id) && actionAvailable(a, ctx)).map((a) => ({
    id: a.id,
    label: a.label,
    group: a.group,
    enabled: actionEnabled(a, ctx),
  }));
}
