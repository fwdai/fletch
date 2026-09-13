import type { GhRepoSummary, GhStatus } from "@desktop/api/types/providers";
import { Icon } from "@desktop/components/Icon";
import { parseRepoSpec } from "@desktop/util/repoSpec";
import { useEffect, useState } from "react";
import { childPath } from "../../lib/paths";
import { useStore } from "../../store";
import { FolderPickerSheet } from "./FolderPicker";
import type { RunAddProject } from "./useAddProject";

/** Clone from GitHub: one of the user's repos when `gh` is signed in, or a
 *  typed `owner/repo` / URL either way. */
export function CloneForm({
  busy,
  error,
  setError,
  run,
}: {
  busy: boolean;
  error: string | null;
  setError: (text: string) => void;
  run: RunAddProject;
}) {
  const ghStatus = useStore((s) => s.ghStatus);
  const ghRepoList = useStore((s) => s.ghRepoList);
  const cloneRepo = useStore((s) => s.cloneRepo);
  const lastDestParent = useStore((s) => s.lastDestParent);

  const [gh, setGh] = useState<GhStatus | null>(null);
  const [probing, setProbing] = useState(true);
  const [repos, setRepos] = useState<GhRepoSummary[]>([]);
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [typed, setTyped] = useState("");
  // Defaults to the folder the last clone on this host went into.
  const [parent, setParent] = useState(lastDestParent ?? "");
  const [picking, setPicking] = useState(false);

  useEffect(() => {
    let live = true;
    const probe = async () => {
      const status = await ghStatus();
      if (!live) return;
      setGh(status);
      if (!status.authenticated) return;
      const list = await ghRepoList();
      if (live) setRepos(list);
    };
    probe()
      .catch((e: unknown) => {
        if (live) setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (live) setProbing(false);
      });
    return () => {
      live = false;
    };
  }, [ghStatus, ghRepoList, setError]);

  // A typed spec wins while it has anything in it; picking a repo clears it.
  const spec = typed.trim() || (selected ?? "");
  const parsed = parseRepoSpec(spec);
  const query = filter.trim().toLowerCase();
  const shown = query
    ? repos.filter((r) => r.name_with_owner.toLowerCase().includes(query))
    : repos;
  const canClone = parsed.valid && !!parent && !busy;

  return (
    <>
      {probing && <div className="empty">Checking GitHub…</div>}
      {!probing && gh?.authenticated && (
        <>
          <div className="search">
            <Icon name="search" size={15} />
            <input
              placeholder={`Filter ${gh.login ?? "your"} repositories`}
              value={filter}
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              onChange={(e) => setFilter(e.target.value)}
            />
          </div>
          <div className="card">
            {shown.map((repo) => (
              <button
                type="button"
                key={repo.name_with_owner}
                className="row"
                onClick={() => {
                  setSelected(repo.name_with_owner);
                  setTyped("");
                }}
              >
                <Icon name="github" size={15} />
                <div className="main">
                  <div className="lbl">{repo.name_with_owner}</div>
                  {repo.description && <div className="sub">{repo.description}</div>}
                </div>
                {repo.is_private && <span className="val mono">private</span>}
                {selected === repo.name_with_owner && !typed.trim() ? (
                  <Icon name="check" size={18} strokeWidth={2} className="check-ic" />
                ) : (
                  <span style={{ width: 18 }} />
                )}
              </button>
            ))}
            {shown.length === 0 && <div className="empty">No matches</div>}
          </div>
        </>
      )}
      {!probing && !gh?.authenticated && (
        <div className="empty">
          <b>GitHub isn't connected</b>
          {gh?.installed
            ? "Run gh auth login on your Mac to pick from your repositories."
            : "Install the GitHub CLI on your Mac to pick from your repositories."}
        </div>
      )}
      <div className="ap-field">
        <label htmlFor="ap-spec">
          {gh?.authenticated ? "Or paste owner/repo or a URL" : "owner/repo or a URL"}
        </label>
        <input
          id="ap-spec"
          placeholder="fwdai/fletch"
          value={typed}
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          onChange={(e) => setTyped(e.target.value)}
        />
      </div>
      <div className="sect ap-sect">Destination</div>
      <div className="card">
        <button type="button" className="row" onClick={() => setPicking(true)}>
          <Icon name="folder" size={16} />
          <div className="main">
            <div className="lbl mono">
              {parent ? childPath(parent, parsed.name ?? "…") : "Choose a folder"}
            </div>
            <div className="sub">
              {parent ? "Where the clone lands" : "Browse your Mac for a parent folder"}
            </div>
          </div>
          <Icon name="chevR" size={16} className="chev" />
        </button>
      </div>
      {error && <div className="err ap-err">{error}</div>}
      <button
        type="button"
        className="btn primary block ap-use"
        disabled={!canClone}
        onClick={() => void run(() => cloneRepo(spec, parent))}
      >
        {busy ? "Cloning…" : "Clone repository"}
      </button>
      <FolderPickerSheet
        open={picking}
        onClose={() => setPicking(false)}
        start={parent || "~"}
        onPick={setParent}
      />
    </>
  );
}
