import type { AgentRecord, AgentStatus, DiffStats } from "@/api";
import { Icon } from "@/components/Icon";
import { PanelToggle } from "@/components/PanelToggle";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { firstLine, formatAge } from "@/util/format";
import { useMinuteClock } from "@/util/hooks";
import { AccountPicker } from "./AccountPicker";
import { ForkMenu } from "./ForkMenu";
import { ViewToggle } from "./ViewToggle";

/** Header strip above the workspace body. Houses the left-sidebar toggle, the
 *  agent's title over its codename + branch + diff + age, the account picker
 *  (account providers only), the Custom/Native view switcher, and the
 *  right-panel toggle. A sub-agent thread's header (SubagentThread/ThreadHeader)
 *  keeps this exact skeleton — dot, title line, meta line — so stepping into
 *  and out of a thread moves nothing but the words. */
interface Props {
  agent: AgentRecord;
}

/** What the agent is working on, as the sidebar names it: its own title once
 *  it has set one, else the first line of the prompt. */
export function agentTitle(agent: Pick<AgentRecord, "title" | "task">): string {
  return agent.title || firstLine(agent.task, 80) || "Untitled";
}

export function WorkspaceHeader({ agent }: Props) {
  const nativeGate = useGate("nativeView");
  const switchView = useAppStore((s) => s.switchView);
  const switchInFlight = useAppStore((s) => s.switchInFlight[agent.id]);
  // Native view is gated behind an experimental flag: while it's off the
  // switcher is hidden, and Workspace renders any native-mode agent as chat so
  // it can't get stranded behind a missing toggle.
  const nativeView = useAppStore((s) => s.features.nativeView);
  const railOpen = useAppStore((s) => s.transcriptRailOpen);
  const toggleRail = useAppStore((s) => s.toggleTranscriptRail);
  const now = useMinuteClock();
  // Use shortstats (5s app-wide poll) rather than full git state, since
  // the header shows shortstats regardless of which right-rail tab is
  // open — and `gitStates` only refreshes while the Git tab is mounted.
  const shortstats = useAppStore((s) => s.gitShortstats[agent.id] ?? null);

  // No branch until the first push (deferred branching) — drop the branch
  // segment entirely rather than showing a placeholder, leaving just the
  // diffstat and age (`+0 -0 · now`).
  const branch = agent.repos[0]?.branch ?? null;
  const age = formatAge(agent.created_at, now);

  return (
    <div className="center-h flex-center">
      <PanelToggle side="left" />

      <div className="task">
        <div className="t-name">
          <StatusDot tone={dotTone(agent.status)} />
          <span title={agentTitle(agent)}>{agentTitle(agent)}</span>
        </div>
        <div className="t-meta">
          {agent.name}
          {branch && <> · {branch}</>} · <DiffLabel stats={shortstats} />
          {age && <> · {age}</>}
        </div>
      </div>

      <AccountPicker agent={agent} trigger="header" />

      {nativeView && (
        <ViewToggle
          value={agent.view}
          onChange={(v) => switchView(agent.id, v)}
          disabled={switchInFlight}
          // The native TUI resumes the agent's session, which only exists once
          // the first turn lands (claude gets one up front, so it's never
          // gated). Matches the backend switch_view guard.
          //
          // A remote host answers no `switch_view` (the view is the structured
          // one there, which is also what it forces for phone spawns), so the
          // gate disables the same option with its own reason.
          nativeDisabled={!agent.session_id || nativeGate !== null}
          nativeReason={nativeGate ?? "Available after the agent's first turn"}
        />
      )}

      {/* Only meaningful while the terminal is on screen — in the custom view
          the transcript IS the pane. */}
      {nativeView && agent.view === "native" && (
        <IconButton
          active={railOpen}
          tip={railOpen ? "Hide transcript" : "Show transcript"}
          onClick={toggleRail}
        >
          <Icon name="sparkle" />
        </IconButton>
      )}

      {/* The "start a new thread of work" entry point, anchored on the whole
          conversation. Hides itself on a remote environment — see ForkMenu. */}
      <ForkMenu agentId={agent.id} turnId={null} tip="Fork this workspace and conversation" />

      <PanelToggle side="right" />
    </div>
  );
}

function DiffLabel({ stats }: { stats: DiffStats | null }) {
  const additions = stats ? String(stats.additions) : "--";
  const deletions = stats ? String(stats.deletions) : "--";
  const changed = Boolean(stats && (stats.additions > 0 || stats.deletions > 0));

  return (
    <span className={`t-diff ${changed ? "has-changes" : ""}`}>
      <span className="t-diff-add">+{additions}</span>{" "}
      <span className="t-diff-del">-{deletions}</span>
    </span>
  );
}

/** The header dot's vocabulary: live green, starting amber, failed red, else
 *  quiet grey. Shared with the thread header, which maps a thread's state
 *  onto the same four. */
export type DotTone = "running" | "spawning" | "error" | "idle";

function dotTone(status: AgentStatus): DotTone {
  return status === "running" || status === "spawning" || status === "error" ? status : "idle";
}

export function StatusDot({ tone }: { tone: DotTone }) {
  const bg =
    tone === "running"
      ? "var(--success)"
      : tone === "spawning"
        ? "var(--warn)"
        : tone === "error"
          ? "var(--danger)"
          : "var(--fg-3)";
  return (
    <span
      aria-hidden="true"
      style={{
        width: 7,
        height: 7,
        borderRadius: "50%",
        background: bg,
        boxShadow:
          tone === "running"
            ? "0 0 0 2px color-mix(in oklch, var(--success), transparent 78%)"
            : "none",
        flexShrink: 0,
      }}
    />
  );
}
