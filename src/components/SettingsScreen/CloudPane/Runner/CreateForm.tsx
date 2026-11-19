import { type KeyboardEvent, useState } from "react";
import type { CloudProviderStatus } from "@/api";
import { Button } from "@/components/ui/Button";
import { CopyCmd } from "@/components/ui/CopyCmd";
import { TextInput } from "@/components/ui/TextInput";
import { StatusLine } from "../StatusLine";

/** No runner yet: what one is and what it costs, the Claude token it signs
 *  its agents in with, an optional name, and Create. The token is held in this
 *  form alone until it is sent, and the form is gone while the runner is
 *  created, so it is kept nowhere after. */
export function CreateForm({
  provider,
  onCreate,
}: {
  provider: CloudProviderStatus;
  onCreate: (claudeToken: string, name: string) => void;
}) {
  const [token, setToken] = useState("");
  const [name, setName] = useState("");
  const ready = !!token.trim();

  const create = () => {
    if (ready) onCreate(token.trim(), name.trim());
  };
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") create();
  };

  return (
    <>
      <StatusLine status="No runner">
        <Button variant="primary" disabled={!ready} onClick={create}>
          Create runner
        </Button>
      </StatusLine>
      <div className="set-gh-lede text-sm">
        A runner is Fletch on a machine at {provider.label} that stays on: agents keep running with
        this Mac closed, and your other devices can follow and review them. {provider.label} bills
        your account for its machine while it runs and its disk until you delete it; each of its
        workspaces gets a machine of its own there too.
      </div>
      <div className="set-form-grid">
        <label className="set-field">
          <span className="set-field-label text-sm">Claude token</span>
          <TextInput
            mono
            type="password"
            placeholder="sk-ant-oat01-…"
            value={token}
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => setToken(e.target.value)}
            onKeyDown={onKeyDown}
          />
        </label>
        <label className="set-field">
          <span className="set-field-label text-sm">Name</span>
          <TextInput
            placeholder="Fletch runner"
            value={name}
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => setName(e.target.value)}
            onKeyDown={onKeyDown}
          />
        </label>
      </div>
      <div className="set-gh-hint text-sm">
        Run <CopyCmd cmd="claude setup-token" /> in a terminal and paste what it prints. The token
        goes to the runner's disk and is not kept on this Mac.
      </div>
    </>
  );
}
