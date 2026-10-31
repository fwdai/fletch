// Where a project's agents run under the automatic engine: the mirror of
// Rust `sandbox::placement`, so every surface that explains a placement — the
// project page, the title bar — reads the same answer the spawn path acts on.

import type { ProjectRef } from "@/api";

/** Project-settings key the host's spawn path reads (`sandbox::placement::KEY`).
 *  Absent is automatic. */
export const PLACEMENT_KEY = "sandbox.placement";

export type Placement = "auto" | "host" | "box";

/** The stored choice, as `Placement::parse` reads it. */
export function parsePlacement(raw: string | undefined): Placement {
  return raw === "host" || raw === "box" ? raw : "auto";
}

/** What automatic resolves to for a project (`Placement::resolve`): the stored
 *  choice, or `host` for a repository the host found to build a macOS or iOS
 *  app (`ProjectRef.native_app`). `detected` says the host, not the user,
 *  put it on this Mac. */
export function effectivePlacement(
  raw: string | undefined,
  project: Pick<ProjectRef, "native_app"> | undefined,
): { placement: Placement; detected: boolean } {
  const stored = parsePlacement(raw);
  if (stored === "auto" && project?.native_app) return { placement: "host", detected: true };
  return { placement: stored, detected: false };
}
