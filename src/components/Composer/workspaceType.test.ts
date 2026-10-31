import { describe, expect, it } from "vitest";
import type { WorkspaceTypeOption } from "@/api";
import {
  defaultWorkspaceType,
  shownWorkspaceType,
  workspaceTypeLabel,
  workspaceTypeOverride,
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

  it("shows the choice while it is offered, else the default", () => {
    const options = [host({ default: true }), box()];
    expect(shownWorkspaceType(options, undefined)).toBe("host");
    expect(shownWorkspaceType(options, "box")).toBe("box");
    const refused = [host({ default: true }), box({ available: false, reason: "off" })];
    expect(shownWorkspaceType(refused, "box")).toBe("host");
  });

  it("sends only an available choice that differs from the default", () => {
    const hostDefault = [host({ default: true }), box()];
    expect(workspaceTypeOverride(hostDefault, undefined)).toBeUndefined();
    expect(workspaceTypeOverride(hostDefault, "host")).toBeUndefined();
    expect(workspaceTypeOverride(hostDefault, "box")).toBe("box");

    const boxDefault = [host(), box({ default: true })];
    expect(workspaceTypeOverride(boxDefault, "host")).toBe("host");
    expect(workspaceTypeOverride(boxDefault, "box")).toBeUndefined();

    const refused = [host({ default: true }), box({ available: false, reason: "off" })];
    expect(workspaceTypeOverride(refused, "box")).toBeUndefined();

    // With no default (a container engine pinned) either pick is a real choice.
    expect(workspaceTypeOverride([host(), box()], "host")).toBe("host");
    // A kind the host did not list is never sent.
    expect(workspaceTypeOverride([host({ default: true })], "box")).toBeUndefined();
  });

  it("names each type", () => {
    expect(workspaceTypeLabel("host")).toBe("This Mac");
    expect(workspaceTypeLabel("box")).toBe("Isolated VM");
  });
});
