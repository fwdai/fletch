import { describe, expect, it } from "vitest";
import type { FileStatus, Tour } from "@/api";
import type { ViewItem } from "@/components/Workspace/messages/pair";
import { APP_ACTION_PREFIX } from "@/delegation";
import {
  deriveStepsFromTranscript,
  noteFrom,
  type Step,
  stepIndexFor,
  stepsFromTour,
} from "./model";

const changed = (...paths: string[]): FileStatus[] =>
  paths.map((path) => ({ path, kind: "modified", staged: false, additions: 1, deletions: 0 }));

let ids = 0;
const tool = (name: string, input: unknown, isError = false): ViewItem => {
  ids += 1;
  const id = `call-${ids}`;
  return {
    kind: "tool_pair",
    call: { kind: "tool_call", id, name, input },
    result: { kind: "tool_result", tool_use_id: id, content: "ok", is_error: isError },
  };
};
const edit = (path: string) =>
  tool("Edit", { file_path: path, old_string: "before", new_string: "after" });
const failedEdit = (path: string) =>
  tool("Edit", { file_path: path, old_string: "x", new_string: "y" }, true);
const patch = (...paths: string[]) =>
  tool("apply_patch", {
    input: [
      "*** Begin Patch",
      ...paths.flatMap((p) => [`*** Update File: ${p}`, "@@", "-old", "+new"]),
      "*** End Patch",
    ].join("\n"),
  });
const read = (path: string) => tool("Read", { file_path: path });
const say = (text: string): ViewItem => ({ kind: "agent_message", text });
const prompt = (text = "Please do the thing"): ViewItem => ({ kind: "user_message", text });

/** A transcript step's shape, for the fields the test cares about. */
const step = (note: string, paths: string[], turn?: number): Step => ({
  title: null,
  note,
  anchors: paths.map((path) => ({ path })),
  stale: false,
  turn,
});

describe("stepsFromTour", () => {
  it("maps each step and anchor 1:1 and marks a step stale when none of its files is in view", () => {
    const tour: Tour = {
      agent_id: "a1",
      turn_id: null,
      created_at: 0,
      steps: [
        {
          title: "The store",
          note: "Adds the slice.",
          anchors: [
            { path: "src/store.ts", start: 3, end: 9 },
            { path: "src/gone.ts", start: 1, end: 2 },
          ],
        },
        { title: null, note: "Reverted.", anchors: [{ path: "src/gone.ts", start: 1, end: 1 }] },
        {
          title: null,
          note: "Absolute.",
          anchors: [{ path: "/w/repo/src/view.tsx", start: 5, end: 7 }],
        },
        { title: null, note: "Nothing.", anchors: [] },
      ],
    };
    expect(stepsFromTour(tour, changed("src/store.ts", "src/view.tsx"))).toEqual([
      {
        title: "The store",
        note: "Adds the slice.",
        anchors: [
          { path: "src/store.ts", range: { start: 3, end: 9 } },
          { path: "src/gone.ts", range: { start: 1, end: 2 } },
        ],
        stale: false,
      },
      {
        title: null,
        note: "Reverted.",
        anchors: [{ path: "src/gone.ts", range: { start: 1, end: 1 } }],
        stale: true,
      },
      {
        title: null,
        note: "Absolute.",
        anchors: [{ path: "src/view.tsx", range: { start: 5, end: 7 } }],
        stale: false,
      },
      { title: null, note: "Nothing.", anchors: [], stale: true },
    ]);
  });
});

describe("deriveStepsFromTranscript", () => {
  const files = changed("src/store.ts", "src/view.tsx", "src/style.css");

  it("orders steps by edit, each captioned with the prose just before it", () => {
    const items = [
      prompt(),
      say("First the store."),
      read("src/store.ts"),
      edit("src/store.ts"),
      say("Then the view."),
      edit("src/view.tsx"),
    ];
    expect(deriveStepsFromTranscript(items, files)).toEqual([
      step("First the store.", ["src/store.ts"], 0),
      step("Then the view.", ["src/view.tsx"], 0),
    ]);
  });

  it("extends a step with consecutive edits to its own files", () => {
    const items = [
      prompt(),
      say("Store."),
      edit("src/store.ts"),
      say("More of the store."),
      edit("src/store.ts"),
      edit("src/store.ts"),
      edit("src/view.tsx"),
      edit("src/store.ts"),
    ];
    // The extending edit consumed "More of the store.", so the view's step,
    // which the agent didn't narrate, isn't captioned with it.
    expect(deriveStepsFromTranscript(items, files)).toEqual([
      step("Store.", ["src/store.ts"], 0),
      step("Edited view.tsx", ["src/view.tsx"], 0),
      step("Edited store.ts", ["src/store.ts"], 0),
    ]);
  });

  it("starts a step when an edit reaches a file the step doesn't cover", () => {
    const items = [
      prompt(),
      edit("src/store.ts"),
      patch("src/store.ts", "src/view.tsx"),
      edit("src/view.tsx"),
    ];
    expect(deriveStepsFromTranscript(items, files)).toEqual([
      step("Edited store.ts", ["src/store.ts"], 0),
      step("Edited 2 files", ["src/store.ts", "src/view.tsx"], 0),
    ]);
  });

  it("gives an unnarrated step the narration of an edit that extends it", () => {
    const items = [prompt(), edit("src/store.ts"), say("Fixing the store."), edit("src/store.ts")];
    expect(deriveStepsFromTranscript(items, files)).toEqual([
      step("Fixing the store.", ["src/store.ts"], 0),
    ]);
  });

  it("matches absolute tool paths to the checkout-relative file, the longest match winning", () => {
    const items = [prompt(), edit("/w/repo/src/a.ts"), edit("/w/repo/a.ts")];
    expect(deriveStepsFromTranscript(items, changed("a.ts", "src/a.ts"))).toEqual([
      step("Edited a.ts", ["src/a.ts"], 0),
      step("Edited a.ts", ["a.ts"], 0),
    ]);
  });

  it("drops edits to files not in view, and steps they leave without a file", () => {
    const items = [
      prompt(),
      say("A scratch file."),
      edit("/tmp/scratch.txt"),
      edit("src/store.ts"),
      say("Two files, one since reverted."),
      patch("src/view.tsx", "src/reverted.ts"),
    ];
    // The scratch edit took its narration with it.
    expect(deriveStepsFromTranscript(items, files)).toEqual([
      step("Edited store.ts", ["src/store.ts"], 0),
      step("Two files, one since reverted.", ["src/view.tsx"], 0),
    ]);
  });

  it("skips streaming prose and failed edits; a retry takes the narration", () => {
    const items: ViewItem[] = [
      prompt(),
      say("Fix the store."),
      { kind: "agent_message", text: "Half a thoug", streaming: true },
      failedEdit("src/store.ts"),
      edit("src/store.ts"),
    ];
    expect(deriveStepsFromTranscript(items, files)).toEqual([
      step("Fix the store.", ["src/store.ts"], 0),
    ]);
  });

  it("numbers turns by the prompts the user wrote, and a prompt ends the step", () => {
    const items = [
      edit("src/style.css"),
      prompt(),
      edit("src/store.ts"),
      { kind: "user_message", text: `${APP_ACTION_PREFIX}commit` } as ViewItem,
      edit("src/view.tsx"),
      say("All done."),
      prompt("One more thing"),
      edit("src/view.tsx"),
    ];
    // The closing summary of turn 0 doesn't caption turn 1's first edit.
    expect(deriveStepsFromTranscript(items, files)).toEqual([
      step("Edited style.css", ["src/style.css"], undefined),
      step("Edited store.ts", ["src/store.ts"], 0),
      step("Edited view.tsx", ["src/view.tsx"], 0),
      step("Edited view.tsx", ["src/view.tsx"], 1),
    ]);
  });
});

describe("noteFrom", () => {
  it("keeps the first paragraph of prose, without headings, code or list markers", () => {
    expect(noteFrom("## Plan\n\n- Move the store\n- then wire the view\n\nLater.")).toBe(
      "Move the store then wire the view",
    );
    expect(noteFrom("```ts\nconst x = 1;\n```\nNow the view.")).toBe("Now the view.");
    expect(noteFrom("# Only a heading")).toBe("");
  });

  it("cuts long prose on a word boundary with an ellipsis", () => {
    const note = noteFrom("lorem ".repeat(100));
    expect(note.length).toBeLessThanOrEqual(280);
    expect(note).toMatch(/^(lorem )*lorem…$/);
  });
});

describe("stepIndexFor", () => {
  const on = (path: string): Step => ({ title: null, note: "", anchors: [{ path }], stale: false });
  const steps = [on("a"), on("b"), on("a"), on("c"), on("a")];

  it("picks the step with the file nearest the last one, the later on a tie", () => {
    expect(stepIndexFor(steps, "a", 0)).toBe(0);
    expect(stepIndexFor(steps, "a", 1)).toBe(2);
    expect(stepIndexFor(steps, "a", 3)).toBe(4);
    expect(stepIndexFor(steps, "b", 4)).toBe(1);
  });

  it("is -1 for a file no step covers, and on the file list", () => {
    expect(stepIndexFor(steps, "z", 0)).toBe(-1);
    expect(stepIndexFor(steps, null, 0)).toBe(-1);
  });
});
