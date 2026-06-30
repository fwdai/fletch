import type { ReactNode } from "react";

interface Props {
  label: string;
  description?: string;
  children: ReactNode;
}

export function SettingsRow({ label, description, children }: Props) {
  return (
    <div className="sp-row flex-center">
      <div className="sp-l text-base">
        {label}
        {description && <small>{description}</small>}
      </div>
      {children}
    </div>
  );
}

export function SettingsSection({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="sp-section">
      <div className="sp-title text-xs">{title}</div>
      {children}
    </div>
  );
}
