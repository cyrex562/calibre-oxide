// Keyboard shortcut matching (issue 1.3 of the #816 epic).
//
// The third surface over the action registry, after the toolbar
// (#817) and the context menu (#1.2). Before this the keymap held
// exactly two bindings, both reader-only, and `LibraryView.vue` had no
// `keydown` handler at all -- so the shortcuts settings panel
// configured almost nothing.
//
// # Why this is its own module
//
// The interesting part of a shortcut system is not dispatch, it is
// deciding when *not* to fire. A single-letter binding that steals the
// "a" you were typing into the search box is worse than no shortcut at
// all, and that logic is invisible in a component -- it only shows up
// when someone loses a search query to it. Keeping it pure means the
// rules can be stated and tested directly.

import type { LibraryActionId } from "./actions";

/** Bindings, keyed by action id, holding a `KeyboardEvent.key` value. */
export type ShortcutMap = Partial<Record<LibraryActionId, string>>;

/**
 * Whether the event landed somewhere the user is composing text, and
 * so must be left alone.
 *
 * Covers the three real cases: form controls, `contenteditable`
 * regions (the comments editor), and anything that has opted into
 * handling its own keys via a `textbox`-ish ARIA role.
 */
export function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;

  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  if (target.isContentEditable) return true;

  const role = target.getAttribute("role");
  return role === "textbox" || role === "searchbox" || role === "combobox";
}

/**
 * The action a key press should trigger, or `null` for "leave it
 * alone".
 *
 * Refuses in three situations:
 *
 * 1. The user is typing (see `isTypingTarget`).
 * 2. A modifier is held. `Ctrl`/`Cmd`/`Alt` combinations belong to the
 *    browser and the OS -- binding a bare letter must not also hijack
 *    `Ctrl+A`. `Shift` is *not* treated as a modifier here, because
 *    `KeyboardEvent.key` already reports a shifted letter as its
 *    upper-case form, which is how "S" can mean something different
 *    from "s".
 * 3. Nothing is bound to the key.
 */
export function shortcutFor(event: KeyboardEvent, keymap: ShortcutMap): LibraryActionId | null {
  if (event.ctrlKey || event.metaKey || event.altKey) return null;
  if (isTypingTarget(event.target)) return null;
  // Enter and Space already mean "press" on a focused button or link: a binding
  // on them must not turn pressing a toolbar button into something else.
  if ((event.key === "Enter" || event.key === " ") && isActivatable(event.target)) return null;

  for (const [id, key] of Object.entries(keymap)) {
    if (key && key === event.key) return id as LibraryActionId;
  }
  return null;
}

/** Whether the element is something Enter or Space presses (a button, a link, a menu item). */
export function isActivatable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (tag === "BUTTON" || tag === "A" || tag === "SUMMARY") return true;
  const role = target.getAttribute("role");
  return role === "button" || role === "link" || role === "menuitem" || role === "menuitemcheckbox" || role === "option" || role === "tab" || role === "switch";
}

/**
 * A key as it is written on a keycap or in a tooltip: "D" for "d", "Shift+S"
 * for "S", "Del", "Space". The binding is the real `KeyboardEvent.key`; this
 * is only how it is shown.
 */
export function formatKey(key: string): string {
  if (!key) return "";
  if (key === " ") return "Space";
  if (key === "Delete") return "Del";
  if (key.length === 1 && key >= "a" && key <= "z") return key.toUpperCase();
  if (key.length === 1 && key >= "A" && key <= "Z") return `Shift+${key}`;
  return key;
}

/**
 * The shortcut hint for an action, from the keymap that is actually in
 * force (the defaults plus the user's rebinding), or `null` when the action
 * has no key. Tooltips and menus show this instead of text written by hand,
 * which went stale the first time a binding changed.
 */
export function shortcutHint(id: LibraryActionId, keymap: ShortcutMap): string | null {
  const key = keymap[id];
  return key ? formatKey(key) : null;
}

/**
 * Whether a modal dialog is open anywhere in the document. Dialogs mark
 * themselves `aria-modal="true"`, so the library shortcuts need not know
 * each dialog's open flag (a list that kept missing one, so Del with the
 * Trash open deleted the library book behind it).
 */
export function isModalOpen(root: ParentNode = document): boolean {
  return root.querySelector('[aria-modal="true"]') !== null;
}
