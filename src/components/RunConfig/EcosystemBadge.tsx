import { ECOSYSTEM_LABEL } from "./types";

/** The stack line shown atop every run-config surface. When detection
 *  couldn't identify the ecosystem it reads "Unknown" — the fields below
 *  stay editable either way, so the user can still configure the project. */
export function EcosystemBadge({ ecosystem }: { ecosystem: string | null }) {
  return ecosystem ? (
    <>
      Detected · <code>{ECOSYSTEM_LABEL[ecosystem] ?? ecosystem}</code>
    </>
  ) : (
    <>
      Stack · <code>Unknown</code>
    </>
  );
}
