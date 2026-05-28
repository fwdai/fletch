import { dbSelect, dbDelete, dbUpsert } from "./db";

export interface SettingRow {
  key: string;
  value: string;
}

export async function getSetting(key: string): Promise<string | null> {
  const rows = await dbSelect<SettingRow>("settings", { where: { key } });
  return rows[0]?.value ?? null;
}

export async function getSettingParsed<T>(key: string, fallback: T): Promise<T> {
  const raw = await getSetting(key);
  if (raw == null) return fallback;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return fallback;
  }
}

export async function setSetting(key: string, value: unknown): Promise<void> {
  const encoded = typeof value === "string" ? value : JSON.stringify(value);
  await dbUpsert("settings", { key, value: encoded }, "key");
}

export async function deleteSetting(key: string): Promise<void> {
  await dbDelete("settings", { key });
}

export async function getAllSettings(): Promise<Record<string, string>> {
  const rows = await dbSelect<SettingRow>("settings", {});
  const result: Record<string, string> = {};
  for (const row of rows) {
    result[row.key] = row.value;
  }
  return result;
}
