import { Icon } from "@/components/Icon";

/** Rotating-arrow busy indicator, for a button mid-action or an inline "working
 *  on it" line. Sibling of `Loader` (the three-dot bounce): use `Spinner` when
 *  the work is a discrete operation the user kicked off, `Loader` when content
 *  is still arriving.
 *
 *  Inherits its color from the parent — the caller's text color is the point. */
export function Spinner({ size = 13, className }: { size?: number; className?: string }) {
  return (
    <Icon name="refresh" size={size} className={["ui-spin", className].filter(Boolean).join(" ")} />
  );
}
