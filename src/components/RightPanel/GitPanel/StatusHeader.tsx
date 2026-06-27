import type { GitState, MergeState, PrState } from "../../../api";
import type { GitPanelState } from "../primaryActions";
import { ViewOnGitHub } from "./shared";

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
      // GitHub's combined merge gate (spec §7): the legitimate green "ready"
      // appears only on `clean`. Without checks data, fall back to
      // `mergeable` — which only means "no conflicts", never an all-clear.
      const pill = n != null ? `PR #${n}` : "PR";
      switch (mergeState) {
        case "clean":
          return { kind: "ready", pill, text: "ready to merge", ext: true };
        case "unstable":
          return { kind: "changes", pill, text: "optional checks failing", ext: true };
        case "blocked":
          return {
            kind: "att",
            pill,
            text: checksFailed > 0 ? "checks failing" : "review required",
            ext: true,
          };
        case "behind":
          return { kind: "att", pill, text: `behind ${base}`, ext: true };
        case "dirty":
          return { kind: "att", pill, text: `conflicts with ${base}`, ext: true };
        case "draft":
          return { kind: "info", pill, text: "draft", ext: true };
        case "unknown":
        case "has_hooks":
          return { kind: "info", pill, text: "checking…", ext: true };
        default:
          return pr?.mergeable
            ? { kind: "info", pill, text: "no conflicts", ext: true }
            : { kind: "att", pill, text: "can’t merge yet", ext: true };
      }
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
        {h.ext && pr?.url && <ViewOnGitHub href={pr.url} className="hdr-ext" size={13} />}
      </div>
    </div>
  );
}
