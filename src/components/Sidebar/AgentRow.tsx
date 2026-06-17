import { useState } from "react";
import type { MouseEvent } from "react";
import type { AgentRecord, AgentStatus, PrState, ShortStats } from "../../api";
import type { DraftAgent } from "../../store";
import { useAppStore } from "../../store";
import { providerChip, providerLabel } from "../../data/providers";
import { Icon } from "../Icon";
import { ProviderIcon } from "../ProviderIcon";
import { formatAge } from "../../util/format";
import { useMinuteClock } from "../../util/hooks";
import { AgentStatsPopover, type AgentStats } from "./AgentStatsPopover";

interface RealRowProps {
  kind: "real";
  agent: AgentRecord;
  active: boolean;
  onClick: () => void;
}
interface DraftRowProps {
  kind: "draft";
  draft: DraftAgent;
  active: boolean;
  onClick: () => void;
}

type Props = RealRowProps | DraftRowProps;

export function AgentRow(props: Props) {
  if (props.kind === "draft") return <DraftRow {...props} />;
  return <RealRow {...props} />;
}

// ── real agent ───────────────────────────────────────────────────────────────

function RealRow({ agent, active, onClick }: RealRowProps) {
  const rawTokens = useAppStore((s) => s.tokens[agent.id]);
  const prState = useAppStore((s) => s.prStates[agent.id] ?? null);
  const shortstats = useAppStore((s) => s.gitShortstats[agent.id]);
  const unseen = useAppStore((s) => s.unseenResults[agent.id] ?? false);
  const stop = useAppStore((s) => s.stop);
  const archive = useAppStore((s) => s.archive);
  const now = useMinuteClock();
  const [statsOpen, setStatsOpen] = useState(false);

  const branch = agent.repos[0]?.branch ?? null;
  const taskOrBranch = firstShort(agent.task) || branch || "—";
  const age = formatAge(agent.created_at, now);
  // spawning is the start of a run — show it as "working" too, not a dead row
  const working = agent.status === "running" || agent.status === "spawning";

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

  // The status rail doubles as the left spine: colored for live/terminal
  // states, a merged PR claims purple, everything else is a faint grey.
  const railClass =
    working ? "run" :
    agent.status === "error" ? "err" :
    prState?.state === "merged" ? "merged" : "idle";

  const hasChanges =
    !!shortstats && (shortstats.additions > 0 || shortstats.deletions > 0);

  const onStop = (e: MouseEvent) => {
    e.stopPropagation();
    stop(agent.id);
  };
  const onArchive = (e: MouseEvent) => {
    e.stopPropagation();
    archive(agent.id);
  };

  return (
    <div className={`agent ${active ? "active" : ""}`} onClick={onClick}>
      <span className={`ag-rail ${railClass}`} />
      <div className="agent-row">
        <span className={`ag-name ${working ? "shimmer" : ""}`}>{agent.name}</span>
        <span
          className="ag-prov-chip tip"
          data-tip={providerLabel(agent.provider)}
          data-tip-down=""
        >
          <ProviderIcon slug={agent.provider} {...providerChip(agent.provider)} size={14} />
        </span>
        <span className="ag-slot">
          <span className="ag-meta">
            {working && <span className="ag-loader" aria-label="Working" />}
            {agent.status === "idle" && !active && unseen && (
              <span
                className="ag-unseen tip"
                data-tip="New results to review"
                aria-label="New results to review"
              />
            )}
            {agent.status === "error" && <span className="ag-badge err">error</span>}
          </span>
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
        </span>
      </div>
      <div className="agent-sub">
        <span className="a-task">{taskOrBranch}</span>
        {prState ? (
          <PrBadge pr={prState} />
        ) : hasChanges ? (
          <DiffStat stats={shortstats} />
        ) : null}
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

function DraftRow({ draft, active, onClick }: DraftRowProps) {
  const removeDraft = useAppStore((s) => s.removeDraft);

  function onDiscard(e: React.MouseEvent) {
    e.stopPropagation();
    removeDraft(draft.id);
  }

  return (
    <div className={`agent ${active ? "active" : ""}`} onClick={onClick}>
      <span className="ag-rail idle" />
      <div className="agent-row">
        <span className="ag-name ag-name-draft">{draft.name}</span>
        <span
          className="ag-prov-chip tip"
          data-tip={providerLabel(draft.provider)}
          data-tip-down=""
        >
          <ProviderIcon slug={draft.provider} {...providerChip(draft.provider)} size={14} />
        </span>
        <span className="ag-slot">
          <span className="ag-meta">
            <span className="ag-badge new">new</span>
          </span>
          <span className="ag-actions">
            <button
              className="ag-act tip"
              data-tip="Discard"
              onClick={onDiscard}
              aria-label="Discard"
            >
              <Icon name="close" size={11} />
            </button>
          </span>
        </span>
      </div>
      <div className="agent-sub">
        <span className="a-task a-task-draft">Define task…</span>
      </div>
    </div>
  );
}

// ── pieces ─────────────────────────────────────────────────────────────────

/** Compact PR pill mirroring the git-panel status. Note: CI/checks state is
 *  not yet in PrState, so an open PR shows a neutral pill (no pass/fail tint). */
function PrBadge({ pr }: { pr: PrState }) {
  if (pr.state === "merged") {
    return (
      <span className="ag-badge pr-merged tip" data-tip={`PR #${pr.number} · merged`}>
        <Icon name="merge" size={10} />#{pr.number}
      </span>
    );
  }
  const cls = pr.state === "closed" ? "pr-closed" : "pr-open";
  return (
    <span className={`ag-badge ${cls} tip`} data-tip={`PR #${pr.number} · ${pr.state}`}>
      <Icon name="pr" size={10} />PR
    </span>
  );
}

function DiffStat({ stats }: { stats: ShortStats }) {
  return (
    <span className="a-diff" title={`${stats.file_count} file(s) changed`}>
      {stats.additions > 0 && <span className="add">+{stats.additions}</span>}
      {stats.additions > 0 && stats.deletions > 0 && " "}
      {stats.deletions > 0 && <span className="del">−{stats.deletions}</span>}
    </span>
  );
}

// ── helpers ──────────────────────────────────────────────────────────────────

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
