import { useState } from "react";
import type { AgentRecord } from "../../api";
import { Icon } from "../Icon";
import { RunSettingsSheet, type SetupRow } from "./RunSettingsSheet";

// ── Static mock data (UI pass — replaced by real data in wiring PR) ──────────
// Rows match the Quorum v2 prototype exactly: 3 groups, 8 rows.
// Each entry carries an inferred value + WHERE it was detected from, so the
// run-settings sheet can surface that as the "auto-detected" baseline and the
// user can override any one of them.

const RUN_SETUP: SetupRow[] = [
  { id: "pm",      group: "Environment", key: "Package manager", value: "pnpm 9.7.1",   source: "package.json · packageManager" },
  { id: "node",    group: "Environment", key: "Node version",    value: "v22.4.0",      source: ".nvmrc" },
  { id: "install", group: "Scripts",     key: "Install",         value: "pnpm install", source: "convention (pm + install)" },
  { id: "dev",     group: "Scripts",     key: "Dev",             value: "pnpm dev",     source: "package.json · scripts.dev" },
  { id: "build",   group: "Scripts",     key: "Build",           value: "pnpm build",   source: "package.json · scripts.build" },
  { id: "test",    group: "Scripts",     key: "Test",            value: "pnpm test",    source: "package.json · scripts.test" },
  { id: "port",    group: "Server",      key: "Port",            value: "3000",         source: "next.config.js" },
  { id: "env",     group: "Server",      key: "Env file",        value: ".env.local",   source: "auto-detected" },
];

interface LogEntry { c: string; t: string; }

const RUN_LOGS: LogEntry[] = [
  { c: "term-dim",     t: "$ pnpm dev" },
  { c: "",             t: "" },
  { c: "term-bold",    t: "  ▲ Next.js 15.4.0" },
  { c: "term-dim",     t: "  - Local:        http://localhost:3000" },
  { c: "term-dim",     t: "  - Workspace:    ~/dev/atlas-web/.quorum/patagonia" },
  { c: "term-dim",     t: "  - Environments: .env.local" },
  { c: "",             t: "" },
  { c: "term-success", t: " ✓ Ready in 1.4s" },
  { c: "term-dim",     t: " ○ Compiling /billing ..." },
  { c: "term-success", t: " ✓ Compiled /billing in 482ms (1290 modules)" },
  { c: "term-dim",     t: " GET /billing 200 in 612ms" },
  { c: "term-dim",     t: " GET /api/billing/customer 200 in 84ms" },
  { c: "term-warn",    t: " ⚠ Fast Refresh had to perform a full reload due to a runtime error" },
  { c: "term-success", t: " ✓ Compiled /api/billing/portal in 211ms" },
  { c: "term-dim",     t: " POST /api/billing/checkout 303 in 142ms" },
  { c: "term-info",    t: "   → redirect: https://billing.stripe.com/session/..." },
];

const LOG_CLASS: Record<string, string> = {
  "term-prompt":  "p",
  "term-dim":     "d",
  "term-success": "s",
  "term-warn":    "w",
  "term-error":   "e",
  "term-info":    "i",
  "term-bold":    "b",
};

// ── Component ────────────────────────────────────────────────────────────────

export function RunPanel({ agent }: { agent: AgentRecord }) {
  const [running, setRunning] = useState(true);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [overrides, setOverrides] = useState<Record<string, string>>({});

  const valueOf = (id: string) => {
    const row = RUN_SETUP.find((r) => r.id === id);
    return overrides[id] ?? row?.value ?? "";
  };

  const devCmd = valueOf("dev");
  const port   = valueOf("port");

  const onApply = (next: Record<string, string>) => {
    setOverrides(next);
    setSettingsOpen(false);
    setRunning(true);
  };

  const hasOverrides = Object.keys(overrides).length > 0;

  return (
    <div className="run-wrap">
      {/* ── Bar ── */}
      <div className="run-bar v2">
        <button
          className={`run-go ${running ? "live" : "stopped"}`}
          onClick={() => setRunning((v) => !v)}
          aria-label={running ? "Stop" : "Start"}
        >
          <Icon name={running ? "stop" : "play"} size={11} />
        </button>

        <div className="run-cmd">
          <span className="p">$</span>
          <span className="cmd-text">{devCmd}</span>
        </div>

        <a
          href={`http://localhost:${port}`}
          target="_blank"
          rel="noreferrer"
          className={`run-link${running ? "" : " disabled"}`}
          onClick={(e) => { if (!running) e.preventDefault(); }}
        >
          <span className="colon">:</span>
          <span className="port">{port}</span>
          <Icon name="external" size={10} />
        </a>

        <button
          className={`run-gear${settingsOpen ? " active" : ""}${hasOverrides ? " has-overrides" : ""}`}
          aria-label="Run configuration"
          onClick={() => setSettingsOpen((v) => !v)}
        >
          <Icon name="settings" size={13} />
          {hasOverrides && <span className="dot" />}
        </button>
      </div>

      {/* ── Logs ── */}
      <div className="run-logs">
        {RUN_LOGS.map((l, i) => {
          const cls = LOG_CLASS[l.c] ?? "";
          const text = l.t.split('localhost:3000').join(`localhost:${port}`);
          return <div key={i} className={cls || undefined}>{text}</div>;
        })}
        {running && (
          <div className="p">
            {"› "}
            <span className="term-cursor" />
          </div>
        )}
      </div>

      {/* ── Settings sheet ── */}
      {settingsOpen && (
        <RunSettingsSheet
          rows={RUN_SETUP}
          overrides={overrides}
          agent={agent}
          onClose={() => setSettingsOpen(false)}
          onApply={onApply}
        />
      )}
    </div>
  );
}
