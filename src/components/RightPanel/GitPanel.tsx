import { open } from "@tauri-apps/plugin-shell";
import { type ReactNode, type RefObject, useCallback, useEffect, useRef, useState } from "react";
import type { AgentRecord, FileStatus, GitState, PrState } from "../../api";
import { useAppStore } from "../../store";
import { usePoll } from "../../util/hooks";
import { Icon, type IconName } from "../Icon";
import { IconButton } from "../ui/IconButton";
import { primaryFor, secondaryFor, type ActionTone, type GitPanelState } from "./primaryActions";

function deriveState(git: GitState | null, pr: PrState | null): GitPanelState {
  if (!git) return "loading";
  if (git.files.some((f) => f.kind === "conflicted")) return "conflicts";
  if (pr?.state === "merged") return "merged";
  if (pr?.state === "open")   return "pr-open";
  if (pr?.state === "closed") return "pr-closed";
  if (git.files.length > 0)  return "changes";
  if (git.ahead > 0)         return "pushed";
  return "clean";
}

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
): HeaderInfo {
  const n = pr?.number;
  switch (state) {
    case "loading":   return { kind: "neutral", text: "Loading…" };
    case "changes":   return { kind: "changes", pill: "Uncommitted", text: branch, diff: true };
    case "pushed":    return { kind: "info", pill: "Pushed", text: branch };
    case "conflicts": return { kind: "att", pill: "Conflicts", text: branch, sub: `← ${base}` };
    case "pr-open":
      // `mergeable` only means "no conflicts" — not that checks passed — so the
      // header stays neutral blue, never a green "ready" all-clear.
      return pr?.mergeable
        ? { kind: "info", pill: n != null ? `PR #${n}` : "PR", text: "no conflicts", ext: true }
        : { kind: "att", pill: n != null ? `PR #${n}` : "PR", text: "can’t merge yet", ext: true };
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
}: {
  state: GitPanelState;
  branch: string;
  base: string;
  git: GitState | null;
  pr: PrState | null;
}) {
  const h = describeHeader(state, branch, base, pr);
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
  primaryKey,
  tone,
  mainDisabled,
  busyLabel,
  onSelect,
  onRun,
}: {
  items: SplitActionItem[];
  selectedKey: string;
  primaryKey: string;
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
                {a.key === primaryKey && <span className="di-tag">default</span>}
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

function PRCard({ pr, base }: { pr: PrState; base: string }) {
  return (
    <div className="git-card">
      <div className="git-card-h">Pull request</div>
      <div className="git-card-title">{pr.title}</div>
      <div className="git-card-meta">#{pr.number} · open</div>
      <div className="git-card-row">
        {pr.mergeable ? (
          // `mergeable` ⇒ no merge conflicts only; it says nothing about checks.
          <span className="ok">✓ No merge conflicts</span>
        ) : (
          <span className="att">△ Can’t merge cleanly with {base} — update your branch</span>
        )}
      </div>
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
  const sendUserMessage  = useAppStore((s) => s.sendUserMessage);

  // Poll git state for the focused agent at 1s while this panel is mounted.
  const pollGitState = useCallback(
    () => fetchGitState(agent.id),
    [agent.id, fetchGitState],
  );
  usePoll(pollGitState, 1000, [pollGitState]);

  useEffect(() => {
    void fetchPrState(agent.id);
  }, [agent.id, fetchPrState]);

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

  // Single dispatch table for every action a state can offer — the split
  // button's main click and its menu both route through here by key.
  function runAction(key: string) {
    switch (key) {
      // ── delegated to the coding agent (agent mode) ──
      case "agent-commit-pr":
        void sendUserMessage(
          agent.id,
          "Commit all current changes with a clear, conventional commit message, then open a pull request with a concise, descriptive title and body.",
        );
        showNotice("Asked the agent to commit & open a PR");
        break;
      case "agent-commit":
        void sendUserMessage(
          agent.id,
          "Commit all current changes with a clear, conventional commit message.",
        );
        showNotice("Asked the agent to commit");
        break;
      case "agent-resolve":
        void sendUserMessage(
          agent.id,
          "Resolve the current git merge conflicts: inspect each conflicted file, reconcile both sides correctly, and complete the merge.",
        );
        showNotice("Asked the agent to resolve conflicts");
        break;
      case "agent-update-branch":
        // PR can't merge cleanly with the base (the base advanced). This is NOT
        // a local in-progress merge — the agent must sync the base in first.
        void sendUserMessage(
          agent.id,
          `This pull request can't merge cleanly with ${base}. Update this branch with the latest ${base} (rebase onto it, or merge it in), resolve any conflicts that arise, and push so the PR becomes mergeable again.`,
        );
        showNotice(`Asked the agent to update the branch with ${base}`);
        break;
      case "agent-fix":
        void sendUserMessage(
          agent.id,
          "Some CI checks are failing on this pull request. Investigate the failures and fix them.",
        );
        showNotice("Asked the agent to fix the failing checks");
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
        void runBusy("Committing & opening PR…", async () => {
          const ok = await commitAndOpenPr(agent.id, msg.trim());
          if (ok) revertOverride();
        });
        break;
      case "open-pr":
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
  };
  const primary   = primaryFor(panelState, counts);
  const secondary = secondaryFor(panelState, counts);

  // All actions for this state, primary first. The main button shows whichever
  // is currently selected; the default selection is the primary.
  const items: SplitActionItem[] = [
    { key: primary.key, label: primary.label, icon: primary.icon },
    ...secondary.map((s) => ({ key: s.key, label: s.label, icon: s.icon, kbd: s.kbd })),
  ];

  // Selected action: defaults to the primary, resets whenever the state (or the
  // clean-state primary, which flips with `behind`) changes, and on agent swap.
  const [selectedKey, setSelectedKey] = useState(primary.key);
  useEffect(() => {
    setSelectedKey(primary.key);
  }, [panelState, primary.key, agent.id]);

  // The CTA's main button is disabled while loading git state, while an action
  // is in flight, and when Merge is selected but the PR can't merge yet.
  const mainDisabled =
    selectedKey === "loading" ||
    (selectedKey === "merge" && !mergeable);
  // Tone applies only when the selected action is the state's primary; picking
  // an alternate from the menu falls back to the neutral accent fill.
  const tone: ActionTone = selectedKey === primary.key ? primary.tone ?? "accent" : "accent";

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
  const showFiles  = panelState === "changes" || panelState === "conflicts";
  const showCommit = panelState === "changes";

  return (
    <div className="git-wrap">
      {/* ── color-coded status header: the at-a-glance state signal ── */}
      <StatusHeader state={panelState} branch={branch} base={base} git={gitState} pr={prState} />

      {/* ── scrollable body: the changes are the focus ── */}
      <div className={`git-body ${busy ? "busy" : ""}`}>
        {panelState === "pr-open"   && prState && <PRCard pr={prState} base={base} />}
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
            onSubmit={() => runAction(selectedKey)}
          />
        )}

        <div className="git-act">
          {busy ? (
            <div className="git-act-status info">
              <Spinner />
              <span className="lbl">{busy}</span>
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
            </div>
          )}
          <SplitAction
            items={items}
            selectedKey={selectedKey}
            primaryKey={primary.key}
            tone={tone}
            mainDisabled={mainDisabled}
            busyLabel={busy}
            onSelect={setSelectedKey}
            onRun={() => runAction(selectedKey)}
          />
        </div>
      </div>
    </div>
  );
}
