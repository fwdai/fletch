// The chat log, through the desktop's own row model: runs of tool calls and
// thoughts fold into one summarizing line, every settled turn but the latest
// folds behind "Worked for …", and each row opens to its detail — the same
// rows, presenters and styles as the Mac (`@desktop/components/Workspace/
// messages`). What is the phone's own is how a message, a notice and a
// sub-agent card draw: a tool row that launched a sub-agent is a thread entry
// rather than something the agent did, so it renders as a card that opens that
// thread on its own screen (`openThread`) instead of expanding in place.

import type { BackgroundTask } from "@desktop/adapters/shared/backgroundTasks";
import {
  isSubagentCall,
  threadLabel,
  threadState,
  threadType,
} from "@desktop/adapters/shared/subagents";
import { Icon } from "@desktop/components/Icon";
import {
  TranscriptHost,
  type TranscriptHostValue,
} from "@desktop/components/Workspace/messages/host";
import {
  type PairCache,
  type ViewItem,
  visibleItems,
} from "@desktop/components/Workspace/messages/pair";
import {
  RowEnv,
  type RowEnvValue,
  RowView,
  rowViewKey,
} from "@desktop/components/Workspace/messages/RowView";
import { buildRows, newRowCache, openTurnStart } from "@desktop/components/Workspace/messages/rows";
import { ToolStep } from "@desktop/components/Workspace/messages/StepGroup";
import { UNDELIVERED_LABEL } from "@desktop/helpers/mergeTurns";
import { useCallback, useMemo, useRef } from "react";
import type { ChatItem, DisplayPolicy } from "../../adapters";
import { SentChips } from "../../attachments";
import { Md } from "../../components/Md";
import { useTick } from "../../lib/hooks";
import { threadStatus } from "../../lib/thread";

type ToolCall = Extract<ChatItem, { kind: "tool_call" }>;
type ToolResult = Extract<ChatItem, { kind: "tool_result" }>;

/** Background tasks by the tool_use id that launched them. */
export type TasksByToolUse = Record<string, BackgroundTask>;

/** The items a screen's transcript shows, derived once from the log: the
 *  provider's display policy, the blank-message filter and tool pairing, in
 *  the one pass the desktop uses (`visibleItems`). Tool-pair identity survives
 *  streaming deltas (PairCache). */
export function useVisibleItems(log: ChatItem[] | undefined, policy: DisplayPolicy): ViewItem[] {
  const cache = useRef<PairCache>(new Map());
  return useMemo(() => visibleItems(log ?? [], policy, cache.current), [log, policy]);
}

/** Send a message again, as a new turn. */
export type Resend = (text: string, attachments?: string[]) => void;

function Item({ item, resend }: { item: ChatItem; resend?: Resend }) {
  switch (item.kind) {
    case "user_message":
      return (
        <div className="msg-user rise">
          {item.text}
          <SentChips paths={item.attachments} />
          {item.undelivered && (
            <div className="undelivered">
              {UNDELIVERED_LABEL[item.undelivered]}
              {resend && (
                <button type="button" onClick={() => resend(item.text, item.attachments)}>
                  Resend
                </button>
              )}
            </div>
          )}
        </div>
      );
    case "queued_message":
      return (
        <div className="msg-user queued rise">
          {item.text}
          <SentChips paths={item.attachments} />
        </div>
      );
    case "agent_message":
      return (
        <div className="msg-text rise">
          <Md text={item.text} />
        </div>
      );
    case "notice":
      if (item.subtype === "turn_end") {
        return (
          <div className="turn-end rise">
            <i />
            <Icon name="checkCircle" size={13} style={{ color: "var(--success)" }} />
            {item.text === "success" ? "Turn complete" : item.text}
            <i />
          </div>
        );
      }
      return <div className={`notice rise${item.is_error ? " err" : ""}`}>{item.text}</div>;
    default:
      return null;
  }
}

export const Dots = () => (
  <span className="working-dots">
    <i />
    <i />
    <i />
  </span>
);

/** A sub-agent launch: two lines (its name; its type and status) and a
 *  disclosure chevron. Tapping opens the thread on its own screen. */
function SubagentCard({
  call,
  result,
  task,
  busy,
  openThread,
}: {
  call: ToolCall;
  result: ToolResult | null;
  task?: BackgroundTask;
  busy: boolean;
  openThread?: (toolUseId: string) => void;
}) {
  const state = threadState(result, task, busy);
  // A running card's "quiet Nm" hint is a function of the clock, not of any
  // event — a sub-agent that outlives its idle parent gets no other render —
  // so running cards tick once a minute, the desktop card's cadence.
  useTick(60_000, state === "running");
  const label = threadLabel(call);
  const type = threadType(call);
  const status = threadStatus(state, task, Date.now());
  return (
    <button
      type="button"
      className={`subcard ${state}`}
      data-tool-use-id={call.id}
      onClick={() => openThread?.(call.id)}
      disabled={!openThread}
      aria-label={`Open sub-agent thread: ${label}`}
    >
      <span className="ic">
        <Icon name="subagent" size={14} />
      </span>
      <span className="body">
        <span className="nm">{label}</span>
        <span className="mt">
          {type && <>{type} · </>}
          {status}
        </span>
      </span>
      {state === "running" && <Dots />}
      <Icon name="chevR" size={16} className="go" />
    </button>
  );
}

/** `items` is the agent's log (or a thread's) as the screen derived it once
 *  (`useVisibleItems`). `busy` is the owning agent's (or thread's) liveness,
 *  which the open turn's rows read their running state from. `openThread` opens a
 *  sub-agent's thread; without it the cards are inert. `resend` backs the
 *  Resend on a message the agent never read; without it there is none. */
export function Transcript({
  agentId,
  provider,
  items: view,
  tasks,
  busy = false,
  openThread,
  resend,
}: {
  agentId: string;
  provider?: string;
  items: ViewItem[];
  tasks?: TasksByToolUse;
  busy?: boolean;
  openThread?: (toolUseId: string) => void;
  resend?: Resend;
}) {
  // Run and fold identity survives streaming deltas (RowCache), so settled
  // rows skip re-rendering while the live one grows.
  const rowCache = useRef(newRowCache());
  const { rows, tailRunKey } = useMemo(() => buildRows(view, rowCache.current), [view]);

  const host = useMemo<TranscriptHostValue>(
    () => ({
      taskFor: (toolUseId) => tasks?.[toolUseId],
      focus: null,
      clearFocus: () => {},
      Markdown: Md,
    }),
    [tasks],
  );

  const renderItem = useCallback(
    (index: number, rowBusy: boolean) => {
      const item = view[index];
      if (item.kind === "tool_pair") {
        const { call, result } = item;
        if (isSubagentCall(call)) {
          return (
            <SubagentCard
              call={call}
              result={result}
              task={tasks?.[call.id]}
              busy={rowBusy}
              openThread={openThread}
            />
          );
        }
        return <ToolStep call={call} result={result} agentId={agentId} busy={rowBusy} />;
      }
      return <Item item={item} resend={resend} />;
    },
    [view, tasks, openThread, resend, agentId],
  );

  const env = useMemo<RowEnvValue>(
    () => ({
      agentId,
      provider,
      items: view,
      liveBusy: busy,
      openTurnStart: openTurnStart(view),
      tailRunKey,
      renderItem,
    }),
    [agentId, provider, view, busy, tailRunKey, renderItem],
  );

  return (
    <TranscriptHost.Provider value={host}>
      <RowEnv.Provider value={env}>
        {rows.map((row) => (
          <RowView key={rowViewKey(row, view)} row={row} />
        ))}
      </RowEnv.Provider>
    </TranscriptHost.Provider>
  );
}
