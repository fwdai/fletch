import { EMPTY_AGENTS, useAppStore } from "../../store";
import { Icon } from "../Icon";
import { IconButton } from "../ui/IconButton";
import { Composer } from "../Composer";
import { WorkspaceHeader } from "./WorkspaceHeader";
import { ChatView } from "./ChatView";
import { NativeView, useNativeSend } from "./NativeView";
import { EmptyWorkspace } from "./EmptyWorkspace";
import type { AgentRecord } from "../../api";

/** Center pane orchestrator. Decides whether to show: a draft empty
 *  state, the chat view, the native xterm view, or a fallback
 *  placeholder. Listens to the global `viewMode` from the store. */
export function Workspace() {
  const workspace = useAppStore((s) => s.workspace);
  const agents = useAppStore((s) => s.workspace?.agents ?? EMPTY_AGENTS);
  const selectedId = useAppStore((s) => s.selectedAgentId);
  const drafts = useAppStore((s) => s.drafts);
  const activeDraftId = useAppStore((s) => s.activeDraftId);
  const viewMode = useAppStore((s) => s.viewMode);
  const leftCollapsed = useAppStore((s) => s.leftCollapsed);
  const toggleLeft = useAppStore((s) => s.toggleLeft);

  const draft = activeDraftId ? drafts.find((d) => d.id === activeDraftId) : null;
  if (draft) return <EmptyWorkspace draft={draft} />;

  const agent = agents.find((a) => a.id === selectedId);
  if (!workspace || !agent) {
    return (
      <div className="pane center">
        <div className="center-h">
          <IconButton
            tip={leftCollapsed ? "Show sidebar (⌘B)" : "Hide sidebar (⌘B)"}
            onClick={toggleLeft}
          >
            <Icon name="sidebarL" />
          </IconButton>
        </div>
        <Placeholder
          title={!workspace ? "Loading…" : agents.length === 0 ? "No agents yet" : "Pick an agent"}
          body={
            !workspace
              ? "Connecting to amux…"
              : agents.length === 0
                ? "Click the + button on a project to spawn one, or add a repo from the sidebar."
                : "Choose an agent from the sidebar to attach."
          }
        />
      </div>
    );
  }

  return (
    <div className="pane center fade-in" key={agent.id}>
      <WorkspaceHeader agent={agent} />
      {viewMode === "native" ? (
        <NativeBody agent={agent} />
      ) : (
        <ChatView agent={agent} />
      )}
    </div>
  );
}

function NativeBody({ agent }: { agent: AgentRecord }) {
  // Wraps NativeView + a Composer below it that writes to the PTY.
  // Native and custom share the composer shell so the bottom of the
  // window always feels the same.
  const { canSend, send } = useNativeSend(agent);
  return (
    <div className="chat" style={{ background: "#1a1c20" }}>
      <NativeView agent={agent} />
      <div className="composer-wrap" style={{ background: "var(--bg-1)" }}>
        <Composer
          defaultProvider="claude"
          disabled={!canSend}
          placeholder={canSend ? "Message claude — ⌘↵ to send" : "Agent is not ready"}
          onSend={({ text }) => send(text)}
        />
      </div>
    </div>
  );
}

function Placeholder({ title, body }: { title: string; body: string }) {
  return (
    <div className="empty-msg" style={{ margin: "auto", maxWidth: 320 }}>
      <div className="et">{title}</div>
      <div>{body}</div>
    </div>
  );
}
