// bits.tsx — small shared pieces of the inspector: labelled fields, the inline
// error strip, and the agent select button.

import { Icon } from "../../../components/Icon";
import type { ResolvedAgent } from "../../shared";
import { AgentAvatar } from "../AgentAvatar";

export function Field({
  label,
  hint,
  required,
  children,
}: {
  label: string;
  hint?: string;
  required?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div className="wb-field">
      <div className="wb-label">
        {label} {required && <span className="req">*</span>}
        {hint && <span className="hint">{hint}</span>}
      </div>
      {children}
    </div>
  );
}

/** Validation messages at the top of an inspector pane; nothing when clean. */
export function ErrorStrip({ errors }: { errors: string[] | undefined }) {
  if (!errors?.length) return null;
  return (
    <div className="wb-errs">
      {errors.map((msg) => (
        <span className="wb-err" key={msg}>
          <Icon name="close" size={9} /> {msg}
        </span>
      ))}
    </div>
  );
}

/** The agent select button: identity when assigned, a dashed placeholder when
 *  not. Opens the agent picker popover via `onClick`. */
export function AgentButton({
  agent,
  placeholder,
  onClick,
}: {
  agent: ResolvedAgent | null;
  placeholder: string;
  onClick: (e: React.MouseEvent) => void;
}) {
  return (
    <button className="wb-agent-btn" onClick={onClick}>
      {agent ? (
        <AgentAvatar
          custom={agent.custom}
          slug={agent.providerId}
          short={agent.short}
          hue={agent.hue}
          size={28}
        />
      ) : (
        <span className="wb-step-mono empty">
          <Icon name="plus" size={12} />
        </span>
      )}
      <span className="wb-agent-btn-l">
        <div className={`wb-an ${agent ? "" : "empty"}`}>{agent ? agent.name : placeholder}</div>
        {agent && (
          <div className="wb-am">
            {agent.custom ? `${agent.baseLabel} · ${agent.model}` : agent.model}
          </div>
        )}
      </span>
      <Icon name="chevD" size={13} />
    </button>
  );
}
