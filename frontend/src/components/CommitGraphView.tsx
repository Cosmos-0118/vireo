import React, { useCallback, useEffect, useMemo, useState } from 'react';
import {
  Check,
  ChevronDown,
  ChevronRight,
  CloudDownload,
  Copy,
  ExternalLink,
  FileCode,
  FileDiff,
  Folder,
  FolderOpen,
  FolderTree,
  GitBranch,
  GitBranchPlus,
  GitCommitHorizontal,
  GitCompare,
  GitFork,
  GitMerge,
  List,
  MoreHorizontal,
  RefreshCw,
  RotateCcw,
  Search,
  Undo2,
  X,
} from 'lucide-react';
import { api } from '../api';
import type {
  CommitDetailRes,
  CommitFileChange,
  CommitLogItem,
  GitStatusRes,
} from '../types';
import { buildCompactFileTree, type FileTreeNode } from '../utils/fileTree';
import { computeGitGraphLayout, parseRemoteUrl } from '../utils/gitGraph';
import {
  buildCommitContextMenuItems,
  FloatingContextMenu,
  type ContextMenuItem,
} from './CommitContextMenu';
import { CommitGraphSvg, WorkingTreeGraphSvg } from './CommitGraphSvg';
import { fileStatusColor, RefPill } from './GitBadges';
import { GitActionModal, type GitModalState } from './GitActionModal';

interface CommitGraphViewProps {
  gitStatus: GitStatusRes | null;
  workspacePath: string;
  initialSelectedHash?: string | null;
  onOpenCommitDiff: (
    commit: { hash: string; short_hash: string; subject: string; author: string; date: string },
    file: CommitFileChange,
    allFiles: CommitFileChange[]
  ) => void;
  onOpenCompareDiff: (
    commit: { hash: string; short_hash: string; subject: string },
    file: CommitFileChange,
    allFiles: CommitFileChange[]
  ) => void;
  onOpenDiff: (path: string, staged?: boolean) => void;
  onRefreshGit: () => Promise<void>;
  onToast: (msg: string, isError?: boolean) => void;
}

export const CommitGraphView: React.FC<CommitGraphViewProps> = ({
  gitStatus,
  workspacePath,
  initialSelectedHash,
  onOpenCommitDiff,
  onOpenCompareDiff,
  onOpenDiff,
  onRefreshGit,
  onToast,
}) => {
  const [commits, setCommits] = useState<CommitLogItem[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const [allBranches, setAllBranches] = useState(true);
  const [search, setSearch] = useState('');
  const [selectedHash, setSelectedHash] = useState<string | null>(initialSelectedHash || null);
  const [detail, setDetail] = useState<CommitDetailRes | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [compareMode, setCompareMode] = useState<{
    commit: CommitLogItem;
    files: CommitFileChange[];
  } | null>(null);
  const [treeView, setTreeView] = useState(true);
  const [collapsedDirs, setCollapsedDirs] = useState<Record<string, boolean>>({});
  const [inspectorHeight, setInspectorHeight] = useState(270);
  const [copiedHash, setCopiedHash] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [modal, setModal] = useState<GitModalState>(null);
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    items: ContextMenuItem[];
  } | null>(null);

  const remoteHost = useMemo(
    () => parseRemoteUrl(gitStatus?.remote_url || ''),
    [gitStatus?.remote_url]
  );

  const fetchCommits = useCallback(
    async (opts?: { append?: boolean; skip?: number }) => {
      setLoading(true);
      try {
        const res = await api.gitLog({
          limit: 100,
          skip: opts?.skip ?? 0,
          all: allBranches,
          search: search.trim() || undefined,
        });
        setCommits((prev) => (opts?.append ? [...prev, ...res.commits] : res.commits));
        setHasMore(res.has_more);
      } catch (e: unknown) {
        onToast(e instanceof Error ? e.message : 'Failed to load commit history', true);
      } finally {
        setLoading(false);
      }
    },
    [allBranches, search, onToast]
  );

  useEffect(() => {
    const t = window.setTimeout(() => {
      fetchCommits({ append: false, skip: 0 });
    }, search ? 200 : 0);
    return () => window.clearTimeout(t);
  }, [fetchCommits, workspacePath, gitStatus?.head_hash, gitStatus?.branch, search]);

  useEffect(() => {
    if (initialSelectedHash) {
      setSelectedHash(initialSelectedHash);
    }
  }, [initialSelectedHash]);

  useEffect(() => {
    if (!selectedHash) {
      setDetail(null);
      return;
    }
    let cancelled = false;
    setDetailLoading(true);
    api
      .gitCommitDetail(selectedHash)
      .then((d) => {
        if (!cancelled) setDetail(d);
      })
      .catch(() => {
        if (!cancelled) setDetail(null);
      })
      .finally(() => {
        if (!cancelled) setDetailLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [selectedHash]);

  const layout = useMemo(() => computeGitGraphLayout(commits), [commits]);

  const copyText = useCallback(
    async (text: string, label: string, flashId?: string) => {
      try {
        await navigator.clipboard.writeText(text);
        if (flashId) {
          setCopiedHash(flashId);
          setTimeout(() => setCopiedHash((cur) => (cur === flashId ? null : cur)), 1400);
        }
        onToast(`Copied ${label}`);
      } catch {
        onToast('Failed to copy to clipboard', true);
      }
    },
    [onToast]
  );

  const runGitAction = useCallback(
    async (fn: () => Promise<void>) => {
      setBusy(true);
      try {
        await fn();
        setModal(null);
        await onRefreshGit();
        await fetchCommits({ append: false, skip: 0 });
      } catch (e: unknown) {
        onToast(e instanceof Error ? e.message : String(e), true);
      } finally {
        setBusy(false);
      }
    },
    [onRefreshGit, fetchCommits, onToast]
  );

  const handleCompareWithWorkingTree = useCallback(
    async (commit: CommitLogItem) => {
      setSelectedHash(commit.hash);
      try {
        const files = await api.gitCompareFiles(commit.hash);
        setCompareMode({ commit, files });
        if (files.length === 0) {
          onToast(`Working tree matches ${commit.short_hash}`);
        }
      } catch (e: unknown) {
        onToast(e instanceof Error ? e.message : 'Compare failed', true);
      }
    },
    [onToast]
  );

  const openCommitContextMenu = (e: React.MouseEvent, commit: CommitLogItem) => {
    e.preventDefault();
    e.stopPropagation();
    const items = buildCommitContextMenuItems(commit, remoteHost, {
      onInspect: (c) => {
        setCompareMode(null);
        setSelectedHash(c.hash);
      },
      onCompareWithWorkingTree: handleCompareWithWorkingTree,
      onCopyFullHash: (c) => copyText(c.hash, `SHA (${c.short_hash})`, c.hash),
      onCopyShortHash: (c) => copyText(c.short_hash, `short SHA (${c.short_hash})`),
      onCopyMessage: async (c) => {
        try {
          const d = await api.gitCommitDetail(c.hash);
          const full = d.body ? `${d.subject}\n\n${d.body}` : d.subject;
          copyText(full, 'commit message');
        } catch {
          copyText(c.message, 'commit message');
        }
      },
      onCopySummary: (c) => copyText(`${c.short_hash} - ${c.message} (${c.author}, ${c.date})`, 'commit summary'),
      onCopyPatch: async (c) => {
        try {
          const res = await api.gitCommitPatch(c.hash);
          copyText(res.patch, `patch for ${c.short_hash}`);
        } catch (err: unknown) {
          onToast(err instanceof Error ? err.message : 'Failed to load patch', true);
        }
      },
      onCopyRemoteUrl: (_c, url) => copyText(url, `${remoteHost?.label || 'remote'} URL`),
      onOpenRemoteUrl: async (_c, url) => {
        try {
          await api.openExternalUrl(url);
        } catch {
          window.open(url, '_blank', 'noopener');
        }
      },
      onCherryPick: (c) => setModal({ kind: 'cherry-pick', commit: c }),
      onRevert: (c) => setModal({ kind: 'revert', commit: c }),
      onReset: (c) => setModal({ kind: 'reset', commit: c, currentBranch: gitStatus?.branch || 'HEAD' }),
      onCheckoutCommit: (c) => setModal({ kind: 'checkout-commit', commit: c }),
      onCreateBranch: (c) => setModal({ kind: 'create-branch', commit: c }),
      onCreateTag: (c) => setModal({ kind: 'create-tag', commit: c }),
      onMergeCommit: (c) => setModal({ kind: 'merge-commit', commit: c, currentBranch: gitStatus?.branch || 'HEAD' }),
    });
    setContextMenu({ x: e.clientX, y: e.clientY, items });
  };

  const startInspectorResize = (e: React.MouseEvent) => {
    e.preventDefault();
    let lastY = e.clientY;
    const onMove = (ev: MouseEvent) => {
      const dy = ev.clientY - lastY;
      lastY = ev.clientY;
      setInspectorHeight((h) => Math.max(170, Math.min(520, h - dy)));
    };
    const onUp = () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  };

  const activeFiles = compareMode ? compareMode.files : detail?.files || [];
  const fileTree = useMemo(() => buildCompactFileTree(activeFiles), [activeFiles]);

  const handleOpenFileRow = (f: CommitFileChange) => {
    if (compareMode) {
      onOpenCompareDiff(
        {
          hash: compareMode.commit.hash,
          short_hash: compareMode.commit.short_hash,
          subject: compareMode.commit.message,
        },
        f,
        compareMode.files
      );
    } else if (detail) {
      onOpenCommitDiff(
        {
          hash: detail.hash,
          short_hash: detail.short_hash,
          subject: detail.subject,
          author: detail.author,
          date: detail.date,
        },
        f,
        detail.files
      );
    }
  };

  const renderTreeNodes = (nodes: Array<FileTreeNode<CommitFileChange>>, depth = 0): React.ReactNode =>
    nodes.map((node) => {
      if (node.kind === 'folder') {
        const collapsed = !!collapsedDirs[node.path];
        return (
          <div key={`dir-${node.path}`}>
            <div
              onClick={() =>
                setCollapsedDirs((prev) => ({ ...prev, [node.path]: !prev[node.path] }))
              }
              style={{ paddingLeft: `${8 + depth * 14}px` }}
              className="flex items-center gap-1.5 py-1 pr-2 text-xs text-[var(--text-muted)] hover:bg-[var(--bg-hover)] rounded cursor-pointer select-none"
            >
              {collapsed ? <ChevronRight size={13} /> : <ChevronDown size={13} />}
              {collapsed ? (
                <Folder size={13} className="text-[var(--accent)] shrink-0" />
              ) : (
                <FolderOpen size={13} className="text-[var(--accent)] shrink-0" />
              )}
              <span className="truncate font-medium">{node.name}</span>
              <span className="ml-auto text-[10px] text-[var(--text-dim)] tabular-nums">
                {node.fileCount}
              </span>
            </div>
            {!collapsed && renderTreeNodes(node.children, depth + 1)}
          </div>
        );
      }

      const f = node.item;
      return (
        <div
          key={`file-${f.path}`}
          onClick={() => handleOpenFileRow(f)}
          style={{ paddingLeft: `${12 + depth * 14}px` }}
          className="flex items-center justify-between gap-2 py-1 pr-2.5 text-xs hover:bg-[var(--bg-hover)] rounded cursor-pointer group"
          title={f.old_path ? `${f.old_path} → ${f.path}` : f.path}
        >
          <div className="flex items-center gap-1.5 min-w-0">
            <FileCode size={13} className="text-[var(--text-dim)] shrink-0" />
            <span
              className={`truncate ${f.status === 'D' ? 'line-through opacity-75' : ''}`}
              style={{ color: fileStatusColor(f.status) }}
            >
              {node.name}
            </span>
          </div>
          <div className="flex items-center gap-2 shrink-0 tabular-nums font-mono text-[11px]">
            {f.binary ? (
              <span className="text-[10px] text-[var(--text-dim)]">BIN</span>
            ) : (
              <>
                {f.additions > 0 && <span className="text-[var(--added-fg)]">+{f.additions}</span>}
                {f.deletions > 0 && <span className="text-[var(--removed-fg)]">-{f.deletions}</span>}
              </>
            )}
            <span
              className="w-3.5 text-center font-bold text-[11px]"
              style={{ color: fileStatusColor(f.status) }}
            >
              {f.status}
            </span>
          </div>
        </div>
      );
    });

  const graphColWidth = Math.max(44, Math.min(layout.maxLanes, 12) * 16 + 22);
  const uncommittedCount = gitStatus?.files.length ?? 0;

  return (
    <div className="flex-1 flex flex-col min-h-0 bg-[var(--bg-base)] select-none">
      {/* Top Toolbar */}
      <div className="flex items-center justify-between gap-2 px-3 py-2 border-b border-[var(--border)] bg-[var(--bg-sidebar)] shrink-0 flex-wrap">
        <div className="flex items-center gap-2 min-w-0">
          <GitFork size={15} className="text-[var(--accent)] shrink-0" />
          <span className="text-xs font-semibold tracking-wide text-[var(--text-primary)]">
            Git Commit Graph
          </span>
          {gitStatus?.branch && (
            <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-[11px] bg-[var(--bg-elevated)] border border-[var(--border)] text-[var(--text-muted)]">
              <GitBranch size={11} className="text-[var(--accent)]" />
              {gitStatus.branch}
            </span>
          )}
          <span className="text-[11px] text-[var(--text-dim)] tabular-nums">
            {commits.length} commit{commits.length === 1 ? '' : 's'}
          </span>
        </div>

        <div className="flex items-center gap-1.5">
          {/* Search input */}
          <div className="relative flex items-center">
            <Search size={12.5} className="absolute left-2.5 text-[var(--text-dim)] pointer-events-none" />
            <input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Filter commits, authors, SHAs, refs..."
              className="w-56 pl-7 pr-6 py-1 text-xs rounded-md bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none focus:border-[var(--accent)]"
            />
            {search && (
              <button
                onClick={() => setSearch('')}
                className="absolute right-1.5 text-[var(--text-dim)] hover:text-[var(--text-primary)] cursor-pointer"
              >
                <X size={12} />
              </button>
            )}
          </div>

          {/* Scope toggle */}
          <div className="flex rounded-md border border-[var(--border)] overflow-hidden text-xs">
            <button
              onClick={() => setAllBranches(true)}
              className={`px-2 py-1 cursor-pointer transition-colors ${
                allBranches
                  ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium'
                  : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
              }`}
              title="Show all local and remote branches"
            >
              All Branches
            </button>
            <button
              onClick={() => setAllBranches(false)}
              className={`px-2 py-1 cursor-pointer transition-colors ${
                !allBranches
                  ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium'
                  : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
              }`}
              title="Show current branch only"
            >
              Current
            </button>
          </div>

          <button
            disabled={busy}
            onClick={() =>
              runGitAction(async () => {
                const res = await api.gitFetch();
                if (!res.ok) throw new Error(res.stderr || 'Fetch failed');
                onToast('Fetched latest remote refs');
              })
            }
            className="px-2 py-1 text-xs rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-primary)] flex items-center gap-1 cursor-pointer"
            title="Git Fetch (--all --prune)"
          >
            <CloudDownload size={13} />
            Fetch
          </button>

          <button
            onClick={() => fetchCommits({ append: false, skip: 0 })}
            className="p-1.5 rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
            title="Refresh Graph"
          >
            <RefreshCw size={13} className={loading ? 'animate-spin' : ''} />
          </button>
        </div>
      </div>

      {/* Column Headers */}
      <div className="flex items-center h-7 px-3 border-b border-[var(--border)] bg-[var(--bg-sidebar)]/60 text-[10.5px] font-semibold uppercase tracking-wider text-[var(--text-dim)] shrink-0">
        <div style={{ width: graphColWidth }} className="shrink-0">
          Graph
        </div>
        <div className="flex-1 min-w-0 px-2">Description</div>
        <div className="w-32 shrink-0 px-2 hidden md:block">Author</div>
        <div className="w-28 shrink-0 px-2 hidden sm:block">Date</div>
        <div className="w-28 shrink-0 px-2 text-right">Commit</div>
      </div>

      {/* Commit Rows */}
      <div className="flex-1 overflow-y-auto min-h-0">
        {/* Virtual Uncommitted Changes Row */}
        {uncommittedCount > 0 && !search && (
          <div
            onClick={() => {
              const first = gitStatus?.files[0];
              if (first) onOpenDiff(first.path, first.staged && !first.unstaged);
            }}
            className="flex items-center h-8 px-3 border-b border-[var(--border)]/40 hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
          >
            <div style={{ width: graphColWidth }} className="shrink-0 flex items-center">
              <WorkingTreeGraphSvg
                maxLanes={layout.maxLanes}
                headLane={layout.headLane}
                headColorIdx={layout.headColorIdx}
                laneWidth={16}
                rowHeight={32}
                maxVisibleLanes={12}
              />
            </div>
            <div className="flex-1 min-w-0 px-2 flex items-center gap-2">
              <span className="text-xs italic font-medium text-[var(--modified-fg)]">
                Uncommitted Changes ({uncommittedCount} file{uncommittedCount === 1 ? '' : 's'})
              </span>
              <span className="font-mono text-[11px] text-[var(--added-fg)]">
                +{gitStatus?.total_additions ?? 0}
              </span>
              <span className="font-mono text-[11px] text-[var(--removed-fg)]">
                -{gitStatus?.total_deletions ?? 0}
              </span>
            </div>
            <div className="w-32 shrink-0 px-2 text-xs text-[var(--text-dim)] hidden md:block">
              Working Tree
            </div>
            <div className="w-28 shrink-0 px-2 text-xs text-[var(--text-dim)] hidden sm:block">
              Now
            </div>
            <div className="w-28 shrink-0 px-2 text-right font-mono text-[11px] text-[var(--text-dim)]">
              *
            </div>
          </div>
        )}

        {layout.rows.map((row) => {
          const c = row.commit;
          const isSelected = selectedHash === c.hash;
          return (
            <div
              key={c.hash}
              onClick={() => {
                setCompareMode(null);
                setSelectedHash(c.hash);
              }}
              onContextMenu={(e) => openCommitContextMenu(e, c)}
              className={`group flex items-center h-8 px-3 border-b border-[var(--border)]/30 cursor-pointer transition-colors ${
                isSelected
                  ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
                  : 'hover:bg-[var(--bg-hover)]/80 text-[var(--text-primary)]'
              }`}
            >
              {/* Graph SVG */}
              <div style={{ width: graphColWidth }} className="shrink-0 flex items-center">
                <CommitGraphSvg
                  row={row}
                  maxLanes={layout.maxLanes}
                  laneWidth={16}
                  rowHeight={32}
                  nodeRadius={4}
                  maxVisibleLanes={12}
                />
              </div>

              {/* Description + Ref Pills */}
              <div className="flex-1 min-w-0 px-2 flex items-center gap-1.5 overflow-hidden">
                {c.refs.map((r, i) => (
                  <RefPill key={`${r.kind}-${r.name}-${i}`} r={r} />
                ))}
                {row.isMerge && (
                  <span
                    className="inline-flex items-center gap-0.5 px-1 py-[1px] rounded text-[10px] font-medium bg-purple-500/15 text-purple-400 border border-purple-500/30 shrink-0"
                    title={`Merge commit (${c.parents.length} parents)`}
                  >
                    <GitMerge size={9.5} />
                    Merge
                  </span>
                )}
                <span className={`truncate text-[12.5px] ${c.is_head ? 'font-semibold' : ''}`}>
                  {c.message}
                </span>
              </div>

              {/* Author */}
              <div className="w-32 shrink-0 px-2 text-xs text-[var(--text-muted)] truncate hidden md:block">
                {c.author}
              </div>

              {/* Relative Date */}
              <div
                className="w-28 shrink-0 px-2 text-[11.5px] text-[var(--text-dim)] truncate hidden sm:block"
                title={c.iso_date}
              >
                {c.date}
              </div>

              {/* Short SHA + quick copy & context menu */}
              <div className="w-28 shrink-0 px-2 flex items-center justify-end gap-1">
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    copyText(c.hash, `SHA (${c.short_hash})`, c.hash);
                  }}
                  className="font-mono text-[11px] px-1.5 py-0.5 rounded bg-[var(--bg-elevated)]/70 hover:bg-[var(--bg-elevated)] text-[var(--text-muted)] hover:text-[var(--text-primary)] flex items-center gap-1 cursor-pointer"
                  title="Copy full commit SHA"
                >
                  {copiedHash === c.hash ? (
                    <Check size={10} className="text-[var(--added-fg)]" />
                  ) : null}
                  {c.short_hash}
                </button>
                <button
                  onClick={(e) => openCommitContextMenu(e, c)}
                  className="p-1 rounded opacity-0 group-hover:opacity-100 hover:bg-[var(--bg-elevated)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer transition-opacity"
                  title="Commit actions"
                >
                  <MoreHorizontal size={13} />
                </button>
              </div>
            </div>
          );
        })}

        {commits.length === 0 && !loading && (
          <div className="p-8 text-center text-xs text-[var(--text-dim)]">
            {search ? `No commits match "${search}".` : 'No commits found in this repository.'}
          </div>
        )}

        {hasMore && (
          <div className="p-3 flex justify-center">
            <button
              disabled={loading}
              onClick={() => fetchCommits({ append: true, skip: commits.length })}
              className="px-3.5 py-1.5 text-xs rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-primary)] cursor-pointer disabled:opacity-50"
            >
              {loading ? 'Loading...' : 'Load More Commits'}
            </button>
          </div>
        )}
      </div>

      {/* Bottom Commit Inspector Split Pane */}
      {(selectedHash || compareMode) && (
        <>
          <div
            onMouseDown={startInspectorResize}
            className="h-1 cursor-row-resize bg-[var(--border)] hover:bg-[var(--accent)] shrink-0 transition-colors"
          />
          <div
            style={{ height: inspectorHeight }}
            className="shrink-0 flex flex-col md:flex-row min-h-0 bg-[var(--bg-sidebar)] border-t border-[var(--border)]"
          >
            {/* Left: Commit Metadata & Quick Actions */}
            <div className="w-full md:w-[44%] border-b md:border-b-0 md:border-r border-[var(--border)] flex flex-col min-h-0">
              <div className="flex items-center justify-between px-3 py-2 border-b border-[var(--border)] shrink-0">
                <div className="flex items-center gap-2 min-w-0">
                  {compareMode ? (
                    <>
                      <GitCompare size={14} className="text-[var(--accent)] shrink-0" />
                      <span className="text-xs font-semibold truncate">
                        Comparing {compareMode.commit.short_hash} → Working Tree
                      </span>
                    </>
                  ) : (
                    <>
                      <GitCommitHorizontal size={14} className="text-[var(--accent)] shrink-0" />
                      <span className="font-mono text-xs font-semibold text-[var(--accent)]">
                        {detail?.short_hash || selectedHash?.slice(0, 7)}
                      </span>
                    </>
                  )}
                </div>
                <button
                  onClick={() => {
                    setSelectedHash(null);
                    setCompareMode(null);
                  }}
                  className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer"
                  title="Close commit details"
                >
                  <X size={13} />
                </button>
              </div>

              <div className="flex-1 overflow-y-auto p-3 space-y-2.5 text-xs select-text">
                {detailLoading && !detail ? (
                  <div className="text-[var(--text-dim)]">Loading commit details...</div>
                ) : detail ? (
                  <>
                    <div className="text-[13.5px] font-semibold text-[var(--text-primary)] leading-snug">
                      {detail.subject}
                    </div>
                    {detail.body && (
                      <pre className="whitespace-pre-wrap font-sans text-xs text-[var(--text-muted)] bg-[var(--bg-input)] p-2 rounded border border-[var(--border)]/70 leading-relaxed">
                        {detail.body}
                      </pre>
                    )}

                    {/* Quick Action Bar */}
                    <div className="flex flex-wrap items-center gap-1.5 pt-0.5 select-none">
                      <button
                        onClick={() => copyText(detail.hash, `SHA (${detail.short_hash})`, detail.hash)}
                        className="px-2 py-1 text-[11px] rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] flex items-center gap-1 cursor-pointer"
                        title="Copy full commit SHA"
                      >
                        <Copy size={11} />
                        Copy SHA
                      </button>
                      <button
                        onClick={() => {
                          const found = commits.find((x) => x.hash === detail.hash);
                          if (found) setModal({ kind: 'cherry-pick', commit: found });
                        }}
                        className="px-2 py-1 text-[11px] rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] flex items-center gap-1 cursor-pointer"
                        title="Cherry-pick onto current branch"
                      >
                        <GitCommitHorizontal size={11} className="text-[var(--accent)]" />
                        Cherry-Pick
                      </button>
                      <button
                        onClick={() => {
                          const found = commits.find((x) => x.hash === detail.hash);
                          if (found) setModal({ kind: 'revert', commit: found });
                        }}
                        className="px-2 py-1 text-[11px] rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] flex items-center gap-1 cursor-pointer"
                        title="Revert commit"
                      >
                        <Undo2 size={11} />
                        Revert
                      </button>
                      <button
                        onClick={() => {
                          const found = commits.find((x) => x.hash === detail.hash);
                          if (found)
                            setModal({
                              kind: 'reset',
                              commit: found,
                              currentBranch: gitStatus?.branch || 'HEAD',
                            });
                        }}
                        className="px-2 py-1 text-[11px] rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--removed-fg)] flex items-center gap-1 cursor-pointer"
                        title="Reset current branch to this commit"
                      >
                        <RotateCcw size={11} />
                        Reset
                      </button>
                      <button
                        onClick={() => {
                          const found = commits.find((x) => x.hash === detail.hash);
                          if (found) setModal({ kind: 'create-branch', commit: found });
                        }}
                        className="px-2 py-1 text-[11px] rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] flex items-center gap-1 cursor-pointer"
                        title="Create branch at this commit"
                      >
                        <GitBranchPlus size={11} />
                        Branch
                      </button>
                      <button
                        onClick={async () => {
                          try {
                            const res = await api.gitCommitPatch(detail.hash);
                            copyText(res.patch, `patch for ${detail.short_hash}`);
                          } catch (e: unknown) {
                            onToast(e instanceof Error ? e.message : 'Failed to copy patch', true);
                          }
                        }}
                        className="px-2 py-1 text-[11px] rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] flex items-center gap-1 cursor-pointer"
                        title="Copy commit diff patch"
                      >
                        <FileDiff size={11} />
                        Copy Patch
                      </button>
                      {remoteHost && (
                        <button
                          onClick={async () => {
                            const url = remoteHost.commitUrl(detail.hash);
                            try {
                              await api.openExternalUrl(url);
                            } catch {
                              window.open(url, '_blank', 'noopener');
                            }
                          }}
                          className="px-2 py-1 text-[11px] rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--accent)] flex items-center gap-1 cursor-pointer"
                        >
                          <ExternalLink size={11} />
                          {remoteHost.label}
                        </button>
                      )}
                    </div>

                    {/* Metadata grid */}
                    <div className="grid grid-cols-[68px_1fr] gap-y-1 text-[11.5px] pt-1 border-t border-[var(--border)]/60">
                      <span className="text-[var(--text-dim)]">Author</span>
                      <span className="text-[var(--text-primary)] truncate">
                        {detail.author}{' '}
                        <span className="text-[var(--text-dim)]">&lt;{detail.author_email}&gt;</span>
                      </span>
                      <span className="text-[var(--text-dim)]">Date</span>
                      <span className="text-[var(--text-primary)]">
                        {detail.date}{' '}
                        <span className="text-[var(--text-dim)]">
                          ({new Date(detail.iso_date).toLocaleString()})
                        </span>
                      </span>
                      <span className="text-[var(--text-dim)]">Commit</span>
                      <span className="font-mono text-[11px] text-[var(--text-muted)] break-all">
                        {detail.hash}
                      </span>
                      {detail.parents.length > 0 && (
                        <>
                          <span className="text-[var(--text-dim)]">
                            Parent{detail.parents.length > 1 ? 's' : ''}
                          </span>
                          <div className="flex flex-wrap gap-1.5">
                            {detail.parents.map((p) => (
                              <button
                                key={p}
                                onClick={() => {
                                  setCompareMode(null);
                                  setSelectedHash(p);
                                }}
                                className="font-mono text-[11px] text-[var(--accent)] hover:underline cursor-pointer"
                                title="Jump to parent commit"
                              >
                                {p.slice(0, 7)}
                              </button>
                            ))}
                          </div>
                        </>
                      )}
                    </div>
                  </>
                ) : null}
              </div>
            </div>

            {/* Right: Changed Files in Commit */}
            <div className="flex-1 flex flex-col min-h-0 min-w-0">
              <div className="flex items-center justify-between px-3 py-2 border-b border-[var(--border)] shrink-0">
                <div className="flex items-center gap-2 text-xs">
                  <span className="font-semibold text-[var(--text-muted)] uppercase text-[11px] tracking-wider">
                    Changed Files ({activeFiles.length})
                  </span>
                  {!compareMode && detail && (
                    <span className="font-mono text-[11px] tabular-nums">
                      <span className="text-[var(--added-fg)]">+{detail.total_additions}</span>{' '}
                      <span className="text-[var(--removed-fg)]">-{detail.total_deletions}</span>
                    </span>
                  )}
                </div>
                <button
                  onClick={() => setTreeView((v) => !v)}
                  className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
                  title={treeView ? 'Switch to Flat List' : 'Switch to Folder Tree'}
                >
                  {treeView ? <List size={13.5} /> : <FolderTree size={13.5} />}
                </button>
              </div>
              <div className="flex-1 overflow-y-auto p-1.5">
                {activeFiles.length === 0 ? (
                  <div className="p-4 text-xs text-[var(--text-dim)] text-center">
                    No file changes.
                  </div>
                ) : treeView ? (
                  renderTreeNodes(fileTree, 0)
                ) : (
                  activeFiles.map((f) => {
                    const name = f.path.split('/').pop() || f.path;
                    const dir = f.path.includes('/')
                      ? f.path.slice(0, f.path.lastIndexOf('/'))
                      : '';
                    return (
                      <div
                        key={f.path}
                        onClick={() => handleOpenFileRow(f)}
                        className="flex items-center justify-between gap-2 px-2 py-1 text-xs hover:bg-[var(--bg-hover)] rounded cursor-pointer"
                      >
                        <div className="flex items-center gap-1.5 min-w-0">
                          <FileCode size={13} className="text-[var(--text-dim)] shrink-0" />
                          <span
                            className={`truncate ${f.status === 'D' ? 'line-through opacity-75' : ''}`}
                            style={{ color: fileStatusColor(f.status) }}
                          >
                            {name}
                          </span>
                          {dir && (
                            <span className="truncate text-[11px] text-[var(--text-dim)]">
                              {dir}
                            </span>
                          )}
                        </div>
                        <div className="flex items-center gap-2 shrink-0 font-mono text-[11px] tabular-nums">
                          {f.additions > 0 && (
                            <span className="text-[var(--added-fg)]">+{f.additions}</span>
                          )}
                          {f.deletions > 0 && (
                            <span className="text-[var(--removed-fg)]">-{f.deletions}</span>
                          )}
                          <span
                            className="w-3.5 text-center font-bold"
                            style={{ color: fileStatusColor(f.status) }}
                          >
                            {f.status}
                          </span>
                        </div>
                      </div>
                    );
                  })
                )}
              </div>
            </div>
          </div>
        </>
      )}

      {/* Floating Context Menu */}
      {contextMenu && (
        <FloatingContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          items={contextMenu.items}
          onClose={() => setContextMenu(null)}
        />
      )}

      {/* Action Confirmation / Input Modals */}
      <GitActionModal
        modal={modal}
        busy={busy}
        onClose={() => setModal(null)}
        onConfirmCherryPick={(hash, noCommit) =>
          runGitAction(async () => {
            const res = await api.gitCherryPick(hash, noCommit);
            if (!res.ok) throw new Error(res.stderr || res.stdout || 'Cherry-pick failed');
            onToast(
              noCommit
                ? `Applied ${hash.slice(0, 7)} without committing`
                : `Cherry-picked ${hash.slice(0, 7)}`
            );
          })
        }
        onConfirmRevert={(hash, noCommit) =>
          runGitAction(async () => {
            const res = await api.gitRevert(hash, noCommit);
            if (!res.ok) throw new Error(res.stderr || res.stdout || 'Revert failed');
            onToast(`Reverted ${hash.slice(0, 7)}`);
          })
        }
        onConfirmReset={(hash, mode) =>
          runGitAction(async () => {
            const res = await api.gitReset(hash, mode);
            if (!res.ok) throw new Error(res.stderr || res.stdout || 'Reset failed');
            onToast(`Reset branch (${mode}) to ${hash.slice(0, 7)}`);
          })
        }
        onConfirmCreateBranch={(name, startPoint) =>
          runGitAction(async () => {
            const res = await api.gitCheckout(name, true, startPoint);
            if (!res.ok) throw new Error(res.stderr || 'Branch creation failed');
            onToast(`Created and switched to branch ${name}`);
          })
        }
        onConfirmCreateTag={(name, target, message) =>
          runGitAction(async () => {
            const res = await api.gitCreateTag(name, target, message || undefined);
            if (!res.ok) throw new Error(res.stderr || 'Tag creation failed');
            onToast(`Created tag ${name}`);
          })
        }
        onConfirmCheckoutCommit={(hash) =>
          runGitAction(async () => {
            const res = await api.gitCheckout(hash, false);
            if (!res.ok) throw new Error(res.stderr || 'Checkout failed');
            onToast(`Checked out ${hash.slice(0, 7)} (detached HEAD)`);
          })
        }
        onConfirmMergeCommit={(hash) =>
          runGitAction(async () => {
            const res = await api.gitMerge(hash);
            if (!res.ok) throw new Error(res.stderr || res.stdout || 'Merge failed');
            onToast(`Merged ${hash.slice(0, 7)}`);
          })
        }
        onConfirmDiscardFile={() => {}}
        onConfirmDiscardAll={() => {}}
      />
    </div>
  );
};
