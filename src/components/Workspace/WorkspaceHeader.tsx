import { useEffect, useRef } from "react";
import type { AgentRecord, AgentStatus, DiffStats } from "@/api";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import { formatAge } from "@/util/format";
import { useMinuteClock } from "@/util/hooks";
import { ForkMenu, type ForkOption } from "./ForkMenu";
import { ViewToggle } from "./ViewToggle";

/** Workspace-level fork options — the "start a new thread of work" entry point,
 *  spanning both axes (git-action turns aside, there's no single message to
 *  anchor on here, so context is whole-conversation or none). */
const HEADER_FORK_OPTIONS: ForkOption[] = [
  {
    key: "full-clean",
    label: "Full history · clean workspace",
    code: "clean",
    context: { kind: "full" },
  },
  {
    key: "full-carry",
    label: "Full history · with current code",
    code: "carry",
    context: { kind: "full" },
  },
  {
    key: "fresh-carry",
    label: "Fresh chat · with current code",
    code: "carry",
    context: { kind: "none" },
  },
];

/** Header strip above the workspace body. Houses the left-sidebar
 *  toggle, the agent task + meta line, the Custom/Native view
 *  switcher, and the right-panel toggle. */
interface Props {
  agent: AgentRecord;
}

export function WorkspaceHeader({ agent }: Props) {
  const switchView = useAppStore((s) => s.switchView);
  const switchInFlight = useAppStore((s) => s.switchInFlight[agent.id]);
  // Native view is gated behind an experimental flag. While it's off, hide the
  // switcher entirely; and if an agent was left in native mode when the flag
  // flipped off, pull it back to the custom view so it can't get stranded.
  const nativeView = useAppStore((s) => s.features.nativeView);
  const leftCollapsed = useAppStore((s) => s.leftCollapsed);
  const rightCollapsed = useAppStore((s) => s.rightCollapsed);
  const toggleLeft = useAppStore((s) => s.toggleLeft);
  const toggleRight = useAppStore((s) => s.toggleRight);
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

  // Guard against an unbounded retry loop: switchInFlight is a dep, so a failed
  // switch (view stays "native", switchInFlight falls back to false) would
  // otherwise re-fire the effect forever. Attempt the forced switch at most
  // once per agent; reset the guard once it's no longer stranded (the switch
  // landed, or native got re-enabled) so a later off-flip can try again.
  const forcedCustomFor = useRef<string | null>(null);
  useEffect(() => {
    if (nativeView || agent.view !== "native") {
      forcedCustomFor.current = null;
      return;
    }
    if (switchInFlight || forcedCustomFor.current === agent.id) return;
    forcedCustomFor.current = agent.id;
    void switchView(agent.id, "custom");
  }, [nativeView, agent.view, agent.id, switchInFlight, switchView]);

  return (
    <div className="center-h flex-center">
      <IconButton
        tip={leftCollapsed ? "Show sidebar (⌘B)" : "Hide sidebar (⌘B)"}
        onClick={toggleLeft}
      >
        <Icon name="sidebarL" />
      </IconButton>

      <div className="task">
        <div className="t-name">
          <StatusDot status={agent.status} />
          <span>{agent.name}</span>
        </div>
        <div className="t-meta">
          {branch && <>{branch} · </>}
          <DiffLabel stats={shortstats} />
          {age && (
            <>
              {" "}
              · <span>{age}</span>
            </>
          )}
        </div>
      </div>

      {nativeView && (
        <ViewToggle
          value={agent.view}
          onChange={(v) => switchView(agent.id, v)}
          disabled={switchInFlight}
          // The native TUI resumes the agent's session, which only exists once
          // the first turn lands (claude gets one up front, so it's never
          // gated). Matches the backend switch_view guard.
          nativeDisabled={!agent.session_id}
          nativeReason="Available after the agent's first turn"
        />
      )}

      <ForkMenu
        agentId={agent.id}
        options={HEADER_FORK_OPTIONS}
        tip="Fork this workspace and conversation"
      />

      <IconButton
        active={!rightCollapsed}
        tip={rightCollapsed ? "Show panel (⌘/)" : "Hide panel (⌘/)"}
        onClick={toggleRight}
      >
        <Icon name="sidebarR" />
      </IconButton>
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

function StatusDot({ status }: { status: AgentStatus }) {
  const bg =
    status === "running"
      ? "var(--success)"
      : status === "spawning"
        ? "var(--warn)"
        : status === "error"
          ? "var(--danger)"
          : "var(--fg-3)";
  return (
    <span
      style={{
        width: 7,
        height: 7,
        borderRadius: "50%",
        background: bg,
        boxShadow:
          status === "running"
            ? "0 0 0 2px color-mix(in oklch, var(--success), transparent 78%)"
            : "none",
        flexShrink: 0,
      }}
    />
  );
}
