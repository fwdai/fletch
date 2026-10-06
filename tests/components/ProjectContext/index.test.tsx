import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { ContextAssertion, ContextOverview, ContextProposal } from "@/api";
import { ContextTab } from "@/components/ProjectContext";
import { AssertionRow } from "@/components/ProjectContext/AssertionRow";
import { ReviewQueue } from "@/components/ProjectContext/ReviewQueue";

// The tab's children call the API on user action only; a static render must
// never reach the transport.
vi.mock("@/api", () => ({
  api: new Proxy(
    {},
    {
      get: (_t, name) => () => {
        throw new Error(`api.${String(name)} called during a static render`);
      },
    },
  ),
  onContextChanged: () => Promise.resolve(() => {}),
}));

const stamp = { author: { kind: "user" as const }, source: { kind: "ui" as const } };
const fuji = {
  author: { kind: "agent" as const, agent_id: "fuji" },
  source: { kind: "agent_turn" as const },
};

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
    ...stamp,
    provenance: {},
    about: ["e1"],
    contradicts: [],
    ...over,
  };
}

const assertionPayload = {
  type: "assertion" as const,
  stamp: { ...stamp, provenance: {} },
  relation: { kind: "new" as const },
  about_pending: [] as string[],
  input: {
    kind: "decision" as const,
    domain: "architectural" as const,
    stance: "adopted" as const,
    statement: "One drainer per project",
    rationale: "",
    paths: [],
    about: ["e1"],
    contradicts: [],
    status: "provisional" as const,
  },
};

const assertionProposal: ContextProposal = {
  id: "p1",
  project_id: "ctx-1",
  payload: assertionPayload,
  evidence: [{ quote: "let's keep one drainer" }],
  status: "pending",
  created_at: 2,
};

const overview: ContextOverview = {
  enabled: true,
  extract: false,
  graph: {
    project_id: "ctx-1",
    entities: [
      {
        id: "e1",
        slug: "roadmap-queue",
        kind: "module",
        name: "Roadmap queue",
        summary: "Dispatches queued items.",
        aliases: ["drainer"],
        paths: ["crates/fletch-core/src/roadmap"],
        status: "active",
        recorded_at: 1,
        ...stamp,
      },
      {
        id: "e2",
        slug: "old",
        kind: "topic",
        name: "Archived topic",
        summary: "",
        aliases: [],
        paths: [],
        status: "archived",
        recorded_at: 1,
        ...stamp,
      },
    ],
    assertions: [
      assertion({ id: "a1", statement: "One drainer", ...fuji, provenance: { repo: "quorum" } }),
      assertion({ id: "a2", statement: "Many drainers" }),
      assertion({ id: "a3", statement: "A drainer per repo" }),
      assertion({ id: "a4", statement: "No drainer", status: "retracted" }),
    ],
    relations: [],
    current: ["a1", "a2", "a3"],
    contradictions: [
      { a: "a2", b: "a1", reasoning: "one vs many" },
      {
        a: "a1",
        b: "a3",
        reasoning: "scope",
        resolution: { reasoning: "per repo is one each", at: 0, by: { kind: "user" } },
      },
      { a: "a1", b: "a4" },
    ],
  },
  proposals: [assertionProposal],
  stats: {
    entities: 1,
    assertions: 0,
    heads: 0,
    provisional: 0,
    contradictions: 0,
    pending_proposals: 1,
    reads: 3,
    top_misses: [],
  },
};

describe("ContextTab", () => {
  it("renders the toggles, the entity list and the pending count without touching the API", () => {
    const html = renderToStaticMarkup(
      <ContextTab
        projectId="p1"
        overview={overview}
        settings={{ "context.extract": "false" }}
        onSave={() => {}}
        locked={false}
      />,
    );
    expect(html).toContain("Project context");
    // The live settings decide the switches: enabled absent → on, extract "false" → off.
    expect(html).toContain('data-on="1" aria-checked="true" role="switch"');
    expect(html).toContain('data-on="0" aria-checked="false" role="switch"');
    // Active entities only, grouped under their kind.
    expect(html).toContain("Modules");
    expect(html).toContain("Roadmap queue");
    expect(html).not.toContain("Archived topic");
    // The review badge carries the pending count.
    expect(html).toContain('class="pc-count text-xs">1<');
    expect(html).toContain("1 entities · 0 current assertions");
  });
});

describe("AssertionRow", () => {
  const { graph } = overview;

  it("marks the open tension with its reasoning and a Resolve, and names the checkout", () => {
    const html = renderToStaticMarkup(
      <AssertionRow assertion={graph.assertions[0]} graph={graph} projectId="p1" />,
    );
    expect(html).toContain("agent fuji · quorum · from agent turn");
    expect(html).toContain(">contradicted<");
    expect(html).toContain("Many drainers");
    expect(html).toContain("one vs many");
    expect(html).toContain(">Resolve<");
    // The resolved and the retracted sides carry no marker, only history.
    expect(html).not.toContain("A drainer per repo");
    expect(html).not.toContain("No drainer");
    expect(html).toContain(">History<");
  });

  it("shows no marker on a side whose tensions are all closed", () => {
    const html = renderToStaticMarkup(
      <AssertionRow assertion={graph.assertions[2]} graph={graph} projectId="p1" />,
    );
    expect(html).not.toContain("contradicted");
    expect(html).not.toContain(">Resolve<");
    expect(html).toContain(">History<");
  });
});

describe("ReviewQueue", () => {
  const { graph } = overview;

  it("gates Accept on a pending subject until its entity is accepted", () => {
    const blocked: ContextProposal = {
      ...assertionProposal,
      id: "p2",
      payload: { ...assertionPayload, about_pending: ["Roadmap-Queue", "sweeper"] },
    };
    const html = renderToStaticMarkup(
      <ReviewQueue proposals={[blocked]} graph={graph} projectId="p1" />,
    );
    expect(html).toContain("about Roadmap queue");
    expect(html).toContain("sweeper</span> (pending entity)");
    expect(html).toContain("accept the entity `sweeper` first");
    expect(html).toMatch(/<button[^>]*disabled[^>]*>Accept</);
  });

  it("renders an entity proposal with a live Accept", () => {
    const entityProposal: ContextProposal = {
      id: "p3",
      project_id: "ctx-1",
      payload: {
        type: "entity",
        stamp: { ...stamp, provenance: {} },
        input: {
          slug: "sweeper",
          kind: "module",
          name: "Sweeper",
          summary: "Clears stale items.",
          aliases: [],
          paths: [],
        },
      },
      evidence: [],
      status: "pending",
      created_at: 3,
    };
    const html = renderToStaticMarkup(
      <ReviewQueue proposals={[entityProposal]} graph={graph} projectId="p1" />,
    );
    expect(html).toContain("New module &quot;Sweeper&quot; (sweeper)");
    expect(html).toContain("Clears stale items.");
    expect(html).not.toMatch(/<button[^>]*disabled[^>]*>Accept</);
    expect(html).toContain(">Dismiss<");
  });
});
