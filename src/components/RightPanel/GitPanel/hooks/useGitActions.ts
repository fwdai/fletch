import { open } from "@tauri-apps/plugin-shell";
import { useCallback } from "react";
import type { PrChecks, PrComment, PrComments } from "@/api";
import { formatCommentForChat } from "@/components/RightPanel/prComments";
import { useAppStore } from "@/store";
import type { Gate } from "../actionGate";

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
  /** Target repo of a multi-repo agent (a checkout's `TrackedRepo.subdir`).
   *  Undefined = the primary repo — byte-identical to the old behavior. */
  subdir?: string;
  base: string;
  hasBranch: boolean;
  customActive: boolean;
  msg: string;
  checks: PrChecks | null;
  comments: PrComments | null;
  /** The PR the lane in view is about: its page, and its number for `focus-pr`. */
  prUrl: string | undefined;
  prNumber: number | undefined;
  /** The one gate (`actionGate.ts`), re-run here so no caller of the dispatch
   *  — a stale menu, the composer's shortcut — reaches the host past it. */
  gate: (key: string) => Gate | null;
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
    subdir,
    base,
    hasBranch,
    customActive,
    msg,
    checks,
    comments,
    prUrl,
    prNumber,
    gate,
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
  const clearCheckoutConfig = useAppStore((s) => s.clearCheckoutConfig);
  const deleteBranch = useAppStore((s) => s.deleteBranch);
  const delegateAction = useAppStore((s) => s.delegateAction);
  const focusPr = useAppStore((s) => s.focusPr);
  const seedComposer = useAppStore((s) => s.seedComposer);
  const openGithubConnect = useAppStore((s) => s.openGithubConnect);

  // Hand control to the coding agent: it writes the judgment part (message /
  // description / conflict edits) and executes the mutation through the app's
  // file RPC. The host composes the `[app-action]` trigger — scoped to this
  // section's repo with `repo="<subdir>"` — and watches the delegation to its
  // end; the panel renders what it reports.
  const delegate = useCallback(
    (action: string, params?: Record<string, string>) => {
      void delegateAction(agentId, action, params, subdir);
    },
    [agentId, subdir, delegateAction],
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

    // The same gate the footer rendered with: a closed one says why instead
    // of dispatching. Quiet gates (loading, a delegation in flight) are already
    // on the status slot.
    const closed = gate(key);
    if (closed) {
      if (!closed.quiet) showNotice(closed.reason);
      return;
    }

    switch (key) {
      case "focus-pr":
        // The one place the panel moves the host's focus: to the PR of the
        // lane in view, so its actions and autopilot follow it.
        if (prNumber != null) void focusPr(agentId, prNumber, subdir);
        break;
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
        delegate("commit-pr", { base });
        break;
      case "agent-commit":
        delegate("commit");
        break;
      case "agent-commit-push":
        delegate("commit-push");
        break;
      case "agent-open-pr":
        delegate("open-pr", { base });
        break;
      case "agent-resolve":
        delegate("resolve-conflicts");
        break;
      case "agent-update-branch":
        // PR can't merge cleanly with the base (the base advanced). This is NOT
        // a local in-progress merge — the agent must sync the base in first.
        delegate("update-branch", { base });
        break;
      case "agent-fix":
        delegate("fix-checks", { failing: (checks?.required_failing ?? []).join(", ") });
        break;
      case "agent-resolve-comments": {
        // The threads waiting on us — not the ones we pushed back on, which a
        // person has to settle (the same split `readiness.ts` makes).
        const waiting = (comments?.unresolved ?? []).filter((t) => !t.we_replied_last).length;
        delegate("resolve-comments", { count: String(waiting) });
        break;
      }
      // ── direct, agent bypassed (user typed their own message) ──
      case "commit-direct":
        if (!customActive) {
          openOverride();
          return;
        }
        void runBusy("Committing…", async () => {
          const ok = await commitChanges(agentId, msg.trim(), subdir);
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
            const ok = await commitChanges(agentId, msg.trim(), subdir);
            if (ok) {
              revertOverride();
              delegate("open-pr", { base });
            }
          });
          break;
        }
        void runBusy("Committing & opening PR…", async () => {
          const ok = await commitAndOpenPr(agentId, msg.trim(), subdir);
          if (ok) revertOverride();
        });
        break;
      case "open-pr":
        // Needs a branch — hand to the agent to name + create one if there
        // isn't one yet; otherwise the direct gh --fill PR.
        if (!hasBranch) {
          delegate("open-pr", { base });
          break;
        }
        void runBusy("Opening PR…", async () => {
          const pr = await createPr(agentId, "", "", subdir);
          // If creation failed (e.g. a PR already exists), the local prState
          // was stale — re-fetch so the panel corrects itself.
          if (!pr) await fetchPrState(agentId);
        });
        break;
      case "view-pr":
        if (prUrl) void open(prUrl);
        break;
      case "merge":
        void runBusy("Merging…", () => mergePr(agentId, subdir));
        break;
      case "archive":
        void runBusy("Archiving…", () => archive(agentId));
        break;
      case "push":
        // Direct git push needs a branch; with none yet, the agent names and
        // creates one, then pushes.
        if (!hasBranch) {
          delegate("push");
          break;
        }
        void runBusy("Pushing…", async () => {
          const r = await pushAgent(agentId, subdir);
          if (r)
            showNotice(r === "up-to-date" ? "Already up to date with origin" : "Pushed to origin");
        });
        break;
      case "pull":
        void runBusy("Pulling…", async () => {
          if (await pullAgent(agentId, subdir)) showNotice("Pulled latest changes");
        });
        break;
      case "rebase":
        void runBusy("Rebasing…", async () => {
          if (await rebaseAgent(agentId, subdir)) showNotice(`Rebased onto ${base}`);
        });
        break;
      case "stash":
        void runBusy("Stashing…", () => stashChanges(agentId, subdir));
        break;
      case "discard":
        void runBusy("Discarding…", () => discardChanges(agentId, subdir));
        break;
      case "abort":
        void runBusy("Aborting…", () => abortMerge(agentId, subdir));
        break;
      case "clear-config":
        void runBusy("Removing settings…", async () => {
          if (await clearCheckoutConfig(agentId, subdir)) showNotice("Git settings removed");
        });
        break;
      case "delete-branch":
        void runBusy("Deleting branch…", () => deleteBranch(agentId, subdir));
        break;
      // "loading" is a non-actionable placeholder.
      default:
        break;
    }
  }

  return { runAction, addCommentToChat };
}
