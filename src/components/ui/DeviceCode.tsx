import { CopyButton } from "./CopyButton";

/** The user code + verification URL of an OAuth device flow, as the user has to
 *  read them: oversized tracked digits with a copy affordance, and the URL to
 *  visit underneath. Shared by every device-flow surface (the GitHub connect
 *  modal, the New Project connect gate) so the code always looks the same. */
export function DeviceCode({
  code,
  verificationUri,
}: {
  code: string;
  /** Where to enter the code. Omit when the surface states it in its own copy. */
  verificationUri?: string;
}) {
  return (
    <>
      <div className="device-code-row">
        <span className="device-code text-2xl">{code}</span>
        <CopyButton text={code} />
      </div>
      {verificationUri && <div className="device-code-uri mono text-sm">{verificationUri}</div>}
    </>
  );
}
