import type { AriaAttributes } from "react";

export type LoaderVariant = "accent" | "muted" | "inherit";
export type LoaderSize = "sm" | "md";

type Props = {
  /** Dot color. Default `accent`. */
  variant?: LoaderVariant;
  /** `sm` = 4px (default), `md` = 5px for inline anchors. */
  size?: LoaderSize;
  className?: string;
} & Pick<AriaAttributes, "aria-label" | "aria-hidden">;

/** Three-dot bounce loader — working / pending / restoring states. */
export function Loader({
  variant = "accent",
  size = "sm",
  className,
  "aria-label": ariaLabel,
  "aria-hidden": ariaHidden,
}: Props) {
  const cls = ["ui-loader", variant, size, className].filter(Boolean).join(" ");
  return (
    <span
      className={cls}
      role={ariaLabel ? "status" : undefined}
      aria-label={ariaLabel}
      aria-hidden={ariaHidden}
    >
      <i />
      <i />
      <i />
    </span>
  );
}
