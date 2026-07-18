import { agentsApi } from "./domains/agents";
import { commandsApi } from "./domains/commands";
import { filesApi } from "./domains/files";
import { gitApi } from "./domains/git";
import { githubApi } from "./domains/github";
import { miscApi } from "./domains/misc";
import { providersApi } from "./domains/providers";
import { runApi } from "./domains/run";
import { sandboxApi } from "./domains/sandbox";
import { sessionApi } from "./domains/session";
import { shellApi } from "./domains/shell";
import { workflowsApi } from "./domains/workflows";
import { workspaceApi } from "./domains/workspace";

// Re-export every `on*` event-listener factory.
export * from "./events";
// Re-export every DTO/type so `import type { … } from "@/api"` keeps working.
export * from "./types/agent";
export * from "./types/checkout";
export * from "./types/commands";
export * from "./types/git";
export * from "./types/pr";
export * from "./types/providers";
export * from "./types/run";
export * from "./types/sandbox";
export * from "./types/session";
export * from "./types/verify";
export * from "./types/workflow";

/** The single `api` facade assembled from the per-domain command wrappers.
 *  Each domain contributes a disjoint set of method names, so the spread order
 *  never shadows a method. */
export const api = {
  ...workspaceApi,
  ...miscApi,
  ...sandboxApi,
  ...githubApi,
  ...agentsApi,
  ...sessionApi,
  ...gitApi,
  ...shellApi,
  ...runApi,
  ...filesApi,
  ...commandsApi,
  ...providersApi,
  ...workflowsApi,
};
