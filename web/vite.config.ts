import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// https://vite.dev/config/
//
// Served as calibre_srv's own browser UI (issue #432/#498) -- a
// separate SPA from `app/` (the Tauri desktop shell), matching
// upstream calibre's own separation between its desktop GUI and its
// content-server browser UI. See #432's issue body for the full
// investigation behind that split.
export default defineConfig({
  plugins: [vue()],
  test: {
    environment: "jsdom",
  },
  server: {
    port: 5180,
    proxy: {
      // During `npm run dev`, proxy API calls to a locally running
      // calibre_srv (see crates/calibre_srv's own README for how to
      // start it against a test library) so the dev server doesn't
      // need calibre_srv to also serve the frontend.
      "/book-manifest": "http://127.0.0.1:8080",
      "/book-file": "http://127.0.0.1:8080",
      "/book-get-last-read-position": "http://127.0.0.1:8080",
      "/book-set-last-read-position": "http://127.0.0.1:8080",
      "/book-get-annotations": "http://127.0.0.1:8080",
      "/book-update-annotations": "http://127.0.0.1:8080",
      "/ajax": "http://127.0.0.1:8080",
      "/get": "http://127.0.0.1:8080",
    },
  },
});
