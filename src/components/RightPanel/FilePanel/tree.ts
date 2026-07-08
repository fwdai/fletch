// Tree model + pure helpers for the Files panel explorer.
import type { CheckoutFile } from "@/api";
import { basename, joinPath, parentDir } from "@/util/format";

// ── tree model ──────────────────────────────────────────────────────────
export type DirNode = {
  type: "dir";
  name: string;
  path: string;
  children: TreeNode[];
  // Number of changed files anywhere under this directory — lets a collapsed
  // folder signal that it contains agent edits.
  changedCount: number;
};
export type FileNode = {
  type: "file";
  name: string;
  path: string;
  status: string | null;
  additions: number;
  deletions: number;
};
export type TreeNode = DirNode | FileNode;

// An in-progress inline edit: renaming an existing node, or creating a new
// file/folder inside `parentDir` ("" = repo root).
export type EditState =
  | { mode: "rename"; path: string; isDir: boolean }
  | { mode: "newFile" | "newFolder"; parentDir: string };

// An open context menu, anchored at the cursor over `node`.
export type MenuState = { x: number; y: number; node: TreeNode };

// ── helpers ────────────────────────────────────────────────────────────────

/** A non-colliding "… copy" path for Duplicate, e.g. `a/foo.ts` →
 *  `a/foo copy.ts`, then `a/foo copy 2.ts`, … against `existing`. */
export function duplicatePath(path: string, existing: Set<string>): string {
  const dir = parentDir(path);
  const base = basename(path);
  const dot = base.lastIndexOf(".");
  const stem = dot > 0 ? base.slice(0, dot) : base;
  const ext = dot > 0 ? base.slice(dot) : "";
  for (let n = 1; ; n++) {
    const suffix = n === 1 ? " copy" : ` copy ${n}`;
    const candidate = joinPath(dir, `${stem}${suffix}${ext}`);
    if (!existing.has(candidate)) return candidate;
  }
}

/** Best-effort message from a rejected Tauri command (errors serialize to a
 *  display string). */
export function errMsg(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Operation failed";
}

/** Build a sorted nested tree (dirs first, then files; alpha within each)
 *  from a flat list of checkout files. `extraDirs` injects directories that
 *  carry no files yet (freshly-created empty folders). */
export function buildTree(files: CheckoutFile[], extraDirs: string[] = []): TreeNode[] {
  const roots: TreeNode[] = [];
  // path → DirNode, so we can attach children as we walk each file's segments.
  const dirIndex = new Map<string, DirNode>();

  const childrenOf = (path: string): TreeNode[] =>
    path === "" ? roots : (dirIndex.get(path) as DirNode).children;

  // Ensure a directory path (and its ancestors) exist as DirNodes.
  const ensureDir = (dir: string): void => {
    const segs = dir.split("/");
    let prefix = "";
    for (let i = 0; i < segs.length; i++) {
      const parent = prefix;
      prefix = prefix ? `${prefix}/${segs[i]}` : segs[i];
      if (!dirIndex.has(prefix)) {
        const node: DirNode = {
          type: "dir",
          name: segs[i],
          path: prefix,
          children: [],
          changedCount: 0,
        };
        dirIndex.set(prefix, node);
        childrenOf(parent).push(node);
      }
    }
  };

  for (const f of files) {
    const segs = f.path.split("/");
    if (segs.length > 1) ensureDir(segs.slice(0, -1).join("/"));
    const parent = segs.length > 1 ? segs.slice(0, -1).join("/") : "";
    childrenOf(parent).push({
      type: "file",
      name: segs[segs.length - 1],
      path: f.path,
      status: f.status,
      additions: f.additions,
      deletions: f.deletions,
    });
  }

  for (const d of extraDirs) if (d) ensureDir(d);

  sortNodes(roots);
  annotateChanged(roots);
  return roots;
}

/** Tally changed files per directory (recursively) so collapsed folders can
 *  show how many edits they hide. Returns this level's changed-file count. */
function annotateChanged(nodes: TreeNode[]): number {
  let total = 0;
  for (const n of nodes) {
    if (n.type === "dir") {
      n.changedCount = annotateChanged(n.children);
      total += n.changedCount;
    } else if (n.status) {
      total += 1;
    }
  }
  return total;
}

function sortNodes(nodes: TreeNode[]): void {
  nodes.sort((a, b) => {
    if (a.type !== b.type) return a.type === "dir" ? -1 : 1;
    return a.name.localeCompare(b.name);
  });
  for (const n of nodes) if (n.type === "dir") sortNodes(n.children);
}

/** Pruned copy containing only files that match the query / changed filter
 *  (and their ancestor directories). */
export function filterTree(nodes: TreeNode[], q: string, changedOnly: boolean): TreeNode[] {
  const out: TreeNode[] = [];
  for (const n of nodes) {
    if (n.type === "dir") {
      const kids = filterTree(n.children, q, changedOnly);
      if (kids.length) out.push({ ...n, children: kids });
    } else {
      const nameOk = !q || n.name.toLowerCase().includes(q);
      const chgOk = !changedOnly || !!n.status;
      if (nameOk && chgOk) out.push(n);
    }
  }
  return out;
}
