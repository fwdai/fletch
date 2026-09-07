// `list_checkout_tree` returns a flat path list; the Code tab renders a
// collapsible tree, so fold the paths into nodes and flatten again per the open
// set. Reusable for any flat path list, not just a checkout.

import type { CheckoutFile } from "@desktop/api/types/checkout";

export interface TreeNode {
  name: string;
  path: string;
  dir: boolean;
  status: string | null;
  children: TreeNode[];
}

export function buildTree(files: CheckoutFile[]): TreeNode[] {
  const root: TreeNode = { name: "", path: "", dir: true, status: null, children: [] };
  for (const file of files) {
    const parts = file.path.split("/");
    let node = root;
    parts.forEach((part, i) => {
      const isLeaf = i === parts.length - 1;
      const path = parts.slice(0, i + 1).join("/");
      let next = node.children.find((c) => c.name === part && c.dir === !isLeaf);
      if (!next) {
        next = {
          name: part,
          path,
          dir: !isLeaf,
          status: isLeaf ? file.status : null,
          children: [],
        };
        node.children.push(next);
      }
      node = next;
    });
  }
  const sort = (nodes: TreeNode[]): TreeNode[] =>
    nodes
      .map((n) => ({ ...n, children: sort(n.children) }))
      .sort((a, b) => (a.dir === b.dir ? a.name.localeCompare(b.name) : a.dir ? -1 : 1));
  return sort(root.children);
}

export const hasChanges = (node: TreeNode): boolean =>
  node.status !== null || node.children.some(hasChanges);

export interface TreeRow {
  path: string;
  name: string;
  depth: number;
  dir: boolean;
  open: boolean;
  status: string | null;
  changed: boolean;
}

export function flattenTree(nodes: TreeNode[], open: Set<string>, depth = 0): TreeRow[] {
  const rows: TreeRow[] = [];
  for (const node of nodes) {
    const isOpen = open.has(node.path);
    rows.push({
      path: node.path,
      name: node.name,
      depth,
      dir: node.dir,
      open: isOpen,
      status: node.status,
      changed: hasChanges(node),
    });
    if (node.dir && isOpen) rows.push(...flattenTree(node.children, open, depth + 1));
  }
  return rows;
}

/** Directories that contain a change, so the tree opens on what the agent
 *  touched instead of collapsed at the root. */
export function defaultOpen(nodes: TreeNode[]): Set<string> {
  const open = new Set<string>();
  const walk = (list: TreeNode[]) => {
    for (const node of list) {
      if (!node.dir) continue;
      if (hasChanges(node)) open.add(node.path);
      walk(node.children);
    }
  };
  walk(nodes);
  return open;
}
