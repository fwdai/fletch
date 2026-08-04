import type { Mergeable, MergeState } from "@/api";

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
  | "computing" // unknown/has_hooks, or no-checks + mergeable unknown — still resolving
  | "no-conflicts"; // no checks data, `mergeable` says mergeable → no conflicts, but CI unknown → not merge-ready (never an all-clear)

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

/** Terse phrasing for a situation — the words a chip or a header strip has room
 *  for ("checks failing", "behind main"). Lives here beside the classification
 *  because it is the same decision said out loud: two surfaces now render this
 *  vocabulary (the Git panel's status header and the roadmap card's gate chip),
 *  and a second copy is how "review required" on one screen becomes "blocked" on
 *  the other. The PR card's longer sentences stay its own — that is a different
 *  register, not a second answer.
 *
 *  `base` names the branch for the two situations that are *about* the base.
 *  Callers that don't know it (a board holding a PR number and no checkout) get
 *  the honest generic word rather than a wrong branch name. */
export function mergeGateLabel(situation: MergeGateSituation, base = "base"): string {
  switch (situation) {
    case "ready":
      return "ready to merge";
    case "mergeable-soft":
      return "optional checks failing";
    case "checks-failing":
      return "checks failing";
    case "review-required":
      return "review required";
    case "behind":
      return `behind ${base}`;
    case "conflicts":
      return `conflicts with ${base}`;
    case "draft":
      return "draft";
    case "computing":
      return "checking…";
    case "no-conflicts":
      return "no conflicts";
  }
}

export interface MergeGateContext {
  /** Number of failing required checks — splits `blocked` into agent-fixable
   *  (checks failing) vs. a pure review gate. */
  checksFailed: number;
  /** `PrState.mergeable` — the only signal when `merge_state` is unavailable. A
   *  tri-state that reports conflict presence, never CI status: `"unknown"`
   *  means GitHub hasn't computed mergeability yet, NOT that it can't merge. */
  mergeable: Mergeable;
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
      // No checks data — fall back to GitHub's coarse tri-state `mergeable`,
      // which reports conflict presence only (never CI status). Crucially,
      // only claim a conflict when GitHub actually reports one: `"unknown"`
      // means "not computed yet" (normal for a while after any push, and the
      // value a DB snapshot always carries), so render it as still-computing —
      // never a false "can't merge — update your branch".
      switch (mergeable) {
        case "mergeable":
          // No conflict — but that says nothing about CI, and with no
          // `merge_state` we have zero check knowledge. "No conflict" is not
          // "safe to merge" (required checks could be failing or unrun), so this
          // is deliberately NOT merge-ready: `mergeAllowed` stays false until a
          // real gate confirms it. The surfaces still say "no conflicts" — an
          // honest, not-an-all-clear signal — off the `no-conflicts` situation.
          return {
            situation: "no-conflicts",
            tone: "info",
            mergeAllowed: false,
            needsUpdate: false,
          };
        case "conflicting":
          return {
            situation: "conflicts",
            tone: "attention",
            mergeAllowed: false,
            needsUpdate: true,
          };
        default: // "unknown"
          return { situation: "computing", tone: "info", mergeAllowed: false, needsUpdate: false };
      }
  }
}
