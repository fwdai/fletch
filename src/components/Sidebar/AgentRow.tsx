import type { KeyboardEvent, MouseEvent } from "react";
import { useState } from "react";
import type { AgentRecord, AgentStatus, PrState, ShortStats } from "@/api";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Mono } from "@/components/SettingsScreen/CustomAgents/Mono";
import { Badge } from "@/components/ui/Badge";
import { lookupModel } from "@/data/modelCatalog";
import { providerChip, providerLabel } from "@/data/providers";
import type { DraftAgent } from "@/store";
import { useAppStore } from "@/store";
import { formatAge } from "@/util/format";
import { useMinuteClock } from "@/util/hooks";
import { type AgentStats, AgentStatsPopover } from "./AgentStatsPopover";

/** The agent rows carry nested buttons (stop/archive/discard), so they can't be
 *  a `<button>`; they use `role="button"` + this handler to stay keyboard
 *  operable (Enter/Space activates the row like a click). */
function activateOnKey(e: KeyboardEvent, fn: () => void) {
  // Ignore key events that bubbled up from a nested control (stop/archive/
  // discard) — otherwise pressing Space/Enter on those buttons would also
  // select the row (and Discard would select a draft as it's removed).
  if (e.target !== e.currentTarget) return;
  if (e.key === "Enter" || e.key === " ") {
    e.preventDefault();
    fn();
  }
}

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
  const usage = useAppStore((s) => s.usage[agent.id]);
  const prState = useAppStore((s) => s.prStates[agent.id] ?? null);
  const shortstats = useAppStore((s) => s.gitShortstats[agent.id]);
  const unseen = useAppStore((s) => s.unseenResults[agent.id] ?? false);
  // Dev-server state for this worktree — orthogonal to the agent's turn status,
  // so it shows as its own play chip beside the identity chip, not among the
  // activity cues (rail / loader / shimmer). Fed by the app-wide `run:state`
  // subscription (see store/app.ts), so it stays correct from any tab.
  const runLive = useAppStore((s) => {
    const phase = s.runPhases[agent.id];
    return phase === "setup" || phase === "running";
  });
  // Dev-server port, if the RunPanel has resolved it this session — surfaced in
  // the running chip's tooltip (":port"). Absent until then.
  const runPort = useAppStore((s) => s.runPorts[agent.id]);
  const pending = useAppStore((s) => s.pendingToolUse[agent.id]);
  const stop = useAppStore((s) => s.stop);
  const archive = useAppStore((s) => s.archive);
  // The custom agent this session was spawned from, if any (and still present).
  // Drives the row's identity chip; falls back to the base provider when the
  // custom agent has since been deleted.
  const customAgent = useAppStore((s) =>
    agent.custom_agent_id ? s.customAgents.find((a) => a.id === agent.custom_agent_id) : undefined,
  );
  const now = useMinuteClock();
  const [statsOpen, setStatsOpen] = useState(false);

  const branch = agent.repos[0]?.branch ?? null;
  const taskOrBranch = firstShort(agent.task) || branch || "—";
  const age = formatAge(agent.created_at, now);
  // spawning is the start of a run — show it as "working" too, not a dead row
  const working = agent.status === "running" || agent.status === "spawning";
  // The agent has paused on a question/plan tool and is waiting for a human
  // answer (AskUserQuestion / ExitPlanMode). Status stays `running`, so this
  // separate signal is what distinguishes "the ball's in your court" from
  // "still thinking". Mirrors ChatView's `awaitingInput`.
  const awaiting = working && !!pending && Object.keys(pending).length > 0;

  const catalog = useAppStore((s) => s.modelCatalog);
  const hasUsage = !!usage && usage.contextTokens > 0;
  // Prefer the window the agent reports (codex does); otherwise look the model
  // up in the catalog (claude/opencode/pi don't report one) so the 1M-context
  // models read true; fall back to a default only when the model is unknown.
  // Mirrors the composer's UsageMeter so both gauges agree.
  const contextWindow =
    usage?.contextWindow ||
    lookupModel(catalog, usage?.model)?.contextWindow ||
    DEFAULT_CONTEXT_WINDOW;
  const stats: AgentStats = {
    launched: age || "just now",
    runtime: liveRuntime(agent.created_at, now, agent.status),
    contextTokens: hasUsage ? usage?.contextTokens : null,
    contextWindow,
    contextPct: hasUsage ? contextPct(usage?.contextTokens, contextWindow) : 0,
    // Fresh input only — matching the composer gauge. Cumulative cache read
    // balloons (the same cached prefix re-read every turn) and is misleading
    // as a session "input" total.
    totalInput: usage ? usage.inputTokens : null,
    totalOutput: usage ? usage.outputTokens : null,
    costUsd: usage ? usage.costUsd : null,
  };

  const stoppable = agent.status === "spawning" || agent.status === "running";
  const archivable =
    agent.status === "idle" || agent.status === "stopped" || agent.status === "error";

  // The status rail doubles as the left spine: colored for live/terminal
  // states, a merged PR claims purple, everything else is a faint grey.
  // A pending question outranks the plain running green — amber says "you".
  const railClass = awaiting
    ? "wait"
    : working
      ? "run"
      : agent.status === "error"
        ? "err"
        : prState?.state === "merged"
          ? "merged"
          : "idle";

  const hasChanges = !!shortstats && (shortstats.additions > 0 || shortstats.deletions > 0);

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
      className={`agent ${active ? "active" : ""} ${awaiting ? "awaiting" : ""}`}
      role="button"
      tabIndex={0}
      aria-current={active ? "page" : undefined}
      onClick={onClick}
      onKeyDown={(e) => activateOnKey(e, onClick)}
    >
      <span className={`ag-rail ${railClass}`} />
      <div className="agent-row flex-center">
        <span className={`ag-name ${working && !awaiting ? "shimmer" : ""}`}>{agent.name}</span>
        <span
          className="ag-prov-chip tip"
          data-tip={
            customAgent
              ? `${customAgent.name} · ${providerLabel(agent.provider)}`
              : providerLabel(agent.provider)
          }
          data-tip-down=""
        >
          {customAgent ? (
            <Mono name={customAgent.name} hue={customAgent.color} size={14} />
          ) : (
            <ProviderIcon slug={agent.provider} {...providerChip(agent.provider)} size={14} />
          )}
        </span>
        {runLive && (
          <span
            className="ag-run tip"
            data-tip={runPort ? `Dev server running on :${runPort}` : "Dev server running"}
            aria-label={runPort ? `Dev server running on port ${runPort}` : "Dev server running"}
          >
            <Icon name="play" size={9} />
          </span>
        )}
        <span className="ag-slot iflex-center">
          <span className={`ag-meta ${agent.status === "error" ? "wide" : ""}`}>
            {awaiting ? (
              <span
                className="ag-waiting tip"
                data-tip="Waiting for user input"
                aria-label="Waiting for user input"
              >
                <Icon name="hand" size={12} />
              </span>
            ) : (
              working && <span className="ag-loader" aria-label="Working" />
            )}
            {agent.status === "idle" && !active && unseen && (
              <span
                className="ag-unseen tip"
                data-tip="New results to review"
                aria-label="New results to review"
              />
            )}
            {agent.status === "error" && <Badge variant="err">error</Badge>}
          </span>
          <span className="ag-actions">
            {stoppable && !awaiting && (
              <button
                className="ag-act iflex-center tip"
                data-tip="Stop"
                onClick={onStop}
                aria-label="Stop"
              >
                <Icon name="stop" size={11} />
              </button>
            )}
            {archivable && (
              <button
                className="ag-act iflex-center tip"
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
      <div className="agent-sub flex-center">
        <span className="a-task">{taskOrBranch}</span>
        {prState ? <PrBadge pr={prState} /> : hasChanges ? <DiffStat stats={shortstats} /> : null}
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
    <div
      className={`agent ${active ? "active" : ""}`}
      role="button"
      tabIndex={0}
      aria-current={active ? "page" : undefined}
      onClick={onClick}
      onKeyDown={(e) => activateOnKey(e, onClick)}
    >
      <span className="ag-rail idle" />
      <div className="agent-row flex-center">
        <span className="ag-name ag-name-draft">{draft.name}</span>
        <span
          className="ag-prov-chip tip"
          data-tip={providerLabel(draft.provider)}
          data-tip-down=""
        >
          <ProviderIcon slug={draft.provider} {...providerChip(draft.provider)} size={14} />
        </span>
        <span className="ag-slot iflex-center">
          <span className="ag-meta wide">
            <Badge variant="new">new</Badge>
          </span>
          <span className="ag-actions">
            <button
              className="ag-act iflex-center tip"
              data-tip="Discard"
              onClick={onDiscard}
              aria-label="Discard"
            >
              <Icon name="close" size={11} />
            </button>
          </span>
        </span>
      </div>
      <div className="agent-sub flex-center">
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
      <Badge variant="pr-merged" tip={`PR #${pr.number} · merged`}>
        <Icon name="merge" size={10} />#{pr.number}
      </Badge>
    );
  }
  const variant = pr.state === "closed" ? "pr-closed" : "pr-open";
  return (
    <Badge variant={variant} tip={`PR #${pr.number} · ${pr.state}`}>
      <Icon name="pr" size={10} />
      PR
    </Badge>
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
  return head.length > max ? `${head.slice(0, max - 1)}…` : head;
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

/** Fallback context window for agents that don't report their own (claude,
 *  opencode, pi all run 200k-class models here). */
const DEFAULT_CONTEXT_WINDOW = 200_000;

function contextPct(tokens: number, window: number): number {
  if (tokens <= 0 || window <= 0) return 0;
  return Math.min(100, Math.round((tokens / window) * 100));
}
