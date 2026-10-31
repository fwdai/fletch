import { describe, expect, it } from "vitest";
import type { WorkspaceTypeOption } from "@/api";
import {
  defaultWorkspaceType,
  shownWorkspaceType,
  workspaceTypeLabel,
  workspaceTypeOffered,
  workspaceTypeToSend,
  workspaceTypeVisible,
} from "./workspaceType";

const host = (over: Partial<WorkspaceTypeOption> = {}): WorkspaceTypeOption => ({
  kind: "host",
  available: true,
  reason: null,
  default: false,
  ...over,
});
const box = (over: Partial<WorkspaceTypeOption> = {}): WorkspaceTypeOption => ({
  kind: "box",
  available: true,
  reason: null,
  default: false,
  ...over,
});

describe("workspace type", () => {
  it("defaults to what the host marks", () => {
    expect(defaultWorkspaceType([host({ default: true }), box()])).toBe("host");
    expect(defaultWorkspaceType([host(), box({ default: true })])).toBe("box");
    // A host pinning a container engine marks neither.
    expect(defaultWorkspaceType([host(), box()])).toBeNull();
    expect(defaultWorkspaceType([])).toBeNull();
  });

  it("keeps an explicit pick on the chip, the default included", () => {
    const options = [host({ default: true }), box()];
    expect(shownWorkspaceType(options, undefined)?.kind).toBe("host");
    expect(shownWorkspaceType(options, "box")?.kind).toBe("box");
    // Picking the default is a pick: if the default then moves, the chip (and
    // the spawn) stay where the user clicked.
    const moved = [host(), box({ default: true })];
    expect(shownWorkspaceType(moved, "host")?.kind).toBe("host");
    // A pick no longer available stays shown, so its reason is on screen.
    const refused = [host({ default: true }), box({ available: false, reason: "off" })];
    expect(shownWorkspaceType(refused, "box")).toEqual(refused[1]);
    // An unavailable default is shown with its reason too.
    const brokenDefault = [host(), box({ default: true, available: false, reason: "no runtime" })];
    expect(shownWorkspaceType(brokenDefault, undefined)?.reason).toBe("no runtime");
  });

  it("hides the chip when no option is the default", () => {
    expect(workspaceTypeVisible(null)).toBe(false);
    expect(workspaceTypeVisible([host(), box()])).toBe(false);
    expect(workspaceTypeVisible([host({ default: true }), box()])).toBe(true);
    expect(workspaceTypeVisible([host(), box({ default: true, available: false })])).toBe(true);
  });

  it("sends an explicit pick only while the control is offered", () => {
    expect(workspaceTypeToSend(undefined, true)).toBeUndefined();
    expect(workspaceTypeToSend("host", true)).toBe("host");
    expect(workspaceTypeToSend("box", true)).toBe("box");
    expect(workspaceTypeToSend("box", false)).toBeUndefined();
    // On a paired host the control is never offered.
    expect(workspaceTypeOffered(true, false)).toBe(false);
    // A dev build (the test run) is unlocked whoever the user is.
    expect(workspaceTypeOffered(false, true)).toBe(true);
  });

  it("names each type", () => {
    expect(workspaceTypeLabel("host")).toBe("This Mac");
    expect(workspaceTypeLabel("box")).toBe("Isolated VM");
  });
});
