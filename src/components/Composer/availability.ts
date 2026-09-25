// Whether an agent can be spawned, and why not.
//
// One rule, read by everything that offers an agent (the composer's picker, the
// Roadmap's new-chat screen) and by everything that acts on one (the send
// button, and `send()` itself). A rule that decides what the user is allowed to
// start must not exist twice, and must not exist in two versions that disagree
// about the same draft.
//
// The first question is always *which machine the agent would run on*, because
// the answer decides which facts are even relevant. An agent spawns where the
// active environment is, so:
//
//   - This Mac: this Mac's probe (`providerPaths`) and this Mac's sandbox
//     engine (`isDockerSupported`) — the rules that have always applied.
//   - A paired host: the host's own `host_providers` answer, and nothing else.
//     This Mac's sandbox preference governs sandboxes *here*; it has no say
//     over a process on someone else's machine, and letting it veto one would
//     hide an agent the host can run perfectly well.

import { isDockerSupported, providerLabel } from "@/data/providers";
import { isContainerEngine, type SandboxEngine, sandboxEngineLabel } from "@/storage/preferences";
import { useAppStore } from "@/store";
import { activeEntry, providerReason } from "@/store/capabilities";
import type { EnvironmentEntry } from "@/store/environments";

export interface Availability {
  /** Why this agent can't be spawned right now, or null when it can. Rendered
   *  as the row's refusal tooltip. */
  reason: string | null;
  /** The short status word the row carries on its right: the probed CLI version
   *  normally, the refusal in brief when there is one. */
  note: string;
  /** What to do about `reason`, for a control that would otherwise *act* — the
   *  send button says "…yet — switch to Claude to send" where the picker row
   *  just says why. Null exactly when `reason` is. */
  fix: string | null;
  /** Whether the agent's binary exists on the machine it would run on. False
   *  only for the "not installed" refusal: an agent that is present but
   *  blocked for another reason (signed out on the host, no container image)
   *  is still `installed`, and a surface that lists agents shows it, greyed,
   *  with that reason — where an absent one is left out of the list entirely. */
  installed: boolean;
}

/** Spawnable: no reason, no fix, and whatever version there is to show. */
const open = (note: string): Availability => ({
  reason: null,
  note,
  fix: null,
  installed: true,
});

/** The facts an availability decision is made from. Everything the hook reads
 *  out of the store, named, so the rule itself is a pure function of them and
 *  can be tested without a store or a React tree. */
export interface AvailabilityFacts {
  /** Where the agent would run. The first thing consulted, because it decides
   *  which of the rest apply. */
  env: EnvironmentEntry;
  /** This Mac's, and only ever consulted for This Mac. */
  sandboxEngine: SandboxEngine;
  providerPaths: Record<string, string | undefined>;
  providerVersions: Record<string, string | undefined>;
  providersProbed: boolean;
}

/** Whether `providerId` can be spawned in `facts.env`, and why not.
 *
 *  Pass a custom agent's `base` to gate it: a custom agent inherits its base
 *  provider's availability exactly. */
export function agentAvailability(facts: AvailabilityFacts, providerId: string): Availability {
  return facts.env.kind === "remote" ? onHost(facts.env, providerId) : onThisMac(facts, providerId);
}

/** The host's answer, and only the host's. Fails open exactly as this Mac's
 *  probe does: a host that has not reported its providers, or does not answer
 *  `host_providers` at all, blocks nothing and shows no version. */
function onHost(env: EnvironmentEntry, providerId: string): Availability {
  const reason = providerReason(env, providerId);
  const row = env.providers?.find((p) => p.id === providerId);
  if (!reason) return open(row?.version ?? "");
  const installed = !!row?.installed;
  return {
    reason,
    note: installed ? "Signed out" : "Not installed",
    // No `fix`: `providerReason` already ends in the remedy (the command to
    // run on the host, or where the credential has to come from), and the
    // desktop has nothing to add — installing and signing in are never on the
    // wire. A second clause here would only read as two dashes in a row.
    fix: null,
    installed,
  };
}

function onThisMac(facts: AvailabilityFacts, providerId: string): Availability {
  const { sandboxEngine, providerPaths, providerVersions, providersProbed } = facts;
  // Fail open on the install gate: only enforce it once a probe has actually
  // succeeded (`providersProbed`). While probing, or if the probe failed,
  // treat as installed so a transient detection error never disables an agent
  // the user really has — nor hides it from a list.
  const installed = !providersProbed || !!providerPaths[providerId];
  // The container gate is checked first: a non-container provider is blocked
  // regardless of install state, and that's the more useful reason. Mirrors
  // `ensure_engine_supports_provider`, which gates on `is_container()` rather
  // than on one runtime — container support is a property of the image set
  // both Docker and Podman build. `installed` is still the probe's answer,
  // though: an absent binary is absent whichever refusal it is reported under.
  if (isContainerEngine(sandboxEngine) && !isDockerSupported(providerId)) {
    const label = sandboxEngineLabel(sandboxEngine);
    return {
      reason: `${providerLabel(providerId)} isn't available in ${label} sandboxes yet`,
      note: `Not in ${label} yet`,
      fix: "switch to Claude to send",
      installed,
    };
  }
  if (!installed) {
    return {
      reason: "Not installed — see Settings › Providers",
      note: "Not installed",
      fix: "install it in Settings › Providers",
      installed: false,
    };
  }
  return open(providerVersions[providerId] ?? "");
}

/** The whole refusal as one sentence, for a control that acts on it. Null when
 *  the agent can be spawned. */
export function blockedSentence(a: Availability): string | null {
  if (!a.reason) return null;
  return a.fix ? `${a.reason} — ${a.fix}` : a.reason;
}

/** [`agentAvailability`] bound to the live store. */
export function useAgentAvailability(): (providerId: string) => Availability {
  const facts: AvailabilityFacts = {
    env: useAppStore(activeEntry),
    sandboxEngine: useAppStore((s) => s.sandboxEngine),
    providerPaths: useAppStore((s) => s.providerPaths),
    providerVersions: useAppStore((s) => s.providerVersions),
    providersProbed: useAppStore((s) => s.providersProbed),
  };
  return (providerId: string) => agentAvailability(facts, providerId);
}
