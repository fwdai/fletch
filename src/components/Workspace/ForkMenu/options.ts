import type { ForkCode, ForkContext } from "@/api";
import { hasTranscriptWriter, providerLabel } from "@/data/providers";

/** One selectable value of a fork axis, with how the menu reads it and why it
 *  can't be picked (null when it can). */
export interface ForkChoice<T> {
  value: T;
  label: string;
  reason: string | null;
}

/** How the forked agent knows the conversation up to the turn, for an agent of
 *  `provider`: the full conversation, resumed as its own transcript, needs a
 *  provider Fletch can write transcripts for; a summary works for any. */
export function contextChoices(provider: string | undefined): ForkChoice<ForkContext>[] {
  return [
    {
      value: "full",
      label: "Full conversation up to here",
      reason: hasTranscriptWriter(provider)
        ? null
        : `${providerLabel(provider)} can only carry a summary.`,
    },
    { value: "summary", label: "Summary up to here", reason: null },
  ];
}

/** What the forked workspace's code starts from. */
export const CODE_CHOICES: ForkChoice<ForkCode>[] = [
  { value: "clean", label: "Clean from base", reason: null },
  { value: "current", label: "Current code", reason: null },
  { value: "at_message", label: "Code as of this message", reason: null },
];

/** The context a menu opens with: the full conversation where it can be
 *  carried, else its summary. */
export function defaultContext(provider: string | undefined): ForkContext {
  return hasTranscriptWriter(provider) ? "full" : "summary";
}

export const DEFAULT_CODE: ForkCode = "clean";
