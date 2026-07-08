import { useEffect, useRef, useState } from "react";
import { type AgentRecord, api, onRunOutput, onRunState } from "@/api";
import { Icon } from "@/components/Icon";
import {
  loadRunOverrides,
  persistRunOverrides,
  reconcileOverrides,
  type SetupRow,
  toSetupRows,
} from "@/components/RunConfig";
import { useAppStore } from "@/store";
import { RunSettingsSheet } from "./RunSettingsSheet";

// Detected run config replaces the old hardcoded defaults. The backend
// (`detect_run_config`) returns rows per ecosystem; the panel shows the
// highest-confidence one. Two settings layers sit on top: the project's
// `run.*` settings (edited in Project Settings), then this agent's
// `run.agent.<id>.*` overrides (edited here in the sheet).

// Strip ANSI escape sequences before rendering. v1 keeps log rendering
// dead-simple (plain text with pre-wrap); colorization can come later.
// Covers CSI (ESC [ ... letter) and OSC (ESC ] ... BEL / ST).
const ANSI_RE = /\x1b\[[0-9;?]*[a-zA-Z]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g;
const stripAnsi = (s: string) => s.replace(ANSI_RE, "");

// Cap the in-memory log so a long-running dev server can't grow React state
// without bound. Keep the tail — what a terminal would show — and trim from
// the front on a line boundary so a half-line never gets rendered.
const MAX_LOG_CHARS = 256 * 1024;
const capLog = (s: string): string => {
  if (s.length <= MAX_LOG_CHARS) return s;
  const tail = s.slice(s.length - MAX_LOG_CHARS);
  const nl = tail.indexOf("\n");
  return nl === -1 ? tail : tail.slice(nl + 1);
};

export function RunPanel({ agent }: { agent: AgentRecord }) {
  // Phase is owned by the store (fed by an app-wide `run:state` subscription) so
  // the Run tab's running dot survives this panel unmounting on a tab switch.
  const phase = useAppStore((s) => s.runPhases[agent.id] ?? "idle");
  const setRunPhase = useAppStore((s) => s.setRunPhase);
  const setRunPort = useAppStore((s) => s.setRunPort);
  const [lastError, setLastError] = useState<string | null>(null);
  const [log, setLog] = useState<string>("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [projectValues, setProjectValues] = useState<Record<string, string>>({});
  const [overrides, setOverrides] = useState<Record<string, string>>({});
  const [rows, setRows] = useState<SetupRow[]>([]);
  const [ecosystem, setEcosystem] = useState<string | null>(null);
  const logRef = useRef<HTMLDivElement | null>(null);
  // Streaming UTF-8 decoder so a multi-byte rune split across two
  // PTY chunks doesn't produce a replacement character. Reset each
  // time we re-subscribe (agent switch).
  const decoderRef = useRef<TextDecoder | null>(null);

  // Load the project's run settings (the base every agent inherits) and this
  // agent's overrides on top of them. Re-loads on agent switch.
  useEffect(() => {
    let cancelled = false;
    if (!agent.project_id) {
      setProjectValues({});
      setOverrides({});
      return;
    }
    loadRunOverrides(agent.project_id).then((loaded) => {
      if (cancelled) return;
      setProjectValues(loaded);
    });
    loadRunOverrides(agent.project_id, agent.id).then((loaded) => {
      if (cancelled) return;
      setOverrides(loaded);
    });
    return () => {
      cancelled = true;
    };
  }, [agent.project_id, agent.id]);

  // Detect the run config for this agent's checkout. Re-runs on agent
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
        setRows(toSetupRows(primary?.rows ?? []));
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
      // Rehydrate the store phase too, so a running app opened after an app
      // reload lights the tab dot even before the next live event arrives.
      setRunPhase(agent.id, snap.phase);
      setLastError(snap.last_error);
      // Snapshot is a one-shot buffer — decode it without streaming
      // mode using its own decoder so it doesn't pollute the live
      // stream decoder.
      const snapDecoder = new TextDecoder("utf-8", { fatal: false });
      setLog(capLog(stripAnsi(snapDecoder.decode(new Uint8Array(snap.log)))));
    });

    onRunOutput((e) => {
      if (e.agent_id !== agent.id) return;
      const chunk = stripAnsi(decoder.decode(new Uint8Array(e.bytes), { stream: true }));
      setLog((prev) => capLog(prev + chunk));
    }).then((un) => {
      if (cancelled) {
        un();
        return;
      }
      unlistenOutput = un;
    });

    onRunState((e) => {
      if (e.agent_id !== agent.id) return;
      // Phase flows through the store via the app-wide listener; this local
      // subscription only mirrors last_error, which the store doesn't track.
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
  }, [agent.id, setRunPhase]);

  // Auto-scroll to bottom on log append.
  useEffect(() => {
    const el = logRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [log]);

  // What each row inherits: the project setting when one exists, else the
  // detected value. Agent-level overrides compare against these, so a value
  // matching the project setting never reads as an override.
  const effectiveRows = rows.map((r) =>
    projectValues[r.id] != null
      ? { ...r, value: projectValues[r.id], origin: "project" as const }
      : r,
  );

  const fieldValue = (id: string) => {
    const row = effectiveRows.find((r) => r.id === id);
    return overrides[id] ?? row?.value ?? "";
  };

  const devCmd = fieldValue("dev");
  const port = fieldValue("port");
  const isActive = phase === "setup" || phase === "running";
  const linkLive = phase === "running";

  // Publish the resolved port to the store so the sidebar's running indicator
  // can show `:port`. The port isn't on the `run:state` event, so the panel —
  // which already resolves detected value + overrides — is the source.
  useEffect(() => {
    if (port) setRunPort(agent.id, port);
  }, [agent.id, port, setRunPort]);

  const onPlay = () => {
    if (isActive) {
      void api.runStop(agent.id);
    } else {
      void api.runStart(agent.id);
    }
  };

  const onApply = (next: Record<string, string>) => {
    // Reconcile the draft against the inherited values: keep only real
    // overrides, and prune keys (including stale ones whose row no longer
    // exists after an ecosystem change) from the DB so the override
    // indicator can't get stuck lit. Persisted under this agent's scope —
    // the project setting is only edited from Project Settings.
    const { cleaned, toSet, toDelete } = reconcileOverrides(effectiveRows, overrides, next);
    if (agent.project_id) {
      persistRunOverrides(agent.project_id, toSet, toDelete, agent.id);
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

        <div className="run-cmd text-sm">
          <span className="p">$</span>
          <span className="cmd-text">{devCmd}</span>
        </div>

        <a
          href={`http://localhost:${port}`}
          target="_blank"
          rel="noreferrer"
          className={`run-link text-xs${linkLive ? "" : " disabled"}`}
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
      <div className="run-logs text-sm" ref={logRef}>
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
          rows={effectiveRows}
          overrides={overrides}
          ecosystem={ecosystem}
          onClose={() => setSettingsOpen(false)}
          onApply={onApply}
        />
      )}
    </div>
  );
}
