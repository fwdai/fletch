import { dbSelect, dbUpsert } from "./db";

export interface SettingRow {
  key: string;
  value: string;
}

export async function setSetting(key: string, value: unknown): Promise<void> {
  const encoded = typeof value === "string" ? value : JSON.stringify(value);
  await dbUpsert("settings", { key, value: encoded }, "key");
}

export async function getAllSettings(): Promise<Record<string, string>> {
  const rows = await dbSelect<SettingRow>("settings", {});
  const result: Record<string, string> = {};
  for (const row of rows) {
    result[row.key] = row.value;
  }
  return result;
}
