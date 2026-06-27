import { open } from "@tauri-apps/plugin-shell";
import type { PrChecks, PrComment, PrComments, PrState } from "../../../../api";
import { Icon } from "../../../Icon";
import { ChecksSection } from "./ChecksSection";
import { CommentsSection } from "./CommentsSection";

export function PRCard({
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
    ms === "clean"
      ? { cls: "ok", text: "✓ Ready to merge" }
      : ms === "unstable"
        ? { cls: "ok", text: "✓ Mergeable — optional checks failing" }
        : ms === "blocked"
          ? { cls: "att", text: "△ Blocked by required checks or reviews" }
          : ms === "behind"
            ? { cls: "att", text: `△ Behind ${base} — update your branch` }
            : ms === "dirty"
              ? { cls: "att", text: `△ Conflicts with ${base} — update your branch` }
              : ms === "draft"
                ? { cls: "ok", text: "Draft — mark ready on GitHub to merge" }
                : ms != null
                  ? { cls: "ok", text: "Computing merge status…" }
                  : pr.mergeable
                    ? { cls: "ok", text: "✓ No merge conflicts" }
                    : {
                        cls: "att",
                        text: `△ Can’t merge cleanly with ${base} — update your branch`,
                      };
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
        <button
          type="button"
          className="git-card-link"
          onClick={() => void open(`${pr.url}/files`)}
        >
          <Icon name="diff" size={11} />
          Files
        </button>
        <button
          type="button"
          className="git-card-link"
          onClick={() => void open(`${pr.url}/commits`)}
        >
          <Icon name="commit" size={11} />
          Commits
        </button>
      </div>
    </div>
  );
}

export function ClosedPRCard({ pr }: { pr: PrState }) {
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
