import { useState } from "react";
import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { providerLabel } from "@/data/providers";
import { EMPTY_AGENTS, useAppStore } from "@/store";
import { RunView } from "@/workflows/run/RunView";
import { ChatView } from "./ChatView";
import { EmptyWorkspace } from "./EmptyWorkspace";
import { MissionControl } from "./MissionControl";
import { NativeView } from "./NativeView";
import { WorkspaceHeader } from "./WorkspaceHeader";

/** Center pane orchestrator. Decides whether to show: a draft empty
 *  state, the chat view, the native xterm view, or a fallback
 *  placeholder. Listens to the global `viewMode` from the store. */
export function Workspace() {
  const workspace = useAppStore((s) => s.workspace);
  const agents = useAppStore((s) => s.workspace?.agents ?? EMPTY_AGENTS);
  const selectedId = useAppStore((s) => s.selectedAgentId);
  const selectedRunId = useAppStore((s) => s.selectedRunId);
  const drafts = useAppStore((s) => s.drafts);
  const activeDraftId = useAppStore((s) => s.activeDraftId);
  const leftCollapsed = useAppStore((s) => s.leftCollapsed);
  const toggleLeft = useAppStore((s) => s.toggleLeft);

  const draft = activeDraftId ? drafts.find((d) => d.id === activeDraftId) : null;
  if (draft) return <EmptyWorkspace draft={draft} key={draft.id} />;

  // A selected workflow run takes the center pane.
  if (selectedRunId) {
    return <RunView id={selectedRunId} key={selectedRunId} />;
  }

  // No agent selected → Home is Mission Control: the fleet review queue. The
  // bare "Loading…" placeholder stands only until the workspace connects.
  const agent = agents.find((a) => a.id === selectedId);
  if (!workspace) {
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
        <Placeholder title="Loading…" body="Connecting to Fletch…" />
      </div>
    );
  }
  if (!agent) return <MissionControl />;

  return (
    <div className="pane center fade-in" key={agent.id}>
      <WorkspaceHeader agent={agent} />
      {agent.status === "error" && <CrashBanner agent={agent} />}
      <SyncHealthBanner agentId={agent.id} />
      {agent.view === "native" ? <NativeBody agent={agent} /> : <ChatView agent={agent} />}
    </div>
  );
}

/** Classify a docker agent's crash reason so the banner can offer the right
 *  recovery action. Keys off the backend's own error strings (engine exit-code
 *  mapping + launch/build errors in sandbox/docker, and the D1 auth CTA):
 *   - "docker-down": daemon stopped / not installed / build couldn't connect →
 *     "Start Docker Desktop".
 *   - "image":       a bad image (missing/broken `claude`, exit 126/127) →
 *     point at the sandbox settings to fix `docker_image`.
 *   - "auth":        no Anthropic credentials in the container → connect Claude.
 *  Order matters: daemon-down is checked first (its message also mentions the
 *  image). Returns null for non-docker crashes (the plain banner). */
function dockerCrashKind(msg: string | null | undefined): "docker-down" | "image" | "auth" | null {
  if (!msg) return null;
  const m = msg.toLowerCase();
  if (
    m.includes("docker desktop") ||
    m.includes("docker daemon") ||
    m.includes("docker isn't running") ||
    m.includes("docker binary not found") ||
    m.includes("is docker installed") ||
    m.includes("preparing the docker sandbox image failed")
  ) {
    return "docker-down";
  }
  if (
    m.includes("connect claude for containers") ||
    m.includes("setup-token") ||
    m.includes("no anthropic credentials") ||
    m.includes("not authenticated")
  ) {
    return "auth";
  }
  if (m.includes("docker_image") || m.includes("sandbox image")) return "image";
  return null;
}

/** Shown when an agent's process exited with an error. Surfaces the crash
 *  reason (otherwise only a red dot in the sidebar) and a Resume action —
 *  both already captured on the record / available in the store, just never
 *  rendered. Sits under the header so the transcript leading up to the crash
 *  stays visible. Docker crashes additionally get a targeted recovery action
 *  (start the daemon / fix the image / connect Claude). */
function CrashBanner({ agent }: { agent: AgentRecord }) {
  const resume = useAppStore((s) => s.resume);
  const startDockerDesktop = useAppStore((s) => s.startDockerDesktop);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);
  const dockerKind = dockerCrashKind(agent.last_error);
  // Guard against a double-click firing two concurrent resumes: status stays
  // "error" until the backend's status event lands, so the banner (and button)
  // linger through that window. `resume` catches internally, so on success the
  // banner unmounts and on failure we re-enable. (Harmless no-op if it unmounts
  // before `finally` runs.)
  const [resuming, setResuming] = useState(false);
  return (
    <div className="crash-banner flex-center" role="alert">
      <div className="crash-text">
        <span className="crash-title">
          {dockerKind === "docker-down"
            ? "Docker isn't available"
            : dockerKind === "image"
              ? "Sandbox image problem"
              : dockerKind === "auth"
                ? "Claude isn't connected for containers"
                : "Agent stopped unexpectedly"}
        </span>
        {agent.last_error && <span className="crash-detail">{agent.last_error}</span>}
      </div>
      {dockerKind === "docker-down" && (
        <Button variant="outline" onClick={() => void startDockerDesktop()}>
          <Icon name="play" size={12} />
          Start Docker Desktop
        </Button>
      )}
      {dockerKind === "image" && (
        <Button variant="outline" onClick={() => openSettingsScreen("experimental")}>
          <Icon name="settings" size={12} />
          Sandbox settings
        </Button>
      )}
      {dockerKind === "auth" && (
        <Button variant="outline" onClick={() => openSettingsScreen("general")}>
          <Icon name="cube" size={12} />
          Connect Claude for containers
        </Button>
      )}
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

/** Non-blocking notice that this session's on-disk transcript couldn't be read
 *  at turn-end — the vendor CLI moved its files (`no_root`), reshaped them
 *  (`format_drift`), they were unreadable (`read_error`), or a read failed
 *  partway so the tail may be missing (`partial_read`) — newly-written history
 *  may not persist. Worded as degraded, not broken: the app still renders the
 *  turn from its live-compiled stream. Only present while the store holds a
 *  degraded status for the agent (cleared by a `healthy` sync-health event). */
function SyncHealthBanner({ agentId }: { agentId: string }) {
  const health = useAppStore((s) => s.syncHealth[agentId]);
  if (!health) return null;
  const provider = providerLabel(health.provider);
  const partial = health.status === "partial_read";
  return (
    <div className="drift-banner flex-center" role="status">
      <div className="crash-text">
        <span className="drift-title">
          {partial ? "Couldn't read some chat history" : "Couldn't read chat history"}
        </span>
        <span className="crash-detail">
          {partial
            ? `Fletch could only read part of this session's history from the ${provider} CLI — some new history may not be saved. The conversation you see is still up to date.`
            : `Fletch couldn't read this session's history from the ${provider} CLI — new history for this session may not be saved. The conversation you see is still up to date.`}
        </span>
      </div>
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
