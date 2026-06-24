import { invoke } from "@tauri-apps/api/core";

/**
 * Call a backend command registered by an extension's `backend/mod.rs`.
 *
 * Routes through the core's single `ext_invoke` command, so extensions add
 * native functionality without touching the core's command list or Tauri's
 * capability/ACL config. Rejects with the handler's error string on failure.
 *
 * For plain table CRUD an extension can skip the backend entirely and use the
 * core's generic `db_*` commands (see src/api.ts) against its own tables.
 *
 *   const { count } = await callExtension<{ count: number }>("demo.local", "count_notes");
 */
export function callExtension<T = unknown>(
  extension: string,
  command: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  return invoke<T>("ext_invoke", { extension, command, args });
}
