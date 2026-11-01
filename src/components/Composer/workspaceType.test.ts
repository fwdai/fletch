import { describe, expect, it } from "vitest";
import type { WorkspaceTypeOption } from "@/api";
import {
  answerWith,
  defaultWorkspaceType,
  shownWorkspaceType,
  UNANSWERED,
  WORKSPACE_TYPES_UNREADABLE,
  workspaceTypeChip,
  workspaceTypeLabel,
  workspaceTypeOffered,
  workspaceTypePickDropped,
  workspaceTypeResponseLands,
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

  it("keeps the last options across a failed read", () => {
    const options = [host({ default: true }), box()];
    const ok = answerWith(UNANSWERED, options);
    expect(ok).toEqual({ options, failed: false });
    const failed = answerWith(ok, "failed");
    expect(failed).toEqual({ options, failed: true });
    // A later success clears the failure.
    expect(answerWith(failed, options)).toEqual({ options, failed: false });
    // With nothing read yet, a failure has no options to keep.
    expect(answerWith(UNANSWERED, "failed")).toEqual({ options: null, failed: true });
  });

  it("lands only the latest read for the current inputs", () => {
    const inputs = { repoPath: "/r", provider: "claude" };
    const request = { id: 3, ...inputs };
    expect(workspaceTypeResponseLands(request, 3, inputs)).toBe(true);
    // A newer read started (on open, or after the inputs changed).
    expect(workspaceTypeResponseLands(request, 4, inputs)).toBe(false);
    // The read asked for another repo or provider than the picker has now.
    expect(workspaceTypeResponseLands(request, 3, { ...inputs, repoPath: "/other" })).toBe(false);
    expect(workspaceTypeResponseLands(request, 3, { ...inputs, provider: "codex" })).toBe(false);
  });

  it("drops the pick only when the host answers the control is not offered", () => {
    expect(workspaceTypePickDropped(UNANSWERED)).toBe(false);
    expect(workspaceTypePickDropped({ options: null, failed: true })).toBe(false);
    const offered = [host({ default: true }), box()];
    expect(workspaceTypePickDropped({ options: offered, failed: false })).toBe(false);
    expect(workspaceTypePickDropped({ options: offered, failed: true })).toBe(false);
    // A container engine pinned in Settings: neither option is the default.
    expect(workspaceTypePickDropped({ options: [host(), box()], failed: false })).toBe(true);
  });

  it("draws the chip for each answer", () => {
    // First read pending: nothing.
    expect(workspaceTypeChip(UNANSWERED, undefined)).toBeNull();
    expect(workspaceTypeChip(UNANSWERED, "box")).toBeNull();
    // Never answered and the read failed: a retry chip, showing the pick if any.
    const never = { options: null, failed: true };
    expect(workspaceTypeChip(never, "box")).toEqual({
      kind: "retry",
      pick: "box",
      label: "Isolated VM",
      tip: WORKSPACE_TYPES_UNREADABLE,
    });
    expect(workspaceTypeChip(never, undefined)).toMatchObject({ kind: "retry", pick: null });
    // Answered: the menu chip, a later failure notwithstanding.
    const options = [host({ default: true }), box()];
    for (const failed of [false, true]) {
      const chip = workspaceTypeChip({ options, failed }, "box");
      expect(chip).toMatchObject({ kind: "menu", options, tip: "Where this workspace runs" });
      expect(chip?.kind === "menu" && chip.shown.kind).toBe("box");
    }
    // An unavailable shown option carries its reason as the tip.
    const refused = [host({ default: true }), box({ available: false, reason: "off" })];
    expect(workspaceTypeChip({ options: refused, failed: false }, "box")).toMatchObject({
      kind: "menu",
      tip: "off",
    });
    // No default: the control stays away.
    expect(workspaceTypeChip({ options: [host(), box()], failed: false }, "box")).toBeNull();
  });
});
