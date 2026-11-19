// The words and glyphs for where a workspace runs: the composer's chip and the
// title bar's capsule both read these, so the two never name one place twice.
// Stored spellings do not change: `box` and `sandbox-exec` on a record, `box`,
// `host` and `cloud:<provider id>` in the composer's pick.

import type { IconName } from "@/components/Icon";

export interface SandboxName {
  label: string;
  /** The label where a control has room for a word or two: "Move to …". */
  short: string;
  icon: IconName;
}

/** A box: one micro-VM per workspace. */
export const LINUX_VM: SandboxName = { label: "Linux VM", short: "Linux VM", icon: "cube" };

/** Seatbelt, on this Mac. */
export const MACOS_SANDBOX: SandboxName = {
  label: "macOS sandbox",
  short: "macOS",
  icon: "folderLock",
};

/** A box on a machine at a cloud provider the user connected. A provider's
 *  own name is the backend's (its status' and its workspace-type row's
 *  `label`); this one is for where it is not known. */
export const CLOUD_MACHINE: SandboxName = {
  label: "Cloud machine",
  short: "Cloud",
  icon: "cloud",
};

/** A cloud machine named for its provider, `provider` being the provider's
 *  `label`: "Machine at …", or the neutral name without one. */
export function cloudMachineLabel(provider: string | null | undefined): string {
  return provider ? `Machine at ${provider}` : CLOUD_MACHINE.label;
}

/** The same within a sentence: "a machine at …", or "a cloud machine". */
export function cloudMachinePhrase(provider: string | null | undefined): string {
  return provider ? `a machine at ${provider}` : `a ${CLOUD_MACHINE.label.toLowerCase()}`;
}
