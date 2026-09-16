// Real, dependency-free bridge to the desktop app's Tauri commands.
//
// web/ is deliberately not a Tauri-aware package (no @tauri-apps/api
// dependency, see package.json) -- it's a plain browser SPA served
// directly by calibre_srv, reused as-is by the desktop app, which
// just navigates its own webview to this same served content
// (app/src-tauri/src/server.rs). Tauri still injects a real
// `window.__TAURI_INTERNALS__` global into any page loaded in that
// webview regardless of whether the page's own bundle imported the JS
// API package -- confirmed against the actual installed
// @tauri-apps/api's own core.js (`invoke` is just
// `window.__TAURI_INTERNALS__.invoke(cmd, args, options)`). Calling
// that global directly here gets real desktop-only functionality
// (e.g. folder import) without adding a Tauri dependency to a package
// that also needs to work as a plain browser tab.

interface TauriInternals {
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
}

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const internals = (window as unknown as { __TAURI_INTERNALS__?: TauriInternals }).__TAURI_INTERNALS__;
  if (!internals) throw new Error("not running inside the desktop app");
  return internals.invoke<T>(cmd, args);
}
