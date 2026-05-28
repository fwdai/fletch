import { useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { UnlistenFn } from "@tauri-apps/api/event";

/**
 * Subscribes to Tauri's window-level file drag-drop stream and reports
 * an `isOver` flag (for a drop-target highlight) plus the absolute paths
 * dropped onto the window.
 *
 * We deliberately do NOT hit-test the drop position against a DOM rect:
 * Tauri reports macOS drag coordinates in an unreliable origin (often
 * the drag-image corner, with a flipped/negative Y), so rect math never
 * lines up. Instead, since only one Composer is mounted at a time (the
 * workspace shows either the new-agent screen or a chat, never both),
 * any file drop on the window is routed to it.
 */
export function useFileDrop(onFiles: (paths: string[]) => void) {
  const [isOver, setIsOver] = useState(false);
  // Latest callback without re-subscribing the window listener.
  const cb = useRef(onFiles);
  cb.current = onFiles;

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let disposed = false;

    getCurrentWebview()
      .onDragDropEvent((e) => {
        const p = e.payload;
        if (p.type === "enter" || p.type === "over") {
          setIsOver(true);
        } else if (p.type === "leave") {
          setIsOver(false);
        } else if (p.type === "drop") {
          if (p.paths.length) cb.current(p.paths);
          setIsOver(false);
        }
      })
      .then((fn) => {
        // Component may unmount before the async listen resolves.
        if (disposed) fn();
        else unlisten = fn;
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return isOver;
}
