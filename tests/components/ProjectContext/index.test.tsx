import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { ContextOverview } from "@/api";
import { ContextTab } from "@/components/ProjectContext";

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
    assertions: [],
    relations: [],
  },
  proposals: [
    {
      id: "p1",
      project_id: "ctx-1",
      payload: {
        type: "assertion",
        stamp: { ...stamp, provenance: {} },
        relation: { kind: "new" },
        input: {
          kind: "decision",
          domain: "architectural",
          stance: "adopted",
          statement: "One drainer per project",
          rationale: "",
          paths: [],
          about: ["e1"],
          contradicts: [],
          status: "provisional",
        },
      },
      evidence: [{ quote: "let's keep one drainer" }],
      status: "pending",
      created_at: 2,
    },
  ],
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
