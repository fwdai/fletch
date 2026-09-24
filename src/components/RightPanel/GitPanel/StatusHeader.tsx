import { open } from "@tauri-apps/plugin-shell";
import type { ReactNode } from "react";
import type { GitState, MergeState, PrState } from "@/api";
import { Icon } from "@/components/Icon";
import type { GitPanelState } from "@/components/RightPanel/primaryActions";
import { describeMergeGate, type MergeGateTone, mergeGateLabel } from "@/mergeGate";
import { ViewOnGitHub } from "./shared";

// ── Color-coded status header ─────────────────────────────────────
// The panel's at-a-glance state signal: a tinted strip whose color carries the
// state before any word is read. clean=green · uncommitted=amber · pushed/PR=
// blue · fixable (can't merge / conflicts)=orange · ready=green · merged=purple.
type HeaderKind = "clean" | "changes" | "info" | "att" | "ready" | "merged" | "neutral";

/** Render the shared merge-gate tone as a header kind. */
const HEADER_KIND_BY_TONE: Record<MergeGateTone, HeaderKind> = {
  ready: "ready",
  warn: "changes",
  attention: "att",
  info: "info",
};

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
  /** Show a trailing ↗ link to the PR on GitHub — for states whose pill is NOT
   *  the PR (uncommitted work / a push on a branch that has one). */
  ext?: boolean;
  /** The pill names the PR, so the pill IS the link: "PR #7 ↗". The one place
   *  the panel links out to the PR itself, so it sits at the top. */
  pillLink?: boolean;
}

export function describeHeader(
  state: GitPanelState,
  branch: string,
  base: string,
  pr: PrState | null,
  mergeState: MergeState | null,
  checksFailed: number,
): HeaderInfo {
  const n = pr?.number;
  // Keep the bound PR's GitHub link reachable from the header while new work
  // takes over the panel — whether that PR is still open (a push updates it) or
  // already merged (this is follow-up work, and the merged PR is its context).
  const prLink = pr?.state === "open" || pr?.state === "merged";
  switch (state) {
    case "loading":
      return { kind: "neutral", text: "Loading…" };
    case "changes":
      return {
        kind: "changes",
        pill: "Uncommitted",
        text: branch,
        diff: true,
        ext: prLink,
      };
    case "pushed":
      return { kind: "info", pill: "Pushed", text: branch, ext: prLink };
    case "conflicts":
      return { kind: "att", pill: "Conflicts", text: branch, sub: `← ${base}` };
    case "pr-open": {
      // Gate classification + tone live in describeMergeGate; the header only
      // picks its terse phrasing and renders the shared tone.
      const pill = n != null ? `PR #${n}` : "PR";
      const gate = describeMergeGate(mergeState, {
        checksFailed,
        mergeable: pr?.mergeable ?? "unknown",
      });
      return {
        kind: HEADER_KIND_BY_TONE[gate.tone],
        pill,
        text: mergeGateLabel(gate.situation, base),
        pillLink: true,
      };
    }
    case "pr-closed":
      return {
        kind: "neutral",
        pill: n != null ? `Closed #${n}` : "Closed",
        text: branch,
        pillLink: true,
      };
    case "merged":
      return {
        kind: "merged",
        pill: n != null ? `Merged #${n}` : "Merged",
        text: `→ ${base}`,
        pillLink: true,
      };
    default:
      return { kind: "clean", text: branch, sub: `← ${base}`, dot: true };
  }
}

/** A checkout Fletch will not run git in (`GitState.blocked_config`). Its git
 *  state is the last one read before the block, so nothing from it — branch,
 *  diff, merge gate — is shown: the header says what the body card says. */
const BLOCKED_HEADER: HeaderInfo = { kind: "att", pill: "Git paused", text: "blocking settings" };

export function StatusHeader({
  state,
  branch,
  base,
  git,
  pr,
  mergeState,
  checksFailed,
  blocked = false,
  controls,
}: {
  state: GitPanelState;
  branch: string;
  base: string;
  git: GitState | null;
  pr: PrState | null;
  mergeState: MergeState | null;
  checksFailed: number;
  /** The checkout's config blocks git; overrides `state`, whose data is stale. */
  blocked?: boolean;
  /** Quiet per-workspace controls for the trailing meta slot (the autopilot
   *  switch) — rendered after the state's own meta, so the GitHub link and the
   *  diff summary keep their places. */
  controls?: ReactNode;
}) {
  const h = blocked
    ? BLOCKED_HEADER
    : describeHeader(state, branch, base, pr, mergeState, checksFailed);
  const adds = git?.additions ?? 0;
  const dels = git?.deletions ?? 0;
  return (
    <div className={`git-hdr flex-center k-${h.kind}`}>
      {h.dot && <span className="hdr-dot" />}
      {h.pill &&
        (h.pillLink && pr?.url ? (
          <button
            type="button"
            className="pill pill-link text-xs"
            title="View on GitHub"
            onClick={() => void open(pr.url)}
          >
            {h.pill}
            <Icon name="external" size={10} />
          </button>
        ) : (
          <span className="pill text-xs">{h.pill}</span>
        ))}
      <span className="bn text-sm">{h.text}</span>
      {h.sub && <span className="base text-xs">{h.sub}</span>}
      <div className="hdr-meta">
        {h.diff && (adds > 0 || dels > 0) && (
          <span className="hdr-diff text-xs">
            {adds > 0 && <span className="add">+{adds}</span>}
            {dels > 0 && <span className="rem">−{dels}</span>}
          </span>
        )}
        {h.ext && pr?.url && <ViewOnGitHub href={pr.url} className="hdr-ext" size={13} />}
        {controls}
      </div>
    </div>
  );
}
