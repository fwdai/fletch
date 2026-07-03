import { useState } from "react";
import { Icon } from "@/components/Icon";
import { Segmented } from "@/components/Settings/Segmented";
import { useAppStore } from "@/store";
import { isValidRepoName } from "@/util/repoSpec";
import { DestRow, type NewProjectShared } from "./shared";

/** Create a brand-new local repo. When GitHub is connected it's also published
 *  (repo created + pushed); otherwise it stays local and the git panel offers
 *  "Publish to GitHub" later — so a GitHub-unaware user is never blocked. */
export function CreateView({ shared, onDone }: { shared: NewProjectShared; onDone: () => void }) {
  const createRepo = useAppStore((s) => s.createRepo);
  const { parent, pickParent, gh } = shared;
  const connected = !!gh?.authenticated;

  const [name, setName] = useState("");
  const [visibility, setVisibility] = useState<"private" | "public">("private");
  const [description, setDescription] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const nameOk = isValidRepoName(name);
  const showNameError = name.trim().length > 0 && !nameOk;
  const canCreate = !!parent && nameOk && !busy;

  const onCreate = async () => {
    if (!canCreate) return;
    setBusy(true);
    setError(null);
    try {
      await createRepo(
        name.trim(),
        parent,
        visibility === "private",
        description.trim() || undefined,
        connected,
      );
      onDone();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <div className="np-body">
      <div className="np-field">
        <label>Project name</label>
        <input
          autoFocus
          placeholder="my-new-project"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        {showNameError && (
          <div className="np-hint e text-sm">Use only letters, digits, “.”, “-”, “_”.</div>
        )}
      </div>

      {connected ? (
        <div className="np-field">
          <label>Visibility</label>
          <Segmented
            value={visibility}
            onChange={setVisibility}
            options={[
              { value: "private", label: "Private" },
              { value: "public", label: "Public" },
            ]}
          />
        </div>
      ) : (
        <div className="np-field">
          <div className="np-hint text-sm">
            Creating a local project. Connect GitHub later to publish it — you can keep working with
            agents, commits, and history offline until then.
          </div>
        </div>
      )}

      <div className="np-field">
        <label>
          Description <span className="np-opt">(optional)</span>
        </label>
        <input
          placeholder="What is this project?"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
        />
      </div>

      <DestRow parent={parent} onPick={pickParent} name={nameOk ? name.trim() : undefined} />

      {error && <div className="np-error text-sm">{error}</div>}

      <div className="np-actions">
        <button
          className="np-primary flex-center text-base"
          disabled={!canCreate}
          onClick={onCreate}
        >
          {busy ? (
            <>
              <Icon name="refresh" size={13} /> Creating…
            </>
          ) : connected ? (
            "Create & publish"
          ) : (
            "Create project"
          )}
        </button>
      </div>
    </div>
  );
}
