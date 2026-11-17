import { agentsApi } from "./domains/agents";
import { boxesApi } from "./domains/boxes";
import { commandsApi } from "./domains/commands";
import { contextApi } from "./domains/context";
import { dictationApi } from "./domains/dictation";
import { filesApi } from "./domains/files";
import { gitApi } from "./domains/git";
import { githubApi } from "./domains/github";
import { issuesApi } from "./domains/issues";
import { miscApi } from "./domains/misc";
import { providersApi } from "./domains/providers";
import { remoteApi } from "./domains/remote";
import { reviewApi } from "./domains/review";
import { roadmapApi } from "./domains/roadmap";
import { runApi } from "./domains/run";
import { sandboxApi } from "./domains/sandbox";
import { sessionApi } from "./domains/session";
import { settingsApi } from "./domains/settings";
import { shellApi } from "./domains/shell";
import { tourApi } from "./domains/tour";
import { usageApi } from "./domains/usage";
import { workflowsApi } from "./domains/workflows";
import { workspaceApi } from "./domains/workspace";
import { type Invoke, invoke } from "./invoke";

// Re-export `makeEvents` and the active environment's `on*` subscriptions.
export * from "./events";
// Re-export every DTO/type so `import type { … } from "@/api"` keeps working.
export * from "./types/agent";
export * from "./types/checkout";
export * from "./types/commands";
export * from "./types/context";
export * from "./types/dictation";
export * from "./types/git";
export * from "./types/issues";
export * from "./types/pr";
export * from "./types/providers";
export * from "./types/remote";
export * from "./types/review";
export * from "./types/roadmap";
export * from "./types/run";
export * from "./types/sandbox";
export * from "./types/session";
export * from "./types/settings";
export * from "./types/tour";
export * from "./types/usage";
export * from "./types/verify";
export * from "./types/workflow";

/** One `api` facade over `invoke`, assembled from the per-domain command
 *  wrappers. Each domain contributes a disjoint set of method names, so the
 *  spread order never shadows a method. The routed domains are built over
 *  `invoke`; the ones that only ever address this desktop (see `invokeLocal`)
 *  take nothing and are spread as they are. */
export function makeApi(invoke: Invoke) {
  return {
    ...workspaceApi(invoke),
    ...miscApi,
    ...sandboxApi(invoke),
    ...boxesApi(invoke),
    ...githubApi(invoke),
    ...issuesApi(invoke),
    ...agentsApi(invoke),
    ...sessionApi(invoke),
    ...settingsApi(invoke),
    ...gitApi(invoke),
    ...shellApi(invoke),
    ...runApi(invoke),
    ...filesApi(invoke),
    ...commandsApi(invoke),
    ...dictationApi,
    ...providersApi,
    ...remoteApi,
    ...workflowsApi(invoke),
    ...roadmapApi(invoke),
    ...usageApi,
    ...contextApi(invoke),
    ...reviewApi(invoke),
    ...tourApi(invoke),
  };
}

export type Api = ReturnType<typeof makeApi>;

/** The `api` of the environment the UI is driving: every routed op resolves the
 *  active transport at call time (see ./invoke). */
export const api = makeApi(invoke);
