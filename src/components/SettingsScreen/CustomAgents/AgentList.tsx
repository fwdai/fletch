import { Icon } from "@/components/Icon";
import { CustomizeSwitch } from "@/components/SettingsScreen/CustomizeSwitch";
import { ArmedDeleteButton, useArmedDelete } from "@/components/SettingsScreen/LibraryList";
import { SetHead } from "@/components/SettingsScreen/primitives";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { PROVIDERS } from "@/data/providers";
import type { CustomAgent } from "@/storage/customAgents";
import { useAppStore } from "@/store";
import { Mono } from "./Mono";

export function AgentList({
  agents,
  onNew,
  onEdit,
  onDuplicate,
  onDelete,
}: {
  agents: CustomAgent[];
  onNew: () => void;
  onEdit: (a: CustomAgent) => void;
  onDuplicate: (a: CustomAgent) => void;
  onDelete: (a: CustomAgent) => void;
}) {
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const { armedId, fire } = useArmedDelete();

  const modelLabel = (a: CustomAgent): string => {
    if (!a.model) return "Default model";
    const found = (modelsByAgent[a.base] ?? []).find((m) => m.id === a.model);
    return found?.name ?? a.model;
  };

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Customize"
        eyebrowAside={<CustomizeSwitch />}
        title="Custom agents"
        desc="Give a base coding agent a name, a model, and a standing brief. Custom agents show up in the composer next to the built-ins."
        actions={
          <Button variant="primary" onClick={onNew}>
            <Icon name="plus" size={13} /> New agent
          </Button>
        }
      />

      {agents.length === 0 ? (
        <button className="ca-empty flex-center text-base" onClick={onNew}>
          <span className="ca-empty-ic iflex-center">
            <Icon name="plus" />
          </span>
          <span>Create your first custom agent</span>
        </button>
      ) : (
        <div className="ca-list">
          {agents.map((a) => {
            const prov = PROVIDERS.find((p) => p.id === a.base);
            return (
              <div
                key={a.id}
                className="ca-card flex-center"
                role="button"
                tabIndex={0}
                onClick={() => onEdit(a)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    onEdit(a);
                  }
                }}
              >
                <Mono name={a.name} hue={a.color} />
                <div className="ca-id">
                  <div className="ca-name flex-center text-base">
                    {a.name}
                    <span className="ca-base iflex-center text-xs">
                      <span
                        className="ca-base-dot"
                        style={{ background: `oklch(0.7 0.1 ${prov?.hue ?? 30})` }}
                      />
                      {prov?.label ?? a.base} · {modelLabel(a)}
                    </span>
                  </div>
                  <div className="ca-desc truncate text-sm">
                    {a.description || a.instructions || "No instructions yet."}
                  </div>
                </div>
                <div className="ca-acts flex-center" onClick={(e) => e.stopPropagation()}>
                  <IconButton
                    size="sm"
                    tipDown
                    tip="Duplicate"
                    aria-label="Duplicate"
                    onClick={() => onDuplicate(a)}
                  >
                    <Icon name="copy" />
                  </IconButton>
                  <IconButton
                    size="sm"
                    tipDown
                    tip="Edit"
                    aria-label="Edit"
                    onClick={() => onEdit(a)}
                  >
                    <Icon name="edit" />
                  </IconButton>
                  <ArmedDeleteButton
                    armed={armedId === a.id}
                    onClick={() => fire(a.id, () => onDelete(a))}
                  />
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
