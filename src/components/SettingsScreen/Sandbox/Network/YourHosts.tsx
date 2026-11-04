import { useState } from "react";
import { api, type EgressDelta } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { TextInput } from "@/components/ui/TextInput";
import { SetGroup, SetRow } from "../../primitives";

/** The hosts the user allowed for every project: the global list, one compact
 *  row each with a way to take it back, and a row to add one. A host answered
 *  "All projects" in the prompt lands here too. Project hosts live on the
 *  project page and are not repeated. */
export function YourHosts({ global }: { global: EgressDelta }) {
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);

  const save = async (add: string[]) => {
    setError(null);
    try {
      await api.setBoxEgressGlobal({ add, remove: [] });
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  };
  const add = async () => {
    const pattern = draft.trim();
    if (!pattern) return;
    if (await save([...global.add, pattern])) setDraft("");
  };

  return (
    <SetGroup label="Your hosts">
      {global.add.map((pattern) => (
        <SetRow
          key={pattern}
          compact
          title={<span className="mono">{pattern}</span>}
          sub="All projects"
        >
          <IconButton
            size="xs"
            danger
            tip="Stop allowing this host"
            aria-label={`Remove ${pattern}`}
            onClick={() => void save(global.add.filter((p) => p !== pattern))}
          >
            <Icon name="close" size={11} />
          </IconButton>
        </SetRow>
      ))}
      <SetRow
        title="Add a host"
        sub={error ?? "Allowed in every project. A host name, or *.domain for its subdomains."}
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
    </SetGroup>
  );
}
