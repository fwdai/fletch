import { type FormEvent, useState } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { accountSlug } from "@/data/providerAccounts";
import type { ProviderId } from "@/data/providers";
import { useAppStore } from "@/store";

/** "Add account": a name, which becomes the account's id and its directory
 *  under `~/.fletch/accounts`. Creating it is instant and signs nothing in —
 *  the new row's Sign in does that — so a mistyped name costs nothing. The
 *  backend's refusal (taken, invalid) shows inline. */
export function AddAccountForm({ providerId }: { providerId: ProviderId }) {
  const addAccount = useAppStore((s) => s.addProviderAccount);
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const id = accountSlug(name);

  const reset = () => {
    setOpen(false);
    setName("");
    setError(null);
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!id || busy) return;
    setBusy(true);
    setError(null);
    try {
      await addAccount(providerId, id);
      reset();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  if (!open) {
    return (
      <div className="set-prov-detail-actions flex-center">
        <Button variant="dashed" size="sm" onClick={() => setOpen(true)}>
          <Icon name="plus" size={12} />
          Add account
        </Button>
      </div>
    );
  }

  return (
    <form className="set-prov-acct-add" onSubmit={(e) => void submit(e)}>
      <div className="set-prov-acct-add-row flex-center">
        <input
          className="set-prov-bin-input mono text-sm"
          value={name}
          placeholder="work"
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          autoFocus
          disabled={busy}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") reset();
          }}
        />
        <Button type="submit" variant="primary" size="sm" disabled={busy || !id}>
          {busy ? "Adding…" : "Add"}
        </Button>
        <Button variant="ghost" size="sm" disabled={busy} onClick={reset}>
          Cancel
        </Button>
      </div>
      {/* The id the name becomes, when that differs from what was typed, so
          "My Team" doesn't surprise as "my-team" after the fact. */}
      {id && id !== name && (
        <span className="set-prov-acct-sub mono text-xs">
          ~/.fletch/accounts/{providerId}/{id}
        </span>
      )}
      {error && <span className="set-prov-acct-error text-sm">{error}</span>}
    </form>
  );
}
