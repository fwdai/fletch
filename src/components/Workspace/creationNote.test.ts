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

const line = (note: ReturnType<typeof creationNote>) =>
  note.kind === "path" ? `${note.before}~/.fletch/workspaces/bryce` : note.text;

describe("creation note", () => {
  it("names where the checkout is created", () => {
    expect(line(creationNote(option({ kind: "box" }), 1))).toBe(
      "A checkout will be created in a Linux VM at ~/.fletch/workspaces/bryce",
    );
    expect(line(creationNote(option({ kind: "host" }), 1))).toBe(
      "A checkout will be created on this Mac at ~/.fletch/workspaces/bryce",
    );
    expect(line(creationNote(option({ kind: "box" }), 3))).toBe(
      "Checkouts of all 3 repositories will be created in a Linux VM at ~/.fletch/workspaces/bryce",
    );
  });

  it("says why when the declared sandbox can't be had", () => {
    expect(
      line(creationNote(option({ kind: "box", available: false, reason: "No VM runtime." }), 1)),
    ).toBe("Linux VM isn't available: No VM runtime");
    expect(line(creationNote(option({ kind: "box", available: false }), 1))).toBe(
      "Linux VM isn't available",
    );
  });

  it("keeps the general line while the host decides", () => {
    expect(line(creationNote(null, 1))).toBe(
      "A checkout and sandbox will be created at ~/.fletch/workspaces/bryce",
    );
  });
});
