import { mount } from "svelte";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./app.css";
import App from "./App.svelte";
import Osd from "./Osd.svelte";
import Settings from "./Settings.svelte";

const label = getCurrentWindow().label;
document.body.classList.add(label);

mount(label === "settings" ? Settings : label === "osd" ? Osd : App, { target: document.getElementById("app")! });
