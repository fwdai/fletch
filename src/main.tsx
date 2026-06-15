import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { runStartupUpdateCheck } from "./util/autoUpdate";
import { useAppStore } from "./store";
import "./app.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

// Check for and download updates on launch (no-op in dev). Fire-and-forget so
// it never delays first paint; once an update is staged it surfaces a toast
// (via the store) letting the user restart now or skip for now.
void runStartupUpdateCheck((version) => {
  useAppStore.getState().setUpdateReady(version);
});
