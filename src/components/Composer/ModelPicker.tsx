import { useState } from "react";
import { PROVIDERS } from "../../data/providers";
import { hasAdapter } from "../../adapters";
import { useAppStore } from "../../store";
import { Icon } from "../Icon";
import { ProviderIcon } from "../ProviderIcon";
import { Chip } from "../ui/Chip";
import { Scrim } from "../ui/Scrim";

interface Props {
  value: string;
  onChange: (id: string) => void;
}

/** Pops a dropdown listing enabled providers. Providers without an adapter
 *  (no backend runner yet, e.g. antigravity) are shown but disabled as
 *  "Soon" so they can't be selected and silently fall back to Claude. */
export function ModelPicker({ value, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const providerFlags = useAppStore((s) => s.providerFlags);
  const providerVersions = useAppStore((s) => s.providerVersions);
  const selected = PROVIDERS.find((p) => p.id === value) ?? PROVIDERS[0];
  const enabled = PROVIDERS.filter((p) => providerFlags[p.id] !== false);

  return (
    <div style={{ position: "relative" }}>
      <Chip bordered onClick={() => setOpen((v) => !v)}>
        <ProviderIcon
          slug={selected.id}
          short={selected.short}
          hue={selected.hue}
          size={15}
        />
        <span style={{ fontWeight: 500 }}>{selected.label}</span>
        <Icon name="chevD" size={9} />
      </Chip>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <div className="dd" style={{ bottom: "calc(100% + 6px)", left: 0 }}>
            <div className="dd-sect">Coding agents</div>
            {enabled.map((p) => {
              const wired = hasAdapter(p.id);
              return (
                <div
                  key={p.id}
                  className={`dd-item ${p.id === value ? "active" : ""} ${wired ? "" : "is-disabled"}`}
                  aria-disabled={!wired}
                  onClick={
                    wired
                      ? () => {
                          onChange(p.id);
                          setOpen(false);
                        }
                      : undefined
                  }
                >
                  <ProviderIcon slug={p.id} short={p.short} hue={p.hue} size={18} />
                  <span className="di-l">{p.label}</span>
                  {wired ? (
                    <span className="di-m">{providerVersions[p.id] ?? p.version}</span>
                  ) : (
                    <span className="set-badge soon">Coming soon</span>
                  )}
                </div>
              );
            })}
          </div>
        </>
      )}
    </div>
  );
}
