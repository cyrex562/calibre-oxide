// The library view's way of being driven from outside the page: keyboard
// shortcuts (#1.3), clicks in the desktop app's native menu, books dropped on the
// window, and the "unsaved drafts" guard on closing it. All of them are window
// listeners, so they live only while the view is shown.
//
// Keyboard shortcuts. The keymap held two bindings, both reader-only, before
// this: the shortcuts settings panel configured almost nothing. The decision of
// whether a key press should fire lives in ./shortcuts.ts, because the part worth
// getting right is when *not* to act: a bare "a" must not steal the letter you
// are typing into the search box.
//
// <keep-alive> keeps the view (query, scroll, selection, details panel) when the
// route goes to the reader or settings; it is only parked, not unmounted. While
// parked it must not react to Del or menu actions, and when it is shown again it
// reloads if something was written in the meantime (see ./libraryEpoch.ts).

import { onActivated, onBeforeUnmount, onDeactivated, onMounted, ref } from "vue";

import { DEFAULT_KEYMAP, fetchProfile, KEYMAP_PROFILE, libraryShortcuts, type KeymapPrefs } from "../settings/api";
import type { LibraryActionId } from "./actions";
import { onMenuAction } from "./desktopMenu";
import { libraryEpoch, needsRefresh } from "./libraryEpoch";
import { draftCount } from "./metadataForm";
import { isModalOpen, shortcutFor } from "./shortcuts";

export interface LibraryKeyboardDeps {
  /** Runs a registry action: from a shortcut or the native menu. */
  runAction(id: LibraryActionId): void;
  deleteSelectedBooks(): void;
  /** Focuses (and selects) the search box. */
  focusSearch(): void;
  toggleView(): void;
  /** A context menu is open and owns the keyboard. */
  contextMenuOpen(): boolean;
  /** Books were dropped on the window (the desktop shell says so). */
  onBooksDropped(event: Event): void;
  /** Shown again after being parked: re-send the native menu, and reload the list if it went stale. */
  resumed(stale: boolean): void;
}

export function useLibraryKeyboard(deps: LibraryKeyboardDeps) {
  const keymap = ref<KeymapPrefs>({ ...DEFAULT_KEYMAP });

  async function loadKeymap() {
    try {
      const prefs = await fetchProfile<KeymapPrefs>(KEYMAP_PROFILE);
      if (prefs) keymap.value = { ...DEFAULT_KEYMAP, ...prefs };
    } catch (e) {
      console.error("failed to load keyboard shortcuts", e);
    }
  }
  void loadKeymap();

  /** False while the route is elsewhere and the view is parked by <keep-alive>. */
  let viewActive = true;

  function onLibraryKeydown(event: KeyboardEvent) {
    // A modal owns the keyboard while it is open -- firing library
    // shortcuts underneath one would act on a view the user cannot
    // currently see.
    // Asked of the document, not a list of open flags: every dialog marks
    // itself aria-modal, so a new one cannot be forgotten here.
    if (deps.contextMenuOpen() || isModalOpen()) return;

    const id = shortcutFor(event, libraryShortcuts(keymap.value));
    if (!id) return;

    event.preventDefault();
    if (id === "focus-search") deps.focusSearch();
    else if (id === "toggle-view") deps.toggleView();
    else if (id === "delete-book") deps.deleteSelectedBooks();
    else deps.runAction(id);
  }

  /**
   * Unsaved metadata drafts live in a module-level store, so collapsing the
   * panel or opening the reader loses nothing; closing or reloading the window
   * does, so that asks first.
   */
  function onBeforeUnload(event: BeforeUnloadEvent) {
    if (draftCount.value === 0) return;
    event.preventDefault();
    event.returnValue = "";
  }

  let unsubscribeMenuAction: (() => void) | null = null;

  /**
   * The window listeners that act on the library: shortcuts, native-menu
   * clicks and dropped books. Live only while the view is shown. (The
   * beforeunload guard for unsaved drafts stays for as long as the view exists,
   * because closing the window must still ask.)
   */
  function attachLiveListeners() {
    if (unsubscribeMenuAction !== null) return;
    window.addEventListener("keydown", onLibraryKeydown);
    window.addEventListener("lexmechanic:books-added", deps.onBooksDropped);
    unsubscribeMenuAction = onMenuAction(deps.runAction);
  }

  function detachLiveListeners() {
    window.removeEventListener("keydown", onLibraryKeydown);
    window.removeEventListener("lexmechanic:books-added", deps.onBooksDropped);
    unsubscribeMenuAction?.();
    unsubscribeMenuAction = null;
  }

  let leftAt: { epoch: number; at: number } | null = null;

  // Registered in onMounted and removed in onBeforeUnmount: a listener added at
  // setup time would pile up one more copy on every remount.
  onMounted(() => {
    window.addEventListener("beforeunload", onBeforeUnload);
    attachLiveListeners();
  });
  onDeactivated(() => {
    viewActive = false;
    detachLiveListeners();
    leftAt = { epoch: libraryEpoch(), at: Date.now() };
  });
  onActivated(() => {
    viewActive = true;
    attachLiveListeners();
    const left = leftAt;
    leftAt = null;
    if (left === null) return; // the first activation, right after mounting
    deps.resumed(needsRefresh(left.epoch, left.at));
  });
  onBeforeUnmount(() => {
    window.removeEventListener("beforeunload", onBeforeUnload);
    detachLiveListeners();
  });

  return { keymap, isViewActive: () => viewActive };
}
