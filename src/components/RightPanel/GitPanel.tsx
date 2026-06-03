import { open } from "@tauri-apps/plugin-shell";
import { type RefObject, useCallback, useEffect, useRef, useState } from "react";
import type { AgentRecord, FileStatus, GitState, PrState } from "../../api";
import { useAppStore } from "../../store";
import { usePoll } from "../../util/hooks";
import { Icon, type IconName } from "../Icon";
import { IconButton } from "../ui/IconButton";
import { primaryFor, secondaryFor, type GitPanelState } from "./primaryActions";

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
  danger,
  mainDisabled,
  onSelect,
  onRun,
}: {
  items: SplitActionItem[];
  selectedKey: string;
  primaryKey: string;
  danger: boolean;
  mainDisabled: boolean;
  onSelect: (key: string) => void;
  onRun: () => void;
}) {
  const [open, setOpen] = useState(false);
  const selected = items.find((a) => a.key === selectedKey) ?? items[0];
  const hasMenu = items.length > 1;
  if (!selected) return null;

  return (
    <div className={`git-split ${danger ? "danger" : ""}`}>
      <button className="gsa-main" disabled={mainDisabled} onClick={onRun}>
        <Icon name={selected.icon} />
        <span className="gsa-label">{selected.label}</span>
      </button>
      {hasMenu && (
        <button
          className="gsa-caret tip"
          data-tip="Choose action"
          aria-label="Choose action"
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

function PRCard({ pr }: { pr: PrState }) {
  return (
    <div className="git-card">
      <div className="git-card-h">Pull request</div>
      <div className="git-card-title">{pr.title}</div>
      <div className="git-card-meta">
        #{pr.number} · alex chaplinsky · 12 comments
      </div>
      <div className="git-card-row">
        <span className="ok">✓ 24 checks passing</span>
        <span className="sep">·</span>
        <span>2 reviewers · 1 approved</span>
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

function MergedCard({ base }: { base: string }) {
  return (
    <div className="git-banner success">
      <Icon name="merge" size={14} />
      <span>Merged into {base}. Workspace can now be archived.</span>
    </div>
  );
}

function ConflictCard({ files }: { files: FileStatus[] }) {
  const conflicted = files.filter((f) => f.kind === "conflicted");
  const first = conflicted[0]?.path ?? "";
  const rest = conflicted.length - 1;
  return (
    <div className="git-banner danger">
      <Icon name="merge" size={14} />
      <span>
        Conflicts in <span className="mono">{first}</span>
        {rest > 0 && ` and ${rest} more`}.
      </span>
    </div>
  );
}

// ── Main panel ────────────────────────────────────────────────────

/** State-aware git panel driven by live git state from the Tauri backend.
 *  Layout follows the Quorum v2 design: a quiet context header, a scrollable
 *  changes list (the focus), and a pinned footer that holds the commit
 *  message, a status line, and one centered split-button action — same place
 *  in every state. The panel is feature-flagged in settings. */
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

  // Reset the override + notice when switching agents so they don't leak
  // between worktrees.
  useEffect(() => {
    setOverride(false);
    setMsg("");
    setNotice(null);
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

  // Single dispatch table for every action a state can offer — the split
  // button's main click and its menu both route through here by key.
  function runAction(key: string) {
    switch (key) {
      // ── delegated to the coding agent (agent mode, no custom message) ──
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
      // ── direct, agent bypassed (user typed their own message) ──
      case "commit-direct":
        if (!customActive) { openOverride(); return; }
        void (async () => {
          const ok = await commitChanges(agent.id, msg.trim());
          if (ok) revertOverride();
        })();
        break;
      case "commit-pr-direct":
        if (!customActive) { openOverride(); return; }
        void (async () => {
          const ok = await commitAndOpenPr(agent.id, msg.trim());
          if (ok) revertOverride();
        })();
        break;
      case "open-pr":
        void (async () => {
          const pr = await createPr(agent.id, "", "");
          // If creation failed (e.g. a PR already exists), the local prState
          // was stale — re-fetch so the panel corrects itself to "View PR".
          if (!pr) await fetchPrState(agent.id);
        })();
        break;
      case "view-pr":      if (prState?.url) void open(prState.url); break;
      case "merge":        void mergePr(agent.id);        break;
      case "archive":      void archive(agent.id);        break;
      case "push":
        void (async () => {
          const r = await pushAgent(agent.id);
          if (r) showNotice(r === "up-to-date" ? "Already up to date with origin" : "Pushed to origin");
        })();
        break;
      case "pull":
        void (async () => { if (await pullAgent(agent.id)) showNotice("Pulled latest changes"); })();
        break;
      case "rebase":
        void (async () => { if (await rebaseAgent(agent.id)) showNotice(`Rebased onto ${base}`); })();
        break;
      case "stash":        void stashChanges(agent.id);   break;
      case "discard":      void discardChanges(agent.id); break;
      case "abort":        void abortMerge(agent.id);     break;
      case "delete-branch": void deleteBranch(agent.id);  break;
      // "resolve" / "loading" are non-actionable placeholders.
      default:             break;
    }
  }

  const branch = gitState?.branch || agent.repos[0]?.branch || "(no branch yet)";
  const base   = gitState?.parent_branch || agent.repos[0]?.parent_branch || "main";

  const counts = {
    files:    gitState?.files.length ?? 0,
    ahead:    gitState?.ahead ?? 0,
    behind,
    unpushed: gitState?.unpushed ?? 0,
    prNumber: prState?.number,
    base,
    customActive,
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

  // The CTA is disabled only where the *selected* action can't run: the
  // conflicts placeholder (no in-app resolver) and while git state loads.
  const mainDisabled = selectedKey === "resolve" || selectedKey === "loading";
  const danger = selectedKey === primary.key && !!primary.danger;

  // Show the changes list only when there are uncommitted files to display.
  const showFiles  = panelState === "changes" || panelState === "conflicts";
  const showCommit = panelState === "changes";

  return (
    <div className="git-wrap">
      {/* ── context header: branch + how it relates to base ── */}
      <div className="git-state">
        <div className="git-branch-row">
          <Icon name="branch" />
          <span className="bn">{branch}</span>
          <span className="base">← {base}</span>
        </div>
        <div className="git-stats">
          <span><span className="num">{gitState?.ahead ?? 0}</span> ahead</span>
          <span><span className="num">{gitState?.behind ?? 0}</span> behind</span>
          {((gitState?.additions ?? 0) > 0 || (gitState?.deletions ?? 0) > 0) && (
            <span className="git-stats-d">
              <span className="add">+{gitState!.additions}</span>
              <span className="rem">−{gitState!.deletions}</span>
            </span>
          )}
        </div>
      </div>

      {/* ── scrollable body: the changes are the focus ── */}
      <div className="git-body">
        {panelState === "pr-open"   && prState && <PRCard pr={prState} />}
        {panelState === "pr-closed" && prState && <ClosedPRCard pr={prState} />}
        {panelState === "merged"    && <MergedCard base={base} />}
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
            <div>All changes are committed. Open a PR to start review.</div>
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
          {notice ? (
            <div className="git-notice">
              <Icon name="check" size={11} />
              <span>{notice}</span>
            </div>
          ) : (
            <div className={`git-act-status ${primary.statusKind}`}>
              <span className="d" />
              <span className="lbl">{primary.statusLabel}</span>
              {primary.statusExtra && <span className="ex">{primary.statusExtra}</span>}
            </div>
          )}
          <SplitAction
            items={items}
            selectedKey={selectedKey}
            primaryKey={primary.key}
            danger={danger}
            mainDisabled={mainDisabled}
            onSelect={setSelectedKey}
            onRun={() => runAction(selectedKey)}
          />
        </div>
      </div>
    </div>
  );
}
