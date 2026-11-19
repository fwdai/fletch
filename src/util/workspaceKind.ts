// The spelling of a cloud workspace type, `cloud:` and the provider's id, as
// `available_workspace_types` lists it and a spawn sends it: read and written
// here only, so no surface parses the string.

import type { CloudWorkspaceKind, WorkspaceKind } from "@/api";

const CLOUD_PREFIX = "cloud:";

/** Whether `kind` puts a workspace on a machine at a cloud provider. */
export function isCloudKind(kind: WorkspaceKind): kind is CloudWorkspaceKind {
  return kind.startsWith(CLOUD_PREFIX);
}

/** The provider id a cloud kind names; null for a kind on this Mac. */
export function cloudProviderOf(kind: WorkspaceKind): string | null {
  return isCloudKind(kind) ? kind.slice(CLOUD_PREFIX.length) : null;
}

/** The workspace type for a machine at `provider`. */
export function cloudKind(provider: string): CloudWorkspaceKind {
  return `${CLOUD_PREFIX}${provider}`;
}

/** Whether a workspace's record puts it on a machine at a cloud provider: a
 *  box like any other, reached over ssh, but not on this Mac. */
export function inCloud(agent: { cloud_provider?: string | null }): boolean {
  return !!agent.cloud_provider;
}
