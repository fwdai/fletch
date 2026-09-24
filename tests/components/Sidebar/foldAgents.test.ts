import { describe, expect, it } from "vitest";
import type { AgentRecord } from "@/api";
import { foldAgents } from "@/components/Sidebar/foldAgents";

const DAY = 86_400_000;
const NOW = Date.parse("2026-09-24T12:00:00Z");

/** A sidebar agent `daysAgo` days old. Lists are newest first, as the
 *  sidebar sorts them. */
function agent(id: string, daysAgo: number, status: AgentRecord["status"] = "idle"): AgentRecord {
  return {
    id,
    project_id: "p",
    name: id,
    provider: "claude",
    repos: [{ repo_path: "/r", subdir: "" }],
    task: "",
    status,
    view: "custom",
    created_at: new Date(NOW - daysAgo * DAY).toISOString(),
  };
}

const ids = (list: AgentRecord[]) => list.map((a) => a.id);

describe("foldAgents", () => {
  it("shows only the recent rows when the project has fewer than the cap", () => {
    const list = [agent("a", 0), agent("b", 1), agent("c", 3), agent("d", 20), agent("e", 40)];
    const { shown, hidden } = foldAgents(list, { now: NOW });
    expect(ids(shown)).toEqual(["a", "b", "c"]);
    expect(ids(hidden)).toEqual(["d", "e"]);
  });

  it("caps a project with many recent rows", () => {
    const list = Array.from({ length: 8 }, (_, i) => agent(`a${i}`, i));
    const { shown, hidden } = foldAgents(list, { now: NOW });
    expect(ids(shown)).toEqual(["a0", "a1", "a2", "a3", "a4"]);
    expect(hidden).toHaveLength(3);
  });

  it("does not fold when nothing would hide", () => {
    const list = [agent("a", 0), agent("b", 2)];
    expect(foldAgents(list, { now: NOW })).toEqual({ shown: list, hidden: [] });
  });

  it("does not fold a single row — the fold row would cost the same space", () => {
    const list = [agent("a", 0), agent("b", 30)];
    expect(ids(foldAgents(list, { now: NOW }).shown)).toEqual(["a", "b"]);
    expect(foldAgents(list, { now: NOW }).hidden).toEqual([]);
  });

  it("keeps the selected row out even when stale or past the cap", () => {
    const list = [...Array.from({ length: 7 }, (_, i) => agent(`a${i}`, i)), agent("old", 45)];
    const { shown, hidden } = foldAgents(list, { now: NOW, selectedId: "old" });
    expect(ids(shown)).toEqual(["a0", "a1", "a2", "a3", "a4", "old"]);
    expect(ids(hidden)).toEqual(["a5", "a6"]);
  });

  it("keeps live rows out regardless of age, but not old errors", () => {
    const list = [
      agent("a", 0),
      agent("stale-running", 30, "running"),
      agent("stale-error", 31, "error"),
      agent("stale-idle", 32),
      agent("stale-stopped", 33, "stopped"),
    ];
    const { shown, hidden } = foldAgents(list, { now: NOW });
    expect(ids(shown)).toEqual(["a", "stale-running"]);
    expect(ids(hidden)).toEqual(["stale-error", "stale-idle", "stale-stopped"]);
  });

  it("counts pinned rows toward the cap", () => {
    const list = [
      agent("live", 0, "running"),
      ...Array.from({ length: 7 }, (_, i) => agent(`a${i}`, i)),
    ];
    const { shown } = foldAgents(list, { now: NOW });
    expect(ids(shown)).toEqual(["live", "a0", "a1", "a2", "a3"]);
  });

  it("hides every row of a project untouched for weeks", () => {
    const list = [agent("a", 20), agent("b", 25), agent("c", 60)];
    const { shown, hidden } = foldAgents(list, { now: NOW });
    expect(shown).toEqual([]);
    expect(hidden).toHaveLength(3);
  });

  it("keeps a row whose timestamp cannot be parsed", () => {
    const bad = { ...agent("bad", 0), created_at: "not a date" };
    const list = [bad, agent("x", 30), agent("y", 31)];
    expect(ids(foldAgents(list, { now: NOW }).shown)).toEqual(["bad"]);
  });
});
