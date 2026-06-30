import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { useAppStore } from "./store";
import { runStartupUpdateCheck } from "./util/autoUpdate";
import { revealAppWindow } from "./util/window";
import "@fontsource/geist-sans/400.css";
import "@fontsource/geist-sans/600.css";
import "./app.css";

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
void runStartupUpdateCheck((version) => {
  useAppStore.getState().setUpdateReady(version);
});
