import { Icon } from "../Icon";

interface Props {
  value: "custom" | "native";
  onChange: (v: "custom" | "native") => void;
  disabled?: boolean;
  /** Disable only the Native option (e.g. a per-turn agent has no session
   *  id yet); `nativeReason` is shown as its tooltip. */
  nativeDisabled?: boolean;
  nativeReason?: string;
}

/** Segmented Custom / Native toggle. Click triggers a backend
 *  `switch_view` via the store action; the store updates `viewMode`
 *  on success. */
export function ViewToggle({ value, onChange, disabled, nativeDisabled, nativeReason }: Props) {
  return (
    <div
      className="view-toggle"
      role="tablist"
      aria-label="Agent view"
      data-disabled={disabled || undefined}
    >
      <Btn active={value === "custom"} disabled={disabled} onClick={() => onChange("custom")}>
        <Icon name="sparkle" /> Custom
      </Btn>
      <Btn
        active={value === "native"}
        disabled={disabled || nativeDisabled}
        title={nativeDisabled ? nativeReason : undefined}
        onClick={() => onChange("native")}
      >
        <Icon name="terminal" /> Native
      </Btn>
    </div>
  );
}

function Btn({
  active,
  disabled,
  onClick,
  children,
  title,
}: {
  active: boolean;
  disabled?: boolean;
  onClick: () => void;
  children: React.ReactNode;
  title?: string;
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      className={active ? "active" : ""}
      disabled={disabled}
      title={title}
      onClick={onClick}
    >
      {children}
    </button>
  );
}
