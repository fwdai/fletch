import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";

/** Payload of the backend's `git-dist:state` event: the portable-git
 *  bootstrap that runs at launch when no usable system git exists. */
export interface GitDistState {
  phase: "unknown" | "checking" | "downloading" | "ready" | "failed";
  source?: "system" | "portable";
  received?: number;
  total?: number | null;
  error?: string;
}

/** Live portable-git bootstrap state. `phase` stays `"unknown"` when no event
 *  arrives after mount — i.e. the bootstrap already settled before this screen
 *  appeared, in which case `checkCli("git")` alone tells the truth.
 *  `onSettled` fires when a download concludes (ready or failed) so callers
 *  can re-run their readiness check. */
export function useGitDist(onSettled?: () => void): GitDistState {
  const [state, setState] = useState<GitDistState>({ phase: "unknown" });
  const onSettledRef = useRef(onSettled);
  onSettledRef.current = onSettled;
  const phaseRef = useRef<GitDistState["phase"]>("unknown");

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let disposed = false;
    void listen<GitDistState>("git-dist:state", (e) => {
      const isSettled = (p: GitDistState["phase"]) => p === "ready" || p === "failed";
      // Fire on the first transition into a settled phase. This covers the
      // normal "downloading" -> "ready"/"failed" path and the race where we
      // mount mid-download and only ever observe the final settled event (the
      // "downloading" progress having been emitted before the listener
      // attached, so phaseRef is still "unknown"). When the bootstrap settled
      // *before* mount no event arrives at all, so this never fires spuriously.
      if (isSettled(e.payload.phase) && !isSettled(phaseRef.current)) {
        onSettledRef.current?.();
      }
      phaseRef.current = e.payload.phase;
      setState(e.payload);
    }).then((fn) => {
      // The effect may have been cleaned up while listen() was in flight.
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return state;
}
