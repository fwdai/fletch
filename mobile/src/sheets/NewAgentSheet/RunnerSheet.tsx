import type { AgentModels } from "@desktop/data/modelCatalog/types";
import { ProviderMark, Sheet } from "../../components/ui";
import { contextLabel, effortsFor, modelsFor, providerOptions } from "../../lib/models";

/** Agent, model and effort in one panel — the prototype's "Runner" sheet. */
export function RunnerSheet({
  open,
  onClose,
  models,
  provider,
  model,
  effort,
  setProvider,
  setModel,
  setEffort,
}: {
  open: boolean;
  onClose: () => void;
  models: AgentModels[];
  provider: string;
  model: string;
  effort: string;
  setProvider: (id: string) => void;
  setModel: (id: string) => void;
  setEffort: (id: string) => void;
}) {
  const list = modelsFor(models, provider);
  const efforts = effortsFor(list.find((m) => m.id === model));
  return (
    <Sheet
      open={open}
      onClose={onClose}
      stacked
      title="Runner"
      right={
        <button type="button" className="tbtn" onClick={onClose}>
          Done
        </button>
      }
    >
      <div className="sect" style={{ marginTop: 6 }}>
        Agent
      </div>
      <div className="chips">
        {providerOptions().map((p) => (
          <button
            type="button"
            key={p.id}
            className={`chip${p.id === provider ? " on" : ""}`}
            onClick={() => {
              setProvider(p.id);
              setModel(modelsFor(models, p.id)[0]?.id ?? "");
            }}
          >
            <ProviderMark id={p.id} />
            {p.label}
          </button>
        ))}
      </div>
      <div className="sect" style={{ marginTop: 18 }}>
        Model
      </div>
      <div className="card">
        {list.map((m) => (
          <button
            type="button"
            key={m.id}
            className="row"
            style={{ minHeight: 46, padding: "9px 14px" }}
            onClick={() => setModel(m.id)}
          >
            <div className="main">
              <div className="lbl">{m.name ?? m.id ?? "Default model"}</div>
            </div>
            {contextLabel(m.contextWindow) && (
              <span className="val mono">{contextLabel(m.contextWindow)}</span>
            )}
            {m.id === model ? <span className="check-ic">✓</span> : <span style={{ width: 18 }} />}
          </button>
        ))}
      </div>
      <div className="sect" style={{ marginTop: 18 }}>
        Effort
      </div>
      <div className="eff wide">
        {efforts.map((e) => (
          <button
            type="button"
            key={e}
            className={e === effort ? "on" : ""}
            onClick={() => setEffort(e)}
          >
            {e}
          </button>
        ))}
      </div>
    </Sheet>
  );
}
