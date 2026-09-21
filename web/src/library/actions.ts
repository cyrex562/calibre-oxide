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
  | "export-catalog"
  | "export-library-archive"
  | "fetch-news"
  | "add-books"
  | "add-folder"
  | "switch-library"
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
  { id: "manage-lists", label: "Manage lists…", group: "library", requires: "none", toolbar: true },
  { id: "custom-columns", label: "Custom columns…", group: "library", requires: "none", toolbar: true },
  { id: "check-library", label: "Check library…", group: "library", requires: "none", toolbar: true },
  { id: "find-duplicates", label: "Find duplicates…", group: "library", requires: "none", toolbar: true },
  { id: "map-metadata", label: "Map authors/tags…", group: "library", requires: "none", toolbar: true },
  { id: "browse-annotations", label: "Annotations…", group: "library", requires: "none", toolbar: true },
  { id: "pick-random", label: "Random book", group: "library", requires: "none", toolbar: true },
  { id: "export-catalog", label: "Export catalog…", group: "library", requires: "none", toolbar: true },
  { id: "export-library-archive", label: "Export library archive…", group: "library", requires: "none", toolbar: true },
  { id: "fetch-news", label: "Fetch news…", group: "library", requires: "none", toolbar: true },
  { id: "add-books", label: "Add Books…", group: "library", requires: "none", toolbar: true },
  { id: "add-folder", label: "Add Folder…", group: "library", requires: "none", toolbar: true, desktopOnly: true },
  { id: "switch-library", label: "Switch library…", group: "library", requires: "none", toolbar: true, desktopOnly: true },

  { id: "select-mode", label: "Select…", group: "view", requires: "none" },
  // Marks are the thing selection is not: they survive a new search,
  // so books can be gathered across several queries and acted on
  // together. Session-only, matching upstream.
  { id: "mark-books", label: "Mark", group: "selection", requires: "selection" },
  { id: "show-marked", label: "Show marked", group: "view", requires: "none", toolbar: false },
  { id: "clear-marks", label: "Clear marks", group: "view", requires: "none", toolbar: false },
  // `toolbar: false` because both already have dedicated controls --
  // the search box and the Table/Grid switch. They are registry
  // entries so a keyboard shortcut can bind to a real action id
  // rather than needing a catalogue of its own.
  { id: "focus-search", label: "Search", group: "view", requires: "none", toolbar: false },
  { id: "toggle-view", label: "Switch table/grid", group: "view", requires: "none", toolbar: false },
  { id: "bulk-edit", label: "Bulk edit", group: "selection", requires: "selection" },
  { id: "save-to-disk", label: "Save to disk", group: "selection", requires: "selection" },

  { id: "read", label: "Read", group: "book", requires: "single-selection", contextMenu: true },
  { id: "open-externally", label: "Open externally", group: "book", requires: "single-selection", contextMenu: true, desktopOnly: true },
  { id: "edit-metadata", label: "Edit metadata", group: "book", requires: "single-selection", contextMenu: true },
  { id: "fetch-metadata", label: "Fetch metadata online…", group: "book", requires: "single-selection", contextMenu: true },
  { id: "convert", label: "Convert…", group: "book", requires: "single-selection", contextMenu: true },
  { id: "tweak-book", label: "Tweak Book…", group: "book", requires: "single-selection", contextMenu: true },
  { id: "quick-view", label: "Quick View…", group: "book", requires: "single-selection", contextMenu: true },
  { id: "test-template", label: "Test template…", group: "book", requires: "single-selection", contextMenu: true },
  { id: "send-email", label: "Send…", group: "book", requires: "single-selection", contextMenu: true },
  { id: "replace-cover", label: "Replace cover…", group: "book", requires: "single-selection", contextMenu: true },
  { id: "similar-books", label: "Similar books", group: "book", requires: "single-selection", contextMenu: true },
  // Selection-scoped rather than book-scoped: polishing a batch is
  // the normal case, and the engine handles books independently.
  { id: "polish", label: "Polish…", group: "selection", requires: "selection" },
  { id: "delete-book", label: "Delete", group: "book", requires: "selection", contextMenu: true },
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
 * Note the two different treatments of "cannot be used": actions that
 * are merely *unavailable* (desktop-only, in a browser) and actions
 * that are *not yet applicable* (need a selection) both drop out of
 * the toolbar rather than rendering disabled, matching how these
 * buttons behaved before the registry existed. A menu makes the
 * opposite choice -- see `buildMenuSpec`.
 */
export function visibleToolbarActions(filter: ToolbarFilter): LibraryAction[] {
  const handled = new Set(filter.handled);
  const hidden = new Set(filter.hidden);
  const suppressed = filter.suppressed ?? new Set<LibraryActionId>();

  return LIBRARY_ACTIONS.filter((a) => {
    if (a.group === "book") return false;
    if (!handled.has(a.id)) return false;
    if (hidden.has(a.id)) return false;
    if (suppressed.has(a.id)) return false;
    if (!actionAvailable(a, filter.ctx)) return false;
    return a.requires === "none" || actionEnabled(a, filter.ctx);
  });
}

/**
 * The toolbar-eligible subset, in registry order. Shape-compatible
 * with the `{ id, label }[]` the settings panel already consumed.
 */
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
