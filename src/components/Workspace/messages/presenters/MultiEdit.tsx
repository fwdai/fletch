import type { ToolPresenter } from "./types";
import { ToolBlock, DiffCount, getStringField, renderToolResult } from "./util";
import { basename } from "../../../../util/format";
import { lineDiffCounts } from "../../../../util/lineDiff";

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
        <ToolBlock label="file">{path}</ToolBlock>
        {edits.map((e, i) => (
          <div key={i}>
            {e.old_string && (
              <ToolBlock label="- old" isError>
                {e.old_string}
              </ToolBlock>
            )}
            {e.new_string && (
              <ToolBlock label="+ new">
                <span style={{ color: "var(--success, #6c9c5e)" }}>
                  {e.new_string}
                </span>
              </ToolBlock>
            )}
          </div>
        ))}
        {result && (
          <ToolBlock label="↳" isError={result.is_error}>
            {renderToolResult(result.content)}
          </ToolBlock>
        )}
      </>
    );
  },
};
