import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { EMPTY_AGENTS, useAppStore } from "../store";
import type { AgentRecord, AgentStatus } from "../api";

// Hardcoded for now; will become an `agent.provider` field when we
// add gemini/codex.
const PROVIDER_LABEL = "claude";

function firstLine(s: string): string {
  const idx = s.indexOf("\n");
  const head = idx === -1 ? s : s.slice(0, idx);
  return head.length > 56 ? head.slice(0, 55) + "…" : head;
}

function statusColor(s: AgentStatus): string {
  switch (s) {
    case "running":
      return "var(--success)";
    case "spawning":
      return "var(--warning)";
    case "error":
      return "var(--danger)";
    default:
      return "var(--text-muted)";
  }
}

function formatAge(iso: string, nowMs: number): string {
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return "";
  const seconds = Math.max(0, Math.floor((nowMs - t) / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  return `${days}d`;
}

function formatTokens(n: number): string {
  if (n < 1_000) return `${n}`;
  if (n < 1_000_000) return `${(n / 1_000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

/** Hook returning a clock that ticks every minute. Used for the age
 *  formatter so labels like "4m" stay current without re-rendering
 *  every second. */
function useMinuteClock(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(id);
  }, []);
  return now;
}

export function AgentList() {
  const agents = useAppStore((s) => s.workspace?.agents ?? EMPTY_AGENTS);
  const selectedId = useAppStore((s) => s.selectedAgentId);
  const selectAgent = useAppStore((s) => s.selectAgent);
  const now = useMinuteClock();

  if (agents.length === 0) {
    return (
      <div className="list">
        <div className="empty">No agents yet. Click + Spawn to start one.</div>
      </div>
    );
  }

  return (
    <div className="list">
      {agents.map((agent) => (
        <AgentRow
          key={agent.id}
          agent={agent}
          selected={selectedId === agent.id}
          onSelect={() => selectAgent(agent.id)}
          now={now}
        />
      ))}
    </div>
  );
}

function AgentRow({
  agent,
  selected,
  onSelect,
  now,
}: {
  agent: AgentRecord;
  selected: boolean;
  onSelect: () => void;
  now: number;
}) {
  const stop = useAppStore((s) => s.stop);
  const resume = useAppStore((s) => s.resume);
  const discard = useAppStore((s) => s.discard);

  const isLive =
    agent.status === "running" ||
    agent.status === "idle" ||
    agent.status === "spawning";
  const isStopped = agent.status === "stopped" || agent.status === "error";

  async function onStop(e: React.MouseEvent) {
    e.stopPropagation();
    const ok = await ask(
      `Stop agent "${agent.name}"? The process will be terminated.`,
      { title: "Stop agent", kind: "warning" },
    );
    if (ok) await stop(agent.id);
  }

  async function onResume(e: React.MouseEvent) {
    e.stopPropagation();
    await resume(agent.id);
  }

  async function onDiscard(e: React.MouseEvent) {
    e.stopPropagation();
    const branchLine = agent.branch
      ? `  • the branch ${agent.branch}\n`
      : "";
    const reflogNote = agent.branch
      ? `\nBranch deletion can be undone via git reflog within ~90 days.`
      : "";
    const ok = await ask(
      `Remove "${agent.name}"?\n\nThis will delete:\n` +
        `  • the worktree at .worktrees/${agent.id} (any uncommitted work)\n` +
        branchLine +
        reflogNote,
      { title: "Remove agent", kind: "warning" },
    );
    if (ok) await discard(agent.id);
  }

  const age = formatAge(agent.created_at, now);
  const rawTokens = useAppStore((s) => s.tokens[agent.id]);
  const tokens =
    typeof rawTokens === "number" && rawTokens > 0
      ? formatTokens(rawTokens)
      : null;

  return (
    <div
      className={`row ${selected ? "selected" : ""}`}
      role="button"
      tabIndex={0}
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect();
        }
      }}
    >
      <span
        className="dot"
        style={{ background: statusColor(agent.status) }}
      />
      <div className="rowtext">
        <div className="row-headline">
          <span className="name">{agent.name}</span>
        </div>
        <div className="meta">
          <span className="provider">{PROVIDER_LABEL}</span>
          <span className="dim">·</span>
          <span className="status-text" data-status={agent.status}>
            {agent.status}
          </span>
          {age && (
            <>
              <span className="dim">·</span>
              <span title={`Created ${agent.created_at}`}>{age}</span>
            </>
          )}
          {tokens && (
            <>
              <span className="dim">·</span>
              <span title={`${rawTokens} input tokens last turn (matches claude /context)`}>
                {tokens} tok
              </span>
            </>
          )}
        </div>
        {agent.task && (
          <div className="task-summary-line">{firstLine(agent.task)}</div>
        )}
      </div>
      <div className="row-actions">
        {isStopped && (
          <IconButton title="Resume" onClick={onResume} tone="accent">
            <PlayIcon />
          </IconButton>
        )}
        {isLive && (
          <IconButton title="Stop" onClick={onStop}>
            <StopIcon />
          </IconButton>
        )}
        <IconButton title="Remove" onClick={onDiscard} tone="danger">
          <TrashIcon />
        </IconButton>
      </div>
    </div>
  );
}

function IconButton({
  title,
  onClick,
  tone,
  children,
}: {
  title: string;
  onClick: (e: React.MouseEvent) => void;
  tone?: "danger" | "accent";
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      className={`iconbtn${tone ? ` tone-${tone}` : ""}`}
      title={title}
      aria-label={title}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

function PlayIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <polygon points="6 4 20 12 6 20 6 4" />
    </svg>
  );
}

function StopIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <rect x="6" y="6" width="12" height="12" rx="1.5" />
    </svg>
  );
}

function TrashIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <polyline points="3 6 5 6 21 6" />
      <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" />
      <path d="M10 11v6" />
      <path d="M14 11v6" />
      <path d="M9 6V4a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2v2" />
    </svg>
  );
}
