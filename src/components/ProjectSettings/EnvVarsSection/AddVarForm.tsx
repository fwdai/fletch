import { useState } from "react";
import { Icon } from "@/components/Icon";

interface Props {
  /** Keys already listed (from `.env` or configured) — for duplicate rejection. */
  existingKeys: Set<string>;
  /** Persist a new variable's value as a shared override. Rejects if the save
   *  fails, so the form can keep the draft and surface the error. */
  onAdd: (key: string, value: string) => Promise<void>;
}

/** A valid POSIX-ish env var name: a letter or underscore, then word chars. */
const NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** Adds a variable that isn't in `.env`. Its value is stored as an override
 *  (keychain) and shared by default — the reason to add one is to proxy it into
 *  the sandbox. Collapsed to a button until opened. */
export function AddVarForm({ existingKeys, onAdd }: Props) {
  const [open, setOpen] = useState(false);
  const [key, setKey] = useState("");
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const reset = () => {
    setKey("");
    setValue("");
    setError(null);
  };
  const cancel = () => {
    reset();
    setOpen(false);
  };

  const submit = async () => {
    const name = key.trim();
    if (!NAME_RE.test(name)) {
      setError("Name must be letters, digits and underscores, not starting with a digit.");
      return;
    }
    if (existingKeys.has(name)) {
      setError(`“${name}” is already listed.`);
      return;
    }
    if (value === "") {
      setError("Enter a value.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onAdd(name, value);
      reset();
      setOpen(false);
    } catch {
      setError("Couldn’t save. Try again.");
    } finally {
      setBusy(false);
    }
  };

  if (!open) {
    return (
      <button
        type="button"
        className="ev-add-btn iflex-center text-sm"
        onClick={() => setOpen(true)}
      >
        <Icon name="plus" size={13} /> Add variable
      </button>
    );
  }

  return (
    <div className="ev-add-form">
      <div className="ev-add-fields flex-center">
        <input
          className="ps-input mono"
          placeholder="NAME"
          aria-label="New variable name"
          value={key}
          autoFocus
          onChange={(e) => setKey(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && submit()}
        />
        <input
          className="ps-input mono"
          placeholder="value"
          aria-label="New variable value"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && submit()}
        />
        <button type="button" className="ps-btn" onClick={submit} disabled={busy}>
          Add
        </button>
        <button
          type="button"
          className="ev-add-cancel iflex-center"
          aria-label="Cancel"
          onClick={cancel}
        >
          <Icon name="close" size={14} />
        </button>
      </div>
      {error && <div className="ev-add-err text-xs">{error}</div>}
    </div>
  );
}
