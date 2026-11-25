import { FileDashed, GitCommit } from '@phosphor-icons/react';
import { EmptyState } from './ui/EmptyState';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
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
import {
  appendUniqueCommits,
  computeGitGraphLayout,
  parseRemoteUrl,
  WORKING_TREE_ID,
} from '../utils/gitGraph';
import {
  buildCommitContextMenuItems,
  FloatingContextMenu,
  type ContextMenuItem,
} from './CommitContextMenu';
import { CommitGraphSvg, graphColumnWidth } from './CommitGraphSvg';
import { useCommitHover } from './CommitHoverCard';
import { fileStatusColor, RefBadges, SyncMark } from './GitBadges';

const FULL_ROW_H = 28;
const FULL_LANE_W = 14;
const FULL_MAX_LANES = 16;
import { GitActionModal, type GitModalState } from './GitActionModal';
import ResizeHandle from './ResizeHandle';
import { setPref } from '../prefs';

const INSPECTOR_KEY = 'vireo-graph-inspector-v1';
const INSPECTOR_DEFAULT = { w: 460, h: 320 };
const INSPECTOR_MIN = { w: 320, h: 170 };
/** Below this page width the detail panel docks under the list instead of beside it. */
const SIDE_DOCK_MIN_WIDTH = 880;
const LIST_MIN_WIDTH = 360;

// Shared column geometry so the sticky header and every row line up exactly.
const COL_AUTHOR = 'w-40 shrink-0 px-3 truncate hidden @[620px]:block';
const COL_DATE = 'w-32 shrink-0 px-3 truncate tabular-nums hidden @[460px]:block';
const COL_SHA = 'w-[84px] shrink-0 pl-3 pr-1';
const COL_ACTIONS = 'w-7 shrink-0 flex items-center justify-center';

function loadInspectorSize() {
  try {
    const raw = JSON.parse(localStorage.getItem(INSPECTOR_KEY) || 'null');
    if (raw && Number.isFinite(raw.w) && Number.isFinite(raw.h)) return { w: raw.w, h: raw.h };
  } catch {
    // ignore
  }
  return INSPECTOR_DEFAULT;
}

function authorColor(name: string) {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) % 360;
  return `hsl(${h} 45% 42%)`;
}

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
  const [inspectorSize, setInspectorSize] = useState(loadInspectorSize);
  const [rootEl, setRootEl] = useState<HTMLDivElement | null>(null);
  const [rootSize, setRootSize] = useState({ w: 0, h: 0 });
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

  const appendInFlight = useRef(false);
  const logSeqRef = useRef(0);
  const fetchCommits = useCallback(
    async (opts?: { append?: boolean; skip?: number }) => {
      if (opts?.append) {
        if (appendInFlight.current) return;
        appendInFlight.current = true;
      }
      const seq = ++logSeqRef.current;
      setLoading(true);
      try {
        const res = await api.gitLog({
          limit: 100,
          skip: opts?.skip ?? 0,
          all: allBranches,
          search: search.trim() || undefined,
        });
        if (seq !== logSeqRef.current) return;
        setCommits((prev) => (opts?.append ? appendUniqueCommits(prev, res.commits) : res.commits));
        setHasMore(res.has_more);
      } catch (e: unknown) {
        if (seq === logSeqRef.current) {
          onToast(e instanceof Error ? e.message : 'Failed to load commit history', true);
        }
      } finally {
        if (opts?.append) appendInFlight.current = false;
        if (seq === logSeqRef.current) setLoading(false);
      }
    },
    [allBranches, search, onToast]
  );

  const gitRev =
    gitStatus?.refs_fingerprint ??
    `${gitStatus?.branch ?? ''}|${gitStatus?.head_hash ?? ''}|${gitStatus?.ahead ?? 0}|${gitStatus?.behind ?? 0}`;

  useEffect(() => {
    if (!search) {
      void fetchCommits({ append: false, skip: 0 });
      return;
    }
    const t = window.setTimeout(() => {
      void fetchCommits({ append: false, skip: 0 });
    }, 180);
    return () => window.clearTimeout(t);
  }, [fetchCommits, workspacePath, gitRev, search]);

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

  useEffect(() => {
    if (!rootEl) return;
    const ro = new ResizeObserver((entries) => {
      const r = entries[0].contentRect;
      setRootSize((prev) =>
        Math.round(prev.w) === Math.round(r.width) && Math.round(prev.h) === Math.round(r.height)
          ? prev
          : { w: r.width, h: r.height }
      );
    });
    ro.observe(rootEl);
    return () => ro.disconnect();
  }, [rootEl]);

  const lastSavedInspectorRef = useRef(inspectorSize);
  useEffect(() => {
    const prev = lastSavedInspectorRef.current;
    if (prev.w === inspectorSize.w && prev.h === inspectorSize.h) return;
    lastSavedInspectorRef.current = inspectorSize;
    setPref(INSPECTOR_KEY, JSON.stringify(inspectorSize), { debounceMs: 150 });
  }, [inspectorSize]);

  const uncommittedCount = gitStatus?.files.length ?? 0;
  const filtered = search.trim().length > 0;
  const hasWorkingTree = uncommittedCount > 0 && !filtered;
  const layout = useMemo(
    () =>
      computeGitGraphLayout(commits, {
        workingTree: hasWorkingTree,
        currentBranch: gitStatus?.branch,
        flat: filtered,
      }),
    [commits, hasWorkingTree, filtered, gitStatus?.branch]
  );
  const upstreamName = useMemo(() => {
    const up = commits.find((c) => c.hash === layout.upstreamHash);
    const branch = gitStatus?.branch;
    return up?.refs.find(
      (r) => r.kind === 'remote' && branch && r.name.slice(r.name.indexOf('/') + 1) === branch
    )?.name;
  }, [commits, layout.upstreamHash, gitStatus?.branch]);

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

  const selectedCommit = selectedHash ? commits.find((c) => c.hash === selectedHash) : undefined;
  const selectedColor = layout.rows.find((r) => r.commit?.hash === selectedHash)?.circleColor;

  const selectByOffset = (delta: number) => {
    const list = layout.rows.filter((r) => r.kind !== 'working-tree').map((r) => r.commit!.hash);
    if (list.length === 0) return;
    const idx = selectedHash ? list.indexOf(selectedHash) : -1;
    const from = idx === -1 ? (delta > 0 ? -1 : list.length) : idx;
    const next = list[Math.max(0, Math.min(list.length - 1, from + delta))];
    setCompareMode(null);
    setSelectedHash(next);
    requestAnimationFrame(() =>
      document.querySelector(`[data-graph-hash="${next}"]`)?.scrollIntoView({ block: 'nearest' })
    );
  };

  const actionBtn =
    'h-[26px] px-2 text-[11.5px] rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-primary)] flex items-center gap-1.5 cursor-pointer';

  const closeInspector = () => {
    setSelectedHash(null);
    setCompareMode(null);
  };

  const renderInspector = () => {
    const d = detail && detail.hash === selectedHash ? detail : null;
    const fileTotals = compareMode ? null : d ? { add: d.total_additions, del: d.total_deletions } : null;

    return (
      <div className="flex flex-col h-full min-h-0 min-w-0">
        {/* Header */}
        <div className="flex items-center gap-2 h-9 px-3 border-b border-[var(--border)] shrink-0">
          {compareMode ? (
            <>
              <GitCompare size={14} className="text-[var(--accent)]" />
              <span className="text-xs font-semibold truncate">
                {compareMode.commit.short_hash} → Working Tree
              </span>
            </>
          ) : (
            <>
              <GitCommitHorizontal size={14} className="text-[var(--accent)]" />
              <button
                onClick={() =>
                  selectedHash && copyText(selectedHash, `SHA (${selectedHash.slice(0, 7)})`, selectedHash)
                }
                className="font-mono text-xs font-semibold text-[var(--accent)] hover:underline flex items-center gap-1 cursor-pointer"
                title="Copy full SHA"
              >
                {selectedHash?.slice(0, 7)}
                {copiedHash === selectedHash ? (
                  <Check size={11} className="text-[var(--added-fg)]" />
                ) : (
                  <Copy size={11} className="opacity-60" />
                )}
              </button>
              {selectedCommit && selectedCommit.refs.length > 0 && (
                <span className="flex min-w-0 overflow-hidden">
                  <RefBadges
                    refs={selectedCommit.refs}
                    color={selectedColor ?? 'var(--accent)'}
                    labelled={3}
                    maxWidth={120}
                    tooltips={false}
                  />
                </span>
              )}
            </>
          )}
          <button
            onClick={closeInspector}
            className="ml-auto p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
            title="Close (Esc)"
          >
            <X size={13} />
          </button>
        </div>

        {/* One scrolling column: what happened, what you can do, what changed */}
        <div className="flex-1 min-h-0 overflow-y-auto select-text">
          {!compareMode && (
            <div className="px-3.5 pt-3 pb-2.5 space-y-2.5">
              {!d ? (
                <div className="space-y-2 py-1">
                  {[75, 50, 35].map((w, i) => (
                    <div
                      key={i}
                      className="h-3 rounded bg-[var(--bg-hover)] animate-pulse"
                      style={{ width: `${w}%` }}
                    />
                  ))}
                </div>
              ) : (
                <>
                  <h3 className="text-[14px] font-semibold leading-snug text-[var(--text-primary)] break-words">
                    {d.subject}
                  </h3>

                  <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[11.5px] text-[var(--text-muted)]">
                    <span
                      className="w-[18px] h-[18px] rounded-full flex items-center justify-center text-[10px] font-semibold text-white shrink-0"
                      style={{ background: authorColor(d.author) }}
                      aria-hidden="true"
                    >
                      {d.author.trim().charAt(0).toUpperCase() || '?'}
                    </span>
                    <span className="text-[var(--text-primary)] font-medium" title={d.author_email}>
                      {d.author}
                    </span>
                    <span className="text-[var(--text-dim)]">committed</span>
                    <span title={new Date(d.iso_date).toLocaleString()}>{d.date}</span>
                    {d.parents.length > 0 && (
                      <span className="flex items-center gap-1 text-[var(--text-dim)]">
                        · {d.parents.length > 1 ? 'parents' : 'parent'}
                        {d.parents.map((pHash) => (
                          <button
                            key={pHash}
                            onClick={() => {
                              setCompareMode(null);
                              setSelectedHash(pHash);
                            }}
                            className="font-mono text-[11px] text-[var(--accent)] hover:underline cursor-pointer"
                            title="Go to parent commit"
                          >
                            {pHash.slice(0, 7)}
                          </button>
                        ))}
                      </span>
                    )}
                  </div>

                  {d.body && (
                    <div className="text-[12px] leading-relaxed text-[var(--text-muted)] whitespace-pre-wrap break-words border-l-2 border-[var(--border)] pl-2.5 max-h-[220px] overflow-y-auto">
                      {d.body}
                    </div>
                  )}

                  <div className="flex flex-wrap items-center gap-1.5 pt-0.5 select-none">
                    <button
                      onClick={() => selectedCommit && setModal({ kind: 'cherry-pick', commit: selectedCommit })}
                      className={actionBtn}
                      title="Apply this commit onto the current branch"
                    >
                      <GitCommitHorizontal size={12} className="text-[var(--accent)]" />
                      Cherry-Pick
                    </button>
                    <button
                      onClick={() => selectedCommit && setModal({ kind: 'revert', commit: selectedCommit })}
                      className={actionBtn}
                      title="Create a commit that undoes this one"
                    >
                      <Undo2 size={12} />
                      Revert
                    </button>
                    <button
                      onClick={() => selectedCommit && setModal({ kind: 'create-branch', commit: selectedCommit })}
                      className={actionBtn}
                      title="Create a branch at this commit"
                    >
                      <GitBranchPlus size={12} />
                      Branch
                    </button>
                    {remoteHost && (
                      <button
                        onClick={async () => {
                          const url = remoteHost.commitUrl(d.hash);
                          try {
                            await api.openExternalUrl(url);
                          } catch {
                            window.open(url, '_blank', 'noopener');
                          }
                        }}
                        className={`${actionBtn} text-[var(--accent-hover)]`}
                        title={`Open on ${remoteHost.label}`}
                      >
                        <ExternalLink size={12} />
                        {remoteHost.label}
                      </button>
                    )}
                    <button
                      onClick={(e) => selectedCommit && openCommitContextMenu(e, selectedCommit)}
                      className={`${actionBtn} px-1.5`}
                      title="More actions (reset, copy patch, tag, merge...)"
                      aria-label="More commit actions"
                    >
                      <MoreHorizontal size={14} />
                    </button>
                  </div>
                </>
              )}
            </div>
          )}

          {/* Files */}
          <div className="sticky top-0 z-[1] flex items-center gap-2 h-8 px-3.5 border-y border-[var(--border)] bg-[var(--bg-sidebar)] text-[11px] select-none">
            <span className="font-semibold uppercase tracking-wider text-[var(--text-muted)]">
              {activeFiles.length} changed file{activeFiles.length === 1 ? '' : 's'}
            </span>
            {fileTotals && (
              <span className="font-mono tabular-nums">
                <span className="text-[var(--added-fg)]">+{fileTotals.add}</span>{' '}
                <span className="text-[var(--removed-fg)]">−{fileTotals.del}</span>
              </span>
            )}
            <button
              onClick={() => setTreeView((v) => !v)}
              className="ml-auto p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
              title={treeView ? 'Switch to flat list' : 'Switch to folder tree'}
            >
              {treeView ? <List size={13.5} /> : <FolderTree size={13.5} />}
            </button>
          </div>
          <div className="p-1.5">
            {!compareMode && !d ? null : activeFiles.length === 0 ? (
              <EmptyState size="sm" icon={FileDashed} title="No file changes" description="This commit doesn't touch any files." />
            ) : treeView ? (
              renderTreeNodes(fileTree, 0)
            ) : (
              activeFiles.map((f) => {
                const name = f.path.split('/').pop() || f.path;
                const dir = f.path.includes('/') ? f.path.slice(0, f.path.lastIndexOf('/')) : '';
                return (
                  <div
                    key={f.path}
                    onClick={() => handleOpenFileRow(f)}
                    title={f.old_path ? `${f.old_path} → ${f.path}` : f.path}
                    className="flex items-center justify-between gap-2 px-2 py-1 text-xs hover:bg-[var(--bg-hover)] rounded cursor-pointer"
                  >
                    <div className="flex items-center gap-1.5 min-w-0">
                      <FileCode size={13} className="text-[var(--text-dim)] shrink-0" />
                      <span
                        className={`truncate shrink-0 max-w-[70%] ${f.status === 'D' ? 'line-through opacity-75' : ''}`}
                        style={{ color: fileStatusColor(f.status) }}
                      >
                        {name}
                      </span>
                      {dir && <span className="truncate text-[11px] text-[var(--text-dim)]">{dir}</span>}
                    </div>
                    <div className="flex items-center gap-2 shrink-0 font-mono text-[11px] tabular-nums">
                      {f.additions > 0 && <span className="text-[var(--added-fg)]">+{f.additions}</span>}
                      {f.deletions > 0 && <span className="text-[var(--removed-fg)]">-{f.deletions}</span>}
                      <span className="w-3.5 text-center font-bold" style={{ color: fileStatusColor(f.status) }}>
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
    );
  };

  const hasInspector = !!(selectedHash || compareMode);
  const dockSide = rootSize.w >= SIDE_DOCK_MIN_WIDTH;
  const inspectorMaxW = Math.max(INSPECTOR_MIN.w, rootSize.w - LIST_MIN_WIDTH - 4);
  const inspectorMaxH = Math.max(INSPECTOR_MIN.h, Math.min(rootSize.h - 150, rootSize.h * 0.6));
  const inspectorW = Math.max(INSPECTOR_MIN.w, Math.min(inspectorMaxW, inspectorSize.w));
  const inspectorH = Math.max(INSPECTOR_MIN.h, Math.min(inspectorMaxH, inspectorSize.h));

  const graphColWidth = Math.max(
    48,
    graphColumnWidth(layout.maxLanes, FULL_LANE_W, FULL_MAX_LANES) + 4
  );

  const hover = useCommitHover({
    remoteHost,
    upstreamName,
    disabled: !!contextMenu || !!modal,
    onCopy: (text, label) => copyText(text, label),
    onOpenRemote: async (url) => {
      try {
        await api.openExternalUrl(url);
      } catch {
        window.open(url, '_blank', 'noopener');
      }
    },
  });

  return (
    <div
      ref={setRootEl}
      className="@container flex-1 flex flex-col min-h-0 min-w-0 bg-[var(--bg-base)] select-none"
      onKeyDown={(e) => {
        if (e.key === 'Escape' && hasInspector) closeInspector();
      }}
    >
      {/* Top Toolbar */}
      <div className="flex items-center justify-between gap-2 px-3 py-2 border-b border-[var(--border)] bg-[var(--bg-sidebar)] shrink-0">
        <div className="flex items-center gap-2 min-w-0 shrink-0">
          <GitFork size={15} className="text-[var(--accent)] shrink-0" />
          <span className="hidden @lg:inline text-xs font-semibold tracking-wide text-[var(--text-primary)]">
            Git Commit Graph
          </span>
          {gitStatus?.branch && (
            <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-[11px] bg-[var(--bg-elevated)] border border-[var(--border)] text-[var(--text-muted)]">
              <GitBranch size={11} className="text-[var(--accent)]" />
              {gitStatus.branch}
            </span>
          )}
          <span className="hidden @2xl:inline shrink-0 text-[11px] text-[var(--text-dim)] tabular-nums">
            {commits.length} commit{commits.length === 1 ? '' : 's'}
          </span>
        </div>

        <div className="flex items-center gap-1.5 min-w-0 flex-1 justify-end">
          {/* Search input */}
          <div className="relative flex items-center min-w-[110px] flex-1 max-w-[260px]">
            <Search size={12.5} className="absolute left-2.5 text-[var(--text-dim)] pointer-events-none" />
            <input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Filter commits, authors, SHAs..."
              className="w-full pl-7 pr-6 py-1 text-xs rounded-md bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none focus:border-[var(--accent)]"
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
          <div className="flex shrink-0 rounded-md border border-[var(--border)] overflow-hidden text-xs">
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
            <span className="hidden @xl:inline">Fetch</span>
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

      <div className={`flex-1 min-h-0 min-w-0 flex ${dockSide ? 'flex-row' : 'flex-col'}`}>
      <div className="@container flex-1 min-w-0 min-h-0 flex flex-col">
      {/* Commit Rows (column header is sticky inside the scroller so the
          scrollbar never shifts rows out of line with their headings) */}
      <div
        className="flex-1 overflow-y-auto min-h-0 outline-none scroll-pt-8"
        tabIndex={0}
        onKeyDown={(e) => {
          if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
            e.preventDefault();
            selectByOffset(e.key === 'ArrowDown' ? 1 : -1);
          }
        }}
        onScroll={(e) => {
          const el = e.currentTarget;
          if (hasMore && !loading && el.scrollTop + el.clientHeight > el.scrollHeight - 200) {
            fetchCommits({ append: true, skip: commits.length });
          }
        }}
      >
        <div className="sticky top-0 z-[2] flex items-center h-8 px-3 border-b border-[var(--border)] bg-[var(--bg-sidebar)] text-[10.5px] font-semibold uppercase tracking-[0.08em] text-[var(--text-dim)]">
          <div style={{ width: graphColWidth }} className="shrink-0">
            Graph
          </div>
          <div className="flex-1 min-w-0 px-2">Description</div>
          <div className={`${COL_AUTHOR} shadow-[inset_1px_0_0_var(--border)]`}>Author</div>
          <div className={`${COL_DATE} shadow-[inset_1px_0_0_var(--border)]`}>Date</div>
          <div className={`${COL_SHA} shadow-[inset_1px_0_0_var(--border)]`}>Commit</div>
          <div className={COL_ACTIONS} />
        </div>
        {layout.rows.map((row) => {
          const c = row.commit;

          if (row.kind === 'working-tree') {
            return (
              <div
                key={WORKING_TREE_ID}
                onClick={() => {
                  const first = gitStatus?.files[0];
                  if (first) onOpenDiff(first.path, first.staged && !first.unstaged);
                }}
                className="flex items-center h-[28px] px-3 cursor-pointer [--graph-row-bg:var(--bg-base)] hover:[--graph-row-bg:var(--bg-hover)] hover:bg-[var(--bg-hover)]"
              >
                <div style={{ width: graphColWidth }} className="shrink-0 flex items-center">
                  <CommitGraphSvg
                    row={row}
                    maxLanes={layout.maxLanes}
                    laneWidth={FULL_LANE_W}
                    rowHeight={FULL_ROW_H}
                    maxVisibleLanes={FULL_MAX_LANES}
                  />
                </div>
                <div className="flex-1 min-w-0 px-2 flex items-center gap-2">
                  <span className="truncate text-[12.5px] italic font-medium text-[var(--modified-fg)]">
                    Uncommitted Changes
                  </span>
                  <span className="shrink-0 text-[11px] text-[var(--text-dim)] tabular-nums">
                    {uncommittedCount} file{uncommittedCount === 1 ? '' : 's'}
                  </span>
                  <span className="shrink-0 font-mono text-[11px] text-[var(--added-fg)]">
                    +{gitStatus?.total_additions ?? 0}
                  </span>
                  <span className="shrink-0 font-mono text-[11px] text-[var(--removed-fg)]">
                    −{gitStatus?.total_deletions ?? 0}
                  </span>
                </div>
                <div className={`${COL_AUTHOR} text-xs text-[var(--text-dim)]`}>Working Tree</div>
                <div className={`${COL_DATE} text-xs text-[var(--text-dim)]`}>Now</div>
                <div className={COL_SHA} />
                <div className={COL_ACTIONS} />
              </div>
            );
          }

          const isSelected = selectedHash === c.hash;
          return (
            <div
              key={c.hash}
              data-graph-hash={c.hash}
              {...hover.rowProps({
                commit: c,
                color: row.circleColor,
                sync: row.sync,
                isMerge: row.isMerge,
              })}
              onClick={() => {
                setCompareMode(null);
                setSelectedHash(c.hash);
              }}
              onContextMenu={(e) => {
                hover.hide();
                openCommitContextMenu(e, c);
              }}
              className={`group flex items-center h-[28px] px-3 cursor-pointer text-[var(--text-primary)] ${
                isSelected
                  ? '[--graph-row-bg:var(--bg-active)] bg-[var(--bg-active)]'
                  : '[--graph-row-bg:var(--bg-base)] hover:[--graph-row-bg:var(--bg-hover)] hover:bg-[var(--bg-hover)]'
              }`}
            >
              <div style={{ width: graphColWidth }} className="shrink-0 flex items-center">
                <CommitGraphSvg
                  row={row}
                  maxLanes={layout.maxLanes}
                  laneWidth={FULL_LANE_W}
                  rowHeight={FULL_ROW_H}
                  maxVisibleLanes={FULL_MAX_LANES}
                />
              </div>

              {/* Description + Ref labels */}
              <div className="flex-1 min-w-0 px-2 flex items-center gap-1.5 overflow-hidden">
                {c.refs.length > 0 && (
                  <span className="flex min-w-0 max-w-[55%] shrink-0 overflow-hidden">
                    <RefBadges refs={c.refs} color={row.circleColor} labelled={3} maxWidth={160} tooltips={false} />
                  </span>
                )}
                {row.sync && <SyncMark dir={row.sync} />}
                <span
                  className={`truncate text-[12.5px] ${c.is_head ? 'font-semibold' : ''} ${
                    row.sync === 'incoming' ? 'text-[var(--text-muted)]' : ''
                  }`}
                >
                  {c.message}
                </span>
              </div>

              <div className={`${COL_AUTHOR} text-xs text-[var(--text-muted)]`} title={c.author}>
                {c.author}
              </div>

              <div className={`${COL_DATE} text-xs text-[var(--text-dim)]`} title={c.date}>
                {c.date}
              </div>

              <div className={`${COL_SHA} flex items-center`}>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    copyText(c.hash, `SHA (${c.short_hash})`, c.hash);
                  }}
                  className="-ml-1 font-mono text-[11px] tabular-nums px-1 h-5 rounded text-[var(--text-dim)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-elevated)] flex items-center gap-1 cursor-pointer"
                  aria-label="Copy full commit SHA"
                >
                  {copiedHash === c.hash && <Check size={10} className="text-[var(--added-fg)]" />}
                  {c.short_hash}
                </button>
              </div>
              <div className={COL_ACTIONS}>
                <button
                  onClick={(e) => {
                    hover.hide();
                    openCommitContextMenu(e, c);
                  }}
                  className="p-0.5 rounded opacity-0 group-hover:opacity-100 focus-visible:opacity-100 hover:bg-[var(--bg-elevated)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
                  aria-label="Commit actions"
                >
                  <MoreHorizontal size={13} />
                </button>
              </div>
            </div>
          );
        })}

        {commits.length === 0 && !loading && (
          <EmptyState
            size="lg"
            icon={GitCommit}
            title={search ? 'No matching commits' : 'No commits yet'}
            description={
              search ? (
                <>
                  Nothing matches <span className="text-[var(--text-muted)]">“{search}”</span>. Try a message, author or hash.
                </>
              ) : (
                'Commits will appear here once you make your first one.'
              )
            }
            className="py-16"
          />
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

      </div>

      {/* Commit detail: beside the list on wide pages, below it on narrow ones */}
      {hasInspector && (
        <>
          <ResizeHandle
            orientation={dockSide ? 'vertical' : 'horizontal'}
            direction={-1}
            label="Resize commit details"
            value={dockSide ? inspectorW : inspectorH}
            min={dockSide ? INSPECTOR_MIN.w : INSPECTOR_MIN.h}
            max={dockSide ? inspectorMaxW : inspectorMaxH}
            onChange={(n) => setInspectorSize((p) => (dockSide ? { ...p, w: n } : { ...p, h: n }))}
            onReset={() =>
              setInspectorSize((p) =>
                dockSide ? { ...p, w: INSPECTOR_DEFAULT.w } : { ...p, h: INSPECTOR_DEFAULT.h }
              )
            }
          />
          <div
            style={dockSide ? { width: inspectorW } : { height: inspectorH }}
            className={`shrink-0 min-h-0 min-w-0 overflow-hidden bg-[var(--bg-sidebar)] ${
              dockSide ? 'border-l border-[var(--border)]' : 'border-t border-[var(--border)]'
            }`}
          >
            {renderInspector()}
          </div>
        </>
      )}
      </div>

      {hover.card}

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
