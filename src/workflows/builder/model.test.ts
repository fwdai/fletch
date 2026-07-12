// Spec ↔ editor-state mapping (spec §5.3 / SLICES F1 acceptance). The load-bearing
// property: for any spec the builder or importer can produce, round-tripping
// through the editor is value-preserving.

import { describe, expect, it } from "vitest";
import type { Definition, Spec } from "../spec";
import { blankEditor, ensureAlias, fromDefinition, toSpec, validateEditor } from "./model";

/** The canonical §5.3 example in the stored (JSON) shape: gates always present,
 *  empty collections omitted — exactly what `wf_def_save` persists. */
function canonicalSpec(): Spec {
  return {
    version: 1,
    name: "feature-pipeline",
    description: "Plan, implement in parallel, review loop, ship",
    budgets: { turns: 120, wall_clock_mins: 240, tokens: 2000000 },
    agents: {
      planner: {
        base: "claude",
        model: "opus",
        instructions: "You are a senior architect. Produce small, independently testable slices.\n",
      },
      coder: { base: "codex" },
      reviewer: { base: "claude", skills: ["code-review"] },
    },
    workflow: [
      {
        step: {
          id: "plan",
          agent: "planner",
          goal: "Analyze the task and write PLAN.md describing independent slices.",
          gate: { type: "artifact", path: "PLAN.md" },
          budgets: { turns: 3 },
        },
      },
      {
        orchestrate: {
          agent: "planner",
          goal: "Assign one slice from PLAN.md per coder. Answer their questions.",
          children: { agent: "coder", max: 3 },
          join: "all",
          integrate: "merge",
          comms: ["report", "ask"],
          compose: { max_sub_runs: 2, max_depth: 2 },
        },
      },
      {
        loop: {
          max: 3,
          until: { step: "review", verdict: "done" },
          body: [
            {
              step: {
                id: "review",
                agent: "reviewer",
                goal: "Review the full diff vs the run base. Write verdict.json.",
                gate: { type: "verdict" },
              },
            },
            {
              step: {
                id: "fix",
                agent: "coder",
                goal: "Address the reviewer's feedback.",
                gate: { type: "commit" },
              },
            },
          ],
        },
      },
    ],
    finalize: { push: true, open_pr: true, pr_base: "main" },
  };
}

function asDefinition(spec: Spec): Definition {
  return {
    id: "d1",
    name: spec.name,
    description: spec.description ?? "",
    hue: 265,
    spec,
    run_count: 0,
    created_at: 0,
    updated_at: 0,
  };
}

describe("spec ↔ editor mapping", () => {
  it("round-trips the §5.3 example losslessly", () => {
    const spec = canonicalSpec();
    const editor = fromDefinition(asDefinition(spec));
    expect(toSpec(editor)).toEqual(spec);
  });

  it("preserves the definition id and hue for the edit path", () => {
    const editor = fromDefinition(asDefinition(canonicalSpec()));
    expect(editor.id).toBe("d1");
    expect(editor.hue).toBe(265);
  });

  it("resolves the loop exit step by identity, not stale id", () => {
    const editor = fromDefinition(asDefinition(canonicalSpec()));
    const loop = editor.blocks.find((b) => b.kind === "loop");
    expect(loop?.kind).toBe("loop");
    if (loop?.kind === "loop") {
      const review = loop.body.find((b) => b.kind === "step" && b.stepId === "review");
      expect(loop.untilNid).toBe(review?.nid);
    }
  });

  it("prunes aliases no longer referenced by any node", () => {
    const editor = fromDefinition(asDefinition(canonicalSpec()));
    // Drop the loop (removes review→reviewer and fix→coder refs; coder stays via
    // the orchestrate child template).
    editor.blocks = editor.blocks.filter((b) => b.kind !== "loop");
    const spec = toSpec(editor);
    expect(Object.keys(spec.agents).sort()).toEqual(["coder", "planner"]);
    expect(spec.agents.reviewer).toBeUndefined();
  });
});

describe("agent aliasing", () => {
  const customAgents = [
    {
      id: "ca-1",
      name: "Senior Reviewer",
      description: "",
      color: 0,
      base: "claude",
      model: null,
      effort: null,
      instructions: "",
      skillIds: [],
      mcpServerIds: [],
      created_at: 0,
      updated_at: 0,
    },
  ];

  it("creates a base-provider alias and reuses it for the same pick", () => {
    const a = ensureAlias({}, "codex", customAgents);
    expect(a.agents[a.alias]).toEqual({ base: "codex" });
    const b = ensureAlias(a.agents, "codex", customAgents);
    expect(b.alias).toBe(a.alias);
    expect(Object.keys(b.agents)).toHaveLength(1);
  });

  it("creates a custom-agent alias keyed by its local id", () => {
    const a = ensureAlias({}, "ca-1", customAgents);
    expect(a.agents[a.alias]).toEqual({ base: "claude", custom_agent: "ca-1" });
    expect(a.alias).toBe("senior-reviewer");
  });

  it("does not collide a custom agent with a bare base of the same provider", () => {
    const a = ensureAlias({}, "claude", customAgents);
    const b = ensureAlias(a.agents, "ca-1", customAgents);
    expect(b.alias).not.toBe(a.alias);
    expect(Object.keys(b.agents)).toHaveLength(2);
  });
});

describe("validation mirrors the load-bearing §5.2 rules", () => {
  it("accepts the canonical example", () => {
    expect(validateEditor(fromDefinition(asDefinition(canonicalSpec()))).ok).toBe(true);
  });

  it("blocks an unnamed workflow", () => {
    const s = blankEditor(0);
    expect(validateEditor(s).ok).toBe(false);
    expect(validateEditor(s).form.some((m) => m.includes("name"))).toBe(true);
  });

  it("flags an unassigned agent on the offending step", () => {
    const s = blankEditor(0);
    s.name = "x";
    const v = validateEditor(s);
    expect(v.ok).toBe(false);
    const step = s.blocks[0];
    expect(v.byNode[step.nid]?.some((m) => m.includes("agent"))).toBe(true);
  });

  it("flags loop.max = 0 on the loop node", () => {
    const editor = fromDefinition(asDefinition(canonicalSpec()));
    const loop = editor.blocks.find((b) => b.kind === "loop");
    if (loop?.kind === "loop") loop.max = 0;
    const v = validateEditor(editor);
    expect(v.ok).toBe(false);
    if (loop) expect(v.byNode[loop.nid]?.some((m) => m.includes("max"))).toBe(true);
  });

  it("rejects an absolute artifact path", () => {
    const s = blankEditor(0);
    s.name = "x";
    const step = s.blocks[0];
    if (step.kind === "step") {
      step.agent = "claude";
      step.gate = { type: "artifact", path: "/etc/passwd" };
    }
    const v = validateEditor(s);
    expect(v.byNode[step.nid]?.some((m) => m.includes("repo-relative"))).toBe(true);
  });
});
