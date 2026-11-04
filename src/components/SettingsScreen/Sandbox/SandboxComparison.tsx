import { Fragment, useEffect, useState } from "react";
import { api, type GuaranteeStatus, type IsolationReport } from "@/api";
import { Icon, type IconName } from "@/components/Icon";
import { LINUX_VM, MACOS_SANDBOX } from "@/util/sandboxNames";
import { SetDisclosure } from "../primitives";

const MARK: Record<GuaranteeStatus["coverage"], { icon: IconName; label: string }> = {
  enforced: { icon: "check", label: "Yes" },
  partial: { icon: "minus", label: "Partly" },
  unenforced: { icon: "close", label: "No" },
};

function Mark({ status }: { status: GuaranteeStatus | undefined }) {
  if (!status) return <span />;
  const { icon, label } = MARK[status.coverage];
  return (
    <span
      className="set-compare-mark flex-center"
      data-coverage={status.coverage}
      aria-label={label}
      title={label}
    >
      <Icon name={icon} size={13} />
    </span>
  );
}

/** What each sandbox protects, side by side: the engine's own guarantee matrix
 *  (`sandbox::guarantees`), so this cannot drift from what is enforced. Claims
 *  only, as its notes are written for reviewers. Absent on a host that does not
 *  answer it. */
export function SandboxComparison() {
  const [reports, setReports] = useState<IsolationReport[] | null>(null);

  useEffect(() => {
    let live = true;
    api
      .describeSandboxIsolation()
      .then((r) => live && setReports(r))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, []);

  const vm = reports?.find((r) => r.engine === "box");
  const macos = reports?.find((r) => r.engine === "sandbox-exec");
  if (!vm || !macos) return null;

  return (
    <SetDisclosure label="How the two sandboxes compare">
      <div className="set-compare text-sm">
        <span />
        <span className="set-compare-h">{LINUX_VM.label}</span>
        <span className="set-compare-h">{MACOS_SANDBOX.label}</span>
        {vm.guarantees.map((g) => (
          <Fragment key={g.claim}>
            <span>{g.claim}</span>
            <Mark status={g} />
            <Mark status={macos.guarantees.find((m) => m.claim === g.claim)} />
          </Fragment>
        ))}
      </div>
    </SetDisclosure>
  );
}
