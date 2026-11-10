// The selector rule (see `selectorStability`), enforced over the source: every
// inline `useAppStore` selector in `src/` is read, so one that builds its
// result fails here rather than in a release, whether or not a test happens to
// render the component around it.

import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { unstableSelectors } from "./selectorStability";

const SRC = fileURLToPath(new URL("..", import.meta.url));

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const path = join(dir, e.name);
    if (e.isDirectory()) return sourceFiles(path);
    if (!/\.(ts|tsx)$/.test(e.name) || /\.(test|spec)\.tsx?$/.test(e.name)) return [];
    return [path];
  });
}

const lines = (source: string) => unstableSelectors("case.tsx", source).map((f) => f.line);
const flagged = (source: string) => lines(source).length > 0;

describe("unstableSelectors", () => {
  it("flags a selector whose result is built on the spot", () => {
    expect(flagged("useAppStore((s) => s.workspace?.projects ?? []);")).toBe(true);
    expect(flagged("useAppStore((s) => s.agents.filter((a) => a.live));")).toBe(true);
    expect(flagged("useAppStore((s) => ({ a: s.a, b: s.b }));")).toBe(true);
    expect(flagged("useAppStore((s) => Object.values(s.environments));")).toBe(true);
    expect(flagged("useAppStore((s) => (on ? s.list : [s.one]));")).toBe(true);
    expect(flagged("useAppStore((s) => new Set(s.ids));")).toBe(true);
  });

  it("reads slice and concat as an array's, unless the receiver is spelled as a string", () => {
    expect(flagged("useAppStore((s) => s.items.slice());")).toBe(true);
    expect(flagged("useAppStore((s) => s.items.concat(extra));")).toBe(true);
    expect(flagged("useAppStore((s) => s.items?.slice(0, 3) ?? null);")).toBe(true);
    expect(flagged("useAppStore(useShallow((s) => keys.map((k) => s.lists[k].slice(-5))));")).toBe(
      true,
    );
    expect(
      unstableSelectors("case.tsx", "useAppStore((s) => s.id.slice(0, 8));")[0].message,
    ).toMatch(/String\(…\)/);

    expect(flagged("useAppStore((s) => String(s.id).slice(0, 8));")).toBe(false);
    expect(flagged(`useAppStore((s) => \`\${s.first} \`.concat(s.last));`)).toBe(false);
    expect(flagged('useAppStore((s) => "#".concat(s.id));')).toBe(false);
    expect(flagged("useAppStore((s) => s.items.slice(0, 3).length);")).toBe(false);
  });

  it("follows a method that hands back the array it was called on", () => {
    expect(flagged("useAppStore((s) => [...s.items].sort(byName));")).toBe(true);
    expect(flagged("useAppStore((s) => s.items.filter(isLive).reverse());")).toBe(true);
    expect(flagged("useAppStore((s) => s.items.slice().sort(byName));")).toBe(true);
    expect(flagged("useAppStore((s) => s.items.sort(byName));")).toBe(false);
  });

  it("follows a const the selector returns", () => {
    const source = `useAppStore((s) => {
      const live = s.agents.filter((a) => a.live);
      return live;
    });`;
    expect(lines(source)).toEqual([2]);
  });

  it("flags an element built under useShallow", () => {
    // The Code panel's selector as 0.8.23 shipped it.
    const source = `const checkouts = useAppStore(
      useShallow((s) =>
        agent.repos.map((repo, i): CheckoutGit => {
          const key = checkoutKey(agent.id, i === 0 ? undefined : repo.subdir);
          return { subdir: repo.subdir, state: s.gitStates[key], blocked: s.gitBlocked[key] };
        }),
      ),
    );`;
    expect(lines(source)).toEqual([5]);
    expect(flagged("useAppStore(useShallow((s) => keys.map((k) => s.files[k] ?? [])));")).toBe(
      true,
    );
    expect(flagged("useAppStore(useShallow((s) => [s.a, { b: s.b }]));")).toBe(true);
    expect(flagged("useAppStore(useShallow((s) => ({ a: s.a, ids: Object.keys(s.b) })));")).toBe(
      true,
    );
  });

  it("passes the store's own values, primitives and constants", () => {
    expect(flagged("useAppStore((s) => s.gitStates[agent.id]);")).toBe(false);
    expect(flagged("useAppStore((s) => s.workspace?.agents ?? EMPTY_AGENTS);")).toBe(false);
    expect(flagged("useAppStore((s) => Object.keys(s.unseenResults).length);")).toBe(false);
    expect(flagged("useAppStore((s) => s.agents.filter((a) => a.live).length > 0);")).toBe(false);
    expect(flagged("useAppStore((s) => s.agents.find((a) => a.id === id));")).toBe(false);
    expect(flagged("useAppStore((s) => isAgentBusy(s, agent.id, agent.status));")).toBe(false);
    expect(flagged("useAppStore(activeEntry);")).toBe(false);
  });

  it("passes a shallow collection of the store's own values", () => {
    expect(flagged("useAppStore(useShallow((s) => keys.map((k) => s.gitStates[k])));")).toBe(false);
    expect(flagged("useAppStore(useShallow((s) => keys.map((k) => s.prChecks[k] ?? null)));")).toBe(
      false,
    );
    expect(flagged("useAppStore(useShallow((s) => [s.a, s.b, s.c.length]));")).toBe(false);
    expect(flagged("useAppStore(useShallow((s) => ({ a: s.a, b: s.b })));")).toBe(false);
  });
});

describe("store selectors in src/", () => {
  it("return the same value for the same state", () => {
    const found = sourceFiles(SRC).flatMap((path) =>
      unstableSelectors(path, readFileSync(path, "utf8")).map(
        (f) => `${relative(SRC, path)}:${f.line}: ${f.message}`,
      ),
    );
    // Fix: select the store's own values and derive from them in a `useMemo`,
    // or fall back to a module-level constant instead of a literal.
    expect(found).toEqual([]);
  });
});
