import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type BakeStage =
  | "cloning"
  | "booting"
  | "waiting_for_ssh"
  | "installing"
  | "finalizing"
  | "done"
  | "error";

export interface BakeProgress {
  stage: BakeStage;
  message: string;
  tail: string | null;
}

export type AgentStatus =
  | "spawning"
  | "running"
  | "idle"
  | "stopped"
  | "error";

export interface AgentRecord {
  id: string;
  name: string;
  branch: string;
  task: string;
  status: AgentStatus;
  created_at: string;
  last_error?: string | null;
}

export interface Workspace {
  repo_path: string;
  base_image: string;
  agents: AgentRecord[];
}

export interface AgentOutputEvent {
  agent_id: string;
  bytes: number[];
}

export interface AgentStatusEvent {
  agent_id: string;
  status: AgentStatus;
  last_error: string | null;
}

export const api = {
  getWorkspace: () => invoke<Workspace | null>("get_workspace"),
  setRepo: (repoPath: string, baseImage: string) =>
    invoke<Workspace>("set_repo", { repoPath, baseImage }),
  spawnAgent: (name: string, branch: string, task: string) =>
    invoke<AgentRecord>("spawn_agent", { name, branch, task }),
  writeToAgent: (agentId: string, data: string) =>
    invoke<void>("write_to_agent", { agentId, data }),
  resizeAgent: (agentId: string, cols: number, rows: number) =>
    invoke<void>("resize_agent", { agentId, cols, rows }),
  stopAgent: (agentId: string) => invoke<void>("stop_agent", { agentId }),
  discardWorktree: (agentId: string) =>
    invoke<void>("discard_worktree", { agentId }),
  getPublicKey: () => invoke<string>("get_public_key"),
  listBaseImages: () => invoke<string[]>("list_base_images"),
  bakeBaseImage: (imageName: string) =>
    invoke<void>("bake_base_image", { imageName }),
};

export function onBakeProgress(
  cb: (e: BakeProgress) => void,
): Promise<UnlistenFn> {
  return listen<BakeProgress>("bake:progress", (event) => cb(event.payload));
}

export function onAgentOutput(
  cb: (e: AgentOutputEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentOutputEvent>("agent:output", (event) => cb(event.payload));
}

export function onAgentStatus(
  cb: (e: AgentStatusEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentStatusEvent>("agent:status", (event) => cb(event.payload));
}
