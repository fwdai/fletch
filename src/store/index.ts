import { create } from "zustand";
import type { AgentRecord } from "@/api"; // for EMPTY_AGENTS
import { createAccountSlice } from "./account";
import { createAgentInstallSlice } from "./agentInstall";
import { createAppSlice } from "./app";
import { createAppearanceSlice } from "./appearance";
import { createAutopilotSlice } from "./autopilot";
import { createAutopilotLogSlice } from "./autopilotLog";
import { createBackgroundTasksSlice } from "./backgroundTasks";
import { createComposerSlice } from "./composer";
import { createCustomAgentsSlice } from "./customAgents";
import { createDraftsSlice } from "./drafts";
import { createEnvironmentSwitchSlice } from "./environmentSwitch";
import { createEnvironmentsSlice, setEnvironmentsSource } from "./environments";
import { createGitSlice } from "./git";
import { createLocalCommandsSlice } from "./localCommands";
import { createMcpServersSlice } from "./mcpServers";
import { createProvidersSlice } from "./providers";
import { createReposSlice } from "./repos";
import { createSandboxSlice } from "./sandbox";
import { createSkillsSlice } from "./skills";
import type { AppState } from "./types";
import { createUiSlice } from "./ui";
import { createWorkspaceSlice } from "./workspace";

export const EMPTY_AGENTS: readonly AgentRecord[] = Object.freeze([]);

export const useAppStore = create<AppState>()((...a) => ({
  ...createAppSlice(...a),
  ...createEnvironmentsSlice(...a),
  ...createEnvironmentSwitchSlice(...a),
  ...createWorkspaceSlice(...a),
  ...createReposSlice(...a),
  ...createGitSlice(...a),
  ...createComposerSlice(...a),
  ...createDraftsSlice(...a),
  ...createUiSlice(...a),
  ...createAccountSlice(...a),
  ...createAppearanceSlice(...a),
  ...createAutopilotSlice(...a),
  ...createAutopilotLogSlice(...a),
  ...createBackgroundTasksSlice(...a),
  ...createProvidersSlice(...a),
  ...createAgentInstallSlice(...a),
  ...createCustomAgentsSlice(...a),
  ...createSkillsSlice(...a),
  ...createMcpServersSlice(...a),
  ...createSandboxSlice(...a),
  ...createLocalCommandsSlice(...a),
}));

// The transport lookup and the PTY buffers read the active environment straight
// off the store, outside React (see ./environments).
setEnvironmentsSource(useAppStore.getState);

export type { ChatItem } from "@/adapters";
export type { BackgroundTask, BackgroundTaskMap } from "@/adapters/shared/backgroundTasks";
export {
  isSubagentTask,
  liveBackgroundTasks,
  quietForMs,
  subagentActivity,
} from "@/adapters/shared/backgroundTasks";
export type { UsageSnapshot } from "@/adapters/usage";
export type { DraftAgent } from "./drafts";
export type { AppState } from "./types";
