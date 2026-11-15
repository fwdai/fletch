import type { AgentRecord } from "@/api";
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
 *  conversation as a whole — the Custom/Native view switcher, the context
 *  badge — then the right-panel toggle. Where the conversation runs (project,
 *  workspace, git) is the title bar's to say, and whether the agent is working
 *  is said by the sidebar row, the title bar and the composer. A sub-agent
 *  thread's header (SubagentThread/ThreadHeader) keeps this exact skeleton —
 *  one line, led by the title — so stepping into and out of a thread moves
 *  nothing but the words. */
interface Props {
  agent: AgentRecord;
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

  return (
    <div className="center-h flex-center">
      <PanelToggle side="left" />

      <div className="task">
        <div className="t-name">
          <span title={agentTitle(agent)}>{agentTitle(agent)}</span>
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
