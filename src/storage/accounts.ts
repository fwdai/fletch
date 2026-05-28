import { dbSelectOne, dbInsert, dbUpdate } from "./db";

export interface AccountRow {
  id: string;
  name: string;
  email: string | null;
  avatar_url: string | null;
  created_at: number;
}

export async function getAccount(): Promise<AccountRow | null> {
  return dbSelectOne<AccountRow>("accounts", {});
}

export async function getOrCreateAccount(): Promise<AccountRow> {
  const existing = await getAccount();
  if (existing) return existing;
  const id = await dbInsert("accounts", { name: "" });
  return (await dbSelectOne<AccountRow>("accounts", { where: { id } }))!;
}

export async function updateAccount(
  id: string,
  data: Partial<Omit<AccountRow, "id" | "created_at">>,
): Promise<void> {
  await dbUpdate("accounts", { id }, data as Record<string, unknown>);
}
