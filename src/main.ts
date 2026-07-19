import { mount } from "svelte";

import App from "./app/App.svelte";
import "./app.css";

const target = document.getElementById("app");

if (!target) {
  throw new Error("Explora could not find its application mount point.");
}

const app = mount(App, { target });

export default app;
