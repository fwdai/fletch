import { dbSelect, dbSelectOne, dbUpsert } from "./db";

export interface SettingRow {
  key: string;
  value: string;
}

export async function setSetting(key: string, value: unknown): Promise<void> {
  const encoded = typeof value === "string" ? value : JSON.stringify(value);
  await dbUpsert("settings", { key, value: encoded }, "key");
}

/** One setting, or null when it was never written. For the keys nothing
 *  hydrates at startup — `getAllSettings` reads the whole table, which is the
 *  wrong shape for a value only one pane and one deferred task want. */
export async function getSetting(key: string): Promise<string | null> {
  const row = await dbSelectOne<SettingRow>("settings", { where: { key } });
  return row?.value ?? null;
}

export async function getAllSettings(): Promise<Record<string, string>> {
  const rows = await dbSelect<SettingRow>("settings", {});
  const result: Record<string, string> = {};
  for (const row of rows) {
    result[row.key] = row.value;
  }
  return result;
}
