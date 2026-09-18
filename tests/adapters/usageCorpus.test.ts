import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { usageFromRecords } from "@/adapters/usage";
import type { SessionRecord } from "@/api";

// The same transcript corpus the Rust scanner (`src-tauri/src/usage_scan`) is
// tested against, folded through the TS adapters. Two parsers exist on purpose
// — the Rust one walks gigabytes machine-wide, the TS one prices the sessions
// this app runs — so this is the one place that pins them to identical
// semantics: dedupe of repeated content blocks, `<synthetic>` exclusion,
// model-less records kept, the cache_creation TTL breakdown, and codex's
// cached-input subtraction. Change a rule in one parser and this fails until
// the other and `expected.json` agree.

const FIXTURES = join(dirname(fileURLToPath(import.meta.url)), "..", "fixtures", "usage");

interface ExpectedTotals {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

const expected = JSON.parse(readFileSync(join(FIXTURES, "expected.json"), "utf8")) as Record<
  "claude" | "codex",
  ExpectedTotals
>;

/** One session_records row per non-blank transcript line, as the sync ingests
 *  them. */
function recordsOf(provider: "claude" | "codex"): SessionRecord[] {
  return readFileSync(join(FIXTURES, `${provider}.jsonl`), "utf8")
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line, seq) => ({
      seq,
      provider,
      source: "corpus",
      native_id: String(seq),
      agent_version: null,
      body: JSON.parse(line) as SessionRecord["body"],
    }));
}

describe.each(["claude", "codex"] as const)("usage corpus: %s", (provider) => {
  it("folds to the totals the Rust scanner is pinned to", () => {
    const { tokens } = usageFromRecords(provider, recordsOf(provider)).spend;
    const { input, output, cacheRead, cacheWrite } = expected[provider];
    expect(tokens).toEqual({ input, output, cacheRead, cacheWrite });
  });
});
