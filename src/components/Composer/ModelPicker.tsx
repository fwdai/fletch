import { useState } from "react";
import { PROVIDERS } from "../../data/providers";
import { useAppStore } from "../../store";
import { Icon } from "../Icon";
import { Chip } from "../ui/Chip";
import { Scrim } from "../ui/Scrim";

interface Props {
  value: string;
  onChange: (id: string) => void;
}

/** Pops a dropdown listing enabled providers. Only `claude` actually
 *  runs today — the other entries are decorative until we wire more
 *  backends. */
export function ModelPicker({ value, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const providerFlags = useAppStore((s) => s.providerFlags);
  const selected = PROVIDERS.find((p) => p.id === value) ?? PROVIDERS[0];
  const enabled = PROVIDERS.filter((p) => providerFlags[p.id] !== false);

  return (
    <div style={{ position: "relative" }}>
      <Chip bordered onClick={() => setOpen((v) => !v)}>
        <span
          className="dot"
          style={{ background: `oklch(0.7 0.1 ${selected.hue})` }}
        />
        <span style={{ fontWeight: 500 }}>{selected.label}</span>
        <Icon name="chevD" size={9} />
      </Chip>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <div className="dd" style={{ bottom: "calc(100% + 6px)", left: 0 }}>
            <div className="dd-sect">Coding agents</div>
            {enabled.map((p) => (
              <div
                key={p.id}
                className={`dd-item ${p.id === value ? "active" : ""}`}
                onClick={() => {
                  onChange(p.id);
                  setOpen(false);
                }}
              >
                <div className="di-i">
                  <span
                    style={{
                      width: 8,
                      height: 8,
                      borderRadius: "50%",
                      background: `oklch(0.7 0.1 ${p.hue})`,
                    }}
                  />
                </div>
                <span className="di-l">{p.label}</span>
                <span className="di-m">{p.version}</span>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
