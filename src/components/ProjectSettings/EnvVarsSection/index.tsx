import { useEffect, useMemo, useState } from "react";
import { api, type EnvEntry } from "@/api";
import { loadRunEnvDoc, type RunEnvDoc, saveRunEnvDoc, varConfig, withVar } from "@/storage/runEnv";
import { basename } from "@/util/format";
import { EnvVarRow } from "./EnvVarRow";

interface Props {
  projectId: string;
  /** Source repo path — where the (gitignored) `.env` lives. */
  repoPath: string;
}

/** The opt-in environment membrane: lists the project's `.env` variables and
 *  lets the user choose, per variable, whether it's shared into the sandboxed
 *  Run process and whether its value mirrors `.env` or is overridden. Nothing
 *  is shared by default. Autosaves per edit, like the run-config section. */
export function EnvVarsSection({ projectId, repoPath }: Props) {
  const [entries, setEntries] = useState<EnvEntry[] | null>(null);
  const [doc, setDoc] = useState<RunEnvDoc>({ version: 1, vars: [] });
  // Override values (from the keychain), fetched for overridden vars so the
  // chip can show and edit them. Keyed by var name.
  const [overrides, setOverrides] = useState<Record<string, string>>({});

  useEffect(() => {
    let cancelled = false;
    (async () => {
      const [envEntries, loadedDoc] = await Promise.all([
        api.readEnvFileKeys(repoPath).catch(() => [] as EnvEntry[]),
        loadRunEnvDoc(projectId),
      ]);
      if (cancelled) return;
      setEntries(envEntries);
      setDoc(loadedDoc);
      // Pull the current override values for any overridden vars.
      const overridden = loadedDoc.vars.filter((v) => v.source === "override");
      const pairs = await Promise.all(
        overridden.map(
          async (v) => [v.key, (await api.getEnvOverride(projectId, v.key)) ?? ""] as const,
        ),
      );
      if (!cancelled) setOverrides(Object.fromEntries(pairs));
    })();
    return () => {
      cancelled = true;
    };
  }, [projectId, repoPath]);

  const envMap = useMemo(() => new Map((entries ?? []).map((e) => [e.key, e.value])), [entries]);

  // Every `.env` key, plus any configured var missing from `.env` (an
  // override-only or stale entry) so nothing the user set silently vanishes.
  const rows = useMemo(() => {
    const keys = [...envMap.keys()];
    for (const v of doc.vars) if (!envMap.has(v.key)) keys.push(v.key);
    return keys.map((key) => ({ key, envValue: envMap.get(key) }));
  }, [envMap, doc]);

  const persist = (next: RunEnvDoc) => {
    setDoc(next);
    void saveRunEnvDoc(projectId, next);
  };

  const onToggleShare = (key: string, shared: boolean) =>
    persist(withVar(doc, { ...varConfig(doc, key), shared }));

  // Drop an override and go back to mirroring `.env` (the revert button, and
  // the empty/equal-to-`.env` commit cases). No-op if not overridden.
  const revertToMirror = async (key: string) => {
    const cfg = varConfig(doc, key);
    if (cfg.source !== "override") return;
    await api.clearEnvOverride(projectId, key);
    setOverrides((prev) => {
      const next = { ...prev };
      delete next[key];
      return next;
    });
    persist(withVar(doc, { ...cfg, source: "mirror" }));
  };

  // A committed chip value. Empty — or exactly the `.env` value — is *not* an
  // override (it just restates the mirror), so it reverts rather than pinning a
  // redundant one. Only a value that genuinely differs from `.env` becomes an
  // override. Mirrors the run-config revert semantics.
  const onCommit = async (key: string, value: string) => {
    if (value === "" || value === envMap.get(key)) {
      await revertToMirror(key);
      return;
    }
    const cfg = varConfig(doc, key);
    await api.setEnvOverride(projectId, key, value);
    setOverrides((prev) => ({ ...prev, [key]: value }));
    persist(withVar(doc, { ...cfg, source: "override" }));
  };

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Environment variables</h2>
        <p className="ps-section-lead text-sm">
          Variables found in this project’s <code>.env</code>. Nothing is shared with the sandbox
          unless you switch it on. Shared values are mirrored live from <code>.env</code>; edit one
          to override it (e.g. a disposable per-agent database) — use <code>{"{{agent_id}}"}</code>{" "}
          for a per-agent value, and clear the field to revert to <code>.env</code>.
        </p>
      </header>

      {entries === null ? (
        <div className="ps-state text-sm">Loading…</div>
      ) : rows.length === 0 ? (
        <div className="ev-empty-state text-sm">
          No <code>.env</code> found in <span className="mono">{basename(repoPath)}</span>.
        </div>
      ) : (
        <div className="ev-list">
          {rows.map(({ key, envValue }) => (
            <EnvVarRow
              key={key}
              varKey={key}
              envValue={envValue}
              overrideValue={overrides[key]}
              cfg={varConfig(doc, key)}
              onToggleShare={(shared) => onToggleShare(key, shared)}
              onCommit={(value) => onCommit(key, value)}
              onRevert={() => revertToMirror(key)}
            />
          ))}
        </div>
      )}
    </section>
  );
}
