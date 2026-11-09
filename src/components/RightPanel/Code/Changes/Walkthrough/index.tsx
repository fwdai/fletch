// Walkthrough — guides the reader through the workspace's changes one step at
// a time, inside the Code panel's Changes view. Steps come from the agent's own
// tour (`api.getTour`, written through the `tour_set` mailbox op) when it
// submitted one; otherwise they are derived from the transcript: files in the
// order the agent first edited them, each with the prose the agent wrote just
// before the edit. Nothing here blocks or accepts a change — it only moves the
// diff pane and remembers what has been read.
import type { AgentRecord, FileStatus } from "@/api";

/** A 1-indexed, inclusive line range on a file's new side. */
export interface WalkthroughRange {
  start: number;
  end: number;
}

export interface WalkthroughProps {
  agent: AgentRecord;
  /** The changed files in the Changes view's current scope, in display order. */
  files: FileStatus[];
  /** The file the diff pane shows now; null while the file list is showing. */
  currentPath: string | null;
  /** Show `path` in the diff pane, scrolled to `range` when one is given. */
  onGo: (path: string, range?: WalkthroughRange) => void;
  /** Mark `paths` as viewed — moving past a step counts as having read it. */
  onViewed?: (paths: string[]) => void;
}

export function Walkthrough(_props: WalkthroughProps) {
  return null;
}
