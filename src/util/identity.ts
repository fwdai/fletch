import type { GitIdentity } from "@/api";
import type { AccountProfile } from "@/storage/accounts";
import { accountInitials } from "./format";

/** Who the account surfaces (sidebar footer, Settings › Account) show, and
 *  where it came from. The profile the user saved or signed in with wins;
 *  otherwise the global git identity, which nearly every developer has set and
 *  is what their commits already carry; otherwise nothing, and the surfaces
 *  invite the user to fill the profile in rather than calling them "you". */
export interface DisplayIdentity {
  /** Full display name, or "" when nothing is known. */
  name: string;
  email: string;
  /** "" when there is nothing to abbreviate — the avatar shows a glyph then. */
  initials: string;
  source: "profile" | "git" | "none";
}

export function displayIdentity(
  account: AccountProfile | null,
  git: GitIdentity | null,
): DisplayIdentity {
  const first = account?.firstName.trim() ?? "";
  const last = account?.lastName.trim() ?? "";
  const email = account?.email.trim() ?? "";
  if (first || last || email) {
    return {
      name: `${first} ${last}`.trim(),
      email,
      initials: accountInitials(first, last, email),
      source: "profile",
    };
  }
  const gitName = git?.name?.trim() ?? "";
  const gitEmail = git?.email?.trim() ?? "";
  if (gitName || gitEmail) {
    const [gitFirst = "", ...rest] = gitName.split(/\s+/);
    return {
      name: gitName,
      email: gitEmail,
      initials: accountInitials(gitFirst, rest.join(" "), gitEmail),
      source: "git",
    };
  }
  return { name: "", email: "", initials: "", source: "none" };
}
