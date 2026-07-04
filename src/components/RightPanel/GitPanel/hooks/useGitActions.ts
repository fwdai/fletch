import { open } from "@tauri-apps/plugin-shell";
import { useCallback } from "react";
import type { PrChecks, PrComment } from "@/api";
import { appActionMessage, type GitDelegationKind } from "@/components/RightPanel/delegation";
import { formatCommentForChat } from "@/components/RightPanel/prComments";
import { useAppStore } from "@/store";

// Actions that push to / read from GitHub. In local/offline mode these are
// intercepted in `runAction` and replaced with connect-or-publish, so none of
// them reaches the backend only to fail. A compile-time constant — defined at
// module scope so it isn't reallocated on every render.
const NEEDS_GITHUB = new Set([
  "agent-commit-push",
  "agent-commit-pr",
  "agent-open-pr",
  "open-pr",
  "commit-pr-direct",
  "push",
  "merge",
]);

interface GitActionsCtx {
  agentId: string;
  base: string;
  hasBranch: boolean;
  customActive: boolean;
  msg: string;
  checks: PrChecks | null;
  prUrl: string | undefined;
  /** GitHub connection + origin presence — gate push/PR actions to a connect
   *  or publish prompt so nothing fails on click in local/offline mode. */
  githubConnected: boolean;
  hasOrigin: boolean;
  runBusy: (label: string, fn: () => Promise<unknown>) => Promise<unknown>;
  showNotice: (m: string) => void;
  openOverride: () => void;
  revertOverride: () => void;
  fetchPrState: (agentId: string) => unknown;
}

/** The panel's action layer: the single dispatch table every state's actions
 *  route through (split-button main click + menu), plus the two callbacks that
 *  hand control to the coding agent. Behavior is identical to the inline
 *  version — this hook just isolates the imperative orchestration from render. */
export function useGitActions(ctx: GitActionsCtx) {
  const {
    agentId,
    base,
    hasBranch,
    customActive,
    msg,
    checks,
    prUrl,
    githubConnected,
    hasOrigin,
    runBusy,
    showNotice,
    openOverride,
    revertOverride,
    fetchPrState,
  } = ctx;

  const pushAgent = useAppStore((s) => s.pushAgent);
  const pullAgent = useAppStore((s) => s.pullAgent);
  const rebaseAgent = useAppStore((s) => s.rebaseAgent);
  const createPr = useAppStore((s) => s.createPr);
  const mergePr = useAppStore((s) => s.mergePr);
  const publishAgent = useAppStore((s) => s.publishAgent);
  const archive = useAppStore((s) => s.archive);
  const commitChanges = useAppStore((s) => s.commitChanges);
  const commitAndOpenPr = useAppStore((s) => s.commitAndOpenPr);
  const stashChanges = useAppStore((s) => s.stashChanges);
  const discardChanges = useAppStore((s) => s.discardChanges);
  const abortMerge = useAppStore((s) => s.abortMerge);
  const deleteBranch = useAppStore((s) => s.deleteBranch);
  const delegateGitAction = useAppStore((s) => s.delegateGitAction);
  const seedComposer = useAppStore((s) => s.seedComposer);
  const openGithubConnect = useAppStore((s) => s.openGithubConnect);

  // Hand control to the coding agent: it writes the judgment part (message /
  // description / conflict edits) and executes the mutation through the app's
  // file RPC. The panel tracks the delegation until the matching transition.
  const delegate = useCallback(
    (kind: GitDelegationKind, prompt: string) => {
      delegateGitAction(agentId, kind, prompt);
    },
    [agentId, delegateGitAction],
  );

  // "→ chat" on a review comment: drop the formatted comment into this agent's
  // composer (not sent), so the user can edit and send it. Bots like Greptile
  // are inserted verbatim; human comments get a file/line wrapper.
  const addCommentToChat = useCallback(
    (c: PrComment) => {
      seedComposer(agentId, formatCommentForChat(c));
      showNotice("Added to chat — edit & send to the agent");
    },
    [agentId, seedComposer, showNotice],
  );

  // Single dispatch table for every action a state can offer — the split
  // button's main click and its menu both route through here by key.
  function runAction(key: string) {
    // Backstop: if a GitHub action is somehow selected while offline (e.g. a
    // sticky mode from when we were connected), route to the unblock path
    // instead of dispatching a call that would error.
    if (NEEDS_GITHUB.has(key) && !githubConnected) key = "connect-github";
    else if (NEEDS_GITHUB.has(key) && !hasOrigin) key = "publish";

    switch (key) {
      case "connect-github":
        // Start the OAuth device flow right here (in the app-level modal), so a
        // single click begins connecting instead of detouring through Settings.
        openGithubConnect();
        break;
      case "publish":
      case "publish-public": {
        const isPrivate = key === "publish";
        void runBusy("Publishing to GitHub…", async () => {
          const url = await publishAgent(agentId, isPrivate);
          if (url) showNotice(`Published ${isPrivate ? "private" : "public"} repo to GitHub`);
        });
        break;
      }
      // ── delegated to the coding agent (agent mode) ──
      // Each click sends a short `[app-action]` trigger; the full playbook
      // lives in the agent's injected instructions (git_actions.md), keeping
      // the chat free of boilerplate. Params carry only dynamic context.
      case "agent-commit-pr":
        delegate("commit-pr", appActionMessage("commit-pr", { base }));
        break;
      case "agent-commit":
        delegate("commit", appActionMessage("commit"));
        break;
      case "agent-commit-push":
        delegate("commit-push", appActionMessage("commit-push"));
        break;
      case "agent-open-pr":
        delegate("open-pr", appActionMessage("open-pr", { base }));
        break;
      case "agent-resolve":
        delegate("resolve", appActionMessage("resolve-conflicts"));
        break;
      case "agent-update-branch":
        // PR can't merge cleanly with the base (the base advanced). This is NOT
        // a local in-progress merge — the agent must sync the base in first.
        delegate("update-branch", appActionMessage("update-branch", { base }));
        break;
      case "agent-fix":
        delegate(
          "fix-checks",
          appActionMessage("fix-checks", { failing: (checks?.required_failing ?? []).join(", ") }),
        );
        break;
      // ── direct, agent bypassed (user typed their own message) ──
      case "commit-direct":
        if (!customActive) {
          openOverride();
          return;
        }
        void runBusy("Committing…", async () => {
          const ok = await commitChanges(agentId, msg.trim());
          if (ok) revertOverride();
        });
        break;
      case "commit-pr-direct":
        if (!customActive) {
          openOverride();
          return;
        }
        // No branch yet: commit the user's message directly (works on detached
        // HEAD), then let the agent name the branch and write the PR.
        if (!hasBranch) {
          void runBusy("Committing…", async () => {
            const ok = await commitChanges(agentId, msg.trim());
            if (ok) {
              revertOverride();
              delegate("open-pr", appActionMessage("open-pr", { base }));
            }
          });
          break;
        }
        void runBusy("Committing & opening PR…", async () => {
          const ok = await commitAndOpenPr(agentId, msg.trim());
          if (ok) revertOverride();
        });
        break;
      case "open-pr":
        // Needs a branch — hand to the agent to name + create one if there
        // isn't one yet; otherwise the direct gh --fill PR.
        if (!hasBranch) {
          delegate("open-pr", appActionMessage("open-pr", { base }));
          break;
        }
        void runBusy("Opening PR…", async () => {
          const pr = await createPr(agentId, "", "");
          // If creation failed (e.g. a PR already exists), the local prState
          // was stale — re-fetch so the panel corrects itself.
          if (!pr) await fetchPrState(agentId);
        });
        break;
      case "view-pr":
        if (prUrl) void open(prUrl);
        break;
      case "merge":
        void runBusy("Merging…", () => mergePr(agentId));
        break;
      case "archive":
        void runBusy("Archiving…", () => archive(agentId));
        break;
      case "push":
        // Direct git push needs a branch; with none yet, the agent names and
        // creates one, then pushes.
        if (!hasBranch) {
          delegate("push", appActionMessage("push"));
          break;
        }
        void runBusy("Pushing…", async () => {
          const r = await pushAgent(agentId);
          if (r)
            showNotice(r === "up-to-date" ? "Already up to date with origin" : "Pushed to origin");
        });
        break;
      case "pull":
        void runBusy("Pulling…", async () => {
          if (await pullAgent(agentId)) showNotice("Pulled latest changes");
        });
        break;
      case "rebase":
        void runBusy("Rebasing…", async () => {
          if (await rebaseAgent(agentId)) showNotice(`Rebased onto ${base}`);
        });
        break;
      case "stash":
        void runBusy("Stashing…", () => stashChanges(agentId));
        break;
      case "discard":
        void runBusy("Discarding…", () => discardChanges(agentId));
        break;
      case "abort":
        void runBusy("Aborting…", () => abortMerge(agentId));
        break;
      case "delete-branch":
        void runBusy("Deleting branch…", () => deleteBranch(agentId));
        break;
      // "loading" is a non-actionable placeholder.
      default:
        break;
    }
  }

  return { runAction, addCommentToChat };
}
