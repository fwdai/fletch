import { invoke } from "@tauri-apps/api/core";

export async function dbInsert(
  table: string,
  data: Record<string, unknown>,
): Promise<string> {
  return invoke<string>("db_insert", { table, data });
}

export async function dbSelect<T>(
  table: string,
  query: Record<string, unknown> = {},
): Promise<T[]> {
  const rows = await invoke<T[]>("db_select", { table, query });
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
  return invoke<number>("db_update", { table, query: { where }, data });
}

export async function dbDelete(
  table: string,
  where: Record<string, unknown>,
): Promise<number> {
  return invoke<number>("db_delete", { table, query: { where } });
}

export async function dbCount(
  table: string,
  where?: Record<string, unknown>,
): Promise<number> {
  const query = where ? { where } : {};
  return invoke<number>("db_count", { table, query });
}

export async function dbQuery<T>(
  sql: string,
  params: unknown[] = [],
): Promise<T[]> {
  const rows = await invoke<T[]>("db_query", { sql, params });
  return Array.isArray(rows) ? rows : [];
}
