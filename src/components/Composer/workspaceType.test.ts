import { describe, expect, it } from "vitest";
import type { WorkspaceTypeOption } from "@/api";
import {
  answeredOptions,
  declaredWorkspaceType,
  defaultWorkspaceType,
  shownWorkspaceType,
  WORKSPACE_TYPES_UNREADABLE,
  workspaceTypeChip,
  workspaceTypeLabel,
  workspaceTypeOffered,
  workspaceTypePickDropped,
  workspaceTypeResponseLands,
  workspaceTypesKey,
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

  // Changed with the declaration model: the spawn used to carry only an
  // explicit pick, leaving an untouched draft to a default recomputed at spawn
  // time; it now carries whatever the chip declares.
  it("sends what the chip declares, the default included", () => {
    const options = [host({ default: true }), box()];
    expect(workspaceTypeToSend(undefined, true, options)).toBe("host");
    expect(workspaceTypeToSend("box", true, options)).toBe("box");
    // The chip and the spawn are one derivation.
    expect(declaredWorkspaceType(options, "box")).toBe(options[1]);
    // A pick the host no longer lists falls back to the default it shows.
    expect(workspaceTypeToSend("box", true, [host({ default: true })])).toBe("host");
    // An unavailable pick is still sent: the spawn then fails with its reason.
    const refused = [host({ default: true }), box({ available: false, reason: "off" })];
    expect(workspaceTypeToSend("box", true, refused)).toBe("box");
  });

  it("sends nothing when nothing is declared", () => {
    const options = [host({ default: true }), box()];
    // Not offered (a paired host, boxes not rolled out).
    expect(workspaceTypeToSend("box", false, options)).toBeUndefined();
    // No answer for the current inputs: loading, failed, or other inputs'.
    expect(workspaceTypeToSend("box", true, null)).toBeUndefined();
    expect(workspaceTypeToSend(undefined, true, null)).toBeUndefined();
    // A container engine pinned: no option is the default, even with a pick.
    expect(workspaceTypeToSend("box", true, [host(), box()])).toBeUndefined();
    expect(declaredWorkspaceType([host(), box()], "box")).toBeNull();
  });

  it("offers the control on this Mac only", () => {
    // On a paired host the control is never offered.
    expect(workspaceTypeOffered(true, false)).toBe(false);
    // A dev build (the test run) is unlocked whoever the user is.
    expect(workspaceTypeOffered(false, true)).toBe(true);
  });

  it("names each type", () => {
    expect(workspaceTypeLabel("host")).toBe("This Mac");
    expect(workspaceTypeLabel("box")).toBe("Isolated VM");
  });

  it("keys an answer by every input that moves the default", () => {
    const key = workspaceTypesKey("/r", "claude", "auto", true);
    const answer = { key, options: [host({ default: true }), box()] };
    expect(answeredOptions(answer, key)).toBe(answer.options);
    expect(answeredOptions(null, key)).toBeNull();
    for (const other of [
      workspaceTypesKey("/other", "claude", "auto", true),
      workspaceTypesKey("/r", "codex", "auto", true),
      workspaceTypesKey("/r", "claude", "docker", true),
      workspaceTypesKey("/r", "claude", "auto", false),
    ]) {
      expect(other).not.toBe(key);
      expect(answeredOptions(answer, other)).toBeNull();
    }
  });

  it("lands only the latest read for the current inputs", () => {
    const key = workspaceTypesKey("/r", "claude", "auto", true);
    const request = { id: 3, key };
    expect(workspaceTypeResponseLands(request, 3, key)).toBe(true);
    // A newer read started (on open, or after the inputs changed).
    expect(workspaceTypeResponseLands(request, 4, key)).toBe(false);
    // The read asked for other inputs than the picker has now, or none is on screen.
    const other = workspaceTypesKey("/r", "codex", "auto", true);
    expect(workspaceTypeResponseLands(request, 3, other)).toBe(false);
    expect(workspaceTypeResponseLands(request, 3, null)).toBe(false);
  });

  it("drops the pick only when the host answers the control is not offered", () => {
    expect(workspaceTypePickDropped(null)).toBe(false);
    expect(workspaceTypePickDropped([host({ default: true }), box()])).toBe(false);
    // A container engine pinned in Settings: neither option is the default.
    expect(workspaceTypePickDropped([host(), box()])).toBe(true);
  });

  it("draws the chip for each answer", () => {
    // No answer for these inputs yet: nothing.
    expect(workspaceTypeChip(null, false, undefined)).toBeNull();
    expect(workspaceTypeChip(null, false, "box")).toBeNull();
    // No answer and the read failed: a retry chip, showing the pick if any.
    expect(workspaceTypeChip(null, true, "box")).toEqual({
      kind: "retry",
      pick: "box",
      label: "Isolated VM",
      tip: WORKSPACE_TYPES_UNREADABLE,
    });
    expect(workspaceTypeChip(null, true, undefined)).toMatchObject({ kind: "retry", pick: null });
    // Answered: the menu chip, a later failure notwithstanding.
    const options = [host({ default: true }), box()];
    for (const failed of [false, true]) {
      const chip = workspaceTypeChip(options, failed, "box");
      expect(chip).toMatchObject({ kind: "menu", options, tip: "Where this workspace runs" });
      expect(chip?.kind === "menu" && chip.shown.kind).toBe("box");
    }
    // An unavailable shown option carries its reason as the tip.
    const refused = [host({ default: true }), box({ available: false, reason: "off" })];
    expect(workspaceTypeChip(refused, false, "box")).toMatchObject({ kind: "menu", tip: "off" });
    // No default: the control stays away.
    expect(workspaceTypeChip([host(), box()], false, "box")).toBeNull();
  });
});
