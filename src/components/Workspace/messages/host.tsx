// The seam between the transcript rows and the app around them. The rows —
// runs of steps, folds, tool rows, thoughts, the presenters — are shared by the
// desktop chat and the phone verbatim; what differs is where background tasks
// live, how a "reveal this row" request arrives, and which markdown component
// renders prose. Each app provides those once, here, instead of the rows
// reaching into a store.

import { type ComponentType, createContext, useContext } from "react";
import type { BackgroundTask } from "@/adapters/shared/backgroundTasks";

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

/** Prose rendered by the host's markdown component. */
export function HostMarkdown({ text }: { text: string }) {
  const { Markdown } = useTranscriptHost();
  return <Markdown text={text} />;
}
