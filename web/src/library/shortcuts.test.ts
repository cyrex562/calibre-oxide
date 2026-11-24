import { describe, expect, it } from "vitest";

import { formatKey, isTypingTarget, shortcutFor, shortcutHint, type ShortcutMap, isModalOpen } from "./shortcuts";
import { LIBRARY_ACTIONS } from "./actions";
import { DEFAULT_KEYMAP, libraryShortcuts } from "../settings/api";

const MAP: ShortcutMap = { "add-books": "a", "focus-search": "/", "save-to-disk": "S", "delete-book": "Delete" };

/** A keydown event aimed at `target`, defaulting to a plain div. */
function key(k: string, opts: { target?: HTMLElement; ctrl?: boolean; meta?: boolean; alt?: boolean; shift?: boolean } = {}): KeyboardEvent {
  const target = opts.target ?? document.createElement("div");
  const event = new KeyboardEvent("keydown", { key: k, ctrlKey: opts.ctrl, metaKey: opts.meta, altKey: opts.alt, shiftKey: opts.shift });
  Object.defineProperty(event, "target", { value: target });
  return event;
}

function el(tag: string, attrs: Record<string, string> = {}): HTMLElement {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, v);
  return node;
}

describe("isTypingTarget", () => {
  // The whole reason single-letter shortcuts are safe. Getting this
  // wrong means a shortcut eats the letter you were typing, which is
  // invisible until someone loses a search query to it.
  it("recognises form controls", () => {
    expect(isTypingTarget(el("input"))).toBe(true);
    expect(isTypingTarget(el("textarea"))).toBe(true);
    expect(isTypingTarget(el("select"))).toBe(true);
  });

  it("recognises contenteditable regions", () => {
    const node = el("div");
    // jsdom does not implement `isContentEditable` from the attribute.
    Object.defineProperty(node, "isContentEditable", { value: true });
    expect(isTypingTarget(node)).toBe(true);
  });

  it("recognises elements that claim a text-entry role", () => {
    expect(isTypingTarget(el("div", { role: "textbox" }))).toBe(true);
    expect(isTypingTarget(el("div", { role: "searchbox" }))).toBe(true);
    expect(isTypingTarget(el("div", { role: "combobox" }))).toBe(true);
  });

  it("leaves ordinary elements alone", () => {
    expect(isTypingTarget(el("div"))).toBe(false);
    expect(isTypingTarget(el("button"))).toBe(false);
    expect(isTypingTarget(el("tr"))).toBe(false);
  });

  it("copes with a null or non-element target", () => {
    expect(isTypingTarget(null)).toBe(false);
    expect(isTypingTarget(window)).toBe(false);
  });
});

describe("shortcutFor", () => {
  it("matches a bound key", () => {
    expect(shortcutFor(key("a"), MAP)).toBe("add-books");
    expect(shortcutFor(key("/"), MAP)).toBe("focus-search");
    expect(shortcutFor(key("Delete"), MAP)).toBe("delete-book");
  });

  it("returns null for an unbound key", () => {
    expect(shortcutFor(key("q"), MAP)).toBeNull();
  });

  it("never fires while the user is typing", () => {
    expect(shortcutFor(key("a", { target: el("input") }), MAP)).toBeNull();
    expect(shortcutFor(key("/", { target: el("textarea") }), MAP)).toBeNull();
  });

  // Binding a bare "a" must not also hijack Ctrl+A / Cmd+A.
  it("ignores modified key presses", () => {
    expect(shortcutFor(key("a", { ctrl: true }), MAP)).toBeNull();
    expect(shortcutFor(key("a", { meta: true }), MAP)).toBeNull();
    expect(shortcutFor(key("a", { alt: true }), MAP)).toBeNull();
  });

  // `KeyboardEvent.key` reports a shifted letter in upper case, which
  // is the whole mechanism keeping "s" and "S" distinct -- so Shift is
  // deliberately not treated as a disqualifying modifier.
  it("treats a shifted letter as its own binding", () => {
    expect(shortcutFor(key("S", { shift: true }), MAP)).toBe("save-to-disk");
    expect(shortcutFor(key("s", {}), MAP)).toBeNull();
  });

  it("ignores an empty binding rather than matching an empty key", () => {
    expect(shortcutFor(key(""), { "add-books": "" })).toBeNull();
  });
});

describe("the default keymap", () => {
  it("binds only single unmodified keys, which the typing guard makes safe", () => {
    for (const [action, binding] of Object.entries(libraryShortcuts(DEFAULT_KEYMAP))) {
      expect(binding, `${action} has no binding`).toBeTruthy();
      expect(binding!.includes("+"), `${action} looks like a chord: ${binding}`).toBe(false);
    }
  });

  it("gives every library action a distinct key", () => {
    // Two actions on one key means one of them is unreachable, and
    // which one wins depends on object key order.
    const bindings = Object.values(libraryShortcuts(DEFAULT_KEYMAP));
    expect(new Set(bindings).size).toBe(bindings.length);
  });

  it("does not collide with the reader bindings", () => {
    const library = new Set(Object.values(libraryShortcuts(DEFAULT_KEYMAP)));
    expect(library.has(DEFAULT_KEYMAP.readerNext)).toBe(false);
    expect(library.has(DEFAULT_KEYMAP.readerPrev)).toBe(false);
  });

  it("resolves each default binding back to its own action", () => {
    const map = libraryShortcuts(DEFAULT_KEYMAP);
    for (const [action, binding] of Object.entries(map)) {
      expect(shortcutFor(key(binding!), map)).toBe(action);
    }
  });
});

describe("isModalOpen", () => {
  it("sees any open aria-modal dialog, whichever dialog it is", () => {
    const host = document.createElement("div");
    expect(isModalOpen(host)).toBe(false);
    host.innerHTML = '<div role="dialog" aria-label="Trash" aria-modal="true"></div>';
    expect(isModalOpen(host)).toBe(true);
    host.innerHTML = '<div role="dialog" aria-modal="false"></div><aside></aside>';
    expect(isModalOpen(host)).toBe(false);
  });
});

describe("Enter and Space", () => {
  const MAP_ENTER: ShortcutMap = { read: "Enter", "select-mode": " " };

  it("opens the book from a table row or from nothing in particular", () => {
    expect(shortcutFor(key("Enter", { target: el("tr", { role: "row" }) }), MAP_ENTER)).toBe("read");
    expect(shortcutFor(key("Enter"), MAP_ENTER)).toBe("read");
  });

  // Enter on a focused toolbar button presses the button; a binding on Enter must not
  // turn every toolbar click-by-keyboard into "read the open book".
  it("leaves Enter and Space to a focused button, link or menu item", () => {
    for (const target of [el("button"), el("a", { href: "#" }), el("div", { role: "menuitem" }), el("div", { role: "button" })]) {
      expect(shortcutFor(key("Enter", { target }), MAP_ENTER)).toBeNull();
      expect(shortcutFor(key(" ", { target }), MAP_ENTER)).toBeNull();
    }
  });

  it("still leaves Enter to a text box (an open cell editor, the search box)", () => {
    expect(shortcutFor(key("Enter", { target: el("input") }), MAP_ENTER)).toBeNull();
  });

  it("only guards Enter and Space: a letter bound while a button has focus still fires", () => {
    expect(shortcutFor(key("a", { target: el("button") }), MAP)).toBe("add-books");
  });
});

describe("shortcut hints", () => {
  it("writes a key the way a keycap shows it", () => {
    expect(formatKey("d")).toBe("d");
    expect(formatKey("ArrowRight")).toBe("→");
    expect(formatKey("S")).toBe("Shift+S");
    expect(formatKey("Delete")).toBe("Del");
    expect(formatKey(" ")).toBe("Space");
    expect(formatKey("Enter")).toBe("Enter");
    expect(formatKey("/")).toBe("/");
    expect(formatKey("")).toBe("");
  });

  // The tooltips used to carry text written beside each action: "Edit metadata (E)" for a key that
  // opened Bulk edit, "Add books (Ctrl+A)" for a chord nothing handled.
  it("is read from the keymap in force, so a rebinding shows and an unbound action shows nothing", () => {
    const map = libraryShortcuts(DEFAULT_KEYMAP);
    expect(shortcutHint("find-duplicates", map)).toBe("d");
    expect(shortcutHint("edit-metadata", map)).toBe("e");
    expect(shortcutHint("bulk-edit", map)).toBe("Shift+E");
    expect(shortcutHint("read", map)).toBe("Enter");
    expect(shortcutHint("find-duplicates", { ...map, "find-duplicates": "g" })).toBe("g");
    expect(shortcutHint("organize-files", map)).toBeNull();
  });

  it("agrees with what pressing the key does, for every default binding", () => {
    const map = libraryShortcuts(DEFAULT_KEYMAP);
    for (const [id, binding] of Object.entries(map)) {
      expect(shortcutFor(key(binding!), map), `${id}: its hint says ${shortcutHint(id as never, map)}`).toBe(id);
    }
  });

  it("is not written beside the actions any more: no Ctrl chord appears in the registry", () => {
    for (const a of LIBRARY_ACTIONS) expect((a as { accel?: string }).accel, a.id).toBeUndefined();
  });

  it("binds Enter to Read and e to Edit metadata", () => {
    expect(DEFAULT_KEYMAP.read).toBe("Enter");
    expect(DEFAULT_KEYMAP["edit-metadata"]).toBe("e");
  });
});
