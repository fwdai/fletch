import type { ReactNode } from "react";

interface Props {
  label: string;
  description?: string;
  children: ReactNode;
}

export function SettingsRow({ label, description, children }: Props) {
  return (
    <div className="sp-row">
      <div className="sp-l">
        {label}
        {description && <small>{description}</small>}
      </div>
      {children}
    </div>
  );
}

export function SettingsSection({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <div className="sp-section">
      <div className="sp-title">{title}</div>
      {children}
    </div>
  );
}
