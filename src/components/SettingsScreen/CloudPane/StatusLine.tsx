import type { ReactNode } from "react";
import { Icon } from "@/components/Icon";

/** The line a provider leads with: where its connection stands, and the one
 *  thing to do about it. The chrome of the Account pane's GitHub and Linear
 *  rows (`set-gh-*`). */
export function StatusLine({ status, children }: { status: string; children?: ReactNode }) {
  return (
    <div className="set-gh-row flex-center">
      <Icon name="cloud" size={16} />
      <span className="set-gh-status text-base">{status}</span>
      {children}
    </div>
  );
}
