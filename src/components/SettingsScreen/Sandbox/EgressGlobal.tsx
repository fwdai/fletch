import { useState } from "react";
import { api, type EgressDelta } from "@/api";
import { Button } from "@/components/ui/Button";
import { TextInput } from "@/components/ui/TextInput";
import { useGate } from "@/store/capabilities";
import { EGRESS_CATEGORIES, useEgressOverview } from "@/util/useEgressOverview";
import { SetDisclosure, SetGroup, SetRow, SetToggle } from "../primitives";

/** Network › every project: the hosts boxes may reach, as Fletch's template
 *  (maintained with the app, grouped by what it serves) plus the user's own
 *  changes — hosts added for every project, template entries turned off. A
 *  project's `.fletch/settings.json` can change them again for that project.
 *  Telemetry the agent CLIs reach for is listed as blocked quietly. */
export function EgressGlobal({ last }: { last?: boolean }) {
  const gate = useGate("egressPolicy");
  const { overview } = useEgressOverview(null, !!gate);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);

  if (gate) {
    return (
      <SetGroup label="Network" last={last}>
        <SetRow title="Hosts boxes may reach" sub={gate} />
      </SetGroup>
    );
  }
  if (!overview) return null;
  const global = overview.global;

  const save = async (next: EgressDelta) => {
    setError(null);
    try {
      await api.setBoxEgressGlobal(next);
    } catch (e) {
      setError(String(e));
    }
  };
  const toggleTemplate = (pattern: string, on: boolean) =>
    save({
      add: global.add,
      remove: on ? global.remove.filter((p) => p !== pattern) : [...global.remove, pattern],
    });
  const add = async () => {
    const pattern = draft.trim();
    if (!pattern) return;
    await save({ add: [...global.add, pattern], remove: global.remove });
    setDraft("");
  };

  return (
    <SetGroup label="Network" last={last}>
      <SetRow
        title="Hosts boxes may reach"
        sub="Fletch keeps this list current. Your changes here apply to every project; a project's .fletch/settings.json can change them again."
      />
      {EGRESS_CATEGORIES.map(({ key, label }) => {
        const entries = overview.template.filter((t) => t.category === key);
        const off = entries.filter((t) => global.remove.includes(t.pattern)).length;
        return (
          <SetDisclosure
            key={key}
            label={`${label} · ${entries.length - off} of ${entries.length} on`}
          >
            {entries.map((t) => {
              const on = !global.remove.includes(t.pattern);
              return (
                <SetRow key={t.pattern} title={<span className="mono">{t.pattern}</span>}>
                  <SetToggle on={on} onClick={() => void toggleTemplate(t.pattern, !on)} />
                </SetRow>
              );
            })}
          </SetDisclosure>
        );
      })}
      <SetRow
        title="Your hosts"
        sub={
          error ?? "Allowed for every project's boxes. A host name, or *.domain for its subdomains."
        }
      >
        <TextInput
          mono
          value={draft}
          placeholder="static.example.com"
          spellCheck={false}
          invalid={!!error}
          onChange={(e) => {
            setError(null);
            setDraft(e.target.value);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") void add();
          }}
        />
        <Button variant="outline" onClick={() => void add()} disabled={!draft.trim()}>
          Add
        </Button>
      </SetRow>
      {global.add.map((pattern) => (
        <SetRow key={pattern} title={<span className="mono">{pattern}</span>}>
          <Button
            variant="ghost"
            onClick={() =>
              void save({ add: global.add.filter((p) => p !== pattern), remove: global.remove })
            }
          >
            Remove
          </Button>
        </SetRow>
      ))}
      <SetDisclosure label={`Blocked quietly · ${overview.quiet.length} telemetry hosts`}>
        <SetRow
          title="Telemetry and error reporting"
          sub="The agent CLIs reach for these on their own and work without them. A box is refused them without a prompt; allow one above to let it through."
        />
        {overview.quiet.map((pattern) => (
          <SetRow key={pattern} title={<span className="mono">{pattern}</span>} />
        ))}
      </SetDisclosure>
    </SetGroup>
  );
}
