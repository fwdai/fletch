import { dbInsert, dbSelectOne, dbUpdate } from "./db";

export interface AccountRow {
  id: string;
  /** Derived "First Last" string kept in sync on save for any consumer
   *  that wants a single display name. Prefer first_name / last_name. */
  name: string;
  first_name: string | null;
  last_name: string | null;
  email: string | null;
  avatar_url: string | null;
  oauth_provider: string | null;
  oauth_id: string | null;
  created_at: number;
}

/** Editable profile fields surfaced in the Account settings screen. */
export interface AccountProfile {
  id: string;
  firstName: string;
  lastName: string;
  email: string;
  avatarUrl: string | null;
}

export function toProfile(row: AccountRow): AccountProfile {
  return {
    id: row.id,
    firstName: row.first_name ?? "",
    lastName: row.last_name ?? "",
    email: row.email ?? "",
    avatarUrl: row.avatar_url,
  };
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

/** Persist edited profile fields. Also writes a derived `name` ("First Last")
 *  so existing single-name consumers stay valid. */
export async function saveAccountProfile(
  id: string,
  patch: Pick<AccountProfile, "firstName" | "lastName" | "email">,
): Promise<void> {
  const name = `${patch.firstName} ${patch.lastName}`.trim();
  await dbUpdate(
    "accounts",
    { id },
    {
      first_name: patch.firstName,
      last_name: patch.lastName,
      email: patch.email,
      name,
    },
  );
}

/** Identity returned by the `oauth_device_login` backend command. */
export interface OAuthProfile {
  provider: string;
  provider_user_id: string;
  name: string | null;
  email: string | null;
  avatar_url: string | null;
}

/** Persist identity fetched from an OAuth provider onto the single account.
 *  Splits the provider display name into first/last for the profile fields. */
export async function linkOAuthAccount(id: string, profile: OAuthProfile): Promise<void> {
  const full = (profile.name ?? "").trim();
  const space = full.indexOf(" ");
  const firstName = space === -1 ? full : full.slice(0, space);
  const lastName = space === -1 ? "" : full.slice(space + 1);
  await dbUpdate(
    "accounts",
    { id },
    {
      oauth_provider: profile.provider,
      oauth_id: profile.provider_user_id,
      name: full,
      first_name: firstName,
      last_name: lastName,
      email: profile.email,
      avatar_url: profile.avatar_url,
    },
  );
}
