import type { CSSProperties } from "react";
import { ICON_PATHS, type IconName } from "./paths";

export type { IconName } from "./paths";

export function Icon({
  name,
  size = 16,
  sw = 1.5,
  className,
  style,
}: {
  name: IconName;
  size?: number;
  /** Stroke width; the design set is drawn for 1.5. */
  sw?: number;
  className?: string;
  style?: CSSProperties;
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={sw}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      style={style}
      aria-hidden="true"
    >
      {ICON_PATHS[name] ?? ICON_PATHS.dot}
    </svg>
  );
}
