import { useState } from "react";
import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { EMPTY_AGENTS, useAppStore } from "@/store";
import { ChatView } from "./ChatView";
import { EmptyWorkspace } from "./EmptyWorkspace";
import { NativeView } from "./NativeView";
import { WorkspaceHeader } from "./WorkspaceHeader";

/** Center pane orchestrator. Decides whether to show: a draft empty
 *  state, the chat view, the native xterm view, or a fallback
 *  placeholder. Listens to the global `viewMode` from the store. */
export function Workspace() {
  const workspace = useAppStore((s) => s.workspace);
  const agents = useAppStore((s) => s.workspace?.agents ?? EMPTY_AGENTS);
  const selectedId = useAppStore((s) => s.selectedAgentId);
  const drafts = useAppStore((s) => s.drafts);
  const activeDraftId = useAppStore((s) => s.activeDraftId);
  const leftCollapsed = useAppStore((s) => s.leftCollapsed);
  const toggleLeft = useAppStore((s) => s.toggleLeft);

  const draft = activeDraftId ? drafts.find((d) => d.id === activeDraftId) : null;
  if (draft) return <EmptyWorkspace draft={draft} key={draft.id} />;

  const agent = agents.find((a) => a.id === selectedId);
  if (!workspace || !agent) {
    return (
      <div className="pane center">
        <div className="center-h flex-center">
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
              ? "Connecting to Fletch…"
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
      {agent.status === "error" && <CrashBanner agent={agent} />}
      {agent.view === "native" ? <NativeBody agent={agent} /> : <ChatView agent={agent} />}
    </div>
  );
}

/** Shown when an agent's process exited with an error. Surfaces the crash
 *  reason (otherwise only a red dot in the sidebar) and a Resume action —
 *  both already captured on the record / available in the store, just never
 *  rendered. Sits under the header so the transcript leading up to the crash
 *  stays visible. */
function CrashBanner({ agent }: { agent: AgentRecord }) {
  const resume = useAppStore((s) => s.resume);
  // Guard against a double-click firing two concurrent resumes: status stays
  // "error" until the backend's status event lands, so the banner (and button)
  // linger through that window. `resume` catches internally, so on success the
  // banner unmounts and on failure we re-enable. (Harmless no-op if it unmounts
  // before `finally` runs.)
  const [resuming, setResuming] = useState(false);
  return (
    <div className="crash-banner flex-center" role="alert">
      <div className="crash-text">
        <span className="crash-title">Agent stopped unexpectedly</span>
        {agent.last_error && <span className="crash-detail">{agent.last_error}</span>}
      </div>
      <Button
        variant="outline"
        disabled={resuming}
        onClick={() => {
          setResuming(true);
          void resume(agent.id).finally(() => setResuming(false));
        }}
      >
        <Icon name="play" size={12} />
        {resuming ? "Resuming…" : "Resume"}
      </Button>
    </div>
  );
}

function NativeBody({ agent }: { agent: AgentRecord }) {
  return (
    <div className="chat" style={{ background: "#1a1c20" }}>
      <NativeView agent={agent} />
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
