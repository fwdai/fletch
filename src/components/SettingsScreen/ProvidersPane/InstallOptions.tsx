// The missing-state detail: what Fletch will run, the exact command, and the
// two ways out of "not installed" — let us install it, or point us at a copy
// you already have. Deliberately not a menu of package managers: there is
// exactly one one-click method per agent, its vendor's own installer.

import type { ReactNode } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { CopyButton } from "@/components/ui/CopyButton";
import { DocsLink } from "@/components/ui/DocsLink";

export function InstallOptions({
  providerLabel,
  /** The pinned installer for this platform, or undefined when the agent has
   *  no scripted installer (antigravity, pi) — then there is no Install. */
  command,
  docs,
  onInstall,
  /** The shared binary-path editor — "already installed somewhere else?". */
  locate,
}: {
  providerLabel: string;
  command?: string;
  docs: string;
  onInstall: () => void;
  locate: ReactNode;
}) {
  return (
    <>
      <p className="set-prov-hint text-sm">
        {command
          ? `Fletch runs ${providerLabel}'s official installer and re-scans your PATH when it finishes. Nothing else on your system is touched.`
          : `${providerLabel} has no one-click installer. Follow the vendor's guide, then re-scan — or point Fletch at the binary below.`}
      </p>

      {command && (
        <div className="set-prov-opt flex-center">
          <span className="set-prov-opt-ic iflex-center">
            <Icon name="terminal" size={14} />
          </span>
          <span className="set-prov-opt-t">
            <span className="set-prov-opt-n text-sm">Official installer</span>
            <span className="set-prov-opt-s truncate mono text-xs">{command}</span>
          </span>
          <span className="set-prov-opt-a flex-center">
            <CopyButton text={command} tip="Copy command" />
            <Button variant="primary" size="sm" onClick={onInstall}>
              <Icon name="arrowDown" size={12} />
              Install
            </Button>
          </span>
        </div>
      )}

      <p className="set-prov-hint text-sm">
        Already installed somewhere else? Locate the binary and Fletch will use it.
      </p>
      {locate}

      <div className="set-prov-detail-actions flex-center">
        <span className="grow" />
        <DocsLink url={docs} label="Install guide" />
      </div>
    </>
  );
}
