import { createApp } from "vue";
import { createRouter, createWebHistory } from "vue-router";
import App from "./App.vue";
import ReaderView from "./components/ReaderView.vue";
import "./style.css";

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: "/", redirect: "/read" },
    { path: "/read/:bookId?/:fmt?", name: "read", component: ReaderView, props: true },
  ],
});

createApp(App).use(router).mount("#app");
