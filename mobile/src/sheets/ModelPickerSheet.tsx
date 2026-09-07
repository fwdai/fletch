import { PickerSheet } from "../components/ui";
import { providerLabel } from "../lib/agents";
import { ignore } from "../lib/ignore";
import { contextLabel, modelsFor, useModels } from "../lib/models";
import { agentOf, useStore } from "../store";

export function ModelPickerSheet({
  open,
  onClose,
  agentId,
}: {
  open: boolean;
  onClose: () => void;
  agentId?: string;
}) {
  const agent = useStore((s) => (agentId ? agentOf(s.workspace, agentId) : undefined));
  const setModel = useStore((s) => s.setModel);
  const models = useModels();
  if (!agent) return null;
  const list = modelsFor(models, agent.provider);
  return (
    <PickerSheet
      open={open}
      onClose={onClose}
      title="Model"
      sectionLabel={providerLabel(agent.provider)}
      items={list.map((m) => ({
        id: m.id,
        label: m.name ?? m.id ?? "Default model",
        right: contextLabel(m.contextWindow),
      }))}
      value={agent.model ?? ""}
      onChange={(id) => void setModel(agent.id, id).catch(ignore)}
    />
  );
}
