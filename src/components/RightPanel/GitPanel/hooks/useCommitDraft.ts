import { useCallback, useEffect, useRef, useState } from "react";
import type { GitPanelState } from "@/components/RightPanel/primaryActions";

/** Commit-message authorship (agent mode). By default the agent writes the
 *  message + PR (the field is collapsed). `override` = the user opened the
 *  field to write their own; once `msg` has content (`customActive`) the commit
 *  goes direct, bypassing the agent. The half-written draft is dropped on agent
 *  switch and whenever the panel leaves the changes state. */
export function useCommitDraft(agentId: string, panelState: GitPanelState) {
  const [override, setOverride] = useState(false);
  const [msg, setMsg] = useState("");
  const commitRef = useRef<HTMLTextAreaElement>(null);
  const customActive = override && msg.trim().length > 0;

  // Reset on agent switch so a draft doesn't leak between worktrees.
  useEffect(() => {
    setOverride(false);
    setMsg("");
  }, [agentId]);

  // Leaving the changes state drops any half-written override.
  useEffect(() => {
    if (panelState !== "changes") {
      setOverride(false);
      setMsg("");
    }
  }, [panelState]);

  const openOverride = useCallback(() => {
    setOverride(true);
    // Defer focus until the textarea has animated in.
    requestAnimationFrame(() => commitRef.current?.focus());
  }, []);
  const revertOverride = useCallback(() => {
    setOverride(false);
    setMsg("");
  }, []);

  return { override, msg, setMsg, commitRef, customActive, openOverride, revertOverride };
}
