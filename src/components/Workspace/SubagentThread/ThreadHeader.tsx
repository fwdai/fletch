// The thread's header strip: the workspace header's skeleton (dot, title
// line, meta line) with the title line grown into a breadcrumb. The agent's
// title — in exactly the place it sits on the conversation's header, so it
// does not move when a thread opens — is the way back (hover says so); every
// deeper crumb is a jump to that thread. The meta line carries
// the thread's type and status where the conversation shows branch and diff.
// When the launching log holds more than one sub-agent, a stepper on the
// right moves through its siblings.

import { Fragment } from "react";
import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { PanelToggle } from "@/components/PanelToggle";
import { IconButton } from "@/components/ui/IconButton";
import { type BackgroundTask, useAppStore } from "@/store";
import { useMinuteClock } from "@/util/hooks";
import { agentTitle, type DotTone, StatusDot } from "../WorkspaceHeader";
import {
  type ResolvedThread,
  type ThreadState,
  threadLabel,
  threadStatusText,
  threadSteps,
  threadType,
} from "./thread";

const DOT_TONE: Record<ThreadState, DotTone> = {
  running: "running",
  failed: "error",
  done: "idle",
};

export function ThreadHeader({
  agent,
  path,
  thread,
  state,
  task,
}: {
  agent: AgentRecord;
  path: string[];
  thread: ResolvedThread | null;
  state: ThreadState;
  task: BackgroundTask | undefined;
}) {
  const openSubagentThread = useAppStore((s) => s.openSubagentThread);
  const closeSubagentThread = useAppStore((s) => s.closeSubagentThread);
  const now = useMinuteClock();

  const siblings = thread?.siblings ?? [];
  const index = thread?.index ?? -1;
  const prev = index > 0 ? siblings[index - 1] : undefined;
  const next = index >= 0 && index < siblings.length - 1 ? siblings[index + 1] : undefined;
  const goTo = (id: string) => openSubagentThread(agent.id, [...path.slice(0, -1), id]);

  const type = thread ? threadType(thread.call) : "";
  const status = thread ? threadStatusText(state, threadSteps(thread.items), task, now) : "";
  const title = agentTitle(agent);

  return (
    <div className="center-h flex-center thread-h">
      <PanelToggle side="left" />

      <div className="task">
        <div className="t-name">
          <StatusDot tone={DOT_TONE[state]} />
          <nav className="thread-crumbs" aria-label="Thread path">
            <button
              type="button"
              className="crumb tip"
              data-tip="Back to conversation"
              onClick={closeSubagentThread}
            >
              <span className="crumb-text" title={title}>
                {title}
              </span>
            </button>
            {thread ? (
              thread.trail.map((call, i) => {
                const label = threadLabel(call);
                const last = i === thread.trail.length - 1;
                return (
                  <Fragment key={call.id}>
                    <span className="crumb-sep" aria-hidden="true">
                      ›
                    </span>
                    {last ? (
                      <span className="crumb current" aria-current="page">
                        <span className="crumb-text" title={label}>
                          {label}
                        </span>
                      </span>
                    ) : (
                      <button
                        type="button"
                        className="crumb"
                        onClick={() => openSubagentThread(agent.id, path.slice(0, i + 1))}
                      >
                        <span className="crumb-text" title={label}>
                          {label}
                        </span>
                      </button>
                    )}
                  </Fragment>
                );
              })
            ) : (
              <>
                <span className="crumb-sep" aria-hidden="true">
                  ›
                </span>
                <span className="crumb current">
                  <span className="crumb-text">Sub-agent</span>
                </span>
              </>
            )}
          </nav>
        </div>
        <div className="t-meta">
          {type && <>{type} · </>}
          <span className={`thread-status ${state}`}>{status}</span>
        </div>
      </div>

      {siblings.length > 1 && (
        <div className="thread-nav flex-center" role="group" aria-label="Sub-agents in this log">
          <IconButton
            size="sm"
            tip={prev ? `Previous: ${threadLabel(prev)}` : undefined}
            aria-label="Previous sub-agent"
            disabled={!prev}
            onClick={() => prev && goTo(prev.id)}
          >
            <Icon name="chevU" />
          </IconButton>
          <span className="thread-count">
            {index + 1} / {siblings.length}
          </span>
          <IconButton
            size="sm"
            tip={next ? `Next: ${threadLabel(next)}` : undefined}
            aria-label="Next sub-agent"
            disabled={!next}
            onClick={() => next && goTo(next.id)}
          >
            <Icon name="chevD" />
          </IconButton>
        </div>
      )}

      <PanelToggle side="right" />
    </div>
  );
}
