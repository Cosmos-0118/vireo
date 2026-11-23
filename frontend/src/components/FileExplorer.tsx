import { FolderDashed, MagnifyingGlass } from '@phosphor-icons/react';
import { EmptyState } from './ui/EmptyState';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { setPathDrag } from '../utils/dnd';
import {
  AtSign,
  ChevronDown,
  ChevronRight,
  Copy,
  FileCode,
  FoldVertical,
  GitCompare,
  ListFilter,
  LocateFixed,
  Minus,
  Plus,
  RefreshCw,
  Undo2,
  X,
} from 'lucide-react';
import { api } from '../api';
import type { GitFileStatus, GitStatusRes, TreeEntry } from '../types';
import { FileIcon, FolderIcon, isDimFile } from './FileIcon';
import { fileStatusColor } from './GitBadges';
import { FloatingContextMenu, type ContextMenuItem } from './CommitContextMenu';
import { GitActionModal, type GitModalState } from './GitActionModal';

interface FileExplorerProps {
  active: boolean;
  gitStatus: GitStatusRes | null;
  selectedPath: string | null;
  workspacePath: string;
  /** Bumped whenever the backend reports filesystem changes. */
  fsVersion: number;
  /** path -> pulse key for files an agent just changed. */
  recentPaths?: Record<string, number>;
  onOpenFile: (path: string) => void;
  onOpenDiff: (path: string, staged?: boolean) => void;
  onAskAgent: (text: string) => void;
  onRefreshGit: () => Promise<void>;
  onToast: (msg: string, isError?: boolean) => void;
}

interface Row {
  entry: TreeEntry;
  label: string;
  depth: number;
  open: boolean;
  /** Path whose children are shown beneath this row (differs for compact chains). */
  tailPath: string;
  deleted: boolean;
}

const MAX_FILTER_RESULTS = 200;
const noop = () => {};

function parentOf(path: string): string {
  const i = path.lastIndexOf('/');
  return i < 0 ? '' : path.slice(0, i);
}

export function statusBadge(f: GitFileStatus): string {
  if (f.untracked) return 'U';
  const w = f.worktree_status.trim();
  if (w && w !== '.') return w;
  const i = f.index_status.trim();
  return i && i !== '.' ? i : 'M';
}

const STATUS_TITLES: Record<string, string> = {
  M: 'Modified',
  A: 'Added',
  U: 'Untracked',
  D: 'Deleted',
  R: 'Renamed',
};

export const FileExplorer: React.FC<FileExplorerProps> = ({
  active,
  gitStatus,
  selectedPath,
  workspacePath,
  fsVersion,
  recentPaths,
  onOpenFile,
  onOpenDiff,
  onAskAgent,
  onRefreshGit,
  onToast,
}) => {
  const [rootTree, setRootTree] = useState<TreeEntry[]>([]);
  const [children, setChildren] = useState<Record<string, TreeEntry[]>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [filterOpen, setFilterOpen] = useState(false);
  const [filterQuery, setFilterQuery] = useState('');
  const [allFiles, setAllFiles] = useState<string[] | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; entry: TreeEntry } | null>(null);
  const [modal, setModal] = useState<GitModalState>(null);
  const [busy, setBusy] = useState(false);
  const [revealTarget, setRevealTarget] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const childrenRef = useRef(children);
  childrenRef.current = children;

  const dirList = useCallback(async (path: string): Promise<TreeEntry[]> => {
    try {
      return await api.getTree(path);
    } catch {
      return [];
    }
  }, []);

  /** Loads a directory, following single-child folder chains so they can be compacted. */
  const loadChain = useCallback(
    async (path: string): Promise<Record<string, TreeEntry[]>> => {
      const loaded: Record<string, TreeEntry[]> = {};
      let cur = path;
      for (let i = 0; i < 12; i++) {
        const list = childrenRef.current[cur] ?? (await dirList(cur));
        loaded[cur] = list;
        if (list.length === 1 && list[0].is_dir) cur = list[0].path;
        else break;
      }
      return loaded;
    },
    [dirList]
  );

  // Reset on workspace / branch change.
  useEffect(() => {
    setChildren({});
    setExpanded(new Set());
    dirList('').then(setRootTree);
  }, [workspacePath, gitStatus?.branch, dirList]);

  // Live-refresh the root and every loaded folder when the filesystem changes.
  const refreshAll = useCallback(async () => {
    const paths = Object.keys(childrenRef.current);
    const [root, ...fetched] = await Promise.all([
      dirList(''),
      ...paths.map((p) => api.getTree(p).catch(() => null)),
    ]);
    setRootTree(root);
    setChildren(() => {
      const next: Record<string, TreeEntry[]> = {};
      paths.forEach((p, i) => {
        const list = fetched[i];
        if (list) next[p] = list;
      });
      return next;
    });
    setAllFiles(null);
  }, [dirList]);

  useEffect(() => {
    if (!active) return;
    const t = window.setTimeout(refreshAll, 250);
    return () => window.clearTimeout(t);
  }, [fsVersion, active, refreshAll]);

  // Load the flat file list lazily for the filter.
  useEffect(() => {
    if (!filterOpen || allFiles) return;
    api.listFiles().then(setAllFiles).catch(() => setAllFiles([]));
  }, [filterOpen, allFiles]);

  // Git status derivations.
  const { statusByPath, dirtyFolders, deletedByDir } = useMemo(() => {
    const statusByPath = new Map<string, { badge: string; file: GitFileStatus }>();
    const dirtyFolders = new Map<string, 'M' | 'U'>();
    const deletedByDir = new Map<string, TreeEntry[]>();
    for (const f of gitStatus?.files ?? []) {
      const badge = statusBadge(f);
      statusByPath.set(f.path, { badge, file: f });
      const kind: 'M' | 'U' = badge === 'U' || badge === 'A' ? 'U' : 'M';
      let dir = parentOf(f.path);
      while (dir) {
        const prev = dirtyFolders.get(dir);
        if (prev !== 'M') dirtyFolders.set(dir, prev && prev !== kind ? 'M' : kind);
        dir = parentOf(dir);
      }
      if (badge === 'D') {
        const parent = parentOf(f.path);
        const list = deletedByDir.get(parent) ?? [];
        list.push({
          name: f.path.slice(parent ? parent.length + 1 : 0),
          path: f.path,
          is_dir: false,
          size: 0,
        });
        deletedByDir.set(parent, list);
      }
    }
    return { statusByPath, dirtyFolders, deletedByDir };
  }, [gitStatus]);

  const mergedChildren = useCallback(
    (path: string, real: TreeEntry[]): TreeEntry[] => {
      const extra = deletedByDir.get(path);
      if (!extra) return real;
      const names = new Set(real.map((e) => e.path));
      const missing = extra.filter((e) => !names.has(e.path));
      return missing.length ? [...real, ...missing] : real;
    },
    [deletedByDir]
  );

  const rows = useMemo(() => {
    const out: Row[] = [];
    const walk = (entries: TreeEntry[], depth: number) => {
      for (const entry of entries) {
        if (!entry.is_dir) {
          out.push({
            entry,
            label: entry.name,
            depth,
            open: false,
            tailPath: entry.path,
            deleted: statusByPath.get(entry.path)?.badge === 'D',
          });
          continue;
        }
        // Resolve single-child folder chain (e.g. frontend/src/components).
        let tail = entry.path;
        let label = entry.name;
        for (let i = 0; i < 12; i++) {
          const real = children[tail];
          if (!real) break;
          const kids = mergedChildren(tail, real);
          if (kids.length === 1 && kids[0].is_dir) {
            tail = kids[0].path;
            label += `/${kids[0].name}`;
          } else break;
        }
        const open = expanded.has(entry.path);
        out.push({ entry, label, depth, open, tailPath: tail, deleted: false });
        if (open && children[tail]) walk(mergedChildren(tail, children[tail]), depth + 1);
      }
    };
    walk(mergedChildren('', rootTree), 0);
    return out;
  }, [rootTree, children, expanded, mergedChildren, statusByPath]);

  const toggleDir = useCallback(
    async (entry: TreeEntry) => {
      if (expanded.has(entry.path)) {
        setExpanded((prev) => {
          const next = new Set(prev);
          next.delete(entry.path);
          return next;
        });
        return;
      }
      const loaded = await loadChain(entry.path);
      setChildren((prev) => ({ ...prev, ...loaded }));
      setExpanded((prev) => new Set(prev).add(entry.path));
    },
    [expanded, loadChain]
  );

  const collapseAll = useCallback(() => setExpanded(new Set()), []);

  const reveal = useCallback(
    async (path: string) => {
      if (filterOpen) {
        setFilterOpen(false);
        setFilterQuery('');
      }
      const parts = path.split('/');
      const dirs: string[] = [];
      for (let i = 1; i < parts.length; i++) dirs.push(parts.slice(0, i).join('/'));
      const loaded: Record<string, TreeEntry[]> = {};
      for (const d of dirs) {
        loaded[d] = childrenRef.current[d] ?? (await dirList(d));
      }
      setChildren((prev) => ({ ...prev, ...loaded }));
      setExpanded((prev) => {
        const next = new Set(prev);
        dirs.forEach((d) => next.add(d));
        return next;
      });
      setRevealTarget(path);
    },
    [dirList, filterOpen]
  );

  useEffect(() => {
    if (!revealTarget) return;
    const el = scrollRef.current?.querySelector<HTMLElement>(
      `[data-path="${CSS.escape(revealTarget)}"]`
    );
    if (el) {
      el.scrollIntoView({ block: 'center' });
      setRevealTarget(null);
    }
  }, [revealTarget, rows]);

  const runGit = useCallback(
    async (fn: () => Promise<void>) => {
      setBusy(true);
      try {
        await fn();
        setModal(null);
        await onRefreshGit();
      } catch (e: unknown) {
        onToast(e instanceof Error ? e.message : String(e), true);
      } finally {
        setBusy(false);
      }
    },
    [onRefreshGit, onToast]
  );

  const copy = useCallback(
    (text: string, what: string) => {
      navigator.clipboard
        .writeText(text)
        .then(() => onToast(`Copied ${what}`))
        .catch(() => onToast('Copy failed', true));
    },
    [onToast]
  );

  const absolutePath = (rel: string) => {
    const sep = workspacePath.includes('\\') ? '\\' : '/';
    const base = workspacePath.replace(/[\\/]+$/, '');
    return `${base}${sep}${rel.split('/').join(sep)}`;
  };

  const buildMenu = (entry: TreeEntry): ContextMenuItem[] => {
    const st = statusByPath.get(entry.path);
    const items: ContextMenuItem[] = [];
    if (!entry.is_dir) {
      items.push({
        id: 'open',
        label: 'Open File',
        icon: <FileCode size={13} />,
        disabled: st?.badge === 'D',
        onClick: () => onOpenFile(entry.path),
      });
      items.push({
        id: 'diff',
        label: 'Open Changes',
        icon: <GitCompare size={13} />,
        disabled: !st,
        onClick: () => onOpenDiff(entry.path, !!st && st.file.staged && !st.file.unstaged),
      });
    }
    items.push({
      id: 'agent',
      label: 'Mention in Agent Chat',
      icon: <AtSign size={13} />,
      dividerBefore: !entry.is_dir,
      onClick: () => onAskAgent(`@${entry.path}`),
    });
    if (st && !entry.is_dir) {
      if (st.file.unstaged || st.file.untracked) {
        items.push({
          id: 'stage',
          label: 'Stage Changes',
          icon: <Plus size={13} />,
          dividerBefore: true,
          onClick: () =>
            runGit(async () => {
              const res = await api.gitStage({ paths: [entry.path] });
              if (!res.ok) throw new Error(res.stderr || 'Stage failed');
            }),
        });
      }
      if (st.file.staged) {
        items.push({
          id: 'unstage',
          label: 'Unstage Changes',
          icon: <Minus size={13} />,
          dividerBefore: !(st.file.unstaged || st.file.untracked),
          onClick: () =>
            runGit(async () => {
              const res = await api.gitUnstage({ paths: [entry.path] });
              if (!res.ok) throw new Error(res.stderr || 'Unstage failed');
            }),
        });
      }
      items.push({
        id: 'discard',
        label: st.file.untracked ? 'Delete Untracked File…' : 'Discard Changes…',
        icon: <Undo2 size={13} />,
        danger: true,
        onClick: () =>
          setModal({ kind: 'discard-file', path: entry.path, untracked: st.file.untracked }),
      });
    }
    items.push(
      {
        id: 'copy-rel',
        label: 'Copy Relative Path',
        icon: <Copy size={13} />,
        dividerBefore: true,
        onClick: () => copy(entry.path, 'relative path'),
      },
      {
        id: 'copy-abs',
        label: 'Copy Absolute Path',
        icon: <Copy size={13} />,
        onClick: () => copy(absolutePath(entry.path), 'absolute path'),
      }
    );
    return items;
  };

  const filterMatches = useMemo(() => {
    const q = filterQuery.trim().toLowerCase();
    if (!q || !allFiles) return [];
    const out: string[] = [];
    for (const f of allFiles) {
      if (f.toLowerCase().includes(q)) {
        out.push(f);
        if (out.length >= MAX_FILTER_RESULTS) break;
      }
    }
    return out;
  }, [filterQuery, allFiles]);

  const headerBtn =
    'p-1.5 rounded-md hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer';

  const renderRow = (row: Row) => {
    const { entry, depth } = row;
    const st = statusByPath.get(entry.path);
    const dirty = entry.is_dir ? dirtyFolders.get(entry.path) : undefined;
    const isSelected = selectedPath === entry.path;
    const color = st
      ? fileStatusColor(st.badge)
      : !entry.is_dir && isDimFile(entry.path)
      ? 'var(--text-dim)'
      : undefined;
    const canDiff = !entry.is_dir && !!st;

    return (
      <div
        key={entry.path}
        data-path={entry.path}
        draggable
        onDragStart={(e) => setPathDrag(e, { rel: entry.path, abs: absolutePath(entry.path) })}
        onClick={(e) => {
          if (entry.is_dir) toggleDir(entry);
          else if (row.deleted || (e.altKey && st)) onOpenDiff(entry.path);
          else onOpenFile(entry.path);
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          setMenu({ x: e.clientX, y: e.clientY, entry });
        }}
        style={{ paddingLeft: `${8 + depth * 14}px` }}
        className={`group relative mx-1.5 flex items-center gap-1.5 h-[26px] pr-2 rounded-md text-[12.5px] cursor-pointer select-none transition-colors ${
          isSelected
            ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
            : 'hover:bg-[var(--bg-hover)] text-[var(--text-primary)]'
        } ${recentPaths?.[entry.path] !== undefined ? `row-pulse-${recentPaths[entry.path] % 2}` : ''}`}
        title={entry.path}
      >
        {Array.from({ length: depth }, (_, i) => (
          <span
            key={i}
            className="absolute top-0 bottom-0 w-px bg-[var(--border)] opacity-50 group-hover:opacity-90"
            style={{ left: `${8 + i * 14 + 6}px` }}
          />
        ))}
        {entry.is_dir ? (
          <>
            {row.open ? (
              <ChevronDown size={13} className="shrink-0 text-[var(--text-muted)]" />
            ) : (
              <ChevronRight size={13} className="shrink-0 text-[var(--text-muted)]" />
            )}
            <FolderIcon
              name={row.label.split('/').pop() || row.label}
              open={row.open}
              size={15}
            />
          </>
        ) : (
          <>
            <span className="w-[13px] shrink-0" />
            <FileIcon path={entry.path} size={15} />
          </>
        )}
        <span
          className={`truncate min-w-0 flex-1 ${row.deleted ? 'line-through opacity-75' : ''}`}
          style={color ? { color } : undefined}
        >
          {row.label}
        </span>
        <span className="hidden group-hover:flex items-center gap-0.5 shrink-0">
          {canDiff && !row.deleted && (
            <button
              onClick={(e) => {
                e.stopPropagation();
                onOpenDiff(entry.path, st!.file.staged && !st!.file.unstaged);
              }}
              className="p-0.5 rounded hover:bg-[var(--bg-elevated)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
              title="Open Changes"
            >
              <GitCompare size={12.5} />
            </button>
          )}
          <button
            onClick={(e) => {
              e.stopPropagation();
              onAskAgent(`@${entry.path}`);
            }}
            className="p-0.5 rounded hover:bg-[var(--bg-elevated)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
            title="Mention in Agent Chat"
          >
            <AtSign size={12.5} />
          </button>
        </span>
        {st && (
          <span
            className="font-mono text-[11px] font-bold w-4 text-center shrink-0"
            style={{ color: fileStatusColor(st.badge) }}
            title={STATUS_TITLES[st.badge] ?? st.badge}
          >
            {st.badge}
          </span>
        )}
        {dirty && !row.open && (
          <span
            className="w-1.5 h-1.5 rounded-full shrink-0 mr-1"
            style={{ background: dirty === 'U' ? 'var(--added-fg)' : 'var(--modified-fg)' }}
            title="Contains changes"
          />
        )}
      </div>
    );
  };

  return (
    <div className="flex-1 flex flex-col min-h-0">
      <div className="flex items-center justify-between h-10 px-3.5 border-b border-[var(--border)] shrink-0">
        <span className="text-[11.5px] font-semibold uppercase tracking-wider text-[var(--text-muted)]">
          Explorer
        </span>
        <div className="flex items-center gap-0.5">
          <button
            onClick={() => {
              setFilterOpen((v) => !v);
              setFilterQuery('');
            }}
            className={`${headerBtn} ${filterOpen ? 'bg-[var(--bg-active)] text-[var(--text-primary)]' : ''}`}
            title="Filter files"
          >
            <ListFilter size={14} />
          </button>
          <button
            onClick={() => selectedPath && reveal(selectedPath)}
            disabled={!selectedPath}
            className={`${headerBtn} disabled:opacity-40 disabled:cursor-default`}
            title="Reveal active file"
          >
            <LocateFixed size={14} />
          </button>
          <button onClick={collapseAll} className={headerBtn} title="Collapse all">
            <FoldVertical size={14} />
          </button>
          <button onClick={refreshAll} className={headerBtn} title="Refresh files">
            <RefreshCw size={14} />
          </button>
        </div>
      </div>

      {filterOpen && (
        <div className="px-2 py-1.5 border-b border-[var(--border)] shrink-0">
          <div className="flex items-center gap-1 rounded bg-[var(--bg-input)] border border-[var(--border)] focus-within:border-[var(--accent)] px-2">
            <input
              value={filterQuery}
              onChange={(e) => setFilterQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Escape') {
                  setFilterOpen(false);
                  setFilterQuery('');
                }
              }}
              placeholder="Filter files…"
              className="flex-1 min-w-0 py-1 text-xs bg-transparent text-[var(--text-primary)] outline-none"
              autoFocus
            />
            {filterQuery && (
              <button
                onClick={() => setFilterQuery('')}
                className="text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
              >
                <X size={12} />
              </button>
            )}
          </div>
        </div>
      )}

      <div ref={scrollRef} className="flex-1 overflow-y-auto py-1.5">
        {filterOpen && filterQuery.trim() ? (
          filterMatches.length === 0 ? (
            allFiles ? (
              <EmptyState
                size="sm"
                icon={MagnifyingGlass}
                title="No matching files"
                description={<>Nothing matches <span className="text-[var(--text-muted)]">“{filterQuery.trim()}”</span>.</>}
              />
            ) : (
              <div className="p-3 text-xs text-[var(--text-dim)]">Loading…</div>
            )
          ) : (
            filterMatches.map((f) => {
              const st = statusByPath.get(f);
              const name = f.slice(f.lastIndexOf('/') + 1);
              const dir = parentOf(f);
              return (
                <div
                  key={f}
                  data-path={f}
                  draggable
                  onDragStart={(e) => setPathDrag(e, { rel: f, abs: absolutePath(f) })}
                  onClick={() => onOpenFile(f)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    setMenu({
                      x: e.clientX,
                      y: e.clientY,
                      entry: { name, path: f, is_dir: false, size: 0 },
                    });
                  }}
                  className={`mx-1.5 flex items-center gap-1.5 h-[26px] px-2 rounded-md text-[12.5px] cursor-pointer ${
                    selectedPath === f ? 'bg-[var(--bg-active)]' : 'hover:bg-[var(--bg-hover)]'
                  }`}
                  title={f}
                >
                  <FileIcon path={f} size={15} />
                  <span
                    className="truncate shrink-0 max-w-[65%]"
                    style={st ? { color: fileStatusColor(st.badge) } : undefined}
                  >
                    {name}
                  </span>
                  {dir && (
                    <span className="truncate text-[11px] text-[var(--text-dim)]">{dir}</span>
                  )}
                </div>
              );
            })
          )
        ) : rows.length === 0 ? (
          <EmptyState size="sm" icon={FolderDashed} title="This folder is empty" description="Files you create here will show up in the explorer." />
        ) : (
          rows.map(renderRow)
        )}
      </div>

      {menu && (
        <FloatingContextMenu
          x={menu.x}
          y={menu.y}
          items={buildMenu(menu.entry)}
          onClose={() => setMenu(null)}
        />
      )}

      <GitActionModal
        modal={modal}
        busy={busy}
        onClose={() => setModal(null)}
        onConfirmCherryPick={noop}
        onConfirmRevert={noop}
        onConfirmReset={noop}
        onConfirmCreateBranch={noop}
        onConfirmCreateTag={noop}
        onConfirmCheckoutCommit={noop}
        onConfirmMergeCommit={noop}
        onConfirmDiscardFile={(path, untracked) =>
          runGit(async () => {
            const res = await api.gitDiscard(path, untracked);
            if (!res.ok) throw new Error(res.stderr || 'Discard failed');
            onToast(untracked ? `Deleted ${path}` : `Discarded changes in ${path}`);
          })
        }
        onConfirmDiscardAll={noop}
      />
    </div>
  );
};
