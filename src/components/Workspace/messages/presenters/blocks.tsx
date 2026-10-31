// The pieces every tool's detail body is built from, so a Read, a Bash and an
// Edit open into one visual language: a caption naming what the call was about,
// then a code surface holding what it saw or did. Styling lives in Steps.css.

import { type ReactNode, useMemo, useState } from "react";
import { Button } from "@/components/ui/Button";
import { lineDiffLines } from "@/util/lineDiff";
import { stringifyInput } from "@/util/toolText";

/** Lines a surface renders before offering the rest behind "Show all". A long
 *  read or log is still one scroll away; the transcript doesn't pay for 5,000
 *  DOM lines nobody asked to see. */
const LINE_BUDGET = 400;

/** A small line over a surface: what the call was about, or how it went. */
export function Caption({ children, tone }: { children: ReactNode; tone?: "danger" }) {
  return <div className={tone ? `tb-cap ${tone}` : "tb-cap"}>{children}</div>;
}

/** A path with its directory dimmed, so the file name is what the eye lands on. */
export function PathText({ path }: { path: string }) {
  const cut = path.lastIndexOf("/");
  if (cut < 0) return <span className="tb-path">{path}</span>;
  return (
    <span className="tb-path">
      <span className="dir">{path.slice(0, cut + 1)}</span>
      {path.slice(cut + 1)}
    </span>
  );
}

/** Claude's Read numbers its lines `     1→text` (older builds: a tab). When
 *  nearly every line carries one, the numbers move into a gutter. */
const NUMBERED = /^\s*(\d+)(?:→|\t)(.*)$/;

function splitNumbered(lines: string[]): { n: string; text: string }[] | null {
  if (lines.length === 0) return null;
  const out: { n: string; text: string }[] = [];
  let misses = 0;
  for (const line of lines) {
    const m = NUMBERED.exec(line);
    if (m) out.push({ n: m[1], text: m[2] });
    else {
      misses += 1;
      out.push({ n: "", text: line });
    }
  }
  return misses <= Math.max(1, lines.length * 0.1) ? out : null;
}

/** Trim the trailing blank lines a tool's output usually ends with. */
function toLines(text: string): string[] {
  const lines = text.replace(/\s+$/, "").split("\n");
  return lines.length === 1 && lines[0] === "" ? [] : lines;
}

/** Keep the first LINE_BUDGET entries until the user asks for the rest. */
function useBudget<T>(all: T[]): { shown: T[]; rest: number; showAll: () => void } {
  const [all_, setAll] = useState(false);
  if (all_ || all.length <= LINE_BUDGET) return { shown: all, rest: 0, showAll: () => {} };
  return {
    shown: all.slice(0, LINE_BUDGET),
    rest: all.length - LINE_BUDGET,
    showAll: () => setAll(true),
  };
}

function MoreLines({ rest, onClick }: { rest: number; onClick: () => void }) {
  return (
    <div className="tb-more">
      <Button variant="link" size="sm" onClick={onClick}>
        Show {rest} more {rest === 1 ? "line" : "lines"}
      </Button>
    </div>
  );
}

/** Text a tool returned or was handed, on a quiet mono surface. Read-style
 *  numbered output gets its numbers in a gutter. */
export function CodeSurface({ text, isError }: { text: string; isError?: boolean }) {
  const lines = toLines(text);
  const numbered = splitNumbered(lines);
  const rows = numbered ?? lines.map((line) => ({ n: "", text: line }));
  const { shown, rest, showAll } = useBudget(rows);
  if (lines.length === 0) return <Caption>No output</Caption>;
  return (
    <div className={isError ? "tb-code is-error" : "tb-code"}>
      {numbered ? (
        shown.map((l, i) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: a line's position is its identity
          <div className="tb-line" key={i}>
            <span className="tb-gutter">{l.n}</span>
            <span className="tb-text">{l.text || " "}</span>
          </div>
        ))
      ) : (
        <pre className="tb-pre">{shown.map((l) => l.text).join("\n")}</pre>
      )}
      {rest > 0 && <MoreLines rest={rest} onClick={showAll} />}
    </div>
  );
}

/** `before` → `after` as a unified diff: removed lines on a red wash, added
 *  on a green one, kept lines plain between them. The diff is worked out
 *  here, when the surface mounts — not for every collapsed row. */
export function DiffSurface({ before, after }: { before: string; after: string }) {
  const lines = useMemo(() => lineDiffLines(before, after), [before, after]);
  const { shown, rest, showAll } = useBudget(lines);
  if (lines.length === 0) return <Caption>No changes</Caption>;
  return (
    <div className="tb-code tb-diff">
      {shown.map((l, i) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: a line's position is its identity
        <div className={`tb-line ${l.op}`} key={i}>
          <span className="tb-gutter">{l.op === "add" ? "+" : l.op === "del" ? "−" : ""}</span>
          <span className="tb-text">{l.text || " "}</span>
        </div>
      ))}
      {rest > 0 && <MoreLines rest={rest} onClick={showAll} />}
    </div>
  );
}

/** A command and what it printed, as one terminal surface: the prompt line,
 *  a hairline, the output. A failed run tints the frame, not the text — red
 *  paragraphs are hard to read, and the output is what the user came for. */
export function TermSurface({
  command,
  output,
  isError,
}: {
  command: string;
  output: string | null;
  isError?: boolean;
}) {
  const lines = output == null ? [] : toLines(output);
  const { shown, rest, showAll } = useBudget(lines);
  return (
    <div className={isError ? "tb-code tb-term is-error" : "tb-code tb-term"}>
      <div className="tb-cmd">
        <span className="tb-prompt">$</span>
        <pre className="tb-pre">{command}</pre>
      </div>
      {output != null && (
        <div className="tb-out">
          {lines.length === 0 ? (
            <span className="tb-empty">No output</span>
          ) : (
            <pre className="tb-pre">{shown.join("\n")}</pre>
          )}
          {rest > 0 && <MoreLines rest={rest} onClick={showAll} />}
        </div>
      )}
    </div>
  );
}

/** A tool's input as labelled fields rather than raw JSON: scalars inline,
 *  anything nested pretty-printed under its key. */
export function Fields({ input }: { input: unknown }) {
  if (input == null) return null;
  if (typeof input !== "object" || Array.isArray(input)) {
    return <CodeSurface text={stringifyInput(input, 2)} />;
  }
  const entries = Object.entries(input as Record<string, unknown>).filter(
    ([, v]) => v != null && v !== "" && !(Array.isArray(v) && v.length === 0),
  );
  if (entries.length === 0) return null;
  return (
    <dl className="tb-fields">
      {entries.map(([key, value]) => (
        <div className="tb-field" key={key}>
          <dt>{key}</dt>
          <dd>{typeof value === "string" ? value : stringifyInput(value, 2)}</dd>
        </div>
      ))}
    </dl>
  );
}
