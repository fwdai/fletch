import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { whenIdle } from "./remote/defer";
import { startSavedHosts } from "./remote/hosts";
import { useAppStore } from "./store";
import { setupAppMenu } from "./util/appMenu";
import { runStartupUpdateCheck } from "./util/autoUpdate";
import { revealAppWindow } from "./util/window";
import "@fontsource/geist-sans/400.css";
import "@fontsource/geist-sans/600.css";
import "./app.css";
import "./workflows/workflows.css";

const root = document.getElementById("root");
if (!root) throw new Error("#root element missing");
ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

// Reveal the (initially hidden) window after first paint, so the white webview
// flash never shows. See `revealAppWindow`.
revealAppWindow();

// Check for and download updates on launch (no-op in dev). Fire-and-forget so
// it never delays first paint; once an update is staged it surfaces a toast
// (via the store) letting the user restart now or skip for now.
void runStartupUpdateCheck((version, notes) => {
  useAppStore.getState().setUpdateReady(version, notes);
});

// Install the native menu (adds "Check for Updates…"). Fire-and-forget; never
// blocks launch.
void setupAppMenu();

// Paired hosts are dialled once the browser has nothing better to do, never
// before the first paint: they are a second engine this window happens to know
// about, and the local one — which is what every pixel on screen is about — must
// not wait behind a socket to a machine that may be asleep. With no saved hosts
// this is one keyed settings read and then nothing.
whenIdle(() => void startSavedHosts());
