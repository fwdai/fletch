import { type ReactNode, useState } from "react";
import type { EgressDelta } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { TextInput } from "@/components/ui/TextInput";
import { SetGroup, SetRow } from "../../primitives";
import { globalHosts, type HostList } from "./hostList";

/** The hosts the user allowed on top of Fletch's list, one compact row each
 *  with where it lives and a way to take it back, and a row to add one.
 *  Globally that is the list for every project, where a host answered "All
 *  projects" in the prompt lands too. On a project's page it is that
 *  project's own, with `target` (the file an addition goes to) beside the
 *  input and `children` (the files' state) under the label. */
export function YourHosts(
  props: { global: EgressDelta } | { project: HostList; target: ReactNode; children?: ReactNode },
) {
  const list = "project" in props ? props.project : globalHosts(props.global);
  const scoped = "project" in props ? props : null;
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);

  const save = async (task: () => Promise<unknown>) => {
    setError(null);
    try {
      await task();
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  };
  const allow = list.allow;
  const add = async () => {
    const pattern = draft.trim();
    if (pattern && allow && (await save(() => allow(pattern)))) setDraft("");
  };

  return (
    <SetGroup label={scoped ? "Hosts" : "Your hosts"}>
      {scoped?.children}
      {list.hosts.map(({ pattern, where, remove }) => (
        <SetRow key={pattern} compact title={<span className="mono">{pattern}</span>} sub={where}>
          {remove && (
            <IconButton
              size="xs"
              danger
              tip="Stop allowing this host"
              aria-label={`Remove ${pattern}`}
              onClick={() => void save(remove)}
            >
              <Icon name="close" size={11} />
            </IconButton>
          )}
        </SetRow>
      ))}
      {allow && (
        <SetRow
          title={scoped ? "Allow a host" : "Add a host"}
          sub={
            error ??
            `Allowed in ${scoped ? "this" : "every"} project. A host name, or *.domain for its subdomains.`
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
          {scoped?.target}
          <Button variant="outline" onClick={() => void add()} disabled={!draft.trim()}>
            Add
          </Button>
        </SetRow>
      )}
    </SetGroup>
  );
}
