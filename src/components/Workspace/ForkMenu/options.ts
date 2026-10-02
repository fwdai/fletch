import type { ForkCode, ForkContext } from "@/api";

/** One selectable value of a fork axis, with how the menu reads it. */
export interface ForkChoice<T> {
  value: T;
  label: string;
}

/** What a fork is anchored on: a message (the menu under a turn), or the
 *  whole conversation (the workspace header). */
export type ForkScope = "message" | "conversation";

/** What the forked agent knows of the conversation up to the anchor. */
export function contextChoices(scope: ForkScope): ForkChoice<ForkContext>[] {
  return [
    { value: "none", label: "Fresh conversation" },
    {
      value: "summary",
      label: scope === "message" ? "Summary up to here" : "Summary of the conversation",
    },
  ];
}

/** What the forked workspace's code starts from. Only a message has code of
 *  its own to go back to. */
export function codeChoices(scope: ForkScope): ForkChoice<ForkCode>[] {
  const choices: ForkChoice<ForkCode>[] = [
    { value: "clean", label: "Clean from base" },
    { value: "current", label: "Current code" },
  ];
  return scope === "message"
    ? [...choices, { value: "at_message", label: "Code as of this message" }]
    : choices;
}

/** The selection a menu opens with. */
export const DEFAULT_CONTEXT: ForkContext = "summary";
export const DEFAULT_CODE: ForkCode = "clean";
