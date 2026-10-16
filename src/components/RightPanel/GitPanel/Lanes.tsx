import { open } from "@tauri-apps/plugin-shell";
import type { KeyboardEvent, ReactNode } from "react";
import type { GitState } from "@/api";
import { Icon } from "@/components/Icon";
import { Badge } from "@/components/ui/Badge";
import { prTint } from "@/util/prSummary";
import type { Lane } from "./laneModel";

// ── The lanes: the panel's header ────────────────────────────────────────────
// One row per branch-and-PR of the checkout (see `lanes.ts`). With one lane the
// row simply is the header: branch, base, the PR chip, the tree's diff. With
// several, the rows are a radio group — the viewed lane is the one the ladder
// below describes — and the row carrying the autopilot switch is the focused
// PR's, since the switch governs exactly that lane. Viewing a lane is a local
// choice; it never moves the host's focus (that is the footer's "Focus this
// PR", explicit).

export function Lanes({
  lanes,
  viewedId,
  onView,
  git,
  controls,
}: {
  lanes: Lane[];
  viewedId: string | null;
  onView: (id: string) => void;
  git: GitState | null;
  /** The workspace's autopilot switch — rendered on the first lane, which is
   *  the focused PR's whenever there is one. */
  controls?: ReactNode;
}) {
  const selectable = lanes.length > 1;
  return (
    <div
      className={`git-lanes ${selectable ? "pick" : ""}`}
      role={selectable ? "radiogroup" : undefined}
      aria-label="Branches and pull requests"
    >
      {lanes.map((lane, i) => (
        <LaneRow
          key={lane.id}
          lane={lane}
          viewed={lane.id === viewedId}
          selectable={selectable}
          onView={() => onView(lane.id)}
          git={git}
          trailing={i === 0 ? controls : null}
        />
      ))}
    </div>
  );
}

function LaneRow({
  lane,
  viewed,
  selectable,
  onView,
  git,
  trailing,
}: {
  lane: Lane;
  viewed: boolean;
  selectable: boolean;
  onView: () => void;
  git: GitState | null;
  trailing: ReactNode;
}) {
  const { pr } = lane;
  const tint = pr ? prTint(pr.state, pr.checks) : null;
  const base = git?.parent_branch;
  // The tree's diff belongs to the lane it is on, and to no other.
  const adds = lane.current ? (git?.additions ?? 0) : 0;
  const dels = lane.current ? (git?.deletions ?? 0) : 0;
  const prTip = pr
    ? [`#${pr.state.number}`, pr.state.title, tint?.word, "open on GitHub"]
        .filter(Boolean)
        .join(" · ")
    : undefined;

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onView();
    }
  };

  return (
    <div
      className={[
        "git-lane",
        "flex-center",
        lane.current ? "current" : "",
        selectable && viewed ? "viewed" : "",
      ]
        .filter(Boolean)
        .join(" ")}
      role={selectable ? "radio" : undefined}
      aria-checked={selectable ? viewed : undefined}
      tabIndex={selectable ? 0 : undefined}
      onClick={selectable ? onView : undefined}
      onKeyDown={selectable ? onKey : undefined}
    >
      {/* The branch glyph is lit on the lane the working tree is on — the
          lane whose changes and commits the ladder counts. */}
      <span
        className={lane.current && selectable ? "ln-tree tip" : "ln-tree"}
        data-tip={lane.current && selectable ? "The working tree is on this branch" : undefined}
      >
        <Icon name="branch" size={12} />
      </span>
      <span className="ln-branch text-sm" title={lane.branch ?? undefined}>
        {lane.branch ?? "no branch yet"}
      </span>
      {base && <span className="ln-base text-xs">→ {base}</span>}
      {pr && tint ? (
        <button
          type="button"
          className="ln-pr"
          title={prTip}
          aria-label={prTip}
          onClick={(e) => {
            e.stopPropagation();
            void open(pr.state.url);
          }}
        >
          <Badge variant={tint.variant}>
            <Icon name={pr.state.state === "merged" ? "merge" : "pr"} size={10} />#{pr.state.number}
          </Badge>
        </button>
      ) : (
        // The slot a PR will fill: dashed, so the absence is a shape, not a gap.
        <span className="ln-pr-none text-xs">no PR</span>
      )}
      <span className="ln-meta flex-center">
        {(adds > 0 || dels > 0) && (
          <span className="ln-diff text-xs">
            {adds > 0 && <span className="add">+{adds}</span>}
            {dels > 0 && <span className="rem">−{dels}</span>}
          </span>
        )}
        {trailing}
      </span>
    </div>
  );
}
