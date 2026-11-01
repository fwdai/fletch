import { basename } from "@/util/format";
import { lineDiffCounts } from "@/util/lineDiff";
import { renderToolResult } from "@/util/toolText";
import { Caption, DiffSurface, PathText } from "./blocks";
import type { ToolPresenter } from "./types";
import { DiffCount, getStringField } from "./util";

interface SingleEdit {
  old_string: string;
  new_string: string;
}

/** Pull the `edits` array off an unknown input bag, coercing each entry. */
function getEdits(input: unknown): SingleEdit[] {
  if (input && typeof input === "object" && "edits" in input) {
    const raw = (input as { edits: unknown }).edits;
    if (Array.isArray(raw)) {
      return raw.map((e) => ({
        old_string:
          e && typeof e === "object" && typeof (e as SingleEdit).old_string === "string"
            ? (e as SingleEdit).old_string
            : "",
        new_string:
          e && typeof e === "object" && typeof (e as SingleEdit).new_string === "string"
            ? (e as SingleEdit).new_string
            : "",
      }));
    }
  }
  return [];
}

function totalCounts(edits: SingleEdit[]) {
  let additions = 0;
  let deletions = 0;
  for (const e of edits) {
    const c = lineDiffCounts(e.old_string, e.new_string);
    additions += c.additions;
    deletions += c.deletions;
  }
  return { additions, deletions };
}

export const multiEditPresenter: ToolPresenter = {
  icon: "edit",
  summary: (call) => {
    const path = getStringField(call.input, "file_path");
    const { additions, deletions } = totalCounts(getEdits(call.input));
    return (
      <>
        {path ? basename(path) : "(no path)"}
        <DiffCount additions={additions} deletions={deletions} />
      </>
    );
  },
  expanded: (call, result) => {
    const path = getStringField(call.input, "file_path");
    const edits = getEdits(call.input);
    return (
      <>
        {path && (
          <Caption>
            <PathText path={path} />
            {edits.length > 1 && <span className="tb-cap-note">{edits.length} edits</span>}
          </Caption>
        )}
        {edits.map((e, i) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: edits have no ids; their order is fixed
          <DiffSurface key={i} before={e.old_string} after={e.new_string} />
        ))}
        {result?.is_error && <Caption tone="danger">{renderToolResult(result.content)}</Caption>}
      </>
    );
  },
  facts: (call) => ({
    kind: "change",
    path: getStringField(call.input, "file_path"),
    ...totalCounts(getEdits(call.input)),
  }),
  activity: (call) => {
    const path = getStringField(call.input, "file_path");
    return path ? `Editing ${basename(path)}` : "Editing a file";
  },
};
