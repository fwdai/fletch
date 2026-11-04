// The seam between the transcript rows and the app around them. The rows —
// runs of steps, tool rows, thoughts, the presenters — are shared by the
// desktop chat and the phone verbatim; what differs is where background tasks
// live, how a "reveal this row" request arrives, and which markdown component
// renders prose. Each app provides those once, here, instead of the rows
// reaching into a store.

import { type ComponentType, createContext, type ReactNode, useContext } from "react";
import type { BackgroundTask } from "@/adapters/shared/backgroundTasks";
import type { ViewItem } from "./pair";

export interface TranscriptHostValue {
  /** The background task a tool call launched (a dev server, a watcher), if
   *  any — the row keeps saying "running" after its launch result lands. */
  taskFor: (toolUseId: string) => BackgroundTask | undefined;
  /** The row the app asked the transcript to reveal: the sub-agent card the
   *  user just came back from, a row the sidebar named. Null nearly always. */
  focus: { agentId: string; toolUseId: string } | null;
  /** Consume the request once the row has opened and scrolled into view. */
  clearFocus: () => void;
  /** Markdown for prose a presenter shows (a sub-agent's report). */
  Markdown: ComponentType<{ text: string }>;
}

function PlainText({ text }: { text: string }) {
  return <div style={{ whiteSpace: "pre-wrap" }}>{text}</div>;
}

/** Without a provider: no tasks, no reveal requests, prose as plain text. */
export const TranscriptHost = createContext<TranscriptHostValue>({
  taskFor: () => undefined,
  focus: null,
  clearFocus: () => {},
  Markdown: PlainText,
});

export function useTranscriptHost(): TranscriptHostValue {
  return useContext(TranscriptHost);
}

/** What every row needs from the transcript it sits in. It changes with every
 *  streaming delta, so a closed run of steps stays off it and only reads it
 *  once opened: settled history doesn't re-render as the live turn streams. */
export interface RowEnvValue {
  agentId: string;
  provider: string | undefined;
  items: ViewItem[];
  liveBusy: boolean;
  openTurnStart: number;
  /** Key of the run the transcript ends on, if it ends on one. */
  tailRunKey: string | null;
  /** The app's own rendering of one item that isn't a step — a prompt, the
   *  agent's prose, a sub-agent card, a question for the user. `busy` is
   *  whether the item's turn is live. */
  renderItem: (index: number, busy: boolean) => ReactNode;
}

export const RowEnv = createContext<RowEnvValue | null>(null);

/** Prose rendered by the host's markdown component. */
export function HostMarkdown({ text }: { text: string }) {
  const { Markdown } = useTranscriptHost();
  return <Markdown text={text} />;
}
