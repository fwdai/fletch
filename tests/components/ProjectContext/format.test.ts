import { describe, expect, it } from "vitest";
import type { ContextAssertion, ContextEntity, ContextGraph } from "@/api";
import {
  authorLabel,
  contradictedBy,
  flagOn,
  groupAssertions,
  groupEntitiesByKind,
  headsAbout,
  historyOf,
  matchesSearch,
  slugify,
  sourceLabel,
  splitList,
  statusVariant,
  summarizeProposal,
} from "@/components/ProjectContext/format";

const user = { kind: "user" as const };
const ui = { kind: "ui" as const };

function entity(over: Partial<ContextEntity> & { id: string }): ContextEntity {
  return {
    slug: over.id,
    kind: "feature",
    name: over.id,
    summary: "",
    aliases: [],
    paths: [],
    status: "active",
    recorded_at: 0,
    author: user,
    source: ui,
    ...over,
  };
}

function assertion(over: Partial<ContextAssertion> & { id: string }): ContextAssertion {
  return {
    kind: "decision",
    domain: "architectural",
    stance: "adopted",
    statement: over.id,
    rationale: "",
    valid_from: 0,
    paths: [],
    status: "confirmed",
    recorded_at: 0,
    author: user,
    source: ui,
    provenance: {},
    about: ["e1"],
    contradicts: [],
    ...over,
  };
}

/** `current` as the host derives it: live, and nothing live further down the
 *  supersession chain (`compile::is_current`). */
function graph(entities: ContextEntity[], assertions: ContextAssertion[]): ContextGraph {
  const live = (a: ContextAssertion) => a.status !== "retracted" && a.status !== "abandoned";
  const isCurrent = (a: ContextAssertion): boolean => {
    if (!live(a)) return false;
    let next = assertions.find((s) => s.id === a.superseded_by);
    while (next) {
      if (live(next)) return false;
      next = assertions.find((s) => s.id === next?.superseded_by);
    }
    return true;
  };
  return {
    project_id: "p",
    entities,
    assertions,
    relations: [],
    current: assertions.filter(isCurrent).map((a) => a.id),
  };
}

describe("flagOn", () => {
  it("is opt-out: only the literal false is off", () => {
    expect(flagOn(undefined)).toBe(true);
    expect(flagOn("1")).toBe(true);
    expect(flagOn("false")).toBe(false);
  });
});

describe("groupEntitiesByKind", () => {
  it("groups active entities in vocabulary order, sorted by name, dropping empty kinds", () => {
    const groups = groupEntitiesByKind([
      entity({ id: "b", kind: "feature", name: "Beta" }),
      entity({ id: "v", kind: "vision", name: "Vision" }),
      entity({ id: "a", kind: "feature", name: "Alpha" }),
      entity({ id: "old", kind: "module", status: "archived" }),
      entity({ id: "m", kind: "module", status: "merged" }),
    ]);
    expect(groups.map((g) => g.kind)).toEqual(["vision", "feature"]);
    expect(groups[1].entities.map((e) => e.name)).toEqual(["Alpha", "Beta"]);
  });
});

describe("matchesSearch", () => {
  const e = entity({ id: "x", slug: "roadmap-queue", name: "Roadmap queue", aliases: ["drainer"] });
  it("matches slug, name and aliases case-insensitively; blank matches all", () => {
    expect(matchesSearch(e, "")).toBe(true);
    expect(matchesSearch(e, "QUEUE")).toBe(true);
    expect(matchesSearch(e, "drain")).toBe(true);
    expect(matchesSearch(e, "sweep")).toBe(false);
  });
});

describe("headsAbout / groupAssertions", () => {
  it("keeps heads about the entity, grouped by kind then domain, adopted before rejected", () => {
    const g = graph(
      [entity({ id: "e1" })],
      [
        assertion({ id: "old", superseded_by: "new" }),
        assertion({ id: "new", supersedes: { id: "old", reasoning: "moved" }, recorded_at: 2 }),
        assertion({ id: "no", stance: "rejected", recorded_at: 9 }),
        assertion({ id: "fact", kind: "fact", domain: "implementation" }),
        assertion({ id: "elsewhere", about: ["e2"] }),
      ],
    );
    const heads = headsAbout(g, "e1");
    expect(heads.map((a) => a.id)).toEqual(["new", "no", "fact"]);

    const groups = groupAssertions(heads);
    expect(groups.map((x) => x.kind)).toEqual(["decision", "fact"]);
    expect(groups[0].domains[0].domain).toBe("architectural");
    // Adopted first even though the rejected one is newer.
    expect(groups[0].domains[0].assertions.map((a) => a.id)).toEqual(["new", "no"]);
  });
});

describe("historyOf", () => {
  it("walks supersedes ids newest predecessor first and stops on a cycle", () => {
    const g = graph(
      [],
      [
        assertion({ id: "a1" }),
        assertion({ id: "a2", supersedes: { id: "a1", reasoning: "r1" } }),
        assertion({ id: "a3", supersedes: { id: "a2", reasoning: "r2" } }),
        assertion({ id: "loop", supersedes: { id: "loop", reasoning: "self" } }),
        assertion({ id: "dangling", supersedes: { id: "missing", reasoning: "" } }),
      ],
    );
    expect(historyOf(g, "a3").map((a) => a.id)).toEqual(["a2", "a1"]);
    expect(historyOf(g, "a1")).toEqual([]);
    expect(historyOf(g, "loop")).toEqual([]);
    expect(historyOf(g, "dangling")).toEqual([]);
  });
});

describe("contradictedBy", () => {
  it("counts only live heads", () => {
    const g = graph(
      [],
      [
        assertion({ id: "me", contradicts: ["live", "gone", "retracted", "missing"] }),
        assertion({ id: "live" }),
        assertion({ id: "gone", superseded_by: "x" }),
        assertion({ id: "x", supersedes: { id: "gone", reasoning: "moved on" } }),
        assertion({ id: "retracted", status: "retracted" }),
      ],
    );
    expect(contradictedBy(g, g.assertions[0]).map((a) => a.id)).toEqual(["live"]);
  });
});

describe("labels", () => {
  it("names the author and the source", () => {
    expect(authorLabel({ kind: "user" })).toBe("user");
    expect(authorLabel({ kind: "agent", agent_id: "fuji" })).toBe("agent fuji");
    expect(authorLabel({ kind: "extractor" })).toBe("extractor");
    expect(sourceLabel({ kind: "user_turn" })).toBe("from user turn");
    expect(sourceLabel({ kind: "pr", reference: "#12" })).toBe("from PR #12");
    expect(sourceLabel({ kind: "ui" })).toBe("from UI");
  });

  it("maps every status to a badge tone", () => {
    expect(statusVariant("confirmed")).toBe("ok");
    expect(statusVariant("provisional")).toBe("warn");
    expect(statusVariant("retracted")).toBe("err");
    expect(statusVariant("abandoned")).toBe("neutral");
  });

  it("summarises a proposal's payload in one line", () => {
    const stamp = { author: user, source: ui, provenance: {} };
    expect(
      summarizeProposal({
        id: "p1",
        project_id: "p",
        payload: {
          type: "entity",
          stamp,
          input: {
            slug: "queue",
            kind: "module",
            name: "Queue",
            summary: "",
            aliases: [],
            paths: [],
          },
        },
        evidence: [],
        status: "pending",
        created_at: 0,
      }),
    ).toBe('New module "Queue" (queue)');
    expect(
      summarizeProposal({
        id: "p2",
        project_id: "p",
        payload: {
          type: "assertion",
          stamp,
          relation: { kind: "new" },
          input: {
            kind: "constraint",
            domain: "business",
            stance: "rejected",
            statement: "No ads",
            rationale: "",
            paths: [],
            about: ["e1"],
            contradicts: [],
            status: "provisional",
          },
        },
        evidence: [],
        status: "pending",
        created_at: 0,
      }),
    ).toBe("Rejected constraint (business): No ads");
  });
});

describe("form helpers", () => {
  it("splits comma lists and slugifies names", () => {
    expect(splitList(" a, b ,, c ")).toEqual(["a", "b", "c"]);
    expect(slugify("The Roadmap Queue!")).toBe("the-roadmap-queue");
  });
});
