import { createApp } from "vue";
import App from "./App.vue";
// The design tokens are shared with the served UI rather than
// duplicated: the splash is the first screen a user sees, and it had
// its own hardcoded palette with no dark mode at all -- so on a
// dark-mode desktop it flashed a light card before handing over to a
// dark window.
import "../../web/src/style.css";
import "./style.css";

createApp(App).mount("#app");
