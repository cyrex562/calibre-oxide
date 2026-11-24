// Keyboard behaviour of the narrow layout's drawers (C25).
//
// Below the narrow breakpoint the categories and the details open as a drawer over the list.
// A drawer is then modal in feel, so: focus moves into it when it opens, Escape closes it, and
// focus goes back to what opened it (the status bar's toggle). The drawer element carries
// `data-drawer="sidebar" | "details"` and `tabindex="-1"`.

import { nextTick, onBeforeUnmount, onMounted, watch, type Ref } from "vue";

import { isModalOpen } from "./shortcuts";
import type { Panel } from "./layout";

/** Whether an Escape press belongs to the drawer: nothing else (a dialog, a menu) is using it. */
export function escapeClosesDrawer(event: KeyboardEvent, root: ParentNode = document): boolean {
  if (event.key !== "Escape" || event.defaultPrevented || event.isComposing) return false;
  if (isModalOpen(root)) return false;
  return root.querySelector('[role="menu"]') === null;
}

export function useDrawerFocus(drawer: Ref<Panel | null>, close: () => void) {
  let opener: Element | null = null;

  watch(drawer, async (now, before) => {
    if (now && !before) opener = document.activeElement;
    if (now) {
      await nextTick();
      document.querySelector<HTMLElement>(`[data-drawer="${now}"]`)?.focus({ preventScroll: true });
    } else if (before) {
      const back = opener;
      opener = null;
      if (back instanceof HTMLElement && back.isConnected) back.focus({ preventScroll: true });
    }
  });

  function onKeydown(event: KeyboardEvent) {
    if (drawer.value === null || !escapeClosesDrawer(event)) return;
    event.preventDefault();
    close();
  }
  onMounted(() => window.addEventListener("keydown", onKeydown));
  onBeforeUnmount(() => window.removeEventListener("keydown", onKeydown));
}
