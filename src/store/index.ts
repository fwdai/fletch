import { create } from "zustand";
import type { AgentRecord } from "@/api"; // for EMPTY_AGENTS
import { createAccountSlice } from "./account";
import { createAppSlice } from "./app";
import { createAppearanceSlice } from "./appearance";
import { createComposerSlice } from "./composer";
import { createCustomAgentsSlice } from "./customAgents";
import { createDraftsSlice } from "./drafts";
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
  ...createWorkspaceSlice(...a),
  ...createReposSlice(...a),
  ...createGitSlice(...a),
  ...createComposerSlice(...a),
  ...createDraftsSlice(...a),
  ...createUiSlice(...a),
  ...createAccountSlice(...a),
  ...createAppearanceSlice(...a),
  ...createProvidersSlice(...a),
  ...createCustomAgentsSlice(...a),
  ...createSkillsSlice(...a),
  ...createMcpServersSlice(...a),
  ...createSandboxSlice(...a),
  ...createLocalCommandsSlice(...a),
}));

export type { ChatItem } from "@/adapters";
export type { AgentUsage } from "@/adapters/usage";
export type { DraftAgent } from "./drafts";
export type { AppState } from "./types";
