// No source under `src/` knows a cloud provider: its name, how it connects and
// what it bills are what `cloud_providers` and `available_workspace_types`
// answer, so a provider the backend adds is offered, connected and explained
// with nothing changed here. Test fixtures may name one; nothing else may.

import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const SRC = fileURLToPath(new URL(".", import.meta.url));

/** The providers the backend has or plans, as code would spell them: an id as
 *  a string literal or in a workspace kind, a name, a token or CLI word. */
const PROVIDER_LITERALS: RegExp[] = [
  /(["'`])(fly|daytona|e2b)\1/i,
  /cloud:(fly|daytona|e2b)\b/i,
  /fly\.io|flyv1|flyctl|\bfly tokens?\b/i,
  /\b(daytona|e2b)\b/i,
];

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const path = join(dir, e.name);
    if (e.isDirectory()) return sourceFiles(path);
    if (!/\.(ts|tsx)$/.test(e.name) || /\.(test|spec)\.tsx?$/.test(e.name)) return [];
    return [path];
  });
}

const names = (line: string) => PROVIDER_LITERALS.some((re) => re.test(line));

describe("provider neutrality", () => {
  it("knows a provider literal when it sees one, and nothing else", () => {
    for (const line of [
      'if (id === "fly") {',
      "const kind = `cloud:fly`;",
      "Create one in the Fly.io dashboard.",
      'placeholder="FlyV1 fm2_…"',
      "Run fly tokens create org",
      "// More will join (Daytona, E2B).",
    ]) {
      expect(names(line), line).toBe(true);
    }
    for (const line of [
      'className="fly-menu"',
      'type Fly = "issues" | "roadmap";',
      "import { FlyoutMenu } from",
      "a retired run's on-the-fly stages",
      'icon: "cloud"',
    ]) {
      expect(names(line), line).toBe(false);
    }
  });

  it("names no provider outside tests", () => {
    const found = sourceFiles(SRC).flatMap((file) =>
      readFileSync(file, "utf8")
        .split("\n")
        .flatMap((line, i) =>
          names(line) ? [`${relative(SRC, file)}:${i + 1}: ${line.trim()}`] : [],
        ),
    );
    expect(found).toEqual([]);
  });
});
