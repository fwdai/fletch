import { describe, expect, it } from "vitest";
import type { WorkspaceTypeOption } from "@/api";
import { creationNote } from "./creationNote";

const option = (over: Partial<WorkspaceTypeOption>): WorkspaceTypeOption => ({
  kind: "host",
  available: true,
  reason: null,
  default: false,
  ...over,
});

const sentence = (note: ReturnType<typeof creationNote>, path = "~/.fletch/workspaces/bryce") =>
  note.kind === "path" ? `${note.before}${path}${note.after}` : note.text;

describe("creation note", () => {
  it("says a VM is created and nothing lands on this Mac", () => {
    expect(sentence(creationNote(option({ kind: "box" }), 1))).toBe(
      "A Linux VM will be created for this workspace, with the checkout on the VM's own disk at ~/.fletch/workspaces/bryce. Nothing is checked out on this Mac.",
    );
    expect(sentence(creationNote(option({ kind: "box" }), 3))).toContain(
      "with checkouts of all 3 repositories on the VM's own disk at",
    );
  });

  it("says the macOS sandbox checks out on this Mac", () => {
    expect(sentence(creationNote(option({ kind: "host" }), 1))).toBe(
      "A checkout will be created on this Mac at ~/.fletch/workspaces/bryce, and the agent will run there in the macOS sandbox.",
    );
    expect(sentence(creationNote(option({ kind: "host" }), 2))).toMatch(
      /^Checkouts of all 2 repositories will be created on this Mac at /,
    );
  });

  it("says why when the declared sandbox can't be had, with no path", () => {
    expect(
      creationNote(option({ kind: "box", available: false, reason: "No VM runtime." }), 1),
    ).toEqual({ kind: "blocked", text: "The Linux VM isn't available: No VM runtime." });
    expect(creationNote(option({ kind: "box", available: false }), 1)).toEqual({
      kind: "blocked",
      text: "The Linux VM isn't available.",
    });
  });

  it("keeps to the general shape while the host decides", () => {
    expect(sentence(creationNote(null, 1))).toBe(
      "A checkout and sandbox will be created at ~/.fletch/workspaces/bryce",
    );
    expect(sentence(creationNote(null, 2))).toMatch(
      /^Checkouts of all 2 repositories and a sandbox/,
    );
  });
});
