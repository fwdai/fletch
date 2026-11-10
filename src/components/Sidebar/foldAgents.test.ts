import { describe, expect, it } from "vitest";
import type { AgentRecord } from "@/api";
import { foldAgents } from "./foldAgents";

const DAY = 86_400_000;
const NOW = Date.UTC(2026, 10, 10);

/** A workspace launched `daysAgo` days before NOW. */
const agent = (id: string, daysAgo: number, status: AgentRecord["status"] = "idle") =>
  ({ id, status, created_at: new Date(NOW - daysAgo * DAY).toISOString() }) as AgentRecord;

const ids = (list: AgentRecord[]) => list.map((a) => a.id);

describe("foldAgents", () => {
  it("shows a short list whole", () => {
    const agents = [agent("a", 1), agent("b", 2), agent("c", 30)];
    expect(foldAgents(agents, { now: NOW })).toEqual({ shown: agents, hidden: [] });
  });

  it("folds what was launched before the window", () => {
    const agents = [agent("a", 1), agent("b", 2), agent("c", 20), agent("d", 30)];
    const { shown, hidden } = foldAgents(agents, { now: NOW });
    expect(ids(shown)).toEqual(["a", "b"]);
    expect(ids(hidden)).toEqual(["c", "d"]);
  });

  it("keeps at most the cap of recent rows", () => {
    const agents = ["a", "b", "c", "d", "e", "f", "g"].map((id, i) => agent(id, i * 0.5));
    const { shown, hidden } = foldAgents(agents, { now: NOW });
    expect(ids(shown)).toEqual(["a", "b", "c", "d", "e"]);
    expect(ids(hidden)).toEqual(["f", "g"]);
  });

  it("counts an old workspace resumed today as recent", () => {
    const agents = [agent("a", 1), agent("b", 20), agent("c", 30), agent("d", 40)];
    const { shown, hidden } = foldAgents(agents, { now: NOW, activity: { c: NOW - DAY / 2 } });
    expect(ids(shown)).toEqual(["a", "c"]);
    expect(ids(hidden)).toEqual(["b", "d"]);
  });

  it("gives the cap to the rows last used, in list order", () => {
    const agents = ["a", "b", "c", "d", "e", "f", "g"].map((id, i) => agent(id, i * 0.5));
    const { shown, hidden } = foldAgents(agents, { now: NOW, activity: { g: NOW } });
    expect(ids(shown)).toEqual(["a", "b", "c", "d", "g"]);
    expect(ids(hidden)).toEqual(["e", "f"]);
  });

  it("keeps the selected and the live rows out, on top of the cap", () => {
    const agents = [
      ...["a", "b", "c", "d", "e"].map((id, i) => agent(id, i * 0.5)),
      agent("old", 20),
      agent("live", 30, "running"),
      agent("gone", 40),
      agent("gone2", 50),
    ];
    const { shown, hidden } = foldAgents(agents, { now: NOW, selectedId: "old" });
    expect(ids(shown)).toEqual(["a", "b", "c", "d", "e", "old", "live"]);
    expect(ids(hidden)).toEqual(["gone", "gone2"]);
  });
});
