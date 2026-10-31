// A patch handed over as one text: Codex's `apply_patch` and OpenCode's
// `patch`. The row names the files and totals the lines; opened, it's the
// patch as a diff, one caption per file.
//
// Codex's format (verified against its rollout tests): `*** Begin Patch`,
// then per file `*** Update File: path` / `*** Add File: path` /
// `*** Delete File: path`, hunks of lines led by a space, `+` or `-`, and
// `*** End Patch`. A unified diff (`--- a/x` / `+++ b/x` / `@@`) parses the
// same way, so a provider that hands over git's output renders too.

import { basename } from "@/util/format";
import type { DiffLine } from "@/util/lineDiff";
import { renderToolResult } from "@/util/toolText";
import { Caption, CodeSurface, DiffRows, PathText } from "./blocks";
import type { ToolPresenter } from "./types";
import { DiffCount, getStringField } from "./util";

export interface PatchFile {
  path: string;
  op: "update" | "add" | "delete";
  lines: DiffLine[];
}

const CODEX_HEADER = /^\*\*\* (Update|Add|Delete) File: (.+)$/;
const UNIFIED_NEW = /^\+\+\+ (?:b\/)?(.+)$/;
const UNIFIED_OLD = /^--- (?:a\/)?(.+)$/;
/** Lines that frame a patch rather than belong to one. */
const FRAME = /^(\*\*\* |@@|diff |index )/;

export function parsePatch(text: string): PatchFile[] {
  const files: PatchFile[] = [];
  let file: PatchFile | null = null;
  let pendingOld: string | null = null;
  for (const raw of text.split("\n")) {
    const line = raw.replace(/\r$/, "");
    const codex = CODEX_HEADER.exec(line);
    if (codex) {
      const op = codex[1] === "Add" ? "add" : codex[1] === "Delete" ? "delete" : "update";
      file = { path: codex[2].trim(), op, lines: [] };
      files.push(file);
      continue;
    }
    const old = UNIFIED_OLD.exec(line);
    if (old) {
      pendingOld = old[1].trim();
      continue;
    }
    const fresh = UNIFIED_NEW.exec(line);
    if (fresh) {
      const path = fresh[1].trim();
      const op = pendingOld === "/dev/null" ? "add" : path === "/dev/null" ? "delete" : "update";
      file = { path: op === "delete" && pendingOld ? pendingOld : path, op, lines: [] };
      files.push(file);
      pendingOld = null;
      continue;
    }
    if (!file || FRAME.test(line)) continue;
    if (line.startsWith("+")) file.lines.push({ op: "add", text: line.slice(1) });
    else if (line.startsWith("-")) file.lines.push({ op: "del", text: line.slice(1) });
    else if (line.startsWith(" ")) file.lines.push({ op: "keep", text: line.slice(1) });
    else if (line === "") file.lines.push({ op: "keep", text: "" });
    // Anything else is not a diff line; leave it out rather than guess.
  }
  return files;
}

/** The patch text, wherever the provider put it: a bare string, or a field. */
export function patchText(input: unknown): string {
  if (typeof input === "string") return input;
  for (const key of ["patch", "input", "patchText", "diff", "content"]) {
    const v = getStringField(input, key);
    if (v) return v;
  }
  return "";
}

function counts(files: PatchFile[]): { additions: number; deletions: number } {
  let additions = 0;
  let deletions = 0;
  for (const f of files) {
    for (const l of f.lines) {
      if (l.op === "add") additions += 1;
      else if (l.op === "del") deletions += 1;
    }
  }
  return { additions, deletions };
}

function named(files: PatchFile[]): string {
  if (files.length === 0) return "";
  if (files.length <= 2) return files.map((f) => basename(f.path) || f.path).join(", ");
  return `${files.length} files`;
}

export const patchPresenter: ToolPresenter = {
  icon: "diff",
  title: "Patch",
  summary: (call) => {
    const files = parsePatch(patchText(call.input));
    const { additions, deletions } = counts(files);
    return (
      <>
        {named(files) || "(no files)"}
        <DiffCount additions={additions} deletions={deletions} />
      </>
    );
  },
  expanded: (call, result) => {
    const text = patchText(call.input);
    const files = parsePatch(text);
    return (
      <>
        {files.length === 0 && text && <CodeSurface text={text} />}
        {files.map((f) => (
          <div key={`${f.op}:${f.path}`}>
            <Caption>
              <PathText path={f.path} />
              {f.op !== "update" && (
                <span className="tb-cap-note">{f.op === "add" ? "new file" : "deleted"}</span>
              )}
            </Caption>
            {f.lines.length > 0 && <DiffRows lines={f.lines} />}
          </div>
        ))}
        {result?.is_error && <Caption tone="danger">{renderToolResult(result.content)}</Caption>}
      </>
    );
  },
  facts: (call) => {
    const files = parsePatch(patchText(call.input));
    return { kind: "change", paths: files.map((f) => f.path), ...counts(files) };
  },
  activity: (call) => {
    const files = parsePatch(patchText(call.input));
    return files.length > 0 ? `Editing ${named(files)}` : "Applying a patch";
  },
};
