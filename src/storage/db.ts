// The desktop's own SQLite bridge: this app's local rows (accounts, drafts,
// autopilot log), not an engine's workspace. Always the local transport.
import { invokeLocal } from "@/api/invoke";

export async function dbInsert(table: string, data: Record<string, unknown>): Promise<string> {
  return invokeLocal<string>("db_insert", { table, data });
}

export async function dbSelect<T>(
  table: string,
  query: Record<string, unknown> = {},
): Promise<T[]> {
  const rows = await invokeLocal<T[]>("db_select", { table, query });
  return Array.isArray(rows) ? rows : [];
}

export async function dbSelectOne<T>(
  table: string,
  query: Record<string, unknown> = {},
): Promise<T | null> {
  const rows = await dbSelect<T>(table, { ...query, limit: 1 });
  return rows[0] ?? null;
}

export async function dbUpdate(
  table: string,
  where: Record<string, unknown>,
  data: Record<string, unknown>,
): Promise<number> {
  return invokeLocal<number>("db_update", { table, query: { where }, data });
}

export async function dbDelete(table: string, where: Record<string, unknown>): Promise<number> {
  return invokeLocal<number>("db_delete", { table, query: { where } });
}

export async function dbCount(table: string, where?: Record<string, unknown>): Promise<number> {
  const query = where ? { where } : {};
  return invokeLocal<number>("db_count", { table, query });
}

export async function dbUpsert(
  table: string,
  data: Record<string, unknown>,
  conflictColumn: string,
): Promise<string> {
  return invokeLocal<string>("db_upsert", {
    table,
    data,
    conflictColumn,
  });
}

export async function dbQuery<T>(sql: string, params: unknown[] = []): Promise<T[]> {
  const rows = await invokeLocal<T[]>("db_query", { sql, params });
  return Array.isArray(rows) ? rows : [];
}
