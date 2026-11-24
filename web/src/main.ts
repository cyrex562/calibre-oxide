import { createApp } from "vue";
import { createRouter, createWebHistory } from "vue-router";
import App from "./App.vue";
import ReaderView from "./components/ReaderView.vue";
import LibraryView from "./components/LibraryView.vue";
import SettingsView from "./settings/SettingsView.vue";
import { applySavedSkin } from "./library/skin";
import "./style.css";
import "./skin-classic.css";

// Before the app mounts, so the first frame is already in the chosen skin.
applySavedSkin();

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: "/", name: "library", component: LibraryView },
    { path: "/read/:bookId?/:fmt?", name: "read", component: ReaderView, props: true },
    { path: "/settings", name: "settings", component: SettingsView },
  ],
});

createApp(App).use(router).mount("#app");
