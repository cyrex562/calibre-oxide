import { createApp } from "vue";
import { createRouter, createWebHistory } from "vue-router";
import App from "./App.vue";
import LibraryView from "./components/LibraryView.vue";
import { applySavedSkin } from "./library/skin";
import "./style.css";
import "./skin-classic.css";

// Before the app mounts, so the first frame is already in the chosen skin.
applySavedSkin();

const router = createRouter({
  history: createWebHistory(),
  // The library is what every page load shows, so it is in the main bundle;
  // the reader (with its editors) and settings load when first visited.
  routes: [
    { path: "/", name: "library", component: LibraryView },
    { path: "/read/:bookId?/:fmt?", name: "read", component: () => import("./components/ReaderView.vue"), props: true },
    { path: "/settings", name: "settings", component: () => import("./settings/SettingsView.vue") },
  ],
});

createApp(App).use(router).mount("#app");
