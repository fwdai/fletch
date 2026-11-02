import { describe, expect, it } from "vitest";
import type { WorkspaceTypeOption } from "@/api";
import {
  answeredOptions,
  declaredWorkspaceType,
  IMAGE_DOWNLOADING,
  shownWorkspaceType,
  WORKSPACE_TYPES_UNREADABLE,
  workspaceTypeChip,
  workspaceTypeLabel,
  workspaceTypeResponseLands,
  workspaceTypesKey,
  workspaceTypeTip,
  workspaceTypeToSend,
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
    // Not offered (a paired host).
    expect(workspaceTypeToSend("box", false, options)).toBeUndefined();
    // No answer for the current inputs: loading, failed, or other inputs'.
    expect(workspaceTypeToSend("box", true, null)).toBeUndefined();
    expect(workspaceTypeToSend(undefined, true, null)).toBeUndefined();
  });

  it("names each type", () => {
    expect(workspaceTypeLabel("host")).toBe("macOS sandbox");
    expect(workspaceTypeLabel("box")).toBe("Linux VM");
  });

  it("keys an answer by its repo and provider", () => {
    const key = workspaceTypesKey("/r", "claude");
    const answer = { key, options: [host({ default: true }), box()] };
    expect(answeredOptions(answer, key)).toBe(answer.options);
    expect(answeredOptions(null, key)).toBeNull();
    for (const other of [workspaceTypesKey("/other", "claude"), workspaceTypesKey("/r", "codex")]) {
      expect(other).not.toBe(key);
      expect(answeredOptions(answer, other)).toBeNull();
    }
  });

  it("lands only the latest read for the current inputs", () => {
    const key = workspaceTypesKey("/r", "claude");
    const request = { id: 3, key };
    expect(workspaceTypeResponseLands(request, 3, key)).toBe(true);
    // A newer read started (on open, or after the inputs changed).
    expect(workspaceTypeResponseLands(request, 4, key)).toBe(false);
    // The read asked for other inputs than the picker has now, or none is on screen.
    const other = workspaceTypesKey("/r", "codex");
    expect(workspaceTypeResponseLands(request, 3, other)).toBe(false);
    expect(workspaceTypeResponseLands(request, 3, null)).toBe(false);
  });

  it("says in one sentence where the shown option runs, and why", () => {
    expect(workspaceTypeTip(box({ default: true }), null)).toBe("Runs in a Linux VM");
    expect(workspaceTypeTip(host(), null)).toBe("Runs in the macOS sandbox");
    expect(
      workspaceTypeTip(host({ default: true, reason: "this project builds a macOS app" }), null),
    ).toBe("Runs in the macOS sandbox · this project builds a macOS app");
    // The download's live percent rides on the host's downloading reason only.
    const downloading = host({ default: true, reason: IMAGE_DOWNLOADING });
    expect(workspaceTypeTip(downloading, 43)).toBe(
      "Runs in the macOS sandbox · VM image downloading 43%",
    );
    expect(workspaceTypeTip(downloading, null)).toBe(
      "Runs in the macOS sandbox · VM image downloading",
    );
    expect(
      workspaceTypeTip(host({ default: true, reason: "this project builds a macOS app" }), 43),
    ).toBe("Runs in the macOS sandbox · this project builds a macOS app");
    // An unavailable option says why the launch will fail.
    expect(workspaceTypeTip(box({ available: false, reason: "no runtime" }), null)).toBe(
      "no runtime",
    );
  });

  it("draws the chip for each answer", () => {
    // No answer for these inputs yet: nothing.
    expect(workspaceTypeChip(null, false, undefined)).toBeNull();
    expect(workspaceTypeChip(null, false, "box")).toBeNull();
    // No answer and the read failed: a retry chip, showing the pick if any.
    expect(workspaceTypeChip(null, true, "box")).toEqual({
      kind: "retry",
      pick: "box",
      tip: WORKSPACE_TYPES_UNREADABLE,
    });
    expect(workspaceTypeChip(null, true, undefined)).toMatchObject({ kind: "retry", pick: null });
    // Answered: the menu chip, a later failure notwithstanding.
    const options = [host({ default: true }), box()];
    for (const failed of [false, true]) {
      const chip = workspaceTypeChip(options, failed, "box");
      expect(chip).toMatchObject({ kind: "menu", options, tip: "Runs in a Linux VM" });
      expect(chip?.kind === "menu" && chip.shown.kind).toBe("box");
    }
    // An unavailable shown option carries its reason as the tip.
    const refused = [host({ default: true }), box({ available: false, reason: "off" })];
    expect(workspaceTypeChip(refused, false, "box")).toMatchObject({ kind: "menu", tip: "off" });
  });
});
