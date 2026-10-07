// The acceptance rule for host-owned settings, enforced over the source: no key
// the host reads may be written from `src/` through `setSetting`,
// `setProjectSetting`, `deleteProjectSetting` or the generic `db*` bridge — they
// write THIS Mac's database whatever environment is active, so a write there is
// a write the host never sees (docs/remote-protocol.md, "Settings"). The
// dedicated ops in `api/domains/settings.ts` are the only way in.

import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  HOST_PROJECT_SETTING_KEYS,
  HOST_PROJECT_SETTING_PREFIXES,
  HOST_SETTING_KEYS,
  HOST_SETTING_PREFIXES,
  isHostProjectSettingKey,
  isHostSettingKey,
} from "./hostOwnedKeys";

const SRC = fileURLToPath(new URL("..", import.meta.url));

/** Calls that write a settings row on this Mac — the bare storage helpers, not
 *  a method of the same name (`api.setProjectSetting` is the host op). */
const WRITERS =
  /(?<![.\w$])(setSetting|setProjectSetting|deleteProjectSetting|dbUpsert|dbInsert|dbUpdate|dbDelete)\s*\(/g;

const isHostKey = (k: string) => isHostSettingKey(k) || isHostProjectSettingKey(k);

/** A literal's text names a host key — exactly, or as the head of a key built
 *  at runtime (`run.${id}`, `agent_bin_path_${id}`). */
function namesHostKey(text: string): boolean {
  if (isHostKey(text)) return true;
  return [...HOST_SETTING_PREFIXES, ...HOST_PROJECT_SETTING_PREFIXES].some((p) =>
    text.startsWith(p),
  );
}

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const path = join(dir, e.name);
    if (e.isDirectory()) return sourceFiles(path);
    if (!/\.(ts|tsx)$/.test(e.name) || /\.(test|spec)\.tsx?$/.test(e.name)) return [];
    return [path];
  });
}

/** `const NAME = "host.key"` anywhere in `src/`: a host key behind a name. */
function hostKeyConstants(sources: string[]): Set<string> {
  const names = new Set<string>();
  const decl = /\bconst\s+([A-Za-z_$][\w$]*)\s*(?::\s*string\s*)?=\s*(["'`])([^"'`]*)\2/g;
  for (const source of sources) {
    for (const m of source.matchAll(decl)) {
      if (namesHostKey(m[3])) names.add(m[1]);
    }
  }
  return names;
}

/** The argument text of the call whose `(` is at `open`. */
function callArgs(source: string, open: number): string {
  let depth = 0;
  for (let i = open; i < source.length; i++) {
    if (source[i] === "(") depth++;
    else if (source[i] === ")" && --depth === 0) return source.slice(open + 1, i);
  }
  return source.slice(open + 1);
}

/** Every writer call in `source` whose arguments name a host key, by literal
 *  or through one of `constants`. */
function offendingWrites(source: string, constants: Set<string>): string[] {
  const out: string[] = [];
  for (const m of source.matchAll(WRITERS)) {
    const open = (m.index ?? 0) + m[0].length - 1;
    const args = callArgs(source, open);
    const literals = [...args.matchAll(/(["'`])([^"'`]*)\1|`([^`$]*)\$\{/g)].map(
      (l) => l[2] ?? l[3] ?? "",
    );
    const identifiers = args.match(/[A-Za-z_$][\w$]*/g) ?? [];
    if (literals.some(namesHostKey) || identifiers.some((id) => constants.has(id))) {
      out.push(`${m[1]}(${args.replace(/\s+/g, " ").trim()})`);
    }
  }
  return out;
}

describe("host-owned settings", () => {
  const files = sourceFiles(SRC);
  const sources = files.map((f) => readFileSync(f, "utf8"));
  const constants = hostKeyConstants(sources);

  it("are never written from src/ through this Mac's settings bridge", () => {
    const offences = files.flatMap((file, i) =>
      offendingWrites(sources[i], constants).map((call) => `${relative(SRC, file)}: ${call}`),
    );
    expect(offences).toEqual([]);
  });

  // The scanner has to be able to fail, or the test above proves nothing.
  it("catches a literal, a named constant and a built key", () => {
    const named = new Set(["KEY"]);
    expect(offendingWrites(`setProjectSetting(p, "verify.on_turn_end", "1")`, named)).toHaveLength(
      1,
    );
    expect(offendingWrites("deleteProjectSetting(projectId, KEY)", named)).toHaveLength(1);
    // biome-ignore lint/suspicious/noTemplateCurlyInString: the fixture is source text with a template literal in it
    expect(offendingWrites("setProjectSetting(p, `run.${id}`, v)", named)).toHaveLength(1);
    expect(offendingWrites(`setSetting("git_branch_prefix", "x/")`, named)).toHaveLength(1);
    expect(
      offendingWrites(
        `dbUpsert("settings", { key: "agent_bin_path_claude", value: p }, "key")`,
        named,
      ),
    ).toHaveLength(1);
    // A client's own keys stay writable, and the host op is the way in.
    expect(offendingWrites(`setSetting("theme", "dark")`, named)).toEqual([]);
    expect(offendingWrites("api.setProjectSetting(projectId, KEY, null)", named)).toEqual([]);
    expect(offendingWrites(`setProjectSetting(p, "autopilot.enabled", "0")`, named)).toEqual([]);
  });

  it("include every key the host reads, and no secret", () => {
    for (const key of ["notify_turn_complete", "sandbox_engine", "agent_bin_path_codex"]) {
      expect(isHostSettingKey(key)).toBe(true);
    }
    for (const key of [
      "github_token",
      "linear_token",
      "claude_container_token",
      "telemetry_enabled",
      "telemetry_distinct_id",
      "remote.enabled",
      "theme",
    ]) {
      expect(isHostSettingKey(key)).toBe(false);
    }
    for (const key of ["verify.on_turn_end", "run.test", "run.agent.fuji.lint", "run_env"]) {
      expect(isHostProjectSettingKey(key)).toBe(true);
    }
    for (const key of ["autopilot.enabled", "roadmap.code_seq", "roadmap.code_prefix", "run."]) {
      expect(isHostProjectSettingKey(key)).toBe(false);
    }
    expect(HOST_SETTING_KEYS).toHaveLength(17);
    expect(HOST_PROJECT_SETTING_KEYS).toHaveLength(13);
  });
});
