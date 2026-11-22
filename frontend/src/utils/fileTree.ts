export interface FileTreeFolder<T> {
  kind: 'folder';
  name: string;
  path: string;
  children: Array<FileTreeNode<T>>;
  fileCount: number;
}

export interface FileTreeLeaf<T> {
  kind: 'file';
  name: string;
  path: string;
  item: T;
}

export type FileTreeNode<T> = FileTreeFolder<T> | FileTreeLeaf<T>;

/**
 * Builds a VS Code-style compact folder tree from flat file paths.
 * Single-child folder chains (e.g. `frontend/src/components`) are compressed
 * into a single folder node so deep trees stay clean and scannable.
 */
export function buildCompactFileTree<T extends { path: string }>(
  items: T[]
): Array<FileTreeNode<T>> {
  interface MutableDir {
    name: string;
    path: string;
    dirs: Map<string, MutableDir>;
    files: Array<FileTreeLeaf<T>>;
  }

  const root: MutableDir = {
    name: '',
    path: '',
    dirs: new Map(),
    files: [],
  };

  for (const item of items) {
    const parts = item.path.split('/').filter(Boolean);
    if (parts.length === 0) continue;

    let cur = root;
    for (let i = 0; i < parts.length - 1; i++) {
      const seg = parts[i];
      const nextPath = cur.path ? `${cur.path}/${seg}` : seg;
      let child = cur.dirs.get(seg);
      if (!child) {
        child = { name: seg, path: nextPath, dirs: new Map(), files: [] };
        cur.dirs.set(seg, child);
      }
      cur = child;
    }

    const fileName = parts[parts.length - 1];
    cur.files.push({
      kind: 'file',
      name: fileName,
      path: item.path,
      item,
    });
  }

  function finalize(dir: MutableDir): Array<FileTreeNode<T>> {
    const out: Array<FileTreeNode<T>> = [];

    const sortedDirs = Array.from(dir.dirs.values()).sort((a, b) =>
      a.name.localeCompare(b.name)
    );
    for (let d of sortedDirs) {
      // Compress single-folder chains with no direct files
      let label = d.name;
      while (d.files.length === 0 && d.dirs.size === 1) {
        const only = d.dirs.values().next().value as MutableDir;
        label = `${label}/${only.name}`;
        d = only;
      }
      const children = finalize(d);
      const fileCount = children.reduce(
        (acc, c) => acc + (c.kind === 'file' ? 1 : c.fileCount),
        0
      );
      out.push({
        kind: 'folder',
        name: label,
        path: d.path,
        children,
        fileCount,
      });
    }

    const sortedFiles = [...dir.files].sort((a, b) => a.name.localeCompare(b.name));
    out.push(...sortedFiles);
    return out;
  }

  return finalize(root);
}
