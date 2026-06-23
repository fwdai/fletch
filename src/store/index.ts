import { create } from "zustand";
import type { AppState } from "./types";
import { createAppSlice } from "./app";
import { createWorkspaceSlice } from "./workspace";
import { createReposSlice } from "./repos";
import { createGitSlice } from "./git";
import { createComposerSlice } from "./composer";
import { createDraftsSlice } from "./drafts";
import { createUiSlice } from "./ui";
import { createAccountSlice } from "./account";
import { createAppearanceSlice } from "./appearance";
import { createProvidersSlice } from "./providers";
import type { AgentRecord } from "../api"; // for EMPTY_AGENTS

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
}));

export type { AppState } from "./types";
export type { DraftAgent } from "./drafts";
export type { ChatItem } from "../adapters";
export type { AgentUsage } from "../adapters/usage";
