import type { MergeState } from "../../api";

// ── Merge-gate semantics: the single source of truth ──────────────────────
// GitHub's combined merge gate (`mergeStateStatus`, spec §6) feeds three
// surfaces — the status header, the PR card, and the action bar — each of which
// needs to know how merge-ready a PR is and in what tone to say so. Classifying
// `MergeState` independently in each surface meant a backend gate change had to
// be mirrored in three (then four) places and could silently drift. This module
// owns that classification once; surfaces render their own copy off the stable
// `situation`/`tone`, never off the raw `MergeState`.

/** Canonical merge-gate situation — stable across backend gate-semantics
 *  changes, so surfaces switch on this rather than on raw `MergeState`. */
export type MergeGateSituation =
  | "ready" // clean — green light, merge now
  | "mergeable-soft" // unstable — only optional (non-required) checks failing
  | "checks-failing" // blocked by failing required checks — agent-fixable
  | "review-required" // blocked purely by a review/other gate — send to GitHub
  | "behind" // behind base — update the branch
  | "conflicts" // dirty — conflicts with base, update the branch
  | "draft" // draft — mark ready on GitHub before it can merge
  | "computing" // unknown/has_hooks — gate still resolving
  | "no-conflicts" // no checks data, `mergeable` only → no conflicts (not an all-clear)
  | "cant-merge"; // no checks data, not mergeable → can't merge yet

/** Shared severity. A subset of `StatusKind`/`HeaderKind` so every surface can
 *  derive its own tone class from one decision. */
export type MergeGateTone = "ready" | "warn" | "attention" | "info";

export interface MergeGate {
  situation: MergeGateSituation;
  tone: MergeGateTone;
  /** The merge CTA is actually clickable (gate open). Drives the disabled
   *  state of a "Merge" button regardless of whether it's the primary action. */
  mergeAllowed: boolean;
  /** Branch is out of sync with base (behind / conflicting / unmergeable
   *  fallback); gates the "Update branch with agent" remediation. */
  needsUpdate: boolean;
}

export interface MergeGateContext {
  /** Number of failing required checks — splits `blocked` into agent-fixable
   *  (checks failing) vs. a pure review gate. */
  checksFailed: number;
  /** `PrState.mergeable` — the only signal when `merge_state` is unavailable; it
   *  reports the absence of conflicts, never CI status. */
  mergeable: boolean;
}

/** Map GitHub's combined merge gate to the canonical situation + tone every
 *  surface renders from. Pass `mergeState: null` (no checks data) to get the
 *  conservative `mergeable`-only fallback. */
export function describeMergeGate(
  mergeState: MergeState | null,
  { checksFailed, mergeable }: MergeGateContext,
): MergeGate {
  switch (mergeState) {
    case "clean":
      return { situation: "ready", tone: "ready", mergeAllowed: true, needsUpdate: false };
    case "unstable":
      return { situation: "mergeable-soft", tone: "warn", mergeAllowed: true, needsUpdate: false };
    case "blocked":
      // Failing required checks are agent-fixable; a pure review gate is not.
      return checksFailed > 0
        ? {
            situation: "checks-failing",
            tone: "attention",
            mergeAllowed: false,
            needsUpdate: false,
          }
        : {
            situation: "review-required",
            tone: "attention",
            mergeAllowed: false,
            needsUpdate: false,
          };
    case "behind":
      return { situation: "behind", tone: "attention", mergeAllowed: false, needsUpdate: true };
    case "dirty":
      return { situation: "conflicts", tone: "attention", mergeAllowed: false, needsUpdate: true };
    case "draft":
      return { situation: "draft", tone: "info", mergeAllowed: false, needsUpdate: false };
    case "unknown":
    case "has_hooks":
      return { situation: "computing", tone: "info", mergeAllowed: false, needsUpdate: false };
    default:
      // No checks data — `mergeable` only reports the absence of merge
      // conflicts, NOT CI status, so claim no more than that.
      return mergeable
        ? { situation: "no-conflicts", tone: "info", mergeAllowed: true, needsUpdate: false }
        : { situation: "cant-merge", tone: "attention", mergeAllowed: false, needsUpdate: true };
  }
}
