// SubagentThread — a sub-agent's run as a conversation of its own in the
// center pane: the launching agent's chat chrome (header strip, scrolling
// transcript) around the thread's rows, with the ways back and sideways —
// breadcrumb, sibling stepper, footer — that a nested log can't offer.
//
// The rows and their derivation are the chat's own (TranscriptList,
// useTranscriptFrom), so a thread renders exactly like the main log; only the
// source differs — a tool_call's `children` instead of the agent's log.

import { useMemo } from "react";
import type { AgentRecord } from "@/api";
import { Button } from "@/components/ui/Button";
import { Loader } from "@/components/ui/Loader";
import { taskForToolUse, useAppStore } from "@/store";
import { TranscriptList } from "../messages/TranscriptList";
import { ThreadPathProvider } from "../messages/threadPath";
import { isTurnPending } from "../messages/turnPending";
import { useHistoryLoad, useTranscriptFrom } from "../messages/useTranscript";
import { useLiveBusy } from "../useLiveBusy";
import { ThreadFooter } from "./ThreadFooter";
import { ThreadHeader } from "./ThreadHeader";
import { resolveThread, type ThreadState, threadResult, threadState } from "./thread";

export function SubagentThreadView({ agent, path }: { agent: AgentRecord; path: string[] }) {
  const log = useAppStore((s) => s.managedLogs[agent.id]);
  const loading = useAppStore((s) => s.transcriptLoading[agent.id] ?? false);
  const tasks = useAppStore((s) => s.backgroundTasks[agent.id]);
  // After a reload the chat hasn't mounted to pull the history; do it here.
  useHistoryLoad(agent);

  const thread = useMemo(() => resolveThread(log, path), [log, path]);
  const parentBusy = useLiveBusy(agent, false);
  const task = useMemo(() => taskForToolUse(tasks, thread?.call.id), [tasks, thread?.call.id]);
  const result = thread ? threadResult(thread.parent, thread.call) : null;
  const state: ThreadState = thread ? threadState(result, task, parentBusy) : "done";
  const transcript = useTranscriptFrom(agent, thread?.items, loading);
  const live = state === "running";

  return (
    <>
      <ThreadHeader agent={agent} path={path} thread={thread} state={state} task={task} />
      <div className="chat thread">
        {!thread ? (
          <ThreadMissing loading={loading || log === undefined} />
        ) : transcript.items.length === 0 ? (
          <ThreadEmpty live={live} />
        ) : (
          <ThreadPathProvider value={path}>
            <TranscriptList
              agent={agent}
              transcript={transcript}
              liveBusy={live}
              pending={live && isTurnPending(transcript.items)}
              hideNav
            />
          </ThreadPathProvider>
        )}
        <ThreadFooter agent={agent} state={state} />
      </div>
    </>
  );
}

/** The path leads nowhere in the log we have: history still loading, or the
 *  launch is gone (a rewind took it, or the thread outlived its agent's log). */
function ThreadMissing({ loading }: { loading: boolean }) {
  const close = useAppStore((s) => s.closeSubagentThread);
  if (loading) {
    return (
      <div className="writing flex-center thread-state">
        <Loader variant="accent" />
        <span>Loading transcript…</span>
      </div>
    );
  }
  return (
    <div className="empty-msg thread-state">
      <div className="et">Thread not found</div>
      <div>This sub-agent is no longer in the conversation.</div>
      <Button variant="outline" size="sm" onClick={close} style={{ marginTop: 12 }}>
        Back to conversation
      </Button>
    </div>
  );
}

/** The launch is in the log but nothing has threaded under it (yet). */
function ThreadEmpty({ live }: { live: boolean }) {
  return (
    <div className="empty-msg thread-state">
      <div className="et">{live ? "Starting…" : "Nothing recorded"}</div>
      <div>
        {live
          ? "The sub-agent's steps will appear here as it works."
          : "This sub-agent left no transcript of its own."}
      </div>
    </div>
  );
}
