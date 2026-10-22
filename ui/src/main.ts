import { mount } from "svelte";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./app.css";
import App from "./App.svelte";
import Osd from "./Osd.svelte";
import Settings from "./Settings.svelte";

const label = getCurrentWindow().label;
document.body.classList.add(label);

// No browser context menu (Back / Reload / Save as / Inspect): this is an app, not a web page.
document.addEventListener("contextmenu", (e) => e.preventDefault(), true);

mount(label === "settings" ? Settings : label === "osd" ? Osd : App, { target: document.getElementById("app")! });
