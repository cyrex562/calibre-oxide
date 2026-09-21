// Bridge between the action registry and the desktop app's native
// menu bar (issues #817 / #818).
//
// # Why the page drives the menu
//
// The obvious alternative -- build the menu in Rust from a hardcoded
// list -- means a second copy of the action registry that drifts the
// first time an action is added on the web side. Instead the page
// sends a spec describing exactly the actions it can currently handle,
// and the Rust side builds a menu from it. The page is the authority
// on its own capabilities, and the menu cannot offer something that
// would silently do nothing.
//
// # How a click gets back
//
// `web/` deliberately has no `@tauri-apps/api` dependency: it is a
// plain browser SPA that the desktop app navigates its webview to (see
// web/src/tauri.ts). Rather than reimplement Tauri's event-listener
// protocol against `__TAURI_INTERNALS__.transformCallback`, the Rust
// side evaluates a one-line script in the webview that dispatches a
// plain `CustomEvent` on `window`. Ordinary DOM, no npm dependency,
// and it works identically whether the page was served from
// `web/dist` or from a dev server.

import { isTauri, tauriInvoke } from "../tauri";
import { buildMenuSpec, type ActionContext, type LibraryActionId, type MenuActionSpec } from "./actions";

/** The `CustomEvent` name the Rust side dispatches on menu activation. */
export const MENU_ACTION_EVENT = "oxide:menu-action";

/**
 * Pushes the current menu spec to the desktop app.
 *
 * A no-op outside the desktop app. Errors are swallowed deliberately:
 * a menu that failed to rebuild is a cosmetic problem, and the toolbar
 * and context menu offer every one of these actions anyway -- it must
 * not take down the library view.
 */
export async function syncDesktopMenu(handled: Iterable<LibraryActionId>, ctx: ActionContext): Promise<void> {
  if (!isTauri()) return;
  const actions: MenuActionSpec[] = buildMenuSpec(handled, ctx);
  try {
    await tauriInvoke<void>("set_menu_actions", { actions });
  } catch (e) {
    console.warn("could not update the native menu", e);
  }
}

/**
 * Subscribes to native menu activations, returning an unsubscribe
 * function. Safe to call in a browser tab, where the event simply
 * never fires.
 */
export function onMenuAction(handler: (id: LibraryActionId) => void): () => void {
  const listener = (event: Event) => {
    const id = (event as CustomEvent<{ id?: string }>).detail?.id;
    if (id) handler(id as LibraryActionId);
  };
  window.addEventListener(MENU_ACTION_EVENT, listener);
  return () => window.removeEventListener(MENU_ACTION_EVENT, listener);
}
