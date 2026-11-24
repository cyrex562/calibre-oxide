import { describe, expect, it } from "vitest";

import { actionAvailable, actionEnabled, buildMenuSpec, findAction, LIBRARY_ACTIONS, refusalText, toolbarActionEnabled, TOOLBAR_ACTIONS, TOOLBAR_LAYOUT, toolbarLayoutActionIds, toolbarOverflowEntries, toolbarOverflowIds, visibleToolbarActions, type ActionContext, type LibraryAction, type LibraryActionId } from "./actions";

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

  it("does not offer book-scoped actions in the toolbar settings, except the ones the layout places", () => {
    // "Fetch metadata" with no book selected is meaningless as a toolbar
    // button; those actions reach the user through a book's context
    // menu instead. Convert is the exception: it has a place in More.
    const placed = new Set(["convert"]);
    for (const a of LIBRARY_ACTIONS.filter((a) => a.group === "book" && !placed.has(a.id))) {
      expect(a.toolbar, `${a.id} should not be toolbar-eligible`).not.toBe(true);
    }
  });

  it("offers every main action in Settings, Toolbar: Convert, Polish, Merge, Export a copy and Organize", () => {
    const offered = TOOLBAR_ACTIONS.map((a) => a.id);
    for (const id of ["convert", "polish", "merge-books", "export-copy", "organize-files"] as const) {
      expect(offered, `${id} is missing from the toolbar settings`).toContain(id);
      // And the toolbar can actually show it: it is placed in the layout or More.
      expect([...toolbarLayoutActionIds(), ...toolbarOverflowIds()], `${id} has no place on the toolbar`).toContain(id);
    }
  });

  it("hides one of them when the user turns it off", () => {
    const ctx = { selectionCount: 2, isDesktop: false } as unknown as ActionContext;
    const handled: LibraryActionId[] = ["convert", "polish", "merge-books", "export-copy", "organize-files"];
    const shown = (hidden: LibraryActionId[]) => visibleToolbarActions({ handled, hidden, ctx }, { includeBook: true }).map((a) => a.id);
    expect(shown([])).toEqual(expect.arrayContaining(handled));
    expect(shown(["polish", "organize-files"])).not.toContain("polish");
    expect(shown(["polish", "organize-files"])).not.toContain("organize-files");
    expect(shown(["polish", "organize-files"])).toContain("convert");
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
    expect(actionAvailable(action("switch-library"), BROWSER)).toBe(false);
    expect(actionEnabled(action("switch-library"), BROWSER)).toBe(false);
  });

  it("allows desktop-only actions inside the desktop app", () => {
    expect(actionAvailable(action("switch-library"), DESKTOP)).toBe(true);
    expect(actionEnabled(action("switch-library"), DESKTOP)).toBe(true);
  });

  // The folder dialog takes a typed path, so a browser tab can add a folder too.
  // It used to be desktop-only, which left the empty library's second button dead.
  it("offers Add folder in a browser tab", () => {
    expect(actionAvailable(action("add-folder"), BROWSER)).toBe(true);
    expect(actionEnabled(action("add-folder"), BROWSER)).toBe(true);
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

  it("requires two or more books for merge", () => {
    expect(actionEnabled(action("merge-books"), { ...BROWSER, selectionCount: 0 })).toBe(false);
    expect(actionEnabled(action("merge-books"), { ...BROWSER, selectionCount: 1 })).toBe(false);
    expect(actionEnabled(action("merge-books"), { ...BROWSER, selectionCount: 2 })).toBe(true);
    expect(actionEnabled(action("merge-books"), { ...BROWSER, selectionCount: 9 })).toBe(true);
  });

  it("offers Compare as duplicates for two or more books, in the context menu", () => {
    const compare = action("compare-duplicates");
    expect(compare.contextMenu).toBe(true);
    expect(actionEnabled(compare, { ...BROWSER, selectionCount: 1 })).toBe(false);
    expect(actionEnabled(compare, { ...BROWSER, selectionCount: 2 })).toBe(true);
    expect(actionEnabled(compare, { ...BROWSER, selectionCount: 8 })).toBe(true);
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
    expect(buildMenuSpec(["switch-library"], BROWSER)).toEqual([]);
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
  const HANDLED: LibraryActionId[] = ["manage-lists", "custom-columns", "check-library", "find-duplicates", "map-metadata", "browse-annotations", "export-catalog", "export-library-archive", "fetch-news", "add-books", "add-folder", "switch-library", "new-library", "select-mode", "bulk-edit", "save-to-disk"];

  const FTS_SUPPRESSED = new Set<LibraryActionId>(["manage-lists", "custom-columns", "check-library", "find-duplicates", "map-metadata", "browse-annotations", "export-catalog", "export-library-archive", "fetch-news", "select-mode", "bulk-edit", "save-to-disk"]);

  const base = { handled: HANDLED, hidden: [] as LibraryActionId[] };

  // The refactor that introduced this function replaced thirteen
  // hardcoded buttons with one loop. If the filter is ever wrong in a
  // way that empties it, the library view loses every control it has
  // and nothing else in the suite would notice.
  it("renders the full desktop toolbar when nothing is hidden or suppressed", () => {
    const ids = visibleToolbarActions({ ...base, ctx: DESKTOP }).map((a) => a.id);
    // Selection-scoped entries are present with nothing selected: the
    // toolbar renders them disabled rather than reflowing around them.
    expect(ids).toEqual([
      "manage-lists", "custom-columns", "check-library", "find-duplicates", "map-metadata",
      "browse-annotations", "export-catalog", "export-library-archive", "fetch-news",
      "add-books", "add-folder", "switch-library", "new-library",
      "select-mode", "bulk-edit", "save-to-disk",
    ]);
  });

  it("drops the desktop-only actions in a browser tab", () => {
    const ids = visibleToolbarActions({ ...base, ctx: BROWSER }).map((a) => a.id);
    expect(ids).toContain("add-folder");
    expect(ids).not.toContain("switch-library");
    expect(ids).toContain("add-books");
  });

  // The toolbar must not change *shape* with the selection -- a button
  // that appears and disappears moves its neighbours out from under the
  // cursor between one click and the next.
  it("keeps the same entries whether or not books are selected", () => {
    const none = visibleToolbarActions({ ...base, ctx: DESKTOP }).map((a) => a.id);
    const some = visibleToolbarActions({ ...base, ctx: { ...DESKTOP, selectionCount: 2 } }).map((a) => a.id);
    expect(some).toEqual(none);
  });

  it("reports selection actions as disabled until something is selected", () => {
    const bulk = LIBRARY_ACTIONS.find((a) => a.id === "bulk-edit")!;
    expect(toolbarActionEnabled(bulk, DESKTOP)).toBe(false);
    expect(toolbarActionEnabled(bulk, { ...DESKTOP, selectionCount: 2 })).toBe(true);
  });

  it("leaves always-available actions enabled with no selection", () => {
    const add = LIBRARY_ACTIONS.find((a) => a.id === "add-books")!;
    expect(toolbarActionEnabled(add, DESKTOP)).toBe(true);
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
    // and choosing a library always were, and creating one is as valid
    // here as switching to one.
    const ids = visibleToolbarActions({ ...base, suppressed: FTS_SUPPRESSED, ctx: DESKTOP }).map((a) => a.id);
    expect(ids).toEqual(["add-books", "add-folder", "switch-library", "new-library"]);
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

describe("primary toolbar actions", () => {
  // The toolbar renders `primary` actions inline and the rest behind an
  // overflow. Before that split, all fourteen library actions rendered
  // inline and wrapped onto three rows, costing ~160px of vertical
  // space above the book list.
  it("marks only a handful as primary, so the toolbar stays one row", () => {
    const primary = LIBRARY_ACTIONS.filter((a) => a.primary);
    expect(primary.length).toBeGreaterThan(0);
    expect(primary.length).toBeLessThanOrEqual(8);
  });

  it("only offers a permanent slot to something the toolbar shows at all", () => {
    for (const action of LIBRARY_ACTIONS.filter((a) => a.primary)) {
      expect(action.toolbar, `${action.id} is primary but not toolbar-eligible`).not.toBe(false);
    }
  });

  // A book action's availability depends on the selection, so pinning
  // one to the toolbar would give it a slot that is disabled most of
  // the time.
  it("keeps selection-dependent actions out of the permanent set", () => {
    for (const action of LIBRARY_ACTIONS.filter((a) => a.primary)) {
      expect(action.requires, `${action.id} is primary but needs a selection`).toBe("none");
    }
  });

  it("splits the toolbar set cleanly in two, losing nothing", () => {
    const shown = visibleToolbarActions({
      handled: LIBRARY_ACTIONS.map((a) => a.id),
      hidden: [],
      ctx: DESKTOP,
    });
    const primary = shown.filter((a) => a.primary);
    const overflow = shown.filter((a) => !a.primary);
    expect(primary.length + overflow.length).toBe(shown.length);
    // Every action still reachable from one surface or the other.
    expect(new Set([...primary, ...overflow].map((a) => a.id)).size).toBe(shown.length);
  });
});

describe("TOOLBAR_LAYOUT", () => {
  // A typo'd id here is a button that silently does nothing, which no
  // other test would notice.
  it("only names actions that exist", () => {
    const known = new Set(LIBRARY_ACTIONS.map((a) => a.id));
    for (const id of toolbarLayoutActionIds()) {
      expect(known.has(id), `${id} is in TOOLBAR_LAYOUT but not in the registry`).toBe(true);
    }
  });

  it("gives every button an icon", () => {
    for (const item of TOOLBAR_LAYOUT) {
      if (item.kind === "action" || item.kind === "split") {
        const action = LIBRARY_ACTIONS.find((a) => a.id === item.id)!;
        expect(action.icon, `${item.id} has a toolbar slot but no icon`).toBeTruthy();
      }
      if (item.kind === "menu") expect(item.icon).toBeTruthy();
    }
  });

  // A slot whose primary needs a selection is disabled most of the
  // time, which wastes the scarcest space in the window.
  it("never gives a slot to something that needs a selection", () => {
    for (const item of TOOLBAR_LAYOUT) {
      if (item.kind !== "action" && item.kind !== "split") continue;
      const action = LIBRARY_ACTIONS.find((a) => a.id === item.id)!;
      if (action.group === "book" || action.requires !== "none") {
        // Book actions are allowed a slot -- they are the point of a
        // library -- but they must be reachable with one selected.
        expect(["single-selection", "selection", "multi-selection"]).toContain(action.requires);
      }
    }
  });

  it("does not list the same action twice", () => {
    const ids = toolbarLayoutActionIds();
    expect(new Set(ids).size, `duplicates: ${ids.filter((v, i) => ids.indexOf(v) !== i)}`).toBe(ids.length);
  });

  it("has exactly one spring, so the right-hand cluster is unambiguous", () => {
    expect(TOOLBAR_LAYOUT.filter((i) => i.kind === "spring")).toHaveLength(1);
  });

  it("never starts or ends with a separator", () => {
    expect(TOOLBAR_LAYOUT[0].kind).not.toBe("separator");
    expect(TOOLBAR_LAYOUT[TOOLBAR_LAYOUT.length - 1].kind).not.toBe("separator");
  });

  it("puts no empty menu on a split button", () => {
    for (const item of TOOLBAR_LAYOUT) {
      if (item.kind === "split" || item.kind === "menu") {
        expect(item.menu.filter((e) => e !== "-").length, `${item.id} has an empty menu`).toBeGreaterThan(0);
      }
    }
  });
});

describe("toolbar overflow", () => {
  // The regression this exists to prevent: replacing the old static
  // `More ▾` with TOOLBAR_LAYOUT silently orphaned four actions --
  // manage-lists, custom-columns, browse-annotations and pick-random --
  // because nothing checked that every eligible action had a home.
  it("gives every toolbar-eligible action a home", () => {
    const placed = new Set(toolbarLayoutActionIds());
    const overflow = new Set(toolbarOverflowIds());
    const homeless = LIBRARY_ACTIONS
      .filter((a) => a.toolbar && a.group !== "book")
      .filter((a) => !placed.has(a.id) && !overflow.has(a.id))
      .map((a) => a.id);
    expect(homeless, `no toolbar home: ${homeless.join(", ")}`).toEqual([]);
  });

  it("does not repeat a slotted action in the overflow", () => {
    const placed = new Set(toolbarLayoutActionIds());
    for (const id of toolbarOverflowIds()) {
      expect(placed.has(id), `${id} is both slotted and in the overflow`).toBe(false);
    }
  });

  // Convert, Read/View and Fetch news lost their slots to the
  // import / dedupe / organize workflow; nothing was deleted.
  it("keeps Convert, Read, Fetch news and Save to disk one click away, in More", () => {
    const overflow = new Set(toolbarOverflowIds());
    for (const id of ["convert", "read", "open-externally", "quick-view", "fetch-news", "save-to-disk", "polish", "tweak-book", "similar-books"] as LibraryActionId[]) {
      expect(overflow.has(id), `${id} is not reachable from More`).toBe(true);
    }
  });

  it("builds the More menu with separators only between entries", () => {
    const entries = toolbarOverflowEntries();
    expect(entries[0]).not.toBe("-");
    expect(entries[entries.length - 1]).not.toBe("-");
    expect(entries.filter((e) => e !== "-" && !e.startsWith("$")).sort()).toEqual([...toolbarOverflowIds()].sort());
    for (let i = 1; i < entries.length; i++) expect(entries[i] === "-" && entries[i - 1] === "-").toBe(false);
  });

  it("only lets the overflow name book-scoped actions that it lists on purpose", () => {
    for (const id of toolbarOverflowIds()) {
      const a = LIBRARY_ACTIONS.find((x) => x.id === id)!;
      if (a.group === "book") expect(a.contextMenu, `${id}`).toBe(true);
    }
  });
});

describe("the workflow toolbar", () => {
  const slots = () => TOOLBAR_LAYOUT.filter((i) => i.kind === "split" || i.kind === "action");

  it("leads with import, find duplicates, organize, edit metadata, remove, in that order", () => {
    expect(slots().slice(0, 5).map((s) => ("id" in s ? s.id : ""))).toEqual(["add-books", "find-duplicates", "organize-files", "edit-metadata", "delete-book"]);
  });

  it("carries Compare, Merge and Resolve under Find duplicates, and File naming, Fix titles and Export under Organize", () => {
    const menuOf = (id: string) => {
      const s = TOOLBAR_LAYOUT.find((i) => i.kind === "split" && i.id === id);
      return s && s.kind === "split" ? s.menu : [];
    };
    expect(menuOf("find-duplicates")).toEqual(["compare-duplicates", "merge-books", "resolve-duplicates"]);
    expect(menuOf("organize-files")).toEqual(["file-naming", "fix-titles", "export-copy"]);
  });

  it("no longer gives Convert, Read or Fetch news a slot of their own", () => {
    const ids = slots().map((s) => ("id" in s ? s.id : ""));
    for (const id of ["convert", "read", "fetch-news", "save-to-disk"]) expect(ids).not.toContain(id);
  });

  it("shows the layout's book-scoped slots (Edit metadata, Remove) when asked, and never in the flat customizable list", () => {
    const filter = { handled: ["edit-metadata", "delete-book", "add-books"] as LibraryActionId[], hidden: [], ctx: BROWSER };
    expect(visibleToolbarActions(filter).map((a) => a.id)).toEqual(["add-books"]);
    expect(visibleToolbarActions(filter, { includeBook: true }).map((a) => a.id).sort()).toEqual(["add-books", "delete-book", "edit-metadata"]);
  });

  it("labels the Organize and Remove slots as the workflow words", () => {
    const labelOf = (id: string) => {
      const s = TOOLBAR_LAYOUT.find((i) => i.kind === "split" && i.id === id);
      return s && s.kind === "split" ? s.label : undefined;
    };
    expect(labelOf("organize-files")).toBe("Organize");
    expect(labelOf("delete-book")).toBe("Move to Trash");
  });
});

describe("refusalText", () => {
  it("says what the action needs, so a click that does nothing can explain itself", () => {
    expect(refusalText(action("read"), BROWSER)).toBe("Read needs exactly one book selected.");
    expect(refusalText(action("merge-books"), BROWSER)).toBe("Merge books needs two or more books selected.");
    expect(refusalText(action("save-to-disk"), BROWSER)).toBe("Save to disk needs at least one book selected.");
    expect(refusalText(action("switch-library"), BROWSER)).toBe("Switch library is only available in the desktop app.");
  });
});

describe("one word for removing a book", () => {
  it("calls every reversible removal Move to Trash, and keeps Delete for the Trash dialog", () => {
    expect(action("delete-book").label).toBe("Move to Trash");
    expect(action("trash").label).toBe("Open Trash…");
    const labels = LIBRARY_ACTIONS.map((a) => a.label);
    expect(labels.filter((l) => /^(Delete|Remove)\b/.test(l))).toEqual([]);
  });
});

describe("the trash action", () => {
  it("is available everywhere, needs no selection, and lives in the Move to Trash menu rather than the toolbar", () => {
    const trash = LIBRARY_ACTIONS.find((a) => a.id === "trash")!;
    expect(trash).toBeDefined();
    expect(trash.requires).toBe("none");
    expect(trash.toolbar).toBeFalsy();
    expect(TOOLBAR_ACTIONS.map((a) => a.id)).not.toContain("trash");
    const remove = TOOLBAR_LAYOUT.find((i) => i.kind === "split" && i.id === "delete-book");
    expect(remove && "menu" in remove ? remove.menu : []).toContain("trash");
  });

  it("is not suppressed by being a browser tab: restoring a deleted book works the same there", () => {
    const trash = LIBRARY_ACTIONS.find((a) => a.id === "trash")!;
    expect(trash.desktopOnly).toBeFalsy();
  });
});

describe("Show in folder", () => {
  it("is a desktop-only context-menu action named Show in folder", () => {
    const a = LIBRARY_ACTIONS.find((x) => x.id === "open-book-folder")!;
    expect(a.label).toBe("Show in folder");
    expect(a.contextMenu).toBe(true);
    expect(a.desktopOnly).toBe(true);
  });
});
