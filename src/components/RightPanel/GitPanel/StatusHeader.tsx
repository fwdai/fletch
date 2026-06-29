import type { GitState, MergeState, PrState } from "../../../api";
import { describeMergeGate, type MergeGateTone } from "../mergeGate";
import type { GitPanelState } from "../primaryActions";
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

/** Terse merge-gate phrasing for the at-a-glance header. */
const HEADER_TEXT_BY_SITUATION: Record<
  ReturnType<typeof describeMergeGate>["situation"],
  (base: string) => string
> = {
  ready: () => "ready to merge",
  "mergeable-soft": () => "optional checks failing",
  "checks-failing": () => "checks failing",
  "review-required": () => "review required",
  behind: (base) => `behind ${base}`,
  conflicts: (base) => `conflicts with ${base}`,
  draft: () => "draft",
  computing: () => "checking…",
  "no-conflicts": () => "no conflicts",
  "cant-merge": () => "can’t merge yet",
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
  /** Show a trailing ↗ link to the PR on GitHub. */
  ext?: boolean;
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
  switch (state) {
    case "loading":
      return { kind: "neutral", text: "Loading…" };
    // With an open PR, keep its GitHub link reachable from the header even
    // while new uncommitted changes take over the panel.
    case "changes":
      return {
        kind: "changes",
        pill: "Uncommitted",
        text: branch,
        diff: true,
        ext: pr?.state === "open",
      };
    case "pushed":
      return { kind: "info", pill: "Pushed", text: branch };
    case "conflicts":
      return { kind: "att", pill: "Conflicts", text: branch, sub: `← ${base}` };
    case "pr-open": {
      // Gate classification + tone live in describeMergeGate; the header only
      // picks its terse phrasing and renders the shared tone.
      const pill = n != null ? `PR #${n}` : "PR";
      const gate = describeMergeGate(mergeState, {
        checksFailed,
        mergeable: !!pr?.mergeable,
      });
      return {
        kind: HEADER_KIND_BY_TONE[gate.tone],
        pill,
        text: HEADER_TEXT_BY_SITUATION[gate.situation](base),
        ext: true,
      };
    }
    case "pr-closed":
      return { kind: "neutral", pill: "Closed", text: n != null ? `#${n}` : "—", ext: true };
    case "merged":
      return {
        kind: "merged",
        pill: "Merged",
        text: n != null ? `#${n} → ${base}` : `→ ${base}`,
        ext: true,
      };
    default:
      return { kind: "clean", text: branch, sub: `← ${base}`, dot: true };
  }
}

export function StatusHeader({
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
    <div className={`git-hdr flex-center k-${h.kind}`}>
      {h.dot && <span className="hdr-dot" />}
      {h.pill && <span className="pill text-2xs">{h.pill}</span>}
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
      </div>
    </div>
  );
}
