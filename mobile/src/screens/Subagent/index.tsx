// A sub-agent's run as a screen of its own, pushed over the agent's chat the
// way Code and File are: the back button names where it came from, the edge
// swipe pops it, and the bar reads name / type · status. The body is the
// chat's own Transcript over the thread's rows, so it reads exactly like the
// conversation that launched it — and a sub-agent card inside it pushes the
// thread below. The foot stands where the composer would: a sub-agent takes
// its direction from the agent, not from here.

import { taskForToolUse } from "@desktop/adapters/shared/backgroundTasks";
import {
  resolveThread,
  threadLabel,
  threadResult,
  threadState,
  threadType,
} from "@desktop/adapters/shared/subagents";
import { Icon } from "@desktop/components/Icon";
import { useMemo } from "react";
import { applyPolicy, type ChatItem, getAdapter } from "../../adapters";
import { Nav } from "../../components/ui";
import { isAgentBusy } from "../../lib/agents";
import { fmtElapsed, useElapsed } from "../../lib/hooks";
import { joinThreadPath, splitThreadPath, tasksByToolUse, threadStatus } from "../../lib/thread";
import { useStickyScroll } from "../../lib/useStickyScroll";
import { agentOf, useStore } from "../../store";
import { Dots, Transcript } from "../Agent/Transcript";

const EMPTY: ChatItem[] = [];

export function SubagentScreen({ agentId, path: pathProp }: { agentId: string; path: string }) {
  const agent = useStore((s) => agentOf(s, agentId));
  const log = useStore((s) => s.logs[agentId]);
  const tasks = useStore((s) => s.backgroundTasks[agentId]);
  const busy = useStore((s) => (agent ? isAgentBusy(s, agent) : false));
  const push = useStore((s) => s.push);
  const pop = useStore((s) => s.pop);

  const path = useMemo(() => splitThreadPath(pathProp), [pathProp]);
  const thread = useMemo(() => resolveThread(log, path), [log, path]);
  const task = taskForToolUse(tasks, thread?.call.id);
  const result = thread ? threadResult(thread.parent, thread.call) : null;
  const state = thread ? threadState(result, task, busy) : "done";
  const live = state === "running";
  const elapsed = useElapsed(task?.startedAt, live);

  const policy = getAdapter(agent?.provider).policy;
  const items = thread?.items ?? EMPTY;
  const visible = useMemo(() => applyPolicy(items, policy), [items, policy]);
  const byToolUse = useMemo(() => tasksByToolUse(tasks), [tasks]);
  const { ref, onScroll } = useStickyScroll<HTMLDivElement>([items, state]);

  if (!agent) return null;

  // Back names the level above: the agent for a top-level thread, the parent
  // thread for a nested one — what the previous screen's bar said.
  const parentCall =
    thread && thread.trail.length > 1 ? thread.trail[thread.trail.length - 2] : null;
  const backLabel = parentCall ? threadLabel(parentCall) : agent.name;
  const title = thread ? threadLabel(thread.call) : "Sub-agent";
  const type = thread ? threadType(thread.call) : "";
  const status =
    live && task?.startedAt
      ? `${threadStatus(state, task, Date.now())} · ${fmtElapsed(elapsed)}`
      : threadStatus(state, task, Date.now());

  return (
    <>
      <Nav
        onBack={pop}
        backLabel={backLabel}
        title={title}
        sub={
          <>
            {state !== "done" && (
              <span
                className={`dot ${state === "running" ? "running" : "error"}`}
                style={{ width: 6, height: 6 }}
              />
            )}
            {type && <span>{type} ·</span>}
            {status}
          </>
        }
        hair
      />
      <div className="scroll chat thread" ref={ref} onScroll={onScroll}>
        {thread ? (
          <>
            <Transcript
              items={visible}
              tasks={byToolUse}
              busy={live}
              openThread={(id) =>
                push("subagent", { agentId, path: joinThreadPath([...path, id]) })
              }
            />
            {live && (
              <div className="working rise">
                <Dots />
                Sub-agent is working
              </div>
            )}
            {visible.length === 0 && !live && (
              <div className="empty">
                <b>Nothing recorded</b>
                This sub-agent left no transcript of its own.
              </div>
            )}
          </>
        ) : log === undefined ? (
          <div className="empty">Loading the conversation…</div>
        ) : (
          <div className="empty">
            <b>Thread not found</b>
            This sub-agent is no longer in the conversation.
          </div>
        )}
      </div>
      <div className="thread-foot">
        <Icon name="subagent" size={13} />
        <span>
          Read-only · sub-agents take direction from <b>{agent.name}</b>
        </span>
      </div>
    </>
  );
}
