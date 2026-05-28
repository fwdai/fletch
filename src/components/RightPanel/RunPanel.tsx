import { useEffect, useState } from "react";
import type { AgentRecord } from "../../api";
import { Icon } from "../Icon";
import {
  deleteProjectSetting,
  getProjectSettings,
  setProjectSetting,
} from "../../storage/projectSettings";
import { RunSettingsSheet, type SetupRow } from "./RunSettingsSheet";

// Settings keys we persist are prefixed so the project_settings table can
// hold overrides from other panels (env, build, etc.) without collisions.
const RUN_KEY_PREFIX = "run.";
const runKey = (id: string) => `${RUN_KEY_PREFIX}${id}`;

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

  // Load persisted overrides for this project. Re-loads when the
  // selected agent (and thus project) changes.
  useEffect(() => {
    let cancelled = false;
    if (!agent.project_id) {
      setOverrides({});
      return;
    }
    getProjectSettings(agent.project_id).then((all) => {
      if (cancelled) return;
      const loaded: Record<string, string> = {};
      for (const [k, v] of Object.entries(all)) {
        if (k.startsWith(RUN_KEY_PREFIX)) {
          loaded[k.slice(RUN_KEY_PREFIX.length)] = v;
        }
      }
      setOverrides(loaded);
    });
    return () => {
      cancelled = true;
    };
  }, [agent.project_id]);

  const valueOf = (id: string) => {
    const row = RUN_SETUP.find((r) => r.id === id);
    return overrides[id] ?? row?.value ?? "";
  };

  const devCmd = valueOf("dev");
  const port   = valueOf("port");

  const onApply = (next: Record<string, string>) => {
    // Persist only true overrides — anything that matches the inferred
    // default is treated as "no override" and removed from the DB.
    const projectId = agent.project_id;
    const cleaned: Record<string, string> = {};
    if (projectId) {
      const previous = overrides;
      for (const row of RUN_SETUP) {
        const nextVal = next[row.id];
        const wasSet = previous[row.id] !== undefined;
        const isOverride = nextVal !== undefined && nextVal !== row.value;

        if (isOverride) {
          cleaned[row.id] = nextVal;
          if (previous[row.id] !== nextVal) {
            setProjectSetting(projectId, runKey(row.id), nextVal).catch(
              (err) => console.error("setProjectSetting failed", err),
            );
          }
        } else if (wasSet) {
          deleteProjectSetting(projectId, runKey(row.id)).catch((err) =>
            console.error("deleteProjectSetting failed", err),
          );
        }
      }
    } else {
      // No project — keep in-memory only (shouldn't happen in practice).
      for (const row of RUN_SETUP) {
        const nextVal = next[row.id];
        if (nextVal !== undefined && nextVal !== row.value) {
          cleaned[row.id] = nextVal;
        }
      }
    }

    setOverrides(cleaned);
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
          <Icon name={running ? "stop" : "play"} size={12} />
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
