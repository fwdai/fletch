import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { runStartupUpdateCheck } from "./util/autoUpdate";
import "./app.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

// Check for and silently apply updates on launch (no-op in dev). Fire-and-forget
// so it never delays first paint; it relaunches the app if an update installs.
void runStartupUpdateCheck();
