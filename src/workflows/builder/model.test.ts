// Spec ↔ editor-state mapping. The load-bearing
// property: for any spec the builder or importer can produce, round-tripping
// through the editor is value-preserving.

import { describe, expect, it } from "vitest";
import { GATE_MODES } from "../data";
import type { Definition, Gate, Spec } from "../spec";
import { editorStepIds, insertStep, patchStep, removeStep, setAgent, setField } from "./edits";
import {
  blankEditor,
  type EditorState,
  ensureAlias,
  fromDefinition,
  newStep,
  toSpec,
  validateEditor,
} from "./model";

/** A four-step pipeline in the stored (JSON) shape: gates always present, empty
 *  collections omitted — exactly what `wf_def_save` persists. */
function canonicalSpec(): Spec {
  return {
    version: 1,
    name: "feature-pipeline",
    description: "Plan, implement, review, fix",
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
          gate: { type: "verdict" },
        },
      },
      {
        step: {
          id: "implement",
          agent: "coder",
          goal: "Implement the slices from PLAN.md.",
          gate: { type: "commit" },
        },
      },
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
  it("round-trips a step pipeline losslessly", () => {
    const spec = canonicalSpec();
    const editor = fromDefinition(asDefinition(spec));
    expect(toSpec(editor)).toEqual(spec);
  });

  it("reads a step with no gate as a verdict step, like serde does", () => {
    const spec = canonicalSpec();
    delete spec.workflow[0].step.gate;
    const out = toSpec(fromDefinition(asDefinition(spec)));
    expect(out.workflow[0].step.gate).toEqual({ type: "verdict" });
  });

  it("preserves the definition id and hue for the edit path", () => {
    const editor = fromDefinition(asDefinition(canonicalSpec()));
    expect(editor.id).toBe("d1");
    expect(editor.hue).toBe(265);
  });

  it("prunes aliases no longer referenced by any step", () => {
    const editor = fromDefinition(asDefinition(canonicalSpec()));
    editor.blocks = editor.blocks.filter((s) => s.stepId !== "review");
    const spec = toSpec(editor);
    expect(Object.keys(spec.agents).sort()).toEqual(["coder", "planner"]);
    expect(spec.agents.reviewer).toBeUndefined();
  });
});

describe("toSpec only produces what the kernel runs", () => {
  // Compile-time half: a gate kind added to (or missing from) `Gate` breaks this
  // literal, so the runtime checks below can't silently fall behind the type.
  const KERNEL_GATES: Record<Gate["type"], true> = { commit: true, verdict: true };
  const SPEC_KEYS = new Set(["version", "name", "description", "agents", "workflow", "finalize"]);

  /** A non-empty flat list of `{ step }` entries, each carrying exactly the four
   *  step fields and a kernel gate, under a spec with no top-level extras. */
  function expectKernelShape(spec: Spec) {
    expect(Object.keys(spec).every((k) => SPEC_KEYS.has(k))).toBe(true);
    expect(spec.workflow.length).toBeGreaterThan(0);
    for (const entry of spec.workflow) {
      expect(Object.keys(entry)).toEqual(["step"]);
      expect(Object.keys(entry.step).sort()).toEqual(["agent", "gate", "goal", "id"]);
      expect(Object.keys(KERNEL_GATES)).toContain(entry.step.gate?.type);
      expect(Object.keys(entry.step.gate ?? {})).toEqual(["type"]);
    }
  }

  it("offers exactly the kernel's gates in the inspector", () => {
    expect(GATE_MODES.map((m) => m.id).sort()).toEqual(Object.keys(KERNEL_GATES).sort());
  });

  it("serializes every state the builder's edits reach as flat steps", () => {
    let s: EditorState = setField(blankEditor(0), { name: "x", description: "d" });
    expectKernelShape(toSpec(s));

    // Append, insert at the front, insert between, then append again — every
    // way the canvas adds a step.
    for (const index of [undefined, 0, 1, undefined]) {
      s = insertStep(s, newStep(editorStepIds(s)), index);
      expectKernelShape(toSpec(s));
    }

    // Give every step an agent, instructions and each gate the inspector offers.
    for (const [i, step] of s.blocks.entries()) {
      s = setAgent(s, step.nid, i % 2 ? "codex" : "claude", []);
      s = patchStep(s, step.nid, { goal: `goal ${i}`, gate: { type: GATE_MODES[i % 2].id } });
      expectKernelShape(toSpec(s));
    }

    s = removeStep(s, s.blocks[1].nid);
    const spec = toSpec(s);
    expectKernelShape(spec);
    expect(spec.workflow.map((e) => e.step.id)).toEqual(s.blocks.map((b) => b.stepId));
    expect(validateEditor(s).ok).toBe(true);
  });

  it("keeps the shape for a loaded definition", () => {
    expectKernelShape(toSpec(fromDefinition(asDefinition(canonicalSpec()))));
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
  it("accepts the canonical pipeline", () => {
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

  it("flags a duplicate step id on the second occurrence", () => {
    const editor = fromDefinition(asDefinition(canonicalSpec()));
    editor.blocks[2].stepId = "plan";
    const v = validateEditor(editor);
    expect(v.ok).toBe(false);
    expect(v.byNode[editor.blocks[0].nid]).toBeUndefined();
    expect(v.byNode[editor.blocks[2].nid]?.some((m) => m.includes("duplicate"))).toBe(true);
  });
});
