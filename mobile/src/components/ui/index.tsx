// Shared primitives ported from the prototype's ui.jsx: nav bar, bottom sheet,
// list picker, segmented control, toggle, chips, tiny markdown and the
// syntax-tinted code line.

import type { AgentStatus, ProjectRef } from "@desktop/api/types/agent";
import type { PrState } from "@desktop/api/types/pr";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { hueColor, projectHue, providerHue, providerShort } from "../../lib/agents";
import { useSwipe } from "../../lib/swipe";
import { useProviderIcon } from "../../lib/useProviderIcon";
import { Icon, type IconName } from "../Icon";

export function Nav({
  onBack,
  backLabel = "Back",
  title,
  sub,
  right,
  hair,
}: {
  onBack?: () => void;
  backLabel?: string;
  title: ReactNode;
  sub?: ReactNode;
  right?: ReactNode;
  hair?: boolean;
}) {
  return (
    <div className={`nav${hair ? " hair" : ""}`}>
      <div>
        {onBack && (
          <button type="button" className="back" onClick={onBack}>
            <Icon name="chevL" size={22} sw={1.8} />
            <span>{backLabel}</span>
          </button>
        )}
      </div>
      <div className="ttl">
        <div className="t1">{title}</div>
        {sub && <div className="t2">{sub}</div>}
      </div>
      <div className="r">{right}</div>
    </div>
  );
}

export function Sheet({
  open,
  onClose,
  full,
  stacked,
  title,
  left,
  right,
  children,
  foot,
}: {
  open: boolean;
  onClose?: () => void;
  full?: boolean;
  stacked?: boolean;
  title?: ReactNode;
  left?: ReactNode;
  right?: ReactNode;
  children?: ReactNode;
  foot?: ReactNode;
}) {
  const [mounted, setMounted] = useState(open);
  const [shown, setShown] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  useSwipe(
    panel,
    {
      axis: "y",
      enabled: shown && !!onClose,
      // `shown` drops here, not in the `open` effect: same frame as the drag
      // lets go, so the close transition continues from under the finger.
      onCommit: () => {
        setShown(false);
        onClose?.();
      },
    },
    root,
  );
  useEffect(() => {
    if (open) {
      setMounted(true);
      // Two frames: mount off-screen, then release the transform.
      let inner = 0;
      const outer = requestAnimationFrame(() => {
        inner = requestAnimationFrame(() => setShown(true));
      });
      return () => {
        cancelAnimationFrame(outer);
        cancelAnimationFrame(inner);
      };
    }
    setShown(false);
    const t = setTimeout(() => setMounted(false), 500);
    return () => clearTimeout(t);
  }, [open]);
  if (!mounted) return null;
  return (
    <div ref={root} className={`sheet-root${shown ? " open" : ""}`}>
      <button type="button" className="sheet-bg" onClick={onClose} aria-label="Close" />
      <div ref={panel} className={`sheet${full ? " full" : ""}${stacked ? " stacked" : ""}`}>
        <div className="grab" />
        {(title || left || right) && (
          <div className="sheet-head">
            <div className="l">{left}</div>
            <div className="t">{title}</div>
            <div className="rr">{right}</div>
          </div>
        )}
        <div className="sheet-body">{children}</div>
        {foot && <div className="sheet-foot">{foot}</div>}
      </div>
    </div>
  );
}

export interface PickerItem {
  id: string;
  label: string;
  sub?: string | null;
  right?: string | null;
  icon?: ReactNode;
}

export function PickerSheet({
  open,
  onClose,
  title,
  items,
  value,
  onChange,
  searchable,
  sectionLabel,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  items: PickerItem[];
  value: string | null;
  onChange: (id: string) => void;
  searchable?: boolean;
  sectionLabel?: string;
}) {
  const [query, setQuery] = useState("");
  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);
  const list = query
    ? items.filter((it) => it.label.toLowerCase().includes(query.toLowerCase()))
    : items;
  return (
    <Sheet
      open={open}
      onClose={onClose}
      stacked
      title={title}
      right={
        <button type="button" className="tbtn" onClick={onClose}>
          Done
        </button>
      }
    >
      {searchable && (
        <div className="search">
          <Icon name="search" size={15} />
          <input
            placeholder={`Search ${title.toLowerCase()}`}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
      )}
      {sectionLabel && (
        <div className="sect" style={{ marginTop: 6 }}>
          {sectionLabel}
        </div>
      )}
      <div className="card">
        {list.map((it) => (
          <button
            type="button"
            key={it.id}
            className="row"
            onClick={() => {
              onChange(it.id);
              onClose();
            }}
          >
            {it.icon}
            <div className="main">
              <div className="lbl">{it.label}</div>
              {it.sub && <div className="sub">{it.sub}</div>}
            </div>
            {it.right && <span className="val mono">{it.right}</span>}
            {it.id === value ? (
              <Icon name="check" size={18} sw={2} className="check-ic" />
            ) : (
              <span style={{ width: 18 }} />
            )}
          </button>
        ))}
        {list.length === 0 && <div className="empty">No matches</div>}
      </div>
    </Sheet>
  );
}

export interface SegItem {
  id: string;
  label: string;
  count?: number;
}

export function Segmented({
  items,
  value,
  onChange,
}: {
  items: SegItem[];
  value: string;
  onChange: (id: string) => void;
}) {
  const index = Math.max(
    0,
    items.findIndex((it) => it.id === value),
  );
  return (
    <div className="seg" style={{ "--n": items.length, "--i": index } as React.CSSProperties}>
      <span className="ind" />
      {items.map((it) => (
        <button
          type="button"
          key={it.id}
          className={it.id === value ? "on" : ""}
          onClick={() => onChange(it.id)}
        >
          {it.label}
          {it.count != null && it.count > 0 && <span className="cnt">{it.count}</span>}
        </button>
      ))}
    </div>
  );
}

export function Toggle({ on, onChange }: { on: boolean; onChange: (v: boolean) => void }) {
  return (
    <button
      type="button"
      className={`toggle${on ? " on" : ""}`}
      onClick={() => onChange(!on)}
      aria-pressed={on}
    />
  );
}

export function StatusDot({ status }: { status: AgentStatus }) {
  return <span className={`dot ${status}`} />;
}

export function Swatch({ project, size }: { project: ProjectRef; size?: number }) {
  const style = size
    ? {
        width: size,
        height: size,
        borderRadius: Math.round(size * 0.3),
        fontSize: Math.round(size * 0.38),
      }
    : undefined;
  return (
    <span className="swatch" style={{ background: hueColor(projectHue(project)), ...style }}>
      {project.name.slice(0, 1).toUpperCase()}
    </span>
  );
}

/** A provider's brand icon in a hue-tinted square. The SVG comes from the same
 *  website CDN the desktop chip uses (see `useProviderIcon`) and is inlined, so
 *  marks authored with `currentColor` pick up the provider hue. A missing or
 *  unreachable icon falls back to the abbreviation monogram; while the fetch is
 *  in flight the square stays empty rather than flashing a monogram. */
export function ProviderMark({ id, lg }: { id: string; lg?: boolean }) {
  const hue = providerHue(id);
  const { svg, failed } = useProviderIcon(id);
  return (
    <span
      className={`pm${lg ? " lg" : ""}`}
      style={{
        background: `oklch(0.72 0.13 ${hue} / .18)`,
        color: `oklch(0.78 0.13 ${hue})`,
      }}
    >
      {svg && !failed ? (
        <span className="pm-svg" dangerouslySetInnerHTML={{ __html: svg }} />
      ) : failed ? (
        <span
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: lg ? 11 : 8,
            fontWeight: 600,
          }}
        >
          {providerShort(id)}
        </span>
      ) : null}
    </span>
  );
}

export function PrPill({ pr }: { pr: PrState | null | undefined }) {
  if (!pr) return null;
  const tone =
    pr.state === "merged"
      ? "merged"
      : pr.mergeable === "conflicting"
        ? "warn"
        : pr.state === "open"
          ? "ok"
          : "";
  const suffix =
    pr.state === "merged" ? " · merged" : pr.mergeable === "conflicting" ? " · conflicts" : "";
  return (
    <span className={`pill mono ${tone}`}>
      <Icon name={pr.state === "merged" ? "merge" : "pr"} size={11} />#{pr.number}
      {suffix}
    </span>
  );
}

const KEYWORDS =
  /^(pub|fn|let|const|mod|use|enum|struct|impl|return|export|function|if|else|import|from|async|await|match|for|in|self|true|false|default|type|interface)$/;

/** Cheap token tinting for the file and diff viewers. */
export function CodeLine({ text }: { text: string }) {
  const parts = text
    .split(
      /(\/\/.*$|"[^"]*"|'[^']*'|`[^`]*`|\b(?:pub|fn|let|const|mod|use|enum|struct|impl|return|export|function|if|else|import|from|async|await|match|for|in|self|true|false|default|type|interface)\b|\b\d+\b|\b[A-Z][A-Za-z0-9_]+\b)/g,
    )
    .filter(Boolean);
  const color = (p: string) => {
    if (p.startsWith("//")) return "var(--fg-3)";
    if (/^["'`]/.test(p)) return "var(--tk-string)";
    if (/^\d+$/.test(p)) return "var(--tk-number)";
    if (KEYWORDS.test(p)) return "var(--tk-keyword)";
    if (/^[A-Z]/.test(p)) return "var(--tk-type)";
    return undefined;
  };
  return (
    <>
      {parts.map((p, i) => (
        <span
          // biome-ignore lint/suspicious/noArrayIndexKey: token position is its identity
          key={i}
          style={{ color: color(p), fontStyle: p.startsWith("//") ? "italic" : undefined }}
        >
          {p}
        </span>
      ))}
    </>
  );
}

export function Chip({
  icon,
  children,
  onClick,
  active,
  className,
}: {
  icon?: IconName;
  children: ReactNode;
  onClick?: () => void;
  active?: boolean;
  className?: string;
}) {
  const cls = `chip${active ? " on" : ""}${onClick ? "" : " static"}${className ? ` ${className}` : ""}`;
  const body = (
    <>
      {icon && <Icon name={icon} size={12} />}
      {children}
    </>
  );
  return onClick ? (
    <button type="button" className={cls} onClick={onClick}>
      {body}
    </button>
  ) : (
    <span className={cls}>{body}</span>
  );
}
