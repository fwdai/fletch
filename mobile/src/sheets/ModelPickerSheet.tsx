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
        label: m.name,
        right: contextLabel(m.contextWindow),
      }))}
      value={agent.model ?? ""}
      // The default row's empty id means "no pinned model", which the host
      // spells `null` — persisting "" would leave a model set to nothing.
      onChange={(id) => void setModel(agent.id, id || null).catch(ignore)}
    />
  );
}
