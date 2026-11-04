import { Icon } from "@/components/Icon";
import { useCopy } from "@/util/useCopy";
import { IconButton } from "./IconButton";

/** Copies `text` to the clipboard and briefly swaps the copy icon for a check
 *  as confirmation. Used under chat messages; generic enough for any "copy
 *  this string" affordance. */
export function CopyButton({
  text,
  tip = "Copy",
  className,
}: {
  text: string;
  tip?: string;
  className?: string;
}) {
  const { copied, copy } = useCopy(text);

  // No explicit aria-label: IconButton falls back to `tip`, so each call site's
  // own wording ("Copy command") becomes the accessible name.
  return (
    <IconButton size="xs" tip={copied ? "Copied" : tip} className={className} onClick={copy}>
      <Icon name={copied ? "check" : "copy"} size={12} />
    </IconButton>
  );
}
