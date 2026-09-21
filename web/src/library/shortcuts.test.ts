import { describe, expect, it } from "vitest";

import { isTypingTarget, shortcutFor, type ShortcutMap } from "./shortcuts";
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
