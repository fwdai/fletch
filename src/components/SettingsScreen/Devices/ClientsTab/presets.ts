import type { PairingPreset } from "@/api";

/** The pairing presets, in the order the chooser offers them. Mirrors
 *  `remote::PAIRING_PRESETS`; the host is what enforces a grant, so these are
 *  labels for the two sets it already knows. */
export const PAIRING_PRESETS: { value: PairingPreset; label: string }[] = [
  { value: "full", label: "Full" },
  { value: "control", label: "Control" },
];

/** What the narrower preset gives up, in one line. Shown under the chooser and
 *  kept here so the pairing card and the chooser cannot disagree. */
export const CONTROL_PRESET_HELP = "Control can't push, open or merge PRs, or approve publishes.";

/** Which preset a device's scopes amount to. The record carries scopes, never
 *  the preset name it was paired under, so `publish` is what tells the two
 *  apart — and a device paired before scopes existed has every scope, which
 *  reads as Full. */
export function presetOfScopes(scopes: string[]): PairingPreset {
  return scopes.includes("publish") ? "full" : "control";
}

/** A preset as a label. Falls back to the raw name so a host that grew a third
 *  preset still says something true. */
export function presetLabel(preset: string): string {
  return PAIRING_PRESETS.find((p) => p.value === preset)?.label ?? preset;
}
