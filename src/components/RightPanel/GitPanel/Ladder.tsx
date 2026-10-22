import { open } from "@tauri-apps/plugin-shell";
import { type ReactNode, useState } from "react";
import type { FileStatus, PrChecks, PrComment, PrComments, PrState, ReviewState } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { ChangesList } from "./ChangesList";
import { ChecksSection, CommentsSection } from "./cards";
import type { FooterAction } from "./footerModel";
import type { PrOrigin } from "./hooks/usePrOrigin";
import type { LadderRow, RungKey } from "./ladderModel";
import { GitLink } from "./shared";

/** What the Review row can do about the local reviewer, beyond showing its
 *  threads: start or stop a review from the row, fix or dismiss one finding,
 *  and reach the reviewer's own transcript. */
export interface ReviewBodies {
  state: ReviewState | null;
  /** The row's one button — Review / Re-review / Stop — or null when the
   *  host has no reviewer to offer. `reason` says why it can't run now. */
  action: { label: string; reason: string | null; onRun: () => void } | null;
  /** Hand one thread to the coding agent (`resolve-comments`, scoped to it). */
  onFix: (c: PrComment) => void;
  /** Resolve a local finding by hand. */
  onDismiss: (c: PrComment) => void;
  /** Open the hidden reviewer's transcript, once there is one. */
  onOpenReviewer: (() => void) | null;
}

// ── The ladder: the panel's body ─────────────────────────────────────────────
// The rows `ladderModel.ts` derives, drawn as a rail: a mark per stage on one
// vertical line, so the body reads as a single way to landing rather than a
// pile of sections. Every row is one line — title · summary — and a row with
// something to show underneath (files, runs, threads, the PR itself) opens on
// click. The row the next step is about opens by itself; a row the user
// opened or closed stays that way until the lane changes, so an agent writing
// a file never folds the thread being read.

export interface LadderBodies {
  files: FileStatus[];
  onOpenFile: (path: string) => void;
  onRefresh: () => void;
  /** Link to the pushed commits on GitHub, when every commit is on origin. */
  commitsLink: string | null;
  commitsAhead: number;
  pr: PrState | null;
  /** The PR's head branch, when the tree is on another one. */
  prBranch: string | null;
  origin: PrOrigin | null;
  checks: PrChecks | null;
  comments: PrComments | null;
  onAddToChat: (c: PrComment) => void;
  /** How a merged PR's branch ends, on its row: archive the workspace, delete
   *  the branch — each gated on its own (`useFooterModel`). Null for a lane the
   *  footer doesn't act on. */
  ending: { actions: FooterAction[]; onRun: (key: string) => void } | null;
  /** The local reviewer on this lane; null on a lane the footer doesn't act
   *  on, or on a host without one. */
  review: ReviewBodies | null;
}

/** Mount one per lane (`key={lane.id}`): the open/closed choices are the
 *  lane's, and a fresh mount is how they reset. */
export function Ladder({ rows, bodies }: { rows: LadderRow[]; bodies: LadderBodies }) {
  const [toggled, setToggled] = useState<Partial<Record<RungKey, boolean>>>({});
  // The row the next step is about opens by itself. A ladder with no next step
  // (another PR of the set, only looked at) opens the proposal instead, so the
  // lane is never a bare list of marks.
  const anyCurrent = rows.some((r) => r.current);

  return (
    <ol className="git-ladder">
      {rows.map((row) => {
        const body = rowBody(row, bodies);
        const openByDefault = row.current || (!anyCurrent && row.key === "pr");
        const open = body != null && (toggled[row.key] ?? openByDefault);
        const trailing = rowTrailing(row, bodies);
        return (
          <li
            key={row.key}
            className={[
              "git-rung",
              `m-${row.mark}`,
              row.tone ? `t-${row.tone}` : "",
              row.current ? "current" : "",
              open ? "open" : "",
            ]
              .filter(Boolean)
              .join(" ")}
          >
            <div className="rg-row flex-center">
              {body ? (
                <button
                  type="button"
                  className="rg-head flex-center"
                  aria-expanded={open}
                  onClick={() => setToggled((t) => ({ ...t, [row.key]: !open }))}
                >
                  <Mark row={row} />
                  <span className="rg-title text-sm">{row.title}</span>
                  <span className="rg-sum text-sm">{row.summary}</span>
                  <Icon name="chevR" size={10} className="rg-chev" />
                </button>
              ) : (
                <div className="rg-head flex-center">
                  <Mark row={row} />
                  <span className="rg-title text-sm">{row.title}</span>
                  <span className="rg-sum text-sm">{row.summary}</span>
                </div>
              )}
              {trailing}
            </div>
            {open && <div className="rg-body">{body}</div>}
          </li>
        );
      })}
    </ol>
  );
}

/** The stage's mark: a check once done, a spinner while running, otherwise a
 *  dot — hollow for a stage not reached, filled in its tone for one that
 *  wants attention. */
function Mark({ row }: { row: LadderRow }) {
  return (
    <span className="rg-mark" aria-hidden>
      {row.mark === "done" ? (
        <Icon name="check" size={9} />
      ) : row.mark === "running" ? (
        <span className="git-spin sm" />
      ) : (
        <span className="rg-dot" />
      )}
    </span>
  );
}

function rowTrailing(row: LadderRow, b: LadderBodies): ReactNode {
  if (row.key === "changes" && b.files.length > 0) {
    return (
      <IconButton tip="Refresh" size="xs" className="rg-act" onClick={b.onRefresh}>
        <Icon name="refresh" />
      </IconButton>
    );
  }
  if (row.key === "review" && b.review?.action) {
    // The reviewer's one control, on the row itself: start, re-run, or stop
    // (which also cancels a queued start). Disabled with its reason on hover
    // when the host or the settings refuse.
    const { label, reason, onRun } = b.review.action;
    const phase = b.review.state?.phase;
    const stopping = phase === "reviewing" || phase === "queued";
    return (
      <span className={reason ? "tip" : undefined} data-tip={reason ?? undefined}>
        <Button
          variant={stopping ? "ghost" : "outline"}
          size="sm"
          className="rg-review-act"
          disabled={reason != null}
          onClick={onRun}
        >
          <Icon name={stopping ? "stop" : "search"} size={11} />
          {label}
        </Button>
      </span>
    );
  }
  return null;
}

/** What opens under a row, or null when the line says everything. */
function rowBody(row: LadderRow, b: LadderBodies): ReactNode {
  switch (row.key) {
    case "changes":
      return b.files.length > 0 ? <ChangesList files={b.files} onOpen={b.onOpenFile} /> : null;
    case "commits":
      return b.commitsLink ? (
        <div className="rg-line text-sm">
          <GitLink href={b.commitsLink}>
            {b.commitsAhead === 1 ? "View the commit" : `View ${b.commitsAhead} commits`} on GitHub
          </GitLink>
        </div>
      ) : null;
    case "pr":
      return b.pr ? (
        <PrBody
          pr={b.pr}
          branch={b.prBranch}
          origin={b.origin}
          ending={b.pr.state === "merged" ? b.ending : null}
        />
      ) : null;
    case "checks":
      return b.checks && b.checks.total > 0 && b.pr ? (
        <ChecksSection checks={b.checks} prUrl={b.pr.url} />
      ) : null;
    case "review": {
      const threads = b.comments?.unresolved.length ?? 0;
      const reviewer = b.review?.state?.reviewer ?? null;
      const transcript = b.review?.onOpenReviewer ?? null;
      if (threads === 0 && !transcript) return null;
      return (
        <>
          {b.comments && threads > 0 && (
            <CommentsSection
              comments={b.comments}
              onAddToChat={b.onAddToChat}
              onFix={b.review?.onFix}
              onDismiss={b.review?.onDismiss}
              onOpenFile={b.onOpenFile}
              openable={new Set(b.files.map((f) => f.path))}
              localProvider={reviewer?.provider ?? null}
            />
          )}
          {transcript && (
            <div className="rg-line text-sm">
              <button type="button" className="rg-link iflex-center text-xs" onClick={transcript}>
                <Icon name="notebookPen" size={11} />
                Open {reviewer?.name ?? "reviewer"}'s transcript
              </button>
            </div>
          )}
        </>
      );
    }
  }
}

/** The proposal itself: its title, where it came from, the three ways into it
 *  on GitHub — and, once it has merged, the two ways to end its branch. */
function PrBody({
  pr,
  branch,
  origin,
  ending,
}: {
  pr: PrState;
  branch: string | null;
  origin: PrOrigin | null;
  ending: LadderBodies["ending"];
}) {
  const links: { label: string; icon: "github" | "diff" | "commit"; path: string }[] =
    pr.state === "closed"
      ? [{ label: "Overview", icon: "github", path: "" }]
      : [
          { label: "Overview", icon: "github", path: "" },
          { label: "Files", icon: "diff", path: "/files" },
          { label: "Commits", icon: "commit", path: "/commits" },
        ];
  return (
    <div className="rg-pr">
      <div className="rg-pr-title text-sm">{pr.title}</div>
      {(branch || origin) && (
        <div className="rg-pr-from text-xs flex-center" title={branch ?? undefined}>
          <Icon name={branch ? "branch" : "subagent"} size={11} />
          {branch && <span className="rg-pr-branch">{branch}</span>}
          {origin && (
            <span className="rg-pr-origin">
              {branch ? "· " : ""}opened by{" "}
              <button type="button" className="rg-pr-origin-link" onClick={origin.open}>
                {origin.label}
              </button>
            </span>
          )}
        </div>
      )}
      <div className="rg-links flex-center">
        {links.map((l) => (
          <button
            key={l.label}
            type="button"
            className="rg-link iflex-center text-xs"
            onClick={() => void open(`${pr.url}${l.path}`)}
          >
            <Icon name={l.icon} size={11} />
            {l.label}
          </button>
        ))}
      </div>
      {ending && (
        // Shipped: the branch is done. Quiet buttons, not the footer — ending a
        // workspace is a decision, not the next step of the ladder. Each is
        // gated on its own; a disabled one says why on hover.
        <div className="rg-end flex-center">
          {ending.actions.map((a) => (
            <span key={a.key} title={a.reason}>
              <Button
                variant={a.danger ? "ghost" : "outline"}
                danger={a.danger}
                size="sm"
                disabled={a.disabled}
                onClick={() => ending.onRun(a.key)}
              >
                <Icon name={a.icon} size={12} />
                {a.label}
              </Button>
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
