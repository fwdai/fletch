import { describe, expect, it } from "vitest";
import type { RoadmapItem, RoadmapItemEvent } from "@/api";
import { buildInbox, inboxChatPrompt, inboxChip } from "./inbox";

function item(over: Partial<RoadmapItem> & { id: string }): RoadmapItem {
  return {
    project_id: "p1",
    code: over.id.toUpperCase(),
    title: "t",
    why: "",
    horizon: "later",
    status: "captured",
    rank: 1,
    area: null,
    source: "user",
    accept: [],
    deps: [],
    agent_id: null,
    workflow_def_id: null,
    run_id: null,
    pr_url: null,
    pr_number: null,
    hold_reason: null,
    held_by: null,
    held_at: null,
    close_reason: null,
    issue_url: null,
    kind: null,
    triage: null,
    from_item: null,
    created_at: 0,
    updated_at: 0,
    ...over,
  };
}

function event(over: Partial<RoadmapItemEvent> & { item_id: string }): RoadmapItemEvent {
  return {
    id: `e-${over.item_id}`,
    project_id: "p1",
    actor: "drainer",
    kind: "note",
    detail: "Triage pending: no planning chat",
    created_at: 1,
    ...over,
  };
}

describe("buildInbox", () => {
  it("takes only captured rows, folding mined brainstorms away", () => {
    const fresh = item({ id: "a" });
    const idea = item({ id: "b", kind: "idea", triage: { brief: "x", questions: ["q"] } });
    const mined = item({ id: "c", kind: "brainstorm", triage: { mined: true } });
    const ghost = item({ id: "d", status: "proposed", from_item: "c" });
    const open = item({ id: "e", status: "open", from_item: "c" });
    const { open: cards, mined: folded } = buildInbox({
      rows: [fresh, idea, mined, ghost, open],
      latestEvents: [],
    });
    expect(cards.map((e) => e.item.id)).toEqual(["a", "b"]);
    expect(folded.map((e) => e.item.id)).toEqual(["c"]);
    expect(folded[0].derived).toEqual(["D", "E"]);
  });

  it("says an untriaged row is waiting only while the host's note is its latest word", () => {
    const fresh = item({ id: "a" });
    const waiting = buildInbox({ rows: [fresh], latestEvents: [event({ item_id: "a" })] });
    expect(waiting.open[0].waiting).toBe("Triage pending: no planning chat");

    const byPm = buildInbox({
      rows: [fresh],
      latestEvents: [event({ item_id: "a", actor: "pm" })],
    });
    expect(byPm.open[0].waiting).toBeNull();

    const triaged = item({ id: "a", kind: "idea", triage: { brief: "x" } });
    const done = buildInbox({ rows: [triaged], latestEvents: [event({ item_id: "a" })] });
    expect(done.open[0].waiting).toBeNull();
  });
});

describe("inboxChip", () => {
  const one = (over: Partial<RoadmapItem>, derived: string[] = [], waiting: string | null = null) =>
    ({ item: item({ id: "a", ...over }), derived, waiting }) as const;

  it("follows the dial while untriaged", () => {
    expect(inboxChip(one({}), true)).toBe("Triaging…");
    expect(inboxChip(one({}), false)).toBe("Untriaged");
    expect(inboxChip(one({}, [], "no chat"), true)).toBe("Waiting for a PM chat");
  });

  it("counts an idea's questions and names a brainstorm's tickets", () => {
    expect(
      inboxChip(one({ kind: "idea", triage: { brief: "b", questions: ["1", "2"] } }), true),
    ).toBe("Idea · brief, 2 questions");
    expect(inboxChip(one({ kind: "idea", triage: { brief: "b", questions: ["1"] } }), false)).toBe(
      "Idea · brief, 1 question",
    );
    expect(
      inboxChip(one({ kind: "brainstorm", triage: { mined: true } }, ["FLT-1", "FLT-2"]), true),
    ).toBe("Mined → FLT-1, FLT-2");
  });
});

describe("inboxChatPrompt", () => {
  it("names the code and the op that settles it", () => {
    const prompt = inboxChatPrompt(item({ id: "flt-120", title: "remember the port" }));
    expect(prompt).toContain("FLT-120");
    expect(prompt).toContain("remember the port");
    expect(prompt).toContain("roadmap_triage");
  });
});
