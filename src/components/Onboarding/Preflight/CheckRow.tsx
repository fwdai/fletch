import type { CSSProperties, ReactNode } from "react";
import { Icon } from "@/components/Icon";
import type { CheckState } from "./usePreflight";

function StateGlyph({ state }: { state: CheckState }) {
  switch (state) {
    case "checking":
      return <span className="pf-spin" />;
    case "ok":
      return <Icon name="check" size={11} strokeWidth={2.4} />;
    case "attention":
      return <Icon name="alert" size={11} strokeWidth={2} />;
    case "skipped":
      return <Icon name="minus" size={11} strokeWidth={2.2} />;
    default:
      return <span className="pf-dot-idle" />;
  }
}

/** One row of the pre-flight timeline: the status glyph on the rail, a title,
 *  a one-line detail that changes with the state, and — only when the row
 *  needs the user — a body with its actions. The head is a button so a row
 *  already reached can be clicked to bring its panel back on the right. */
export function CheckRow({
  index,
  title,
  detail,
  state,
  active,
  onInspect,
  children,
}: {
  index: number;
  title: string;
  detail: ReactNode;
  state: CheckState;
  active: boolean;
  onInspect: () => void;
  children?: ReactNode;
}) {
  return (
    <li
      className={`pf-row ${state} ${active ? "active" : ""} ob-reveal`}
      style={{ "--d": `${0.18 + index * 0.08}s` } as CSSProperties}
    >
      <button
        type="button"
        className="pf-row-head"
        onClick={onInspect}
        disabled={state === "pending"}
        aria-current={active ? "step" : undefined}
      >
        <span className="pf-dot">
          <StateGlyph state={state} />
        </span>
        <span className="pf-row-text">
          <span className="pf-row-title">{title}</span>
          <span className="pf-row-detail">{detail}</span>
        </span>
      </button>
      {children && <div className="pf-row-body">{children}</div>}
    </li>
  );
}
