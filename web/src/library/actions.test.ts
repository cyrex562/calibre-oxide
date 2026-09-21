import { describe, expect, it } from "vitest";

import { actionAvailable, actionEnabled, buildMenuSpec, findAction, LIBRARY_ACTIONS, TOOLBAR_ACTIONS, visibleToolbarActions, type ActionContext, type LibraryAction, type LibraryActionId } from "./actions";

const DESKTOP: ActionContext = { selectionCount: 0, isDesktop: true };
const BROWSER: ActionContext = { selectionCount: 0, isDesktop: false };

function action(id: LibraryActionId): LibraryAction {
  const found = findAction(id);
  if (!found) throw new Error(`no such action: ${id}`);
  return found;
}

describe("the registry itself", () => {
  it("has no duplicate ids", () => {
    const ids = LIBRARY_ACTIONS.map((a) => a.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  // The compatibility guarantee in actions.ts's own doc: `ToolbarPrefs`
  // blobs already persisted server-side store these id strings in
  // their `hidden`/`order` arrays. Renaming one silently drops a
  // user's saved toolbar layout for that action, which no type error
  // would catch -- the prefs are JSON on a server, not TypeScript.
  it("keeps every pre-existing toolbar action id verbatim, in its original relative order", () => {
    // Adding a *new* toolbar action is fine; renaming, dropping or
    // reordering an existing one is not, because that is what the
    // persisted prefs reference. So this asserts the original ids
    // survive as a subsequence rather than as the whole list.
    const before = ["manage-lists", "custom-columns", "check-library", "find-duplicates", "export-catalog", "export-library-archive", "fetch-news", "add-books", "add-folder", "switch-library"];
    const now = TOOLBAR_ACTIONS.map((a) => a.id);

    for (const id of before) expect(now, `${id} disappeared from the toolbar registry`).toContain(id);
    expect(now.filter((id) => before.includes(id))).toEqual(before);
  });

  it("does not offer book-scoped actions in the toolbar", () => {
    // "Convert…" with no book selected is meaningless as a toolbar
    // button; those actions reach the user through a book's context
    // menu instead.
    for (const a of LIBRARY_ACTIONS.filter((a) => a.group === "book")) {
      expect(a.toolbar, `${a.id} should not be toolbar-eligible`).not.toBe(true);
    }
  });

  it("marks every book-scoped action for the context menu", () => {
    for (const a of LIBRARY_ACTIONS.filter((a) => a.group === "book")) {
      expect(a.contextMenu, `${a.id} should appear in the context menu`).toBe(true);
    }
  });
});

describe("availability", () => {
  it("hides desktop-only actions in a browser tab rather than disabling them", () => {
    // Greying these out would promise something a browser tab can
    // never deliver no matter what the user does.
    expect(actionAvailable(action("add-folder"), BROWSER)).toBe(false);
    expect(actionAvailable(action("switch-library"), BROWSER)).toBe(false);
    expect(actionEnabled(action("add-folder"), BROWSER)).toBe(false);
  });

  it("allows desktop-only actions inside the desktop app", () => {
    expect(actionAvailable(action("add-folder"), DESKTOP)).toBe(true);
    expect(actionEnabled(action("add-folder"), DESKTOP)).toBe(true);
  });

  it("leaves non-desktop actions available everywhere", () => {
    expect(actionAvailable(action("add-books"), BROWSER)).toBe(true);
    expect(actionAvailable(action("export-catalog"), BROWSER)).toBe(true);
  });
});

describe("enablement", () => {
  it("enables 'none' actions with nothing selected", () => {
    expect(actionEnabled(action("add-books"), BROWSER)).toBe(true);
    expect(actionEnabled(action("select-mode"), BROWSER)).toBe(true);
  });

  it("requires at least one book for selection actions", () => {
    const empty = { ...BROWSER, selectionCount: 0 };
    const one = { ...BROWSER, selectionCount: 1 };
    const many = { ...BROWSER, selectionCount: 7 };

    expect(actionEnabled(action("bulk-edit"), empty)).toBe(false);
    expect(actionEnabled(action("bulk-edit"), one)).toBe(true);
    expect(actionEnabled(action("bulk-edit"), many)).toBe(true);
  });

  it("requires exactly one book for single-selection actions", () => {
    // Converting or tweaking acts on one book's formats; upstream
    // gates these on a single selection too.
    expect(actionEnabled(action("convert"), { ...BROWSER, selectionCount: 0 })).toBe(false);
    expect(actionEnabled(action("convert"), { ...BROWSER, selectionCount: 1 })).toBe(true);
    expect(actionEnabled(action("convert"), { ...BROWSER, selectionCount: 2 })).toBe(false);
  });

  it("lets delete act on a whole selection", () => {
    // Deleting is deliberately `selection`, not `single-selection`:
    // removing several books at once is a normal thing to want.
    expect(actionEnabled(action("delete-book"), { ...BROWSER, selectionCount: 5 })).toBe(true);
  });
});

describe("the native menu spec", () => {
  it("only includes actions the caller actually handles", () => {
    // The point of passing `handled` rather than the whole registry:
    // a menu entry with no handler behind it would silently do
    // nothing when clicked.
    const spec = buildMenuSpec(["add-books", "export-catalog"], DESKTOP);
    expect(spec.map((s) => s.id)).toEqual(["export-catalog", "add-books"]);
  });

  it("keeps registry order regardless of the order handlers were listed in", () => {
    const spec = buildMenuSpec(["switch-library", "manage-lists", "add-books"], DESKTOP);
    expect(spec.map((s) => s.id)).toEqual(["manage-lists", "add-books", "switch-library"]);
  });

  it("carries enablement through instead of dropping disabled entries", () => {
    // A disabled entry is worth showing: selecting a book is
    // something the user can go and do, unlike installing a desktop
    // app from inside a browser tab.
    const spec = buildMenuSpec(["bulk-edit"], { ...DESKTOP, selectionCount: 0 });
    expect(spec).toHaveLength(1);
    expect(spec[0].enabled).toBe(false);

    const withSelection = buildMenuSpec(["bulk-edit"], { ...DESKTOP, selectionCount: 3 });
    expect(withSelection[0].enabled).toBe(true);
  });

  it("drops unavailable actions entirely", () => {
    expect(buildMenuSpec(["add-folder"], BROWSER)).toEqual([]);
  });

  it("ignores ids that are not in the registry", () => {
    expect(buildMenuSpec(["not-a-real-action" as LibraryActionId], DESKTOP)).toEqual([]);
  });

  it("carries the group through so the menu can build submenus", () => {
    const spec = buildMenuSpec(["add-books", "bulk-edit"], { ...DESKTOP, selectionCount: 1 });
    expect(spec.find((s) => s.id === "add-books")?.group).toBe("library");
    expect(spec.find((s) => s.id === "bulk-edit")?.group).toBe("selection");
  });
});

describe("what the toolbar renders", () => {
  // The ids LibraryView.vue actually supplies handlers for.
  const HANDLED: LibraryActionId[] = ["manage-lists", "custom-columns", "check-library", "find-duplicates", "map-metadata", "browse-annotations", "export-catalog", "export-library-archive", "fetch-news", "add-books", "add-folder", "switch-library", "select-mode", "bulk-edit", "save-to-disk"];

  const FTS_SUPPRESSED = new Set<LibraryActionId>(["manage-lists", "custom-columns", "check-library", "find-duplicates", "map-metadata", "browse-annotations", "export-catalog", "export-library-archive", "fetch-news", "select-mode", "bulk-edit", "save-to-disk"]);

  const base = { handled: HANDLED, hidden: [] as LibraryActionId[] };

  // The refactor that introduced this function replaced thirteen
  // hardcoded buttons with one loop. If the filter is ever wrong in a
  // way that empties it, the library view loses every control it has
  // and nothing else in the suite would notice.
  it("renders the full desktop toolbar when nothing is hidden or suppressed", () => {
    const ids = visibleToolbarActions({ ...base, ctx: DESKTOP }).map((a) => a.id);
    expect(ids).toEqual(["manage-lists", "custom-columns", "check-library", "find-duplicates", "map-metadata", "browse-annotations", "export-catalog", "export-library-archive", "fetch-news", "add-books", "add-folder", "switch-library", "select-mode"]);
  });

  it("drops the two desktop-only actions in a browser tab", () => {
    const ids = visibleToolbarActions({ ...base, ctx: BROWSER }).map((a) => a.id);
    expect(ids).not.toContain("add-folder");
    expect(ids).not.toContain("switch-library");
    expect(ids).toContain("add-books");
  });

  it("reveals the selection actions only once books are selected", () => {
    const none = visibleToolbarActions({ ...base, ctx: DESKTOP }).map((a) => a.id);
    expect(none).not.toContain("bulk-edit");
    expect(none).not.toContain("save-to-disk");

    const some = visibleToolbarActions({ ...base, ctx: { ...DESKTOP, selectionCount: 2 } }).map((a) => a.id);
    expect(some).toContain("bulk-edit");
    expect(some).toContain("save-to-disk");
  });

  it("honours the user's hidden list", () => {
    const ids = visibleToolbarActions({ ...base, hidden: ["fetch-news", "export-catalog"], ctx: DESKTOP }).map((a) => a.id);
    expect(ids).not.toContain("fetch-news");
    expect(ids).not.toContain("export-catalog");
    expect(ids).toContain("check-library");
  });

  it("keeps only add/switch-library in full-text search mode", () => {
    // FTS replaces the result grid, so the actions that operate on
    // that grid have never been shown beside it -- but adding books
    // and switching library always were.
    const ids = visibleToolbarActions({ ...base, suppressed: FTS_SUPPRESSED, ctx: DESKTOP }).map((a) => a.id);
    expect(ids).toEqual(["add-books", "add-folder", "switch-library"]);
  });

  it("never puts a book-scoped action in the toolbar", () => {
    // Even if a view were to supply a handler for one.
    const ids = visibleToolbarActions({ handled: ["convert", "delete-book", "add-books"], hidden: [], ctx: { ...DESKTOP, selectionCount: 1 } }).map((a) => a.id);
    expect(ids).toEqual(["add-books"]);
  });

  it("renders nothing for a view that handles nothing", () => {
    expect(visibleToolbarActions({ handled: [], hidden: [], ctx: DESKTOP })).toEqual([]);
  });
});
