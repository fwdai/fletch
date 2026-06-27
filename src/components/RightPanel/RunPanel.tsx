import { useEffect, useRef, useState } from "react";
import { type AgentRecord, api, onRunOutput, onRunState, type RunPhase } from "../../api";
import {
  deleteProjectSetting,
  getProjectSettings,
  setProjectSetting,
} from "../../storage/projectSettings";
import { Icon } from "../Icon";
import { RunSettingsSheet, type SetupRow } from "./RunSettingsSheet";
import { reconcileOverrides } from "./reconcileOverrides";

// Settings keys are namespaced under `run.` so the project_settings
// table can hold overrides from other panels without colliding.
const RUN_KEY_PREFIX = "run.";
const runKey = (id: string) => `${RUN_KEY_PREFIX}${id}`;

// Detected run config replaces the old hardcoded defaults. The backend
// (`detect_run_config`) returns rows per ecosystem; the panel shows the
// highest-confidence one. The `run.*` overrides in project_settings layer
// on top of these detected values.
const GROUP_LABEL: Record<string, string> = {
  environment: "Environment",
  scripts: "Scripts",
  server: "Server",
};

// Strip ANSI escape sequences before rendering. v1 keeps log rendering
// dead-simple (plain text with pre-wrap); colorization can come later.
// Covers CSI (ESC [ ... letter) and OSC (ESC ] ... BEL / ST).
const ANSI_RE = /\x1b\[[0-9;?]*[a-zA-Z]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g;
const stripAnsi = (s: string) => s.replace(ANSI_RE, "");

export function RunPanel({ agent }: { agent: AgentRecord }) {
  const [phase, setPhase] = useState<RunPhase>("idle");
  const [lastError, setLastError] = useState<string | null>(null);
  const [log, setLog] = useState<string>("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [overrides, setOverrides] = useState<Record<string, string>>({});
  const [rows, setRows] = useState<SetupRow[]>([]);
  const [ecosystem, setEcosystem] = useState<string | null>(null);
  const logRef = useRef<HTMLDivElement | null>(null);
  // Streaming UTF-8 decoder so a multi-byte rune split across two
  // PTY chunks doesn't produce a replacement character. Reset each
  // time we re-subscribe (agent switch).
  const decoderRef = useRef<TextDecoder | null>(null);

  // Load persisted command overrides for this project. Re-loads when
  // the selected agent (and thus project) changes.
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

  // Detect the run config for this agent's worktree. Re-runs on agent
  // switch. The highest-confidence ecosystem fills the table; an empty
  // result means nothing was recognized (no-op fallback).
  useEffect(() => {
    let cancelled = false;
    api
      .detectRunConfig(agent.id)
      .then((configs) => {
        if (cancelled) return;
        const primary = configs[0];
        setEcosystem(primary?.ecosystem ?? null);
        setRows(
          (primary?.rows ?? []).map((r) => ({
            id: r.id,
            group: GROUP_LABEL[r.group] ?? r.group,
            key: r.key,
            value: r.value,
            source: r.source,
          })),
        );
      })
      .catch((err) => {
        if (cancelled) return;
        console.error("detectRunConfig failed", err);
        setRows([]);
        setEcosystem(null);
      });
    return () => {
      cancelled = true;
    };
  }, [agent.id]);

  // Subscribe to run output and state events for this agent.
  // Rehydrate snapshot on mount/agent-switch so the panel preserves
  // logs from prior starts (and across panel mounts).
  useEffect(() => {
    // `onRunOutput` / `onRunState` return promises that resolve to
    // the unlisten fn. StrictMode runs effects twice in dev, and the
    // cleanup may fire before those promises resolve — so we track a
    // cancelled flag and dispose any unlistener that arrives late.
    // Without this, the first mount's listener leaks and every event
    // is delivered twice.
    let cancelled = false;
    let unlistenOutput: (() => void) | null = null;
    let unlistenState: (() => void) | null = null;
    const decoder = new TextDecoder("utf-8", { fatal: false });
    decoderRef.current = decoder;

    api.runState(agent.id).then((snap) => {
      if (cancelled) return;
      setPhase(snap.phase);
      setLastError(snap.last_error);
      // Snapshot is a one-shot buffer — decode it without streaming
      // mode using its own decoder so it doesn't pollute the live
      // stream decoder.
      const snapDecoder = new TextDecoder("utf-8", { fatal: false });
      setLog(stripAnsi(snapDecoder.decode(new Uint8Array(snap.log))));
    });

    onRunOutput((e) => {
      if (e.agent_id !== agent.id) return;
      const chunk = stripAnsi(decoder.decode(new Uint8Array(e.bytes), { stream: true }));
      setLog((prev) => prev + chunk);
    }).then((un) => {
      if (cancelled) {
        un();
        return;
      }
      unlistenOutput = un;
    });

    onRunState((e) => {
      if (e.agent_id !== agent.id) return;
      setPhase(e.phase);
      setLastError(e.last_error);
    }).then((un) => {
      if (cancelled) {
        un();
        return;
      }
      unlistenState = un;
    });

    return () => {
      cancelled = true;
      unlistenOutput?.();
      unlistenState?.();
    };
  }, [agent.id]);

  // Auto-scroll to bottom on log append.
  useEffect(() => {
    const el = logRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [log]);

  const fieldValue = (id: string) => {
    const row = rows.find((r) => r.id === id);
    return overrides[id] ?? row?.value ?? "";
  };

  const devCmd = fieldValue("dev");
  const port = fieldValue("port");
  const isActive = phase === "setup" || phase === "running";
  const linkLive = phase === "running";

  const onPlay = () => {
    if (isActive) {
      void api.runStop(agent.id);
    } else {
      void api.runStart(agent.id);
    }
  };

  const onApply = (next: Record<string, string>) => {
    // Reconcile the draft against the detected rows: keep only real
    // overrides, and prune keys (including stale ones whose row no longer
    // exists after an ecosystem change) from the DB so the override
    // indicator can't get stuck lit.
    const { cleaned, toSet, toDelete } = reconcileOverrides(rows, overrides, next);
    const projectId = agent.project_id;
    if (projectId) {
      for (const { id, value } of toSet) {
        setProjectSetting(projectId, runKey(id), value).catch((err) =>
          console.error("setProjectSetting failed", err),
        );
      }
      for (const id of toDelete) {
        deleteProjectSetting(projectId, runKey(id)).catch((err) =>
          console.error("deleteProjectSetting failed", err),
        );
      }
    }

    setOverrides(cleaned);
    setSettingsOpen(false);
  };

  const hasOverrides = Object.keys(overrides).length > 0;
  const buttonLabel = isActive ? "Stop" : "Start";

  return (
    <div className="run-wrap">
      {/* ── Bar ── */}
      <div className="run-bar v2">
        <button
          className={`run-go ${isActive ? "live" : "stopped"}`}
          onClick={onPlay}
          aria-label={buttonLabel}
          title={phase === "setup" ? "Setup running — click to stop" : buttonLabel}
        >
          <Icon name={isActive ? "stop" : "play"} size={12} />
        </button>

        <div className="run-cmd">
          <span className="p">$</span>
          <span className="cmd-text">{devCmd}</span>
        </div>

        <a
          href={`http://localhost:${port}`}
          target="_blank"
          rel="noreferrer"
          className={`run-link${linkLive ? "" : " disabled"}`}
          onClick={(e) => {
            if (!linkLive) e.preventDefault();
          }}
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
      <div className="run-logs" ref={logRef}>
        {log.length > 0 && <div>{log}</div>}
        {lastError && phase === "stopped" && <div className="e">{lastError}</div>}
        {isActive && (
          <div className="p">
            {"› "}
            <span className="term-cursor" />
          </div>
        )}
      </div>

      {/* ── Settings sheet ── */}
      {settingsOpen && (
        <RunSettingsSheet
          rows={rows}
          overrides={overrides}
          ecosystem={ecosystem}
          onClose={() => setSettingsOpen(false)}
          onApply={onApply}
        />
      )}
    </div>
  );
}
