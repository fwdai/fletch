import { useCallback, useEffect, useRef, useState } from "react";

/** Commit-message authorship (agent mode). By default the agent writes the
 *  message + PR (the field is collapsed). `override` = the user opened the
 *  field to write their own; once `msg` has content (`customActive`) the commit
 *  goes direct, bypassing the agent. The half-written draft is dropped when
 *  `checkout` changes (another agent, or another repo of the same one) and
 *  whenever there is nothing left to commit (`committable` false). */
export function useCommitDraft(checkout: string, committable: boolean) {
  const [override, setOverride] = useState(false);
  const [msg, setMsg] = useState("");
  const commitRef = useRef<HTMLTextAreaElement>(null);
  const customActive = override && msg.trim().length > 0;

  // Reset on checkout switch so a draft doesn't leak between working trees.
  useEffect(() => {
    setOverride(false);
    setMsg("");
  }, [checkout]);

  // A tree with nothing to commit drops any half-written override.
  useEffect(() => {
    if (!committable) {
      setOverride(false);
      setMsg("");
    }
  }, [committable]);

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
