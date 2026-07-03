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
      const settled = e.payload.phase === "ready" || e.payload.phase === "failed";
      if (settled && phaseRef.current === "downloading") {
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
