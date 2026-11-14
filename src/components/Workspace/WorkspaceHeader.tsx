import type { AgentRecord, AgentStatus } from "@/api";
import { ContextBadge } from "@/components/ContextBadge";
import { Icon } from "@/components/Icon";
import { PanelToggle } from "@/components/PanelToggle";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { agentTitle } from "@/util/agentTitle";
import { ViewToggle } from "./ViewToggle";

/** Header strip above the workspace body: the conversation's own line. Houses
 *  the left-sidebar toggle, the conversation's title, and what belongs to the
 *  conversation as a whole — the turn navigator (`navSlot`), the Custom/Native
 *  view switcher, the context badge — then the right-panel toggle. Where the
 *  conversation runs (project, workspace, git) is the title bar's to say. A
 *  sub-agent thread's header (SubagentThread/ThreadHeader) keeps this exact
 *  skeleton — dot, then one line — so stepping into and out of a thread moves
 *  nothing but the words. */
interface Props {
  agent: AgentRecord;
  /** Receives the element the chat's turn navigator renders into (see
   *  `ChatNav`); omitted where no transcript drives one. */
  navSlot?: (el: HTMLDivElement | null) => void;
}

export function WorkspaceHeader({ agent, navSlot }: Props) {
  const nativeGate = useGate("nativeView");
  const switchView = useAppStore((s) => s.switchView);
  const switchInFlight = useAppStore((s) => s.switchInFlight[agent.id]);
  // Native view is gated behind an experimental flag: while it's off the
  // switcher is hidden, and Workspace renders any native-mode agent as chat so
  // it can't get stranded behind a missing toggle.
  const nativeView = useAppStore((s) => s.features.nativeView);
  const railOpen = useAppStore((s) => s.transcriptRailOpen);
  const toggleRail = useAppStore((s) => s.toggleTranscriptRail);

  return (
    <div className="center-h flex-center">
      <PanelToggle side="left" />

      <div className="task">
        <div className="t-name">
          <StatusDot tone={dotTone(agent.status)} />
          <span title={agentTitle(agent)}>{agentTitle(agent)}</span>
        </div>
      </div>

      {navSlot && <div className="center-h-slot" ref={navSlot} />}

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

      {/* Forking lives under each message (TurnFooter), where it has a point
          to fork from; the header carries what the conversation is using. */}
      <ContextBadge agent={agent} />

      <PanelToggle side="right" />
    </div>
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
