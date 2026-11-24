// The open dialogs, in the order they were opened. Only the last one (the
// topmost) handles Escape and Tab, so a confirm opened over a dialog closes
// on its own Escape and leaves the dialog beneath it open.

import { reactive } from "vue";

const stack = reactive<symbol[]>([]);

/** Register a dialog as open; returns its id for the other calls. */
export function pushDialog(): symbol {
  const id = Symbol("dialog");
  stack.push(id);
  return id;
}

export function removeDialog(id: symbol): void {
  const at = stack.indexOf(id);
  if (at >= 0) stack.splice(at, 1);
}

export function isTopDialog(id: symbol): boolean {
  return stack.length > 0 && stack[stack.length - 1] === id;
}

export function dialogCount(): number {
  return stack.length;
}

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), summary, [contenteditable="true"], [tabindex]:not([tabindex="-1"])';

/** The elements Tab can reach inside `root`, in document order. */
export function focusableIn(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => !el.hidden && el.closest("[hidden], [inert]") === null && el.tabIndex !== -1);
}

/**
 * Move Tab / Shift+Tab so focus stays inside `panel`. Returns true when it
 * took over the key (the caller then calls preventDefault).
 */
export function trapTab(panel: HTMLElement, event: { shiftKey: boolean }, active: Element | null = document.activeElement): boolean {
  const items = focusableIn(panel);
  if (items.length === 0) {
    panel.focus();
    return true;
  }
  const first = items[0];
  const last = items[items.length - 1];
  const inside = active !== null && panel.contains(active);
  if (!inside || (event.shiftKey && (active === first || active === panel))) {
    (event.shiftKey ? last : first).focus();
    return true;
  }
  if (!event.shiftKey && active === last) {
    first.focus();
    return true;
  }
  return false;
}

/** Where focus goes when a dialog opens: the [autofocus] element, the first focusable one, else the panel. */
export function initialFocusTarget(panel: HTMLElement): HTMLElement {
  const items = focusableIn(panel);
  // The title bar's close button is not where anyone wants to start typing.
  return panel.querySelector<HTMLElement>("[autofocus]") ?? items.find((el) => el.closest(".dialog-titlebar") === null) ?? panel;
}
