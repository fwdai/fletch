// The words and glyphs for where a workspace runs: the composer's chip and the
// title bar's capsule both read these, so the two never name one place twice.
// Stored spellings do not change: `box` and `sandbox-exec` on a record, `box`
// and `host` in the composer's pick.

import type { IconName } from "@/components/Icon";

export interface SandboxName {
  label: string;
  icon: IconName;
}

/** A box: one micro-VM per workspace. */
export const LINUX_VM: SandboxName = { label: "Linux VM", icon: "cube" };

/** Seatbelt, on this Mac. */
export const MACOS_SANDBOX: SandboxName = { label: "macOS sandbox", icon: "folderLock" };
