import { useState } from "react";
import { Icon } from "@/components/Icon";

/** A shell command the user can copy in one click — the fallback beside an
 *  in-app install (`xcode-select --install`, a provider's npm one-liner). */
export function CopyCmd({ cmd }: { cmd: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className="rdy-cmd iflex-center"
      title="Copy command"
      onClick={() => {
        void navigator.clipboard.writeText(cmd);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      }}
    >
      <code>{cmd}</code>
      <Icon name={copied ? "check" : "copy"} size={10} />
    </button>
  );
}
