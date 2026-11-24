// Panel layout -- calibre's window is three columns over a status bar:
// categories on the left, the book list in the middle, details on the right.
// Both side panels hide, and the splitters between them drag. Widths persist,
// because a panel that forgets its size every launch is one the user re-drags
// every launch.
//
// The arithmetic and the stored-blob parsing live in ./layout so they can be
// tested without mounting anything.

import { onBeforeUnmount, onMounted, ref, watch } from "vue";

import { getItemCompat } from "./legacyStorage";
import { isNarrow, LAYOUT_KEY, type LayoutPrefs, type Panel, parseLayout, resizedWidth } from "./layout";

export function usePanelLayout() {
  const initialLayout = parseLayout(readStoredLayout());
  const sidebarVisible = ref(initialLayout.sidebarVisible);
  const detailsVisible = ref(initialLayout.detailsVisible);
  const sidebarWidth = ref(initialLayout.sidebarWidth);
  const detailsWidth = ref(initialLayout.detailsWidth);

  function readStoredLayout(): string | null {
    try {
      return getItemCompat(localStorage, LAYOUT_KEY);
    } catch {
      // Storage can be unavailable (private mode, disabled cookies);
      // defaults are a fine answer and not worth failing a render over.
      return null;
    }
  }

  function saveLayout() {
    try {
      localStorage.setItem(
        LAYOUT_KEY,
        JSON.stringify({
          sidebarVisible: sidebarVisible.value,
          detailsVisible: detailsVisible.value,
          sidebarWidth: sidebarWidth.value,
          detailsWidth: detailsWidth.value,
        } satisfies LayoutPrefs),
      );
    } catch {
      // As above: the layout simply will not persist.
    }
  }

  watch([sidebarVisible, detailsVisible], saveLayout);

  // A narrow window shows one side panel at a time, over the list, and starts with
  // both closed: the saved "visible" flags describe the wide layout and are left
  // alone (opening a drawer is not a decision to keep the panel on for next time).
  const narrow = ref(typeof window !== "undefined" && isNarrow(window.innerWidth));
  const drawer = ref<Panel | null>(null);
  function onWindowResize() {
    narrow.value = isNarrow(window.innerWidth);
    if (!narrow.value) drawer.value = null;
  }
  onMounted(() => window.addEventListener("resize", onWindowResize));
  onBeforeUnmount(() => window.removeEventListener("resize", onWindowResize));

  /** Whether a panel is on screen: its saved flag when wide, the open drawer when narrow. */
  function panelShown(which: Panel): boolean {
    if (narrow.value) return drawer.value === which;
    return which === "sidebar" ? sidebarVisible.value : detailsVisible.value;
  }

  function setPanel(which: Panel, open: boolean) {
    if (narrow.value) drawer.value = open ? which : drawer.value === which ? null : drawer.value;
    else if (which === "sidebar") sidebarVisible.value = open;
    else detailsVisible.value = open;
  }

  function togglePanel(which: Panel) {
    setPanel(which, !panelShown(which));
  }

  /**
   * Drags a splitter.
   *
   * Pointer capture is what makes this reliable: without it a fast drag
   * outruns the 4px handle, the element stops receiving moves, and the
   * panel sticks mid-resize with the button still held.
   */
  function startResize(which: Panel, event: PointerEvent) {
    event.preventDefault();
    const handle = event.currentTarget as HTMLElement;
    const startX = event.clientX;
    const startWidth = which === "sidebar" ? sidebarWidth.value : detailsWidth.value;
    handle.setPointerCapture?.(event.pointerId);

    const onMove = (move: PointerEvent) => {
      const next = resizedWidth(which, startWidth, startX, move.clientX);
      if (which === "sidebar") sidebarWidth.value = next;
      else detailsWidth.value = next;
    };
    const onUp = (up: PointerEvent) => {
      handle.releasePointerCapture?.(up.pointerId);
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onUp);
      saveLayout();
    };
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
  }

  return { sidebarVisible, detailsVisible, sidebarWidth, detailsWidth, startResize, narrow, panelShown, setPanel, togglePanel };
}
