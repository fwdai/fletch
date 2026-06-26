import { open } from "@tauri-apps/plugin-shell";
import { type ReactNode, type RefObject, useCallback, useEffect, useRef, useState } from "react";
import type {
  AgentRecord,
  CheckRun,
  FileStatus,
  GitState,
  MergeState,
  PrChecks,
  PrComment,
  PrComments,
  PrState,
} from "../../api";
import { useAppStore } from "../../store";
import { usePoll } from "../../util/hooks";
import { Icon, type IconName } from "../Icon";
import { IconButton } from "../ui/IconButton";
import { commentLocation, formatCommentForChat } from "./prComments";
import {
  appActionMessage,
  delegationDone,
  delegationLabel,
  delegationResolved,
  delegationStep,
  type GitDelegationKind,
} from "./delegation";
import {
  deriveState,
  isCommitAction,
  primaryFor,
  secondaryFor,
  type ActionTone,
  type GitPanelState,
} from "./primaryActions";

/** Status letter for the file badge — matches CSS `.gs.<kind>` selectors. */
function kindLabel(kind: FileStatus["kind"]): string {
  switch (kind) {
    case "modified":   return "M";
    case "added":      return "A";
    case "deleted":    return "D";
    case "renamed":    return "R";
    case "untracked":  return "?";
    case "conflicted": return "!";
    default:           return "?";
  }
}

/** One action as presented by the split button — the unified shape the main
 *  button, the menu, and the dispatcher all key off. */
interface SplitActionItem {
  key: string;
  label: string;
  icon: IconName;
  kbd?: string;
}

/** A small CSS spinner used in loading states (status line + busy CTA). */
function Spinner() {
  return <span className="git-spin" aria-hidden />;
}

/** A quiet inline link out to GitHub — accent text, underline on hover, ↗. */
function GitLink({ href, children }: { href: string; children: ReactNode }) {
  return (
    <button type="button" className="git-link" onClick={() => void open(href)}>
      {children}
      <Icon name="external" size={10} />
    </button>
  );
}

// ── Color-coded status header ─────────────────────────────────────
// The panel's at-a-glance state signal: a tinted strip whose color carries the
// state before any word is read. clean=green · uncommitted=amber · pushed/PR=
// blue · fixable (can't merge / conflicts)=orange · ready=green · merged=purple.
type HeaderKind = "clean" | "changes" | "info" | "att" | "ready" | "merged" | "neutral";

interface HeaderInfo {
  kind: HeaderKind;
  pill?: string;
  /** Primary mono text (branch, or PR phrase like "ready to merge"). */
  text: string;
  /** Trailing muted text after `text` (e.g. "← main"). */
  sub?: string;
  /** Show a leading status dot instead of a pill (clean state). */
  dot?: boolean;
  /** Show the +adds/−dels diff summary on the right (changes state). */
  diff?: boolean;
  /** Show a trailing ↗ link to the PR on GitHub. */
  ext?: boolean;
}

function describeHeader(
  state: GitPanelState,
  branch: string,
  base: string,
  pr: PrState | null,
  mergeState: MergeState | null,
  checksFailed: number,
): HeaderInfo {
  const n = pr?.number;
  switch (state) {
    case "loading":   return { kind: "neutral", text: "Loading…" };
    // With an open PR, keep its GitHub link reachable from the header even
    // while new uncommitted changes take over the panel.
    case "changes":   return { kind: "changes", pill: "Uncommitted", text: branch, diff: true, ext: pr?.state === "open" };
    case "pushed":    return { kind: "info", pill: "Pushed", text: branch };
    case "conflicts": return { kind: "att", pill: "Conflicts", text: branch, sub: `← ${base}` };
    case "pr-open": {
      // GitHub's combined merge gate (spec §7): the legitimate green "ready"
      // appears only on `clean`. Without checks data, fall back to
      // `mergeable` — which only means "no conflicts", never an all-clear.
      const pill = n != null ? `PR #${n}` : "PR";
      switch (mergeState) {
        case "clean":    return { kind: "ready", pill, text: "ready to merge", ext: true };
        case "unstable": return { kind: "changes", pill, text: "optional checks failing", ext: true };
        case "blocked":
          return { kind: "att", pill, text: checksFailed > 0 ? "checks failing" : "review required", ext: true };
        case "behind":   return { kind: "att", pill, text: `behind ${base}`, ext: true };
        case "dirty":    return { kind: "att", pill, text: `conflicts with ${base}`, ext: true };
        case "draft":    return { kind: "info", pill, text: "draft", ext: true };
        case "unknown":
        case "has_hooks":
          return { kind: "info", pill, text: "checking…", ext: true };
        default:
          return pr?.mergeable
            ? { kind: "info", pill, text: "no conflicts", ext: true }
            : { kind: "att", pill, text: "can’t merge yet", ext: true };
      }
    }
    case "pr-closed": return { kind: "neutral", pill: "Closed", text: n != null ? `#${n}` : "—", ext: true };
    case "merged":    return { kind: "merged", pill: "Merged", text: n != null ? `#${n} → ${base}` : `→ ${base}`, ext: true };
    default:          return { kind: "clean", text: branch, sub: `← ${base}`, dot: true };
  }
}

function StatusHeader({
  state,
  branch,
  base,
  git,
  pr,
  mergeState,
  checksFailed,
}: {
  state: GitPanelState;
  branch: string;
  base: string;
  git: GitState | null;
  pr: PrState | null;
  mergeState: MergeState | null;
  checksFailed: number;
}) {
  const h = describeHeader(state, branch, base, pr, mergeState, checksFailed);
  const adds = git?.additions ?? 0;
  const dels = git?.deletions ?? 0;
  return (
    <div className={`git-hdr k-${h.kind}`}>
      {h.dot && <span className="hdr-dot" />}
      {h.pill && <span className="pill">{h.pill}</span>}
      <span className="bn">{h.text}</span>
      {h.sub && <span className="base">{h.sub}</span>}
      <div className="hdr-meta">
        {h.diff && (adds > 0 || dels > 0) && (
          <span className="hdr-diff">
            {adds > 0 && <span className="add">+{adds}</span>}
            {dels > 0 && <span className="rem">−{dels}</span>}
          </span>
        )}
        {h.ext && pr?.url && (
          <button
            type="button"
            className="hdr-ext tip"
            data-tip="View on GitHub"
            aria-label="View on GitHub"
            onClick={() => void open(pr.url)}
          >
            <Icon name="external" size={13} />
          </button>
        )}
      </div>
    </div>
  );
}

// ── Split action button ───────────────────────────────────────────
// A split button with a *selectable* default: the main button shows the
// currently-selected action and runs it on click; the caret opens a menu of
// every action for this state. Picking a menu item only changes which action
// the main button will perform — it does NOT execute. The state's primary is
// tagged "default"; the active selection is highlighted. The menu opens
// upward, since the button is pinned to the panel footer.
function SplitAction({
  items,
  selectedKey,
  tone,
  mainDisabled,
  busyLabel,
  onSelect,
  onRun,
}: {
  items: SplitActionItem[];
  selectedKey: string;
  tone: ActionTone;
  mainDisabled: boolean;
  busyLabel: string | null;
  onSelect: (key: string) => void;
  onRun: () => void;
}) {
  const [open, setOpen] = useState(false);
  const selected = items.find((a) => a.key === selectedKey) ?? items[0];
  const hasMenu = items.length > 1;
  const busy = busyLabel != null;
  if (!selected) return null;

  const toneClass = tone !== "accent" ? tone : "";

  return (
    <div className={`git-split ${toneClass} ${busy ? "busy" : ""}`}>
      <button className="gsa-main" disabled={mainDisabled || busy} onClick={onRun}>
        {busy ? <Spinner /> : <Icon name={selected.icon} />}
        <span className="gsa-label">{busy ? busyLabel : selected.label}</span>
      </button>
      {hasMenu && (
        <button
          className="gsa-caret tip"
          data-tip="Choose action"
          aria-label="Choose action"
          disabled={busy}
          onClick={() => setOpen((v) => !v)}
        >
          <Icon name="chevU" />
        </button>
      )}
      {open && (
        <>
          <div style={{ position: "fixed", inset: 0, zIndex: 199 }} onClick={() => setOpen(false)} />
          <div className="dd gsa-menu">
            {items.map((a) => (
              <div
                key={a.key}
                className={`dd-item ${a.key === selectedKey ? "active" : ""}`}
                onClick={() => {
                  onSelect(a.key);
                  setOpen(false);
                }}
              >
                <div className="di-i"><Icon name={a.icon} size={12} /></div>
                <span className="di-l">{a.label}</span>
                {a.kbd && <span className="di-m">{a.kbd}</span>}
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}

// ── Commit message composer ───────────────────────────────────────
// Lives in one fixed slot directly above the status + action. By default
// (agent mode) it's collapsed to a quiet one-liner explaining the agent will
// write the message + PR, with an inline "Write it yourself" opt-in. Clicking
// it expands a textarea IN PLACE — the note collapses and the field grows in a
// single smooth animation (CSS grid-rows). Typing makes a direct commit that
// bypasses the agent; "Let agent write it" collapses back.
function CommitComposer({
  writing,
  msg,
  setMsg,
  textareaRef,
  onOpen,
  onRevert,
  onSubmit,
}: {
  writing: boolean;
  msg: string;
  setMsg: (v: string) => void;
  textareaRef: RefObject<HTMLTextAreaElement>;
  onOpen: () => void;
  onRevert: () => void;
  onSubmit: () => void;
}) {
  const hasMsg = msg.trim().length > 0;
  return (
    <div className="git-commit">
      {/* collapsed note — animates shut when writing */}
      <div className={`cm-row note ${writing ? "shut" : ""}`} aria-hidden={writing}>
        <div className="cm-row-inner">
          <div className="cm-note">
            Agent will write the commit message &amp; PR.{" "}
            <button className="cm-link" onClick={onOpen} tabIndex={writing ? -1 : 0}>
              Write it yourself
            </button>
          </div>
        </div>
      </div>

      {/* override field — animates open when writing */}
      <div className={`cm-row field ${writing ? "open" : ""}`} aria-hidden={!writing}>
        <div className="cm-row-inner">
          <div className="cm-title">
            <span>Your message</span>
            <span className="grow" />
            <button className="cm-revert" onClick={onRevert} tabIndex={writing ? 0 : -1}>
              <Icon name="close" size={11} />
              <span>Let agent write it</span>
            </button>
          </div>
          <textarea
            ref={textareaRef}
            className="cm-input"
            rows={2}
            placeholder="Describe this commit…"
            value={msg}
            tabIndex={writing ? 0 : -1}
            onChange={(e) => setMsg(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                onSubmit();
              }
            }}
          />
          <div className={`cm-foot ${hasMsg ? "on" : ""}`}>
            {hasMsg ? (
              <>
                <Icon name="branch" size={11} />
                <span>Commits directly with your message — the agent is skipped.</span>
              </>
            ) : (
              <span className="cm-foot-dim">Leave empty to let the agent write it.</span>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

// ── State cards (rendered in the scrollable body) ─────────────────

/** Visual class for one check row: ok / fail / skip dot, or a spinner while
 *  the run is queued / in progress. */
function checkTone(run: CheckRun): "ok" | "fail" | "skip" | "run" {
  if (run.status !== "completed") return "run";
  switch (run.conclusion) {
    case "success": return "ok";
    case "neutral":
    case "skipped":
    case "stale":
    case null:      return "skip";
    default:        return "fail"; // failure, timed_out, cancelled, action_required, …
  }
}

function ChecksSection({ checks, prUrl }: { checks: PrChecks; prUrl: string }) {
  if (checks.total === 0) return null;
  // Failing first, then running, then the rest — the actionable rows lead.
  const weight = (r: CheckRun) => (checkTone(r) === "fail" ? 0 : checkTone(r) === "run" ? 1 : 2);
  const runs = [...checks.runs].sort((a, b) => weight(a) - weight(b));
  const shown = runs.slice(0, 6);
  const hidden = runs.length - shown.length;
  const summary =
    checks.rollup === "failing"
      ? `${checks.failed} failing`
      : checks.rollup === "pending"
        ? `${checks.total - checks.pending} of ${checks.total} done`
        : "all passing";
  return (
    <div className="pr-checks">
      <div className="pr-checks-h">
        <span>Checks</span>
        <span className={`pr-checks-sum ${checks.rollup}`}>{summary}</span>
      </div>
      {shown.map((r) => {
        const tone = checkTone(r);
        return (
          <button
            type="button"
            key={r.name}
            className="pr-check"
            onClick={() => void open(r.url ?? `${prUrl}/checks`)}
          >
            {tone === "run" ? <span className="git-spin sm" /> : <span className={`pc-dot ${tone}`} />}
            <span className="pc-name">{r.name}</span>
            <Icon name="external" size={10} />
          </button>
        );
      })}
      {hidden > 0 && (
        <button type="button" className="pr-checks-more" onClick={() => void open(`${prUrl}/checks`)}>
          +{hidden} more on GitHub
        </button>
      )}
    </div>
  );
}

// ── Review comments ───────────────────────────────────────────────
// Unresolved PR review threads (Greptile / other bots / humans), each
// flattened to its root comment. Mirrors ChecksSection's visual language.
// Each row links out to the thread (↗) and offers a "→ chat" quick action
// that drops the comment into the composer for the user to send to the agent.
function CommentsSection({
  comments,
  onAddToChat,
}: {
  comments: PrComments;
  onAddToChat: (c: PrComment) => void;
}) {
  const list = comments.unresolved;
  if (list.length === 0) return null;
  // Bots (the AI reviewers this feature targets) lead; otherwise stable order.
  const rows = [...list].sort((a, b) => Number(b.is_bot) - Number(a.is_bot));
  return (
    <div className="pr-comments">
      <div className="pr-comments-h">
        <span>Comments</span>
        <span className="pr-comments-sum">{list.length} unresolved</span>
      </div>
      {rows.map((c) => {
        const loc = commentLocation(c);
        return (
          <div key={c.url} className="pr-comment">
            <Icon name={c.is_bot ? "bot" : "user"} size={12} />
            <div className="pc-body">
              <div className="pc-top">
                <span className="pc-author">{c.author}</span>
                {loc && <span className="pc-loc">{loc}</span>}
                {c.replies > 0 && (
                  <span className="pc-replies">+{c.replies} {c.replies === 1 ? "reply" : "replies"}</span>
                )}
              </div>
              <div className="pc-text">{c.body}</div>
            </div>
            <div className="pc-acts">
              <button
                type="button"
                className="pc-act tip"
                data-tip="Add to chat"
                aria-label="Add comment to chat"
                onClick={() => onAddToChat(c)}
              >
                <Icon name="arrowR" size={12} />
              </button>
              <button
                type="button"
                className="pc-act tip"
                data-tip="View on GitHub"
                aria-label="View comment on GitHub"
                onClick={() => void open(c.url)}
              >
                <Icon name="external" size={11} />
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}

function PRCard({
  pr,
  base,
  checks,
  comments,
  onAddToChat,
}: {
  pr: PrState;
  base: string;
  checks: PrChecks | null;
  comments: PrComments | null;
  onAddToChat: (c: PrComment) => void;
}) {
  // One merge-gate line, from `merge_state` when available (spec §7); the
  // `mergeable`-only fallback claims no more than "no conflicts".
  const ms = checks?.merge_state ?? null;
  const gate: { cls: string; text: string } =
    ms === "clean"    ? { cls: "ok",  text: "✓ Ready to merge" } :
    ms === "unstable" ? { cls: "ok",  text: "✓ Mergeable — optional checks failing" } :
    ms === "blocked"  ? { cls: "att", text: "△ Blocked by required checks or reviews" } :
    ms === "behind"   ? { cls: "att", text: `△ Behind ${base} — update your branch` } :
    ms === "dirty"    ? { cls: "att", text: `△ Conflicts with ${base} — update your branch` } :
    ms === "draft"    ? { cls: "ok",  text: "Draft — mark ready on GitHub to merge" } :
    ms != null        ? { cls: "ok",  text: "Computing merge status…" } :
    pr.mergeable      ? { cls: "ok",  text: "✓ No merge conflicts" } :
                        { cls: "att", text: `△ Can’t merge cleanly with ${base} — update your branch` };
  return (
    <div className="git-card">
      <div className="git-card-h">Pull request</div>
      <div className="git-card-title">{pr.title}</div>
      <div className="git-card-meta">#{pr.number} · open</div>
      <div className="git-card-row">
        <span className={gate.cls}>{gate.text}</span>
      </div>
      {checks && <ChecksSection checks={checks} prUrl={pr.url} />}
      {comments && <CommentsSection comments={comments} onAddToChat={onAddToChat} />}
      <div className="git-card-links">
        <button type="button" className="git-card-link" onClick={() => void open(pr.url)}>
          <Icon name="github" size={11} />
          Overview
        </button>
        <button type="button" className="git-card-link" onClick={() => void open(`${pr.url}/files`)}>
          <Icon name="diff" size={11} />
          Files
        </button>
        <button type="button" className="git-card-link" onClick={() => void open(`${pr.url}/commits`)}>
          <Icon name="commit" size={11} />
          Commits
        </button>
      </div>
    </div>
  );
}

function ClosedPRCard({ pr }: { pr: PrState }) {
  return (
    <div className="git-card">
      <div className="git-card-h">Pull request</div>
      <div className="git-card-title">{pr.title}</div>
      <div className="git-card-meta">#{pr.number} · closed</div>
      <button type="button" className="git-card-link" onClick={() => void open(pr.url)}>
        <Icon name="github" size={11} />
        View on GitHub
      </button>
    </div>
  );
}

function ConflictCard({ files }: { files: FileStatus[] }) {
  const conflicted = files.filter((f) => f.kind === "conflicted");
  const first = conflicted[0]?.path ?? "";
  const rest = conflicted.length - 1;
  return (
    <div className="git-banner att">
      <Icon name="merge" size={14} />
      <span>
        Conflicts in <span className="mono">{first}</span>
        {rest > 0 && ` and ${rest} more`}. The agent can reconcile them.
      </span>
    </div>
  );
}

// ── Main panel ────────────────────────────────────────────────────

/** State-aware git panel driven by live git state from the Tauri backend.
 *  Layout: a color-coded status header (the at-a-glance state signal), a
 *  scrollable body (the changes / PR card — the focus), and a pinned footer
 *  holding the commit message plus a responsive action bar (status left,
 *  split-button right; stacks full-width on a narrow panel via a container
 *  query). The panel is feature-flagged in settings. */
export function GitPanel({ agent }: { agent: AgentRecord }) {
  const gitState = useAppStore((s) => s.gitStates[agent.id] ?? null);
  const prState  = useAppStore((s) => s.prStates[agent.id] ?? null);
  const fetchGitState = useAppStore((s) => s.fetchGitState);
  const fetchPrState  = useAppStore((s) => s.fetchPrState);
  const pushAgent  = useAppStore((s) => s.pushAgent);
  const pullAgent  = useAppStore((s) => s.pullAgent);
  const rebaseAgent = useAppStore((s) => s.rebaseAgent);
  const createPr   = useAppStore((s) => s.createPr);
  const mergePr    = useAppStore((s) => s.mergePr);
  const archive    = useAppStore((s) => s.archive);
  const commitChanges    = useAppStore((s) => s.commitChanges);
  const commitAndOpenPr  = useAppStore((s) => s.commitAndOpenPr);
  const stashChanges     = useAppStore((s) => s.stashChanges);
  const discardChanges   = useAppStore((s) => s.discardChanges);
  const abortMerge       = useAppStore((s) => s.abortMerge);
  const deleteBranch     = useAppStore((s) => s.deleteBranch);
  const prChecksEntry = useAppStore((s) => s.prChecks[agent.id]);
  const fetchPrChecks = useAppStore((s) => s.fetchPrChecks);
  const prCommentsEntry = useAppStore((s) => s.prComments[agent.id]);
  const fetchPrComments = useAppStore((s) => s.fetchPrComments);
  const seedComposer = useAppStore((s) => s.seedComposer);
  const delegation = useAppStore((s) => s.gitDelegations[agent.id]);
  const delegateGitAction = useAppStore((s) => s.delegateGitAction);
  const markGitDelegationRunning = useAppStore((s) => s.markGitDelegationRunning);
  const markGitDelegationDequeued = useAppStore((s) => s.markGitDelegationDequeued);
  const clearGitDelegation = useAppStore((s) => s.clearGitDelegation);
  const gitCommitAction = useAppStore((s) => s.gitCommitAction);
  const setGitCommitAction = useAppStore((s) => s.setGitCommitAction);

  // Poll git state for the focused agent at 1s while this panel is mounted.
  const pollGitState = useCallback(
    () => fetchGitState(agent.id),
    [agent.id, fetchGitState],
  );
  usePoll(pollGitState, 1000, [pollGitState]);

  // Poll PR state while the panel is mounted (not just once on mount), so an
  // open PR that gets merged / closed / becomes mergeable on GitHub is
  // reflected here promptly instead of staying stale until the panel remounts.
  // usePoll fires immediately, so the initial fetch still lands on open.
  const pollPrState = useCallback(
    () => fetchPrState(agent.id),
    [agent.id, fetchPrState],
  );
  usePoll(pollPrState, 5000, [pollPrState]);

  // Poll the heavier checks read at 5s, only while a PR is open. An absent
  // entry (undefined) means the first fetch hasn't landed → the panel renders
  // the "checking…" sub-state; null means confirmed unavailable → fall back
  // to mergeable-only behavior.
  const prOpen = prState?.state === "open";
  const pollChecks = useCallback(async () => {
    if (!prOpen) return;
    // Checks + review comments share the slow cadence: both are heavier gh
    // reads that only matter while a PR is open.
    await Promise.all([fetchPrChecks(agent.id), fetchPrComments(agent.id)]);
  }, [agent.id, prOpen, fetchPrChecks, fetchPrComments]);
  usePoll(pollChecks, 5000, [pollChecks]);

  const checks = prChecksEntry ?? null;
  const comments = prCommentsEntry ?? null;
  const mergeState: MergeState | null =
    checks?.merge_state ?? (prOpen && prChecksEntry === undefined ? "unknown" : null);

  const panelState = deriveState(gitState, prState);

  const [selected, setSelected] = useState<string | null>(null);
  useEffect(() => {
    setSelected((prev) => {
      const paths = gitState?.files.map((f) => f.path) ?? [];
      if (prev && paths.includes(prev)) return prev;
      return paths[0] ?? null;
    });
  }, [gitState]);

  // Commit-message authorship (agent mode). By default the agent writes the
  // message + PR (the field is collapsed). `override` = the user opened the
  // field to write their own; once `msg` has content the commit goes direct,
  // bypassing the agent.
  const [override, setOverride] = useState(false);
  const [msg, setMsg] = useState("");
  const commitRef = useRef<HTMLTextAreaElement>(null);
  const customActive = override && msg.trim().length > 0;
  const behind    = gitState?.behind ?? 0;
  const mergeable = prState?.mergeable ?? false;

  // In-flight async action — drives the loading presentation (dimmed body,
  // spinner status, busy CTA). Holds the present-tense verb to show.
  const [busy, setBusy] = useState<string | null>(null);
  const runBusy = useCallback(async (label: string, fn: () => Promise<unknown>) => {
    setBusy(label);
    try {
      return await fn();
    } finally {
      setBusy(null);
    }
  }, []);

  // Transient confirmation for fire-and-forget actions (push/pull/rebase,
  // and agent delegation), which otherwise have no visible effect.
  const [notice, setNotice] = useState<string | null>(null);
  const noticeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const showNotice = useCallback((m: string) => {
    setNotice(m);
    if (noticeTimer.current) clearTimeout(noticeTimer.current);
    noticeTimer.current = setTimeout(() => setNotice(null), 3500);
  }, []);
  useEffect(() => () => { if (noticeTimer.current) clearTimeout(noticeTimer.current); }, []);

  // Delegation lifecycle: while the agent holds control, watch the polled
  // git/PR/check state for the transition that marks the action done. The
  // step decision is pure (`delegationStep`) and handles the tricky cases —
  // a trigger queued behind a pre-existing turn must wait that turn out
  // (its running/settling is not ours), and a settled agent only reads as
  // "gave up" after our own turn ran or the grace window passed.
  useEffect(() => {
    if (!delegation) return;
    const resolved = delegationResolved(delegation.kind, gitState, prState, checks);
    switch (delegationStep(delegation, agent.status, resolved, Date.now())) {
      case "resolve":
        clearGitDelegation(agent.id);
        showNotice(delegationDone(delegation.kind));
        // A fresh PR (or branch update) changes the merge gate — refresh now
        // rather than waiting out the slow poll.
        void fetchPrChecks(agent.id);
        break;
      case "dequeue":
        markGitDelegationDequeued(agent.id);
        break;
      case "mark-running":
        markGitDelegationRunning(agent.id);
        break;
      case "give-up":
        clearGitDelegation(agent.id);
        showNotice(
          delegation.kind === "fix-checks"
            ? delegationDone("fix-checks")
            : "Agent finished — review the chat for details",
        );
        break;
      case "wait":
        break;
    }
  }, [
    delegation, agent.id, agent.status, gitState, prState, checks,
    markGitDelegationRunning, markGitDelegationDequeued, clearGitDelegation,
    showNotice, fetchPrChecks,
  ]);

  // Reset the override + notice + busy when switching agents so they don't
  // leak between worktrees.
  useEffect(() => {
    setOverride(false);
    setMsg("");
    setNotice(null);
    setBusy(null);
  }, [agent.id]);

  // Leaving the changes state drops any half-written override.
  useEffect(() => {
    if (panelState !== "changes") { setOverride(false); setMsg(""); }
  }, [panelState]);

  const openOverride = useCallback(() => {
    setOverride(true);
    // Defer focus until the textarea has animated in.
    requestAnimationFrame(() => commitRef.current?.focus());
  }, []);
  const revertOverride = useCallback(() => { setOverride(false); setMsg(""); }, []);

  const branch = gitState?.branch || agent.repos[0]?.branch || "(no branch yet)";
  const base   = gitState?.parent_branch || agent.repos[0]?.parent_branch || "main";
  // The worktree is detached until its first push; a branch is only born from
  // an agent that names it. So a direct (agent-bypassed) action that needs a
  // branch — push, open PR — can't run yet: it routes through the agent
  // instead, which picks a conventional name and creates the branch.
  const hasBranch = Boolean(gitState?.branch || agent.repos[0]?.branch);

  // Hand control to the coding agent: it writes the judgment part (message /
  // description / conflict edits) and executes the mutation through the
  // app's file RPC (git_commit / open_pr / git_update_branch / git_push).
  // The panel tracks the delegation until the matching transition lands.
  const delegate = useCallback(
    (kind: GitDelegationKind, prompt: string) => {
      delegateGitAction(agent.id, kind, prompt);
    },
    [agent.id, delegateGitAction],
  );

  // "→ chat" on a review comment: drop the formatted comment into this
  // agent's composer (not sent), so the user can edit and send it. Bots like
  // Greptile are inserted verbatim; human comments get a file/line wrapper.
  const addCommentToChat = useCallback(
    (c: PrComment) => {
      seedComposer(agent.id, formatCommentForChat(c));
      showNotice("Added to chat — edit & send to the agent");
    },
    [agent.id, seedComposer, showNotice],
  );

  // Single dispatch table for every action a state can offer — the split
  // button's main click and its menu both route through here by key.
  function runAction(key: string) {
    switch (key) {
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
        if (!customActive) { openOverride(); return; }
        void runBusy("Committing…", async () => {
          const ok = await commitChanges(agent.id, msg.trim());
          if (ok) revertOverride();
        });
        break;
      case "commit-pr-direct":
        if (!customActive) { openOverride(); return; }
        // No branch yet: commit the user's message directly (works on detached
        // HEAD), then let the agent name the branch and write the PR.
        if (!hasBranch) {
          void runBusy("Committing…", async () => {
            const ok = await commitChanges(agent.id, msg.trim());
            if (ok) { revertOverride(); delegate("open-pr", appActionMessage("open-pr", { base })); }
          });
          break;
        }
        void runBusy("Committing & opening PR…", async () => {
          const ok = await commitAndOpenPr(agent.id, msg.trim());
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
          const pr = await createPr(agent.id, "", "");
          // If creation failed (e.g. a PR already exists), the local prState
          // was stale — re-fetch so the panel corrects itself.
          if (!pr) await fetchPrState(agent.id);
        });
        break;
      case "view-pr":      if (prState?.url) void open(prState.url); break;
      case "merge":        void runBusy("Merging…", () => mergePr(agent.id)); break;
      case "archive":      void runBusy("Archiving…", () => archive(agent.id)); break;
      case "push":
        // Direct git push needs a branch; with none yet, the agent names and
        // creates one, then pushes.
        if (!hasBranch) {
          delegate("push", appActionMessage("push"));
          break;
        }
        void runBusy("Pushing…", async () => {
          const r = await pushAgent(agent.id);
          if (r) showNotice(r === "up-to-date" ? "Already up to date with origin" : "Pushed to origin");
        });
        break;
      case "pull":
        void runBusy("Pulling…", async () => {
          if (await pullAgent(agent.id)) showNotice("Pulled latest changes");
        });
        break;
      case "rebase":
        void runBusy("Rebasing…", async () => {
          if (await rebaseAgent(agent.id)) showNotice(`Rebased onto ${base}`);
        });
        break;
      case "stash":        void runBusy("Stashing…", () => stashChanges(agent.id)); break;
      case "discard":      void runBusy("Discarding…", () => discardChanges(agent.id)); break;
      case "abort":        void runBusy("Aborting…", () => abortMerge(agent.id)); break;
      case "delete-branch": void runBusy("Deleting branch…", () => deleteBranch(agent.id)); break;
      // "loading" is a non-actionable placeholder.
      default:             break;
    }
  }

  const counts = {
    files:    gitState?.files.length ?? 0,
    ahead:    gitState?.ahead ?? 0,
    behind,
    unpushed: gitState?.unpushed ?? 0,
    prNumber: prState?.number,
    base,
    customActive,
    mergeable,
    mergeState,
    checksFailed: checks?.failed ?? 0,
    commitAction: gitCommitAction,
    prOpen,
  };
  const primary   = primaryFor(panelState, counts);
  const secondary = secondaryFor(panelState, counts);

  // All actions for this state, primary first. The main button shows whichever
  // is currently selected; the default selection is the primary. A secondary
  // candidate that duplicates the primary is dropped (pr-open lists Merge
  // unconditionally so it stays reachable from any merge_state).
  const items: SplitActionItem[] = [
    { key: primary.key, label: primary.label, icon: primary.icon },
    ...secondary
      .filter((s) => s.key !== primary.key)
      .map((s) => ({ key: s.key, label: s.label, icon: s.icon, kbd: s.kbd })),
  ];

  // Selected action: defaults to the primary, resets whenever the state (or the
  // clean-state primary, which flips with `behind`) changes, and on agent swap.
  const [selectedKey, setSelectedKey] = useState(primary.key);
  useEffect(() => {
    setSelectedKey(primary.key);
  }, [panelState, primary.key, agent.id]);

  // `selectedKey` can be orphaned when a background poll removes its menu item
  // *without* changing `primary.key` — e.g. `mergeable` flips true, dropping
  // "agent-update-branch" from the menu while the primary stays "merge", so the
  // reset effect above doesn't fire. Fall back to the primary (which is always
  // `items[0]`, what the button then displays) so the displayed action, its
  // tone/enabled state, and the dispatched action all stay in agreement.
  const effectiveKey = items.some((i) => i.key === selectedKey) ? selectedKey : primary.key;

  // The CTA's main button is disabled while loading git state, while the
  // agent holds a delegation, and when Merge is selected but the merge gate
  // isn't open (clean/unstable per spec §7; `mergeable` fallback without
  // checks data).
  const mergeAllowed = checks ? mergeState === "clean" || mergeState === "unstable" : mergeable;
  const mainDisabled =
    effectiveKey === "loading" ||
    delegation != null ||
    (effectiveKey === "merge" && !mergeAllowed);
  // Tone applies only when the selected action is the state's primary; picking
  // an alternate from the menu falls back to the neutral accent fill.
  const tone: ActionTone = effectiveKey === primary.key ? primary.tone ?? "accent" : "accent";

  // Pushed state: link the commit count out to GitHub — a single commit when
  // only one is ahead, otherwise the base..branch compare (commit list + full
  // diff). Gated on nothing being unpushed, so the tip is on origin and the
  // link can't 404. Needs the origin web base (github.com remotes only).
  const webBase = gitState?.remote_url ?? null;
  const aheadCount = gitState?.ahead ?? 0;
  const unpushed = gitState?.unpushed ?? 0;
  const pushedLink: string | null =
    webBase && unpushed === 0 && aheadCount > 0
      ? aheadCount === 1 && gitState?.head_sha
        ? `${webBase}/commit/${gitState.head_sha}`
        : `${webBase}/compare/${base}...${branch}`
      : null;

  // Show the changes list only when there are uncommitted files to display.
  // The commit composer yields while the agent holds a delegation.
  const showFiles  = panelState === "changes" || panelState === "conflicts";
  const showCommit = panelState === "changes" && !delegation;

  return (
    <div className="git-wrap">
      {/* ── color-coded status header: the at-a-glance state signal ── */}
      <StatusHeader
        state={panelState}
        branch={branch}
        base={base}
        git={gitState}
        pr={prState}
        mergeState={mergeState}
        checksFailed={checks?.failed ?? 0}
      />

      {/* ── scrollable body: the changes are the focus ── */}
      <div className={`git-body ${busy ? "busy" : ""}`}>
        {panelState === "pr-open"   && prState && (
          <PRCard pr={prState} base={base} checks={checks} comments={comments} onAddToChat={addCommentToChat} />
        )}
        {panelState === "pr-closed" && prState && <ClosedPRCard pr={prState} />}
        {panelState === "conflicts" && gitState && <ConflictCard files={gitState.files} />}

        {showFiles && (
          <div className="git-files">
            <div className="git-files-h">
              <span>Changes <span className="n">{gitState?.files.length ?? 0}</span></span>
              <div className="actions">
                <IconButton tip="Refresh" size="xs" onClick={() => void fetchGitState(agent.id)}>
                  <Icon name="refresh" />
                </IconButton>
              </div>
            </div>
            <div className="git-file-list">
              {(gitState?.files ?? []).map((f) => (
                <div
                  key={f.path}
                  className={`git-file ${selected === f.path ? "active" : ""}`}
                  onClick={() => setSelected(f.path)}
                >
                  <span className={`gs ${f.kind}`}>{kindLabel(f.kind)}</span>
                  <span className="gn">{f.path}</span>
                  <span className="gx">
                    {f.additions > 0 && <span className="add">+{f.additions}</span>}
                    {f.deletions > 0 && <span className="rem">−{f.deletions}</span>}
                  </span>
                </div>
              ))}
            </div>
          </div>
        )}

        {panelState === "loading" && (
          <div className="empty-msg" style={{ margin: "auto" }}>
            <div className="et">Loading…</div>
            <div>Fetching git state.</div>
          </div>
        )}
        {panelState === "pushed" && (
          <div className="empty-msg" style={{ margin: "auto" }}>
            <div className="et">Ready for a pull request</div>
            <div>All changes are committed &amp; pushed. Open a PR to start review.</div>
          </div>
        )}
        {panelState === "merged" && (
          <div className="empty-msg" style={{ margin: "auto" }}>
            <div className="et">Merged into {base}</div>
            <div>This workspace’s work is shipped. Archive it or keep going.</div>
          </div>
        )}
        {panelState === "clean" && (
          <div className="empty-msg" style={{ margin: "auto" }}>
            <div className="et">All clean</div>
            <div>No uncommitted changes. Type a follow-up to start working.</div>
          </div>
        )}
      </div>

      {/* ── pinned footer: commit message + status + action ── */}
      <div className="git-foot">
        {showCommit && (
          <CommitComposer
            writing={override}
            msg={msg}
            setMsg={setMsg}
            textareaRef={commitRef}
            onOpen={openOverride}
            onRevert={revertOverride}
            onSubmit={() => runAction(effectiveKey)}
          />
        )}

        <div className="git-act">
          {busy ? (
            <div className="git-act-status info">
              <Spinner />
              <span className="lbl">{busy}</span>
            </div>
          ) : delegation ? (
            <div className="git-act-status info working">
              <Spinner />
              <span className="lbl">{delegationLabel(delegation.kind)}</span>
            </div>
          ) : notice ? (
            <div className="git-notice">
              <Icon name="check" size={11} />
              <span>{notice}</span>
            </div>
          ) : (
            <div className={`git-act-status ${primary.statusKind}`}>
              <span className="d" />
              <span className="lbl">
                {panelState === "pushed" && pushedLink ? (
                  <>
                    <GitLink href={pushedLink}>{aheadCount === 1 ? "1 commit" : `${aheadCount} commits`}</GitLink>
                    {" pushed · no PR yet"}
                  </>
                ) : (
                  primary.statusLabel
                )}
              </span>
              {primary.statusExtra && <span className="ex">{primary.statusExtra}</span>}
              {/* View on GitHub is a convenience link, not an action — a
                  quiet chip beside the status, never a menu item. */}
              {panelState === "pr-open" && prState?.url && (
                <button
                  type="button"
                  className="st-ext tip"
                  data-tip="View on GitHub"
                  aria-label="View on GitHub"
                  onClick={() => void open(prState.url)}
                >
                  <Icon name="external" size={11} />
                </button>
              )}
            </div>
          )}
          <SplitAction
            items={items}
            selectedKey={effectiveKey}
            tone={tone}
            mainDisabled={mainDisabled}
            busyLabel={busy ?? (delegation ? "Agent working…" : null)}
            onSelect={(key) => {
              setSelectedKey(key);
              // Picking a commit mode is sticky: it becomes the default
              // primary in every workspace until the user picks another.
              if (isCommitAction(key)) setGitCommitAction(key);
            }}
            onRun={() => runAction(effectiveKey)}
          />
        </div>
      </div>
    </div>
  );
}
