import { type ReactNode, useEffect, useRef, useState } from "react";
import { Icon, type IconName } from "@/components/Icon";
import { Loader } from "@/components/ui/Loader";
import { useAppStore } from "@/store";
import { revealToolRow } from "./revealToolRow";

/** Shared chrome for every tool presenter: icon, name, one-line summary,
 *  click-to-expand. Presenters supply the summary and expanded bodies.
 *  `running` marks a tool call still in flight (no result yet, agent busy;
 *  or a backgrounded sub-agent still working after the launch result landed)
 *  so the row shows a live spinner instead of looking identical to a settled
 *  one. `toolUseId` + `agentId` let the row answer a `chatFocus` request from
 *  the sidebar: it opens, scrolls itself into view, and clears the request. */
export function ToolRow({
  name,
  icon = "wrench",
  isError,
  running,
  summary,
  expanded,
  toolUseId,
  agentId,
}: {
  name: string;
  icon?: IconName;
  isError?: boolean;
  running?: boolean;
  summary: ReactNode;
  expanded: ReactNode;
  toolUseId?: string;
  agentId?: string;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const focused = useAppStore(
    (s) =>
      toolUseId !== undefined &&
      s.chatFocus !== null &&
      s.chatFocus.toolUseId === toolUseId &&
      s.chatFocus.agentId === agentId,
  );
  const clearChatFocus = useAppStore((s) => s.clearChatFocus);

  // Open, scroll on the next frame, then consume the request (in that order —
  // see revealToolRow). The cleanup cancels the frame only if the row unmounts
  // before it fires.
  useEffect(() => {
    if (!focused) return;
    return revealToolRow(
      () => rootRef.current,
      () => setOpen(true),
      clearChatFocus,
    );
  }, [focused, clearChatFocus]);

  const dangerColor = isError ? "var(--danger)" : undefined;
  return (
    <div ref={rootRef} data-tool-use-id={toolUseId}>
      <button
        type="button"
        className="m-tool flex-center"
        onClick={() => setOpen((o) => !o)}
        style={{ width: "100%", textAlign: "left", color: dangerColor }}
      >
        <Icon name={icon} size={12} className="t-icon" />
        <span className="t-name" style={{ color: dangerColor }}>
          {name}
        </span>
        <span className="t-arg">{summary}</span>
        {running && <Loader variant="muted" size="sm" aria-label={`${name} running`} />}
        <span className="t-result">{open ? "▾" : "▸"}</span>
      </button>
      {open && <div style={{ padding: "8px 14px 12px" }}>{expanded}</div>}
    </div>
  );
}
