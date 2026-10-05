// Answer a `chatFocus` request (store/ui) for one row: when the sidebar — or
// the thread view's "back" — asks for this tool_use in this agent's chat, run
// `open`, scroll the row into view on the next frame, then consume the
// request. Shared by the generic ToolRow and the sub-agent card so both
// reveal the same way. The frame-ordering subtlety lives in revealToolRow.

import { useEffect, useRef } from "react";
import { useAppStore } from "@/store";
import { revealToolRow } from "./revealToolRow";

export function useChatFocus(
  agentId: string | undefined,
  toolUseId: string | undefined,
  root: () => HTMLElement | null,
  open: () => void,
) {
  const focused = useAppStore(
    (s) =>
      toolUseId !== undefined &&
      s.chatFocus !== null &&
      s.chatFocus.toolUseId === toolUseId &&
      s.chatFocus.agentId === agentId,
  );
  const clearChatFocus = useAppStore((s) => s.clearChatFocus);

  // Callers pass fresh closures each render; held in refs so the effect only
  // re-runs on the request itself (a re-run's cleanup would cancel the frame).
  const rootRef = useRef(root);
  const openRef = useRef(open);
  rootRef.current = root;
  openRef.current = open;

  useEffect(() => {
    if (!focused) return;
    return revealToolRow(
      () => rootRef.current(),
      () => openRef.current(),
      clearChatFocus,
    );
  }, [focused, clearChatFocus]);
}
