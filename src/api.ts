import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type AgentStatus =
  | "spawning"
  | "running"
  | "idle"
  | "stopped"
  | "error";

export type AgentView = "custom" | "native";

export interface AgentRecord {
  id: string;
  name: string;
  branch?: string | null;
  parent_branch?: string | null;
  task: string;
  status: AgentStatus;
  view: AgentView;
  session_id?: string | null;
  created_at: string;
  last_error?: string | null;
  status_message?: string | null;
  /** Most recent turn's `usage.input_tokens` — matches claude's
   *  `/context` figure. Only populated for custom-view turns. */
  context_tokens?: number | null;
}

export interface Workspace {
  repo_path: string;
  agents: AgentRecord[];
}

export interface AgentOutputEvent {
  agent_id: string;
  bytes: number[];
}

/** Raw stream-json event from claude in custom view. Shape varies by
 *  `type`; the UI pattern-matches. */
export interface AgentManagedEvent {
  agent_id: string;
  event: Record<string, unknown> & { type?: string };
}

export interface AgentStatusEvent {
  agent_id: string;
  status: AgentStatus;
  last_error: string | null;
  status_message?: string | null;
}

export interface AgentViewEvent {
  agent_id: string;
  view: AgentView;
}

export interface AgentTokensEvent {
  agent_id: string;
  context_tokens: number;
}

export interface AgentTaskEvent {
  agent_id: string;
  task: string;
}

export interface AgentBranchEvent {
  agent_id: string;
  branch: string;
}

export const api = {
  getWorkspace: () => invoke<Workspace | null>("get_workspace"),
  setRepo: (repoPath: string) => invoke<Workspace>("set_repo", { repoPath }),
  spawnAgent: (view: AgentView) =>
    invoke<AgentRecord>("spawn_agent", { view }),
  writeToAgent: (agentId: string, data: string) =>
    invoke<void>("write_to_agent", { agentId, data }),
  sendUserMessage: (agentId: string, text: string) =>
    invoke<void>("send_user_message", { agentId, text }),
  resizeAgent: (agentId: string, cols: number, rows: number) =>
    invoke<void>("resize_agent", { agentId, cols, rows }),
  switchView: (agentId: string, view: AgentView) =>
    invoke<void>("switch_view", { agentId, view }),
  resumeAgent: (agentId: string) =>
    invoke<void>("resume_agent", { agentId }),
  stopAgent: (agentId: string) => invoke<void>("stop_agent", { agentId }),
  discardAgent: (agentId: string) =>
    invoke<void>("discard_agent", { agentId }),
};

export function onAgentOutput(
  cb: (e: AgentOutputEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentOutputEvent>("agent:output", (event) => cb(event.payload));
}

export function onAgentEvent(
  cb: (e: AgentManagedEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentManagedEvent>("agent:event", (event) => cb(event.payload));
}

export function onAgentStatus(
  cb: (e: AgentStatusEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentStatusEvent>("agent:status", (event) => cb(event.payload));
}

export function onAgentView(
  cb: (e: AgentViewEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentViewEvent>("agent:view", (event) => cb(event.payload));
}

export function onAgentTokens(
  cb: (e: AgentTokensEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentTokensEvent>("agent:tokens", (event) =>
    cb(event.payload),
  );
}

export function onAgentTask(
  cb: (e: AgentTaskEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentTaskEvent>("agent:task", (event) => cb(event.payload));
}

export function onAgentBranch(
  cb: (e: AgentBranchEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentBranchEvent>("agent:branch", (event) =>
    cb(event.payload),
  );
}
