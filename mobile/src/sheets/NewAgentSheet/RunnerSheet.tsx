import { ProviderMark, Sheet } from "../../components/ui";
import { hostProviderBlock } from "../../lib/hostProviders";
import {
  contextLabel,
  effortsFor,
  type ModelsByAgent,
  modelsFor,
  providerOptions,
} from "../../lib/models";
import type { HostProvider } from "../../remote";

/** Agent, model and effort in one panel — the prototype's "Runner" sheet. */
export function RunnerSheet({
  open,
  onClose,
  models,
  hostProviders,
  provider,
  model,
  effort,
  setProvider,
  setModel,
  setEffort,
}: {
  open: boolean;
  onClose: () => void;
  models: ModelsByAgent;
  /** What the host said it can run, or null when it has not said (see
   *  `useHostProviders`). Null offers everything, as before. */
  hostProviders: HostProvider[] | null;
  provider: string;
  model: string;
  effort: string;
  setProvider: (id: string) => void;
  setModel: (id: string) => void;
  setEffort: (id: string) => void;
}) {
  const list = modelsFor(models, provider);
  const efforts = effortsFor(list.find((m) => m.id === model));
  // Said once under the row rather than on every chip: the chips are two words
  // wide and the fix is the same sentence for all of them.
  const unavailable = providerOptions().flatMap((p) => {
    const why = hostProviderBlock(hostProviders, p.id);
    return why ? [`${p.label} ${why}`] : [];
  });
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
        {providerOptions().map((p) => {
          const why = hostProviderBlock(hostProviders, p.id);
          return (
            <button
              type="button"
              key={p.id}
              // An agent the host cannot start is not a choice: the spawn would
              // reach the host and fail there. Fixing it is the operator's, on
              // the host, so there is nothing to offer beyond the reason.
              disabled={why !== null}
              className={`chip${p.id === provider ? " on" : ""}`}
              onClick={() => {
                setProvider(p.id);
                setModel(modelsFor(models, p.id)[0]?.id ?? "");
              }}
            >
              <ProviderMark id={p.id} />
              {p.label}
            </button>
          );
        })}
      </div>
      {unavailable.length > 0 && (
        <div className="chips-note">
          {unavailable.join(" · ")} on the host — install or sign in there.
        </div>
      )}
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
              <div className="lbl">{m.name}</div>
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
