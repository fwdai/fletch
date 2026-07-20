import type { Terminal } from "@xterm/xterm";
import { useEffect, useRef, useState } from "react";
import { type AgentRecord, api, onRunOutput, onRunState } from "@/api";
import { Icon } from "@/components/Icon";
import {
  loadRunOverrides,
  persistRunOverrides,
  reconcileOverrides,
  rowsOrFallback,
  type SetupRow,
  toSetupRows,
} from "@/components/RunConfig";
import { useAppStore } from "@/store";
import { useXterm } from "@/util/useXterm";
import { resolveTheme } from "@/util/xtermTheme";
import { RunSettingsSheet } from "./RunSettingsSheet";

// Detected run config replaces the old hardcoded defaults. The backend
// (`detect_run_config`) returns rows per ecosystem; the panel shows the
// highest-confidence one. Two settings layers sit on top: the project's
// `run.*` settings (edited in Project Settings), then this agent's
// `run.agent.<id>.*` overrides (edited here in the sheet).
//
// Output is rendered by a read-only xterm terminal (same hook as TermPanel),
// so ANSI colors *and* cursor control (spinners, progress-bar rewrites) render
// faithfully. Raw PTY bytes from `run:output` are written straight to the
// terminal — no ANSI stripping, no React state per chunk; xterm owns decoding,
// scrollback, and scrolling.

export function RunPanel({ agent }: { agent: AgentRecord }) {
  // Phase is owned by the store (fed by an app-wide `run:state` subscription) so
  // the Run tab's running dot survives this panel unmounting on a tab switch.
  const phase = useAppStore((s) => s.runPhases[agent.id] ?? "idle");
  const setRunPhase = useAppStore((s) => s.setRunPhase);
  const setRunPort = useAppStore((s) => s.setRunPort);
  const [lastError, setLastError] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [projectValues, setProjectValues] = useState<Record<string, string>>({});
  const [overrides, setOverrides] = useState<Record<string, string>>({});
  const [rows, setRows] = useState<SetupRow[]>([]);
  const [ecosystem, setEcosystem] = useState<string | null>(null);
  const termRef = useRef<Terminal | null>(null);

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

  // Mount a read-only xterm terminal and stream run output into it.
  // Rehydrate the snapshot on mount/agent-switch so the panel preserves logs
  // from prior starts (and across panel mounts). The whole lifecycle re-runs
  // when the agent id changes: the old terminal is disposed and a fresh one
  // replays that agent's snapshot.
  const termContainerRef = useXterm(
    {
      fontSize: 12,
      lineHeight: 1.2,
      theme: resolveTheme(),
      scrollback: 20000,
      // Read-only log view: no input, no blinking input caret.
      disableStdin: true,
      cursorBlink: false,
      cursorInactiveStyle: "none",
    },
    (term) => {
      // StrictMode runs effects twice in dev, and cleanup may fire before the
      // async setup below resolves — so we track a cancelled flag and dispose
      // any listener that registers late. Without this, the first mount's
      // listener leaks and every event is delivered twice.
      let cancelled = false;
      let unlistenOutput: (() => void) | null = null;
      let unlistenState: (() => void) | null = null;
      termRef.current = term;

      // Subscribe to live output BEFORE fetching the snapshot, so no chunk
      // produced during the snapshot round-trip can slip through a gap. Because
      // we subscribe first, `pending` is a *complete, contiguous* byte range
      // from the moment the listener went live (`liveStart`) onward — we never
      // miss a chunk in that range, and we never drop one. The snapshot is only
      // needed to prepend BACKLOG that predates `liveStart`.
      //
      // This matters because the backend log is a capped ring: `snap.log` holds
      // only the tail `[log_seq - log.length, log_seq)`, while `log_seq` counts
      // every byte ever appended. A noisy run can evict, during the round trip,
      // bytes that a buffered chunk still carries — so we must NOT dedupe
      // buffered chunks by offset against `log_seq` (that would drop chunks
      // whose bytes are no longer in `snap.log`). Instead: write the snapshot's
      // prefix up to `liveStart`, then every buffered chunk in full.
      let gateOpen = false;
      const pending: { bytes: Uint8Array; seq: number }[] = [];

      (async () => {
        const unOutput = await onRunOutput((e) => {
          if (e.agent_id !== agent.id) return;
          const bytes = new Uint8Array(e.bytes);
          // Buffer until the handoff completes; after that every chunk is
          // strictly newer than everything written, so write it directly.
          if (gateOpen) term.write(bytes);
          else pending.push({ bytes, seq: e.seq });
        });
        if (cancelled) {
          unOutput();
          return;
        }
        unlistenOutput = unOutput;

        const unState = await onRunState((e) => {
          if (e.agent_id !== agent.id) return;
          // Phase flows through the store via the app-wide listener; this local
          // subscription only mirrors last_error, which the store doesn't track.
          setLastError(e.last_error);
        });
        if (cancelled) {
          unState();
          return;
        }
        unlistenState = unState;

        // With the listeners live, fetch the rehydration snapshot.
        let snapLog: Uint8Array | null = null;
        let snapEnd = 0; // absolute end offset of the snapshot (log_seq)
        try {
          const snap = await api.runState(agent.id);
          if (cancelled) return;
          // Rehydrate the store phase too, so a running app opened after an app
          // reload lights the tab dot even before the next live event arrives.
          setRunPhase(agent.id, snap.phase);
          setLastError(snap.last_error);
          if (snap.log.length > 0) snapLog = new Uint8Array(snap.log);
          snapEnd = snap.log_seq;
        } catch (err) {
          // Snapshot failed: no backlog, but the handoff MUST still complete —
          // otherwise every buffered/future chunk strands in `pending` and the
          // terminal stays blank until remount. Fall through with snapLog=null.
          console.error("runState failed — streaming live output without snapshot", err);
        }
        if (cancelled) return;

        // Handoff. `liveStart` is the absolute offset of the first buffered
        // byte; the snapshot need only supply what precedes it. With no buffered
        // output, the snapshot is the whole story up to `snapEnd`.
        const liveStart = pending.length > 0 ? pending[0].seq - pending[0].bytes.length : snapEnd;
        if (snapLog) {
          const snapStart = snapEnd - snapLog.length;
          // Backlog = snapshot bytes before our live coverage. Clamp to the
          // retained range: if eviction already advanced past `liveStart`
          // (snapStart >= liveStart), there's no backlog to prepend and the
          // buffered chunks cover everything the snapshot could. Writing whole
          // chunks (below) then means no drop, no dup, no reorder.
          const backlogLen = Math.max(0, Math.min(liveStart, snapEnd) - snapStart);
          if (backlogLen > 0) term.write(snapLog.subarray(0, backlogLen));
        }
        // Every buffered chunk in full — never dropped, even if its bytes were
        // evicted from the snapshot tail during the round trip.
        for (const c of pending) term.write(c.bytes);
        pending.length = 0;
        gateOpen = true;
      })();

      return () => {
        cancelled = true;
        termRef.current = null;
        unlistenOutput?.();
        unlistenState?.();
      };
    },
    [agent.id, setRunPhase],
    { autoFocus: false },
  );

  // Re-apply the xterm theme on dark ↔ light switches without recreating the
  // terminal (same approach as TermPanel).
  useEffect(() => {
    const observer = new MutationObserver(() => {
      if (termRef.current) termRef.current.options.theme = resolveTheme();
    });
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });
    return () => observer.disconnect();
  }, []);

  // What each row inherits: the project setting when one exists, else the
  // detected value. Agent-level overrides compare against these, so a value
  // matching the project setting never reads as an override. Falling back to
  // the empty rows lets a project-configured command surface here even when
  // the repo detected nothing.
  const effectiveRows = rowsOrFallback(rows).map((r) =>
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

  // While running, the backend owns the port (it may have bumped the configured
  // one to the next free port and emits the real value via `run:port`). Prefer
  // that store value for the link/label; fall back to the locally resolved
  // (configured) port when idle.
  const storePort = useAppStore((s) => s.runPorts[agent.id]);
  const displayPort = storePort ?? port;

  // Seed the store with the configured port so the sidebar indicator can show
  // `:port` before a run starts — but only while idle, so we never clobber the
  // backend's authoritative (possibly bumped) port during an active run.
  useEffect(() => {
    if (port && !isActive) setRunPort(agent.id, port);
  }, [agent.id, port, isActive, setRunPort]);

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
          href={`http://localhost:${displayPort}`}
          target="_blank"
          rel="noreferrer"
          className={`run-link text-xs${linkLive ? "" : " disabled"}`}
          onClick={(e) => {
            if (!linkLive) e.preventDefault();
          }}
        >
          <span className="colon">:</span>
          <span className="port">{displayPort}</span>
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

      {/* ── Logs (read-only xterm) ── */}
      <div className="xterm-slot">
        <div
          ref={termContainerRef}
          className="xterm-host"
          style={{ inset: "10px 6px 12px 14px" }}
        />
      </div>
      {lastError && phase === "stopped" && <div className="run-error e text-sm">{lastError}</div>}

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
