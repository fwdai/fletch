// The row a sub-agent launch gets in a transcript. Not a tool row: a tool
// call is something the agent did, a sub-agent is a conversation of its own
// that happened here — so the card reads as a thread entry (rail, mark,
// title, live status) whose primary action is opening that thread, with the
// launch prompt and the sub-agent's report one disclosure away for the quick
// check. Inside a thread the same card opens the thread below it.

import { useRef, useState } from "react";
import { Icon } from "@/components/Icon";
import { Loader } from "@/components/ui/Loader";
import { type BackgroundTask, useAppStore } from "@/store";
import { useMinuteClock } from "@/util/hooks";
import {
  type ThreadState,
  threadLabel,
  threadState,
  threadStatusText,
  threadType,
} from "../SubagentThread/thread";
import { agentPresenter } from "./presenters/Agent";
import type { ToolCall, ToolResult } from "./presenters/types";
import { useThreadPath } from "./threadPath";
import { useChatFocus } from "./useChatFocus";

export function SubagentCard({
  call,
  result,
  task,
  agentId,
  busy,
}: {
  call: ToolCall;
  result: ToolResult | null;
  /** The background task this launch started, if it was backgrounded. */
  task: BackgroundTask | undefined;
  agentId?: string;
  /** The launching agent is mid-turn (see MessageItem's `busy`). */
  busy?: boolean;
}) {
  const openSubagentThread = useAppStore((s) => s.openSubagentThread);
  const path = useThreadPath();
  const [details, setDetails] = useState(false);
  // A reveal (back from the thread, or the sidebar) rings the card briefly
  // instead of expanding it — the user is returning, not inspecting.
  const [flash, setFlash] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);
  useChatFocus(
    agentId,
    call.id,
    () => rootRef.current,
    () => setFlash(true),
  );

  const state = threadState(result, task, Boolean(busy));
  const label = threadLabel(call);
  const type = threadType(call);
  const open = () => {
    if (agentId) openSubagentThread(agentId, [...path, call.id]);
  };

  return (
    <div
      ref={rootRef}
      className={`m-subagent ${state}${flash ? " flash" : ""}`}
      data-tool-use-id={call.id}
      onAnimationEnd={() => setFlash(false)}
    >
      <div className="sa-row flex-center">
        <button
          type="button"
          className="sa-open flex-center"
          onClick={open}
          disabled={!agentId}
          aria-label={`Open thread: ${label}`}
        >
          {/* The kind and type live on the mark's tooltip; the row leads with
              the name. The type is spelled out on the thread's own header. */}
          <span
            className="sa-icon tip"
            data-tip={type ? `Sub-agent · ${type}` : "Sub-agent"}
            aria-label={type ? `Sub-agent, ${type}` : "Sub-agent"}
          >
            <Icon name="subagent" size={13} />
          </span>
          <span className="sa-title" title={label}>
            {label}
          </span>
          <ThreadStatus state={state} task={task} />
          <span className="sa-cta">
            Open thread
            <Icon name="chevR" size={12} />
          </span>
        </button>
        <button
          type="button"
          className="sa-toggle"
          aria-expanded={details}
          aria-label={details ? "Hide prompt and report" : "Show prompt and report"}
          onClick={() => setDetails((d) => !d)}
        >
          {details ? "▾" : "▸"}
        </button>
      </div>
      {details && <div className="sa-details">{agentPresenter.expanded(call, result)}</div>}
    </div>
  );
}

/** Status beside the title: how long it ran and what it cost, or how it
 *  failed; while running, what it is on. Step counts are left to the thread's
 *  header — the row has room for the two numbers that matter. The running
 *  variant ticks a minute clock for the "quiet" hint, so it is its own
 *  component and only live cards mount it. */
function ThreadStatus({ state, task }: { state: ThreadState; task: BackgroundTask | undefined }) {
  if (state === "running") return <LiveStatus task={task} />;
  return <span className="sa-status">{threadStatusText(state, 0, task, 0)}</span>;
}

function LiveStatus({ task }: { task: BackgroundTask | undefined }) {
  const now = useMinuteClock();
  return (
    <span className="sa-status">
      <Loader variant="muted" size="sm" aria-label="Sub-agent working" />
      {threadStatusText("running", 0, task, now)}
    </span>
  );
}
