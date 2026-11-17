import { create } from "zustand";
import type { AgentRecord } from "@/api"; // for EMPTY_AGENTS
import { localEngine } from "@/api/engine";
import { createAccountSlice } from "./account";
import { createAgentInstallSlice } from "./agentInstall";
import { createAppSlice } from "./app";
import { createAppearanceSlice } from "./appearance";
import { createAutopilotSlice } from "./autopilot";
import { createBackgroundTasksSlice } from "./backgroundTasks";
import { createCompactionSlice } from "./compaction";
import { createComposerSlice } from "./composer";
import { createCustomAgentsSlice } from "./customAgents";
import { createDraftsSlice } from "./drafts";
import { createEnvironmentSwitchSlice } from "./environmentSwitch";
import {
  createEnvironmentsSlice,
  LOCAL_ENVIRONMENT_ID,
  setEnvironmentsSource,
} from "./environments";
import { createGitSlice } from "./git";
import { createHostScope, type HostStore, type SlicesCreator, useHostId } from "./hostScope";
import { createLocalCommandsSlice } from "./localCommands";
import { createMcpServersSlice } from "./mcpServers";
import { createProvidersSlice } from "./providers";
import { createReposSlice } from "./repos";
import { createReviewSlice } from "./review";
import { createRewindSlice } from "./rewind";
import { createSandboxSlice } from "./sandbox";
import { createSkillsSlice } from "./skills";
import type { AppState } from "./types";
import { createUiSlice } from "./ui";
import { createWorkspaceSlice } from "./workspace";
import { createWorkspaceTypesSlice } from "./workspaceTypes";

export const EMPTY_AGENTS: readonly AgentRecord[] = Object.freeze([]);

/** Every slice, driving `engine` through `set` / `get`: the store's own for this
 *  desktop, a paired host's scoped pair for its actions (./hostScope). */
const createSlices: SlicesCreator = (...a) => ({
  ...createAppSlice(...a),
  ...createEnvironmentsSlice(...a),
  ...createEnvironmentSwitchSlice(...a),
  ...createWorkspaceSlice(...a),
  ...createReposSlice(...a),
  ...createRewindSlice(...a),
  ...createGitSlice(...a),
  ...createComposerSlice(...a),
  ...createDraftsSlice(...a),
  ...createUiSlice(...a),
  ...createAccountSlice(...a),
  ...createAppearanceSlice(...a),
  ...createAutopilotSlice(...a),
  ...createReviewSlice(...a),
  ...createBackgroundTasksSlice(...a),
  ...createProvidersSlice(...a),
  ...createAgentInstallSlice(...a),
  ...createCustomAgentsSlice(...a),
  ...createSkillsSlice(...a),
  ...createMcpServersSlice(...a),
  ...createSandboxSlice(...a),
  ...createWorkspaceTypesSlice(...a),
  ...createLocalCommandsSlice(...a),
  ...createCompactionSlice(...a),
});

/** The store itself: this desktop's state at the top level, each paired host's
 *  in its partition. */
const store = create<AppState>()((set, get) => createSlices(set, get, localEngine));

export const { viewOf, hostStore, startHost, stopHost } = createHostScope(store, createSlices);

function useScopedAppStore(): AppState;
function useScopedAppStore<U>(selector: (state: AppState) => U): U;
function useScopedAppStore<U>(selector?: (state: AppState) => U) {
  const hostId = useHostId();
  if (hostId === LOCAL_ENVIRONMENT_ID) return store(selector as (state: AppState) => U);
  return store((state) => {
    const view = viewOf(state, hostId);
    return selector ? selector(view) : view;
  });
}

/** The app store, as the host the calling component is scoped to sees it
 *  (`EngineScope`). This desktop's scope is the store itself: the selector runs
 *  on the store's own state. `getState` / `setState` / `subscribe` are the
 *  store's own — this desktop's state; code outside React that acts for a
 *  paired host goes through `hostStore(hostId)`. */
export const useAppStore = Object.assign(useScopedAppStore, {
  getState: store.getState,
  getInitialState: store.getInitialState,
  setState: store.setState,
  subscribe: store.subscribe,
}) as typeof store;

/** How many agents finished a turn the user has not looked at, on this desktop
 *  and on every paired host: the window-wide count behind the app icon badge,
 *  whichever host is on screen. */
export function useUnseenCount(): number {
  return store((state) => {
    let count = Object.keys(state.unseenResults).length;
    for (const host of Object.values(state.hostStates)) {
      count += Object.keys(host.unseenResults).length;
    }
    return count;
  });
}

/** The `getState` / `setState` of the host the calling component is scoped to,
 *  for a read that must not subscribe (an event handler, an async callback).
 *  Stable for as long as the scope's host is. */
export function useHostStore(): HostStore {
  return hostStore(useHostId());
}

// The engine lookup and the host settings read the environments straight off
// the store, outside React (see ./environments).
setEnvironmentsSource(store.getState);

export type { ChatItem } from "@/adapters";
export type { BackgroundTask, BackgroundTaskMap } from "@/adapters/shared/backgroundTasks";
export {
  isSubagentTask,
  liveBackgroundTasks,
  QUIET_AFTER_MS,
  quietForMs,
  subagentActivity,
  taskForToolUse,
} from "@/adapters/shared/backgroundTasks";
export type { UsageSnapshot } from "@/adapters/usage";
export type { DraftAgent } from "./drafts";
export type { HostStore } from "./hostScope";
export { useHostId } from "./hostScope";
export type { AppState } from "./types";
