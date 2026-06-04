import { useState } from "react";
import type { MouseEvent } from "react";
import type { AgentRecord, AgentStatus } from "../../api";
import type { DraftAgent } from "../../store";
import { useAppStore } from "../../store";
import { LANDMARK_NAMES } from "../../data/landmarks";
import { Icon, LandmarkGlyph } from "../Icon";
import { formatAge, formatTokens } from "../../util/format";
import { useMinuteClock } from "../../util/hooks";
import { AgentStatsPopover, type AgentStats } from "./AgentStatsPopover";

interface RealRowProps {
  kind: "real";
  agent: AgentRecord;
  active: boolean;
  showGlyph: boolean;
  onClick: () => void;
}
interface DraftRowProps {
  kind: "draft";
  draft: DraftAgent;
  active: boolean;
  showGlyph: boolean;
  onClick: () => void;
}

type Props = RealRowProps | DraftRowProps;

export function AgentRow(props: Props) {
  if (props.kind === "draft") return <DraftRow {...props} />;
  return <RealRow {...props} />;
}

// ── real agent ───────────────────────────────────────────────────────────────

function RealRow({ agent, active, showGlyph, onClick }: RealRowProps) {
  const rawTokens = useAppStore((s) => s.tokens[agent.id]);
  const stop = useAppStore((s) => s.stop);
  const archive = useAppStore((s) => s.archive);
  const now = useMinuteClock();
  const [statsOpen, setStatsOpen] = useState(false);

  const branch = agent.repos[0]?.branch ?? null;
  const taskOrBranch = firstShort(agent.task) || branch || "—";
  const age = formatAge(agent.created_at, now);
  const tokensLabel =
    typeof rawTokens === "number" && rawTokens > 0
      ? formatTokens(rawTokens)
      : null;
  const glyphName = LANDMARK_NAMES.includes(agent.name) ? agent.name : agent.name;

  const stats: AgentStats = {
    launched: age || "just now",
    runtime: liveRuntime(agent.created_at, now, agent.status),
    tokens: rawTokens ?? null,
    contextPct: contextPctFromTokens(rawTokens),
  };

  const stoppable =
    agent.status === "spawning" ||
    agent.status === "running";
  const archivable =
    agent.status === "idle" ||
    agent.status === "stopped" ||
    agent.status === "error";

  const onStop = (e: MouseEvent) => {
    e.stopPropagation();
    stop(agent.id);
  };
  const onArchive = (e: MouseEvent) => {
    e.stopPropagation();
    archive(agent.id);
  };

  return (
    <div
      className={`agent ${active ? "active" : ""} ${showGlyph ? "with-glyph" : ""}`}
      onClick={onClick}
    >
      <div className="agent-row">
        <StatusDot status={agent.status} />
        {showGlyph && (
          <span className="ag-glyph">
            <LandmarkGlyph name={glyphName} />
          </span>
        )}
        <span className="ag-name">{agent.name}</span>
        <span className="ag-provider-inline">· {agent.provider}</span>
        <span className="ag-actions">
          {stoppable && (
            <button
              className="ag-act tip"
              data-tip="Stop"
              onClick={onStop}
              aria-label="Stop"
            >
              <Icon name="stop" size={11} />
            </button>
          )}
          {archivable && (
            <button
              className="ag-act tip"
              data-tip="Archive"
              onClick={onArchive}
              aria-label="Archive"
            >
              <Icon name="archive" size={11} />
            </button>
          )}
        </span>
      </div>
      <div className="agent-sub">
        <span className="a-branch">{taskOrBranch}</span>
        {tokensLabel && <span className="a-changes" title={`${rawTokens} input tokens last turn`}>●</span>}
        <span
          className="a-time"
          onMouseEnter={() => setStatsOpen(true)}
          onMouseLeave={() => setStatsOpen(false)}
        >
          {age}
          {statsOpen && <AgentStatsPopover stats={stats} />}
        </span>
      </div>
    </div>
  );
}

// ── draft (not-yet-spawned) ──────────────────────────────────────────────────

function DraftRow({ draft, active, showGlyph, onClick }: DraftRowProps) {
  const removeDraft = useAppStore((s) => s.removeDraft);

  function onDiscard(e: React.MouseEvent) {
    e.stopPropagation();
    removeDraft(draft.id);
  }

  return (
    <div
      className={`agent ${active ? "active" : ""} ${showGlyph ? "with-glyph" : ""}`}
      onClick={onClick}
    >
      <div className="agent-row">
        <span className="ag-dot" style={{ background: "var(--fg-3)" }} />
        {showGlyph && (
          <span className="ag-glyph">
            <LandmarkGlyph name={draft.name} />
          </span>
        )}
        <span className="ag-name">{draft.name}</span>
        <span className="ag-provider-inline" style={{ color: "var(--accent)" }}>
          · new
        </span>
        <button className="ag-discard" onClick={onDiscard} title="Discard">
          <Icon name="close" />
        </button>
      </div>
      <div className="agent-sub">
        <span className="a-branch">Define task…</span>
      </div>
    </div>
  );
}

// ── helpers ──────────────────────────────────────────────────────────────────

function StatusDot({ status }: { status: AgentStatus }) {
  const color =
    status === "running" ? "var(--success)" :
    status === "spawning" ? "var(--warn)" :
    status === "error" ? "var(--danger)" : "var(--fg-3)";
  const isRunning = status === "running";
  return (
    <span
      className="ag-dot"
      style={{
        background: color,
        boxShadow: isRunning
          ? "0 0 0 2px color-mix(in oklch, var(--success), transparent 78%)"
          : "none",
        animation: isRunning ? "pulse 2s var(--ease) infinite" : undefined,
      }}
    />
  );
}

function firstShort(s: string | null | undefined, max = 36): string {
  if (!s) return "";
  const nl = s.indexOf("\n");
  const head = nl === -1 ? s : s.slice(0, nl);
  return head.length > max ? head.slice(0, max - 1) + "…" : head;
}

function liveRuntime(iso: string, now: number, status: AgentStatus): string {
  if (status === "stopped" || status === "error") return "—";
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return "—";
  const sec = Math.max(0, Math.floor((now - t) / 1000));
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  return `${m}m ${s}s`;
}

function contextPctFromTokens(tokens: number | undefined): number {
  if (typeof tokens !== "number" || tokens <= 0) return 0;
  return Math.min(100, Math.round((tokens / 200_000) * 100));
}
