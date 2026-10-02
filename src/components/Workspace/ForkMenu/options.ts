import type { ForkCode, ForkContext } from "@/api";
import { hasTranscriptWriter, providerLabel } from "@/data/providers";

/** One selectable value of a fork axis, with how the menu reads it and why it
 *  can't be picked (null when it can). */
export interface ForkChoice<T> {
  value: T;
  label: string;
  reason: string | null;
}

/** What a fork is anchored on: a message (the menu under a turn), or the
 *  whole conversation (the workspace header). */
export type ForkScope = "message" | "conversation";

/** How the forked agent knows the conversation up to the anchor, for an agent
 *  of `provider`: the full conversation, resumed as its own transcript, needs
 *  a provider Fletch can write transcripts for; a summary works for any. */
export function contextChoices(
  scope: ForkScope,
  provider: string | undefined,
): ForkChoice<ForkContext>[] {
  const message = scope === "message";
  return [
    {
      value: "full",
      label: message ? "Full conversation up to here" : "Full conversation",
      reason: hasTranscriptWriter(provider)
        ? null
        : `${providerLabel(provider)} can only carry a summary.`,
    },
    {
      value: "summary",
      label: message ? "Summary up to here" : "Summary of the conversation",
      reason: null,
    },
  ];
}

/** What the forked workspace's code starts from. Only a message has code of
 *  its own to go back to. */
export function codeChoices(scope: ForkScope): ForkChoice<ForkCode>[] {
  const choices: ForkChoice<ForkCode>[] = [
    { value: "clean", label: "Clean from base", reason: null },
    { value: "current", label: "Current code", reason: null },
  ];
  return scope === "message"
    ? [...choices, { value: "at_message", label: "Code as of this message", reason: null }]
    : choices;
}

/** The context a menu opens with: the full conversation where it can be
 *  carried, else its summary. */
export function defaultContext(provider: string | undefined): ForkContext {
  return hasTranscriptWriter(provider) ? "full" : "summary";
}

export const DEFAULT_CODE: ForkCode = "clean";
