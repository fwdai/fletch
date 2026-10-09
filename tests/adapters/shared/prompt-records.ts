// Test helpers: the backend's rule for which record a prompt is positioned on
// (`prompt_texts` in crates/fletch-core/src/agent/providers/*.rs), restated so
// a test can check the adapters stamp their prompt bubbles with that record.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const adaptersDir = join(fileURLToPath(new URL(".", import.meta.url)), "..");

type Body = Record<string, unknown>;

/** The bodies of a records fixture under tests/adapters, e.g.
 *  `codex/fixtures/rollout.jsonl`. */
export function readRecordFixture(path: string): Body[] {
  return readFileSync(join(adaptersDir, path), "utf8")
    .split("\n")
    .filter((l) => l.trim().length > 0)
    .map((l) => JSON.parse(l) as Body);
}

const rec = (v: unknown): Body => (v && typeof v === "object" ? (v as Body) : {});

function joinedText(content: unknown): string | undefined {
  if (!Array.isArray(content)) return undefined;
  return content.map((b) => (typeof rec(b).text === "string" ? rec(b).text : "")).join("");
}

/** `codex_prompt_texts`: each top-level prompt event (`user_message`, or a
 *  0.153 `item_completed` `UserMessage`) sits on the user `response_item`
 *  right before it when that has the same text, and on itself otherwise.
 *  Returns the indices of the records prompts sit on. */
export function codexPromptIndices(lines: Body[]): number[] {
  const prompt = (r: Body): string | undefined => {
    if (r.type !== "event_msg" || "parent_tool_use_id" in r) return undefined;
    const p = rec(r.payload);
    if (p.type === "user_message") return typeof p.message === "string" ? p.message : undefined;
    const item = rec(p.item);
    return p.type === "item_completed" && item.type === "UserMessage"
      ? joinedText(item.content)
      : undefined;
  };
  const modelInput = (r: Body): string | undefined => {
    const p = rec(r.payload);
    if (r.type !== "response_item" || "parent_tool_use_id" in r) return undefined;
    return p.type === "message" && p.role === "user" ? joinedText(p.content) : undefined;
  };
  const out: number[] = [];
  lines.forEach((line, i) => {
    const text = prompt(line);
    if (!text) return;
    out.push(i > 0 && modelInput(lines[i - 1]) === text ? i - 1 : i);
  });
  return out;
}
