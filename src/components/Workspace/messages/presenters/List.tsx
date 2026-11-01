// A directory listing: Claude's `LS`, Cursor's `ls`, OpenCode's `list`,
// Antigravity's `list_dir`. The row names the directory; opened, the listing.
// The directory is read off whatever key the provider uses (`path`,
// `DirectoryPath`, …) by the shared field inference.

import { basename } from "@/util/format";
import { renderToolResult } from "@/util/toolText";
import { Caption, CodeSurface, PathText } from "./blocks";
import { pathOf } from "./infer";
import type { ToolPresenter } from "./types";
import { countResultLines, SummaryNote } from "./util";

const dirName = (path: string) => basename(path.replace(/\/+$/, "")) || path;

export const listPresenter: ToolPresenter = {
  icon: "folder",
  title: "List",
  summary: (call, result) => {
    const path = pathOf(call.input);
    const entries = countResultLines(result);
    return (
      <>
        {path ? dirName(path) : "."}
        {entries > 0 && (
          <SummaryNote>
            {entries} {entries === 1 ? "entry" : "entries"}
          </SummaryNote>
        )}
      </>
    );
  },
  expanded: (call, result) => {
    const path = pathOf(call.input);
    return (
      <>
        {path && (
          <Caption>
            <PathText path={path} />
          </Caption>
        )}
        {result && (
          <CodeSurface text={renderToolResult(result.content)} isError={result.is_error} />
        )}
      </>
    );
  },
  facts: (call) => {
    const path = pathOf(call.input);
    return path ? { kind: "read", path } : { kind: "other", subject: "list" };
  },
  activity: (call) => {
    const path = pathOf(call.input);
    return path ? `Listing ${dirName(path)}` : "Listing a directory";
  },
};
