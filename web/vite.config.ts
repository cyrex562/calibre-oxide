import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// https://vite.dev/config/
//
// Served as lexmechanic_srv's own browser UI (issue #432/#498) -- a
// separate SPA from `app/` (the Tauri desktop shell), matching
// upstream calibre's own separation between its desktop GUI and its
// content-server browser UI. See #432's issue body for the full
// investigation behind that split.
const PROXIED = ["/book-manifest", "/book-file", "/book-get-last-read-position", "/book-set-last-read-position", "/book-get-annotations", "/book-update-annotations", "/ajax", "/get"];

export default defineConfig({
  plugins: [vue()],
  test: {
    environment: "jsdom",
    // style.test.ts reads the colour tokens out of style.css; every other stylesheet stays unprocessed.
    css: { include: [/[\\/]style\.css/] },
  },
  server: {
    port: 5180,
    // During `npm run dev`, proxy API calls to a locally running
    // lexmechanic_srv (see crates/lexmechanic_srv's own README for how to
    // start it against a test library) so the dev server doesn't
    // need lexmechanic_srv to also serve the frontend. The browser's
    // `Origin` (the dev server's port) is dropped: lexmechanic_srv refuses
    // state-changing requests from a foreign origin (guard.rs).
    proxy: PROXIED.reduce<Record<string, object>>((acc, path) => {
      acc[path] = {
        target: "http://127.0.0.1:8080",
        configure: (proxy: { on: (ev: string, cb: (req: { removeHeader: (h: string) => void }) => void) => void }) => {
          proxy.on("proxyReq", (req) => req.removeHeader("origin"));
        },
      };
      return acc;
    }, {}),
  },
});
