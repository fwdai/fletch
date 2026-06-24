import { useEffect, useState } from "react";
import { PROVIDERS } from "../../../data/providers";
import { useAppStore } from "../../../store";
import type { CustomAgent } from "../../../storage/customAgents";
import { Icon } from "../../Icon";
import { SetHead } from "../primitives";
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
  // Two-click delete confirm (matches the FileContextMenu "Confirm Delete?"
  // idiom): the first trash click arms the row, the second deletes. Auto-disarms
  // after a few seconds so a stray click never leaves a row stuck in danger.
  const [armedDeleteId, setArmedDeleteId] = useState<string | null>(null);
  useEffect(() => {
    if (!armedDeleteId) return;
    const t = setTimeout(() => setArmedDeleteId(null), 3000);
    return () => clearTimeout(t);
  }, [armedDeleteId]);

  const modelLabel = (a: CustomAgent): string => {
    if (!a.model) return "Default model";
    const found = (modelsByAgent[a.base] ?? []).find((m) => m.id === a.model);
    return found?.name ?? a.model;
  };

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Custom agents"
        title="Custom agents"
        desc="Give a base coding agent a name, a model, and a standing brief. Custom agents show up in the composer next to the built-ins."
        actions={
          <button className="btn-t primary" onClick={onNew}>
            <Icon name="plus" size={13} /> New agent
          </button>
        }
      />

      {agents.length === 0 ? (
        <button className="ca-empty" onClick={onNew}>
          <span className="ca-empty-ic">
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
                className="ca-card"
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
                  <div className="ca-name">
                    {a.name}
                    <span className="ca-base">
                      <span
                        className="ca-base-dot"
                        style={{ background: `oklch(0.7 0.1 ${prov?.hue ?? 30})` }}
                      />
                      {prov?.label ?? a.base} · {modelLabel(a)}
                    </span>
                  </div>
                  <div className="ca-desc">
                    {a.description || a.instructions || "No instructions yet."}
                  </div>
                </div>
                <div className="ca-acts" onClick={(e) => e.stopPropagation()}>
                  <button
                    className="btn-i sm tip"
                    data-tip-down
                    data-tip="Duplicate"
                    aria-label="Duplicate"
                    onClick={() => onDuplicate(a)}
                  >
                    <Icon name="copy" />
                  </button>
                  <button
                    className="btn-i sm tip"
                    data-tip-down
                    data-tip="Edit"
                    aria-label="Edit"
                    onClick={() => onEdit(a)}
                  >
                    <Icon name="edit" />
                  </button>
                  <button
                    className={`btn-i sm tip ${armedDeleteId === a.id ? "danger" : ""}`}
                    data-tip-down
                    data-tip={armedDeleteId === a.id ? "Click again to delete" : "Delete"}
                    aria-label={armedDeleteId === a.id ? "Confirm delete" : "Delete"}
                    onClick={() => {
                      if (armedDeleteId === a.id) {
                        onDelete(a);
                        setArmedDeleteId(null);
                      } else {
                        setArmedDeleteId(a.id);
                      }
                    }}
                  >
                    <Icon name={armedDeleteId === a.id ? "check" : "trash"} />
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
