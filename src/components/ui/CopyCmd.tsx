import { Icon } from "@/components/Icon";
import { useCopy } from "@/util/useCopy";

/** A shell command the user can copy in one click — the fallback beside an
 *  in-app install (`xcode-select --install`, a provider's npm one-liner). */
export function CopyCmd({ cmd }: { cmd: string }) {
  const { copied, copy } = useCopy(cmd);
  return (
    <button type="button" className="copy-cmd iflex-center" title="Copy command" onClick={copy}>
      <code>{cmd}</code>
      <Icon name={copied ? "check" : "copy"} size={10} />
    </button>
  );
}
