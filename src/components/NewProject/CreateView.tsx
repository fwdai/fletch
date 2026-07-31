import { useState } from "react";
import { Segmented } from "@/components/Settings/Segmented";
import { Button } from "@/components/ui/Button";
import { ModalBody } from "@/components/ui/Modal";
import { Spinner } from "@/components/ui/Spinner";
import { TextInput } from "@/components/ui/TextInput";
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
    <ModalBody>
      <div className="modal-field">
        <label className="modal-label text-sm">Project name</label>
        <TextInput
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
        <div className="modal-field">
          <label className="modal-label text-sm">Visibility</label>
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
        <div className="modal-field">
          <div className="np-hint text-sm">
            Creating a local project. Connect GitHub later to publish it — you can keep working with
            agents, commits, and history offline until then.
          </div>
        </div>
      )}

      <div className="modal-field">
        <label className="modal-label text-sm">
          Description <span className="modal-opt">(optional)</span>
        </label>
        <TextInput
          placeholder="What is this project?"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
        />
      </div>

      <DestRow parent={parent} onPick={pickParent} name={nameOk ? name.trim() : undefined} />

      {error && <div className="modal-error text-sm">{error}</div>}

      <div className="modal-actions">
        <Button variant="primary" size="lg" disabled={!canCreate} onClick={onCreate}>
          {busy ? (
            <>
              <Spinner /> Creating…
            </>
          ) : connected ? (
            "Create & publish"
          ) : (
            "Create project"
          )}
        </Button>
      </div>
    </ModalBody>
  );
}
