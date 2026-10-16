import { open } from "@tauri-apps/plugin-shell";
import { type KeyboardEvent, type ReactNode, useRef } from "react";
import { Icon } from "@/components/Icon";
import { Badge } from "@/components/ui/Badge";
import { prTint } from "@/util/prSummary";
import type { Lane } from "./laneModel";

// ── The lanes: the panel's header ────────────────────────────────────────────
// One row per branch-and-PR of the workspace (see `laneModel.ts`). With one
// lane the row simply is the header: branch, base, the PR chip, the tree's
// diff. With several, the rows are a radio group — the viewed lane is the one
// the ladder below describes — and the row carrying the autopilot switch is
// the first, the focused PR's whenever there is one. Viewing a lane is a local
// choice; it never moves the host's focus (that is the footer's "Focus this
// PR", explicit). In a multi-repo workspace each lane says its repo.
//
// `LaneRow` is also the title bar popover's row, so a PR reads the same there.
//
// The group is a radio group proper: one tab stop, on the viewed lane, and the
// arrow keys move the view (Home and End to the ends). Enter and Space on a
// row view it too, for anyone who tabbed in and reached for them.

export function Lanes({
  lanes,
  viewedId,
  onView,
  controls,
}: {
  lanes: Lane[];
  viewedId: string | null;
  onView: (id: string) => void;
  /** The workspace's autopilot switch — rendered on the first lane. */
  controls?: ReactNode;
}) {
  const selectable = lanes.length > 1;
  const groupRef = useRef<HTMLDivElement>(null);

  // Arrows move the view and the focus together, wrapping at the ends. Only
  // from a radio itself: the PR chip and the autopilot switch beside it keep
  // their keys.
  const onGroupKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if ((e.target as HTMLElement).getAttribute("role") !== "radio") return;
    const at = lanes.findIndex((l) => l.id === viewedId);
    const step = (to: number) => {
      const next = lanes[(to + lanes.length) % lanes.length];
      if (!next) return;
      onView(next.id);
      groupRef.current
        ?.querySelectorAll<HTMLElement>('[role="radio"]')
        [(to + lanes.length) % lanes.length]?.focus();
    };
    switch (e.key) {
      case "ArrowDown":
      case "ArrowRight":
        step(at + 1);
        break;
      case "ArrowUp":
      case "ArrowLeft":
        step(at - 1);
        break;
      case "Home":
        step(0);
        break;
      case "End":
        step(lanes.length - 1);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  return (
    <div
      ref={groupRef}
      className={`git-lanes ${selectable ? "pick" : ""}`}
      role={selectable ? "radiogroup" : undefined}
      aria-label="Branches and pull requests"
      onKeyDown={selectable ? onGroupKey : undefined}
    >
      {lanes.map((lane, i) => (
        <LaneRow
          key={lane.id}
          lane={lane}
          viewed={lane.id === viewedId}
          selectable={selectable}
          onView={() => onView(lane.id)}
          trailing={i === 0 ? controls : null}
        />
      ))}
    </div>
  );
}

export function LaneRow({
  lane,
  viewed = false,
  selectable = false,
  onView,
  trailing,
}: {
  lane: Lane;
  viewed?: boolean;
  selectable?: boolean;
  onView?: () => void;
  trailing?: ReactNode;
}) {
  const { pr, git } = lane;
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

  // The radio is the lane's name — glyph, repo, branch, base — a real button,
  // so Enter and Space are its own. The PR chip and the trailing controls are
  // its siblings, never its children: each keeps its own keys and focus.
  const name = (
    <>
      {/* The branch glyph is lit on the lane the working tree is on — the
          lane whose changes and commits the ladder counts. */}
      <span
        className={lane.current && selectable ? "ln-tree tip" : "ln-tree"}
        data-tip={lane.current && selectable ? "The working tree is on this branch" : undefined}
      >
        <Icon name="branch" size={12} />
      </span>
      {lane.scope && <span className="ln-repo text-xs">{lane.scope.label}</span>}
      <span className="ln-branch text-sm" title={lane.branch ?? undefined}>
        {lane.branch ?? "no branch yet"}
      </span>
      {base && <span className="ln-base text-xs">→ {base}</span>}
    </>
  );

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
      // A click on the row's blank space views the lane too; a click on any
      // button in it — the radio, the chip, the switch — is that button's.
      onClick={
        selectable
          ? (e) => {
              if ((e.target as Element).closest("button") == null) onView?.();
            }
          : undefined
      }
    >
      {selectable ? (
        <button
          type="button"
          className="ln-pick flex-center"
          role="radio"
          aria-checked={viewed}
          // Roving tab stop: only the viewed lane is in the tab order.
          tabIndex={viewed ? 0 : -1}
          onClick={onView}
        >
          {name}
        </button>
      ) : (
        <span className="ln-pick flex-center">{name}</span>
      )}
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
