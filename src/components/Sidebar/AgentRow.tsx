import type { KeyboardEvent, MouseEvent } from "react";
import { useState } from "react";
import type { AgentRecord, AgentStatus, PrChecks, PrState, ShortStats } from "@/api";
import { AgentIdentityChip } from "@/components/AgentIdentityChip";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Badge, type BadgeVariant } from "@/components/ui/Badge";
import { lookupModel } from "@/data/modelCatalog";
import { providerChip, providerLabel } from "@/data/providers";
import type { DraftAgent } from "@/store";
import { useAppStore } from "@/store";
import { maxBehind } from "@/store/git";
import { formatAge } from "@/util/format";
import { useMinuteClock } from "@/util/hooks";
import { type AgentPr, useAgentPrs } from "@/util/prState";
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
  // Every PR across the agent's repos (live state with the persisted database
  // snapshot as fallback, so a merged badge survives restarts, offline
  // stretches, and broken checkouts), each with the CI rollup the app-wide
  // refreshAllPrChecks poll recorded for it. Single-repo agents yield at most
  // one entry — exactly the old primary-only read; a multi-repo agent whose
  // only PR lives on a secondary repo still gets its badge.
  const agentPrs = useAgentPrs(agent);
  const shortstats = useAppStore((s) => s.gitShortstats[agent.id]);
  // Base-staleness across the agent's checkouts (stalest wins — a behind
  // secondary must surface even when the primary is fresh). A quiet "base
  // moved" cue, shown only when a base has genuinely moved ahead (behind > 0);
  // an unknown or zero count renders nothing (never a fake 0).
  const behind = useAppStore((s) => maxBehind(s.gitMeta, agent.id));
  const unseen = useAppStore((s) => s.unseenResults[agent.id] ?? false);
  // Dev-server state for this checkout — orthogonal to the agent's turn status,
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
  const promoteAgentToWorkflow = useAppStore((s) => s.promoteAgentToWorkflow);
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
  // states, a merged PR claims purple (every PR of the set, for multi-repo),
  // everything else is a faint grey. A pending question outranks the plain
  // running green — amber says "you".
  const allMerged = agentPrs.length > 0 && agentPrs.every((e) => e.pr.state === "merged");
  const railClass = awaiting
    ? "wait"
    : working
      ? "run"
      : agent.status === "error"
        ? "err"
        : allMerged
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
  const onPromote = (e: MouseEvent) => {
    e.stopPropagation();
    void promoteAgentToWorkflow(agent.id);
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
        <AgentIdentityChip agent={agent} size={14} />
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
            <button
              className="ag-act iflex-center tip"
              data-tip="Promote to workflow"
              onClick={onPromote}
              aria-label="Promote to workflow"
            >
              <Icon name="combine" size={11} />
            </button>
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
        {agentPrs.length === 1 ? (
          <PrBadge pr={agentPrs[0].pr} checks={agentPrs[0].checks} />
        ) : agentPrs.length > 1 ? (
          <MultiPrBadge prs={agentPrs} />
        ) : hasChanges ? (
          <DiffStat stats={shortstats} />
        ) : null}
        {behind != null && behind > 0 && (
          <span
            className="a-stale tip"
            data-tip={`Base has moved ${behind} commit(s) ahead`}
            aria-label={`Base moved ${behind} commits ahead`}
          >
            <Icon name="branch" size={9} />
            {behind}
          </span>
        )}
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

/** Compact PR pill mirroring the git-panel status. An open PR is tinted by its
 *  CI rollup (green pass / red fail) once `checks` land from the app-wide poll;
 *  merged / closed / pending / no-checks keep their own neutral tones. */
function PrBadge({ pr, checks }: { pr: PrState; checks: PrChecks | null }) {
  if (pr.state === "merged") {
    return (
      <Badge variant="pr-merged" tip={`PR #${pr.number} · merged`}>
        <Icon name="merge" size={10} />#{pr.number}
      </Badge>
    );
  }
  if (pr.state === "closed") {
    return (
      <Badge variant="pr-closed" tip={`PR #${pr.number} · closed`}>
        <Icon name="pr" size={10} />
        PR
      </Badge>
    );
  }
  const ci = ciTint(checks);
  return (
    <Badge variant={ci.variant} tip={`PR #${pr.number} · open${ci.tip}`}>
      <Icon name="pr" size={10} />
      PR
    </Badge>
  );
}

/** Aggregate pill for a multi-repo agent with PRs on several repos: `N PRs`,
 *  tinted by the worst status across the set — any open PR with failing checks
 *  → red; any open PR still pending/unchecked → the neutral open blue; all
 *  open PRs passing → green; no open PRs → closed grey over merged purple.
 *  The tooltip itemizes each PR so the pill stays glanceable. */
function MultiPrBadge({ prs }: { prs: AgentPr[] }) {
  const open = prs.filter((e) => e.pr.state === "open");
  let variant: BadgeVariant;
  let icon: "pr" | "merge" = "pr";
  if (open.length > 0) {
    const tints = open.map((e) => ciTint(e.checks).variant);
    variant = tints.includes("pr-fail")
      ? "pr-fail"
      : tints.includes("pr-open")
        ? "pr-open"
        : "pr-pass";
  } else if (prs.some((e) => e.pr.state === "closed")) {
    variant = "pr-closed";
  } else {
    variant = "pr-merged";
    icon = "merge";
  }
  const tip = prs.map((e) => `#${e.pr.number} ${prStatusWord(e)}`).join(" · ");
  return (
    <Badge variant={variant} tip={tip}>
      <Icon name={icon} size={10} />
      {prs.length} PRs
    </Badge>
  );
}

/** One PR's status for the aggregate tooltip: its state, refined by the CI
 *  rollup while open. */
function prStatusWord({ pr, checks }: AgentPr): string {
  if (pr.state !== "open") return pr.state;
  switch (checks?.rollup) {
    case "passing":
      return "open, checks passing";
    case "failing":
      return "open, checks failing";
    case "pending":
      return "open, checks running";
    default:
      return "open";
  }
}

/** Map an open PR's CI rollup to a pill variant + tooltip suffix. Pending and
 *  "no checks configured" stay on the neutral pr-open blue — only a settled
 *  pass/fail earns a color. */
function ciTint(checks: PrChecks | null): { variant: BadgeVariant; tip: string } {
  switch (checks?.rollup) {
    case "passing":
      return { variant: "pr-pass", tip: ` · checks passing (${checks.passed}/${checks.total})` };
    case "failing":
      return { variant: "pr-fail", tip: ` · checks failing (${checks.failed} failed)` };
    case "pending":
      return { variant: "pr-open", tip: ` · checks running (${checks.pending} pending)` };
    default:
      return { variant: "pr-open", tip: "" };
  }
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
