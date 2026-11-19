import { useEffect, useState } from "react";
import { TextInput } from "@/components/ui/TextInput";
import { useAppStore } from "@/store";
import { SetGroup, SetRow } from "../primitives";

/** The count this Mac's engine keeps when none is stored: none, since a spare
 *  is billed whether or not a workspace takes it. */
const DEFAULT_SPARES = "0";

const ABOUT =
  "A spare is a machine made and set up ahead of time, so a new workspace starts on it without waiting for one; its provider bills it while it exists, whether or not a workspace takes it.";

/** Settings › Cloud › how many spare machines each connected provider keeps.
 *  A draft committed on blur or Enter; the engine validates it and makes or
 *  destroys spares to match, and its refusal shows under the field until the
 *  next edit. */
export function SparesRow({ last }: { last?: boolean }) {
  const stored = useAppStore((s) => s.cloudSpares);
  const refresh = useAppStore((s) => s.refreshCloudSpares);
  const save = useAppStore((s) => s.saveCloudSpares);
  const [draft, setDraft] = useState(stored ?? "");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    refresh().catch((e) => setError(String(e)));
  }, [refresh]);

  useEffect(() => {
    setDraft(stored ?? "");
  }, [stored]);

  const commit = async () => {
    const next = draft.trim();
    if (stored === null || next === stored) return;
    setError(await save(next));
  };

  return (
    <SetGroup label="Spare machines" last={last}>
      <SetRow title="Spares per provider" sub={error ?? ABOUT}>
        <TextInput
          mono
          inputMode="numeric"
          aria-label="Spare machines per provider"
          value={draft}
          placeholder={DEFAULT_SPARES}
          disabled={stored === null}
          invalid={!!error}
          spellCheck={false}
          onChange={(e) => {
            setError(null);
            setDraft(e.target.value);
          }}
          onBlur={() => void commit()}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
          }}
        />
      </SetRow>
    </SetGroup>
  );
}
