import type { AgentRecord } from "@/api";
import type { DraftAgent } from "./drafts";
import type { AppState } from "./types";

/** What is in front of the user, by precedence. The one answer for anything
 *  that acts on "the current thing" or has to stay out of a surface's way —
 *  the keyboard shortcuts, the title bar's Open in editor — so none of them
 *  keeps its own list of which screens cover which, and a new surface is a
 *  one-line change here rather than a hunt through the callers.
 *
 *  Mirrors what `App` and `Workspace` render, which is the order that has to
 *  agree with this one: the overlays first (onboarding sits over everything,
 *  History is a sheet over whatever is beneath), then the full-screen
 *  takeovers (settings, usage, the project page — mutually exclusive by the
 *  store's own rules), then the center pane's precedence (a draft, a workflow
 *  run, an agent, Home). */
export type Surface =
  | { kind: "onboarding" }
  | { kind: "history" }
  | { kind: "settings" }
  | { kind: "usage" }
  | { kind: "project"; repoPath: string }
  | { kind: "draft"; draft: DraftAgent }
  | { kind: "run"; runId: string }
  | { kind: "agent"; agent: AgentRecord }
  | { kind: "home" };

export function activeSurface(s: AppState): Surface {
  if (s.onboardingOpen) return { kind: "onboarding" };
  if (s.historyOpen) return { kind: "history" };
  if (s.settingsScreenOpen) return { kind: "settings" };
  if (s.usageScreenOpen) return { kind: "usage" };
  if (s.projectScreenRepoPath) return { kind: "project", repoPath: s.projectScreenRepoPath };
  // A draft only shows while its project is still pinned (see Workspace); one
  // stranded on a removed project falls through to whatever is selected.
  const draft = s.activeDraftId ? s.drafts.find((d) => d.id === s.activeDraftId) : undefined;
  if (draft && s.workspace?.projects.some((p) => p.path === draft.repoPath)) {
    return { kind: "draft", draft };
  }
  if (s.selectedRunId) return { kind: "run", runId: s.selectedRunId };
  const agent = s.workspace?.agents.find((a) => a.id === s.selectedAgentId);
  if (agent) return { kind: "agent", agent };
  return { kind: "home" };
}

/** The agent in front of the user, or null while anything else is — the guard
 *  for every action that would stop, archive, or open a checkout. */
export function surfaceAgent(s: AppState): AgentRecord | null {
  const surface = activeSurface(s);
  return surface.kind === "agent" ? surface.agent : null;
}

/** The project a project-level action targets: the one in front (a draft's,
 *  an agent's, the project page's own), else the one an agent was last
 *  started in, else the first pinned. */
export function surfaceRepoPath(s: AppState): string | undefined {
  const surface = activeSurface(s);
  const repos = s.workspace?.repos ?? [];
  const recent = s.lastRepoPath && repos.includes(s.lastRepoPath) ? s.lastRepoPath : undefined;
  const shown =
    surface.kind === "draft"
      ? surface.draft.repoPath
      : surface.kind === "agent"
        ? surface.agent.repos[0]?.repo_path
        : surface.kind === "project"
          ? surface.repoPath
          : undefined;
  return shown ?? recent ?? repos[0];
}
