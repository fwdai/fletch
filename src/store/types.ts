// Composes the store's `AppState` from the per-slice interfaces, which now live
// alongside their implementations (store/app.ts, store/workspace.ts, …). This
// module re-exports every slice interface (plus the small shared shapes) so the
// long-standing `import type { XSlice } from "./types"` paths — internal and
// external — keep resolving unchanged. Type-only imports are erased at compile
// time, so the slice↔types cycle carries no runtime dependency.

import type { StateCreator } from "zustand";
import type { AccountSlice } from "./account";
import type { AppSlice } from "./app";
import type { AppearanceSlice } from "./appearance";
import type { ComposerSlice } from "./composer";
import type { CustomAgentsSlice } from "./customAgents";
import type { DraftsSlice } from "./drafts";
import type { GitSlice } from "./git";
import type { LocalCommandsSlice } from "./localCommands";
import type { McpServersSlice } from "./mcpServers";
import type { ProvidersSlice } from "./providers";
import type { ReposSlice } from "./repos";
import type { DockerBuildProgress, SandboxSlice } from "./sandbox";
import type { SkillsSlice } from "./skills";
import type { RightPanelTab, UiSlice } from "./ui";
import type { PromoteSeed, SyncHealthInfo, WorkspaceSlice } from "./workspace";

export type {
  AccountSlice,
  AppearanceSlice,
  AppSlice,
  ComposerSlice,
  CustomAgentsSlice,
  DockerBuildProgress,
  DraftsSlice,
  GitSlice,
  LocalCommandsSlice,
  McpServersSlice,
  PromoteSeed,
  ProvidersSlice,
  ReposSlice,
  RightPanelTab,
  SandboxSlice,
  SkillsSlice,
  SyncHealthInfo,
  UiSlice,
  WorkspaceSlice,
};

export type AppState = AppSlice &
  WorkspaceSlice &
  ReposSlice &
  GitSlice &
  ComposerSlice &
  DraftsSlice &
  UiSlice &
  AccountSlice &
  AppearanceSlice &
  ProvidersSlice &
  CustomAgentsSlice &
  SkillsSlice &
  McpServersSlice &
  SandboxSlice &
  LocalCommandsSlice;

export type SliceCreator<T> = StateCreator<AppState, [], [], T>;
