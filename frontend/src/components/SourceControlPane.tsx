import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertTriangle,
  Archive,
  BookmarkPlus,
  ArrowUp,
  Check,
  ChevronDown,
  ChevronRight,
  CloudUpload,
  Copy,
  ExternalLink,
  FileCode,
  Folder,
  FolderOpen,
  FolderTree,
  GitBranch,
  GitCommitHorizontal,
  GitCompare,
  GitFork,
  GitMerge,
  History,
  List,
  LocateFixed,
  Maximize2,
  Minus,
  Pencil,
  MoreHorizontal,
  Plus,
  RefreshCw,
  RotateCcw,
  Search,
  Sparkles,
  Loader2,
  Trash2,
  Undo2,
  X,
} from 'lucide-react';
import { api } from '../api';
import type {
  BranchesRes,
  CommitDetailRes,
  CommitFileChange,
  CommitLogItem,
  GitFileStatus,
  GitStatusRes,
  ToastAction,
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
import { CommitGraphSvg, GraphRails, graphColumnWidth } from './CommitGraphSvg';
import { useCommitHover } from './CommitHoverCard';
import { BranchBar } from './BranchBar';
import { isPushRejected, pullThenPush } from '../utils/gitSync';
import { fileStatusColor, RefBadges, SyncMark } from './GitBadges';

function plural(n: number, word: string) {
  return `${n} ${word}${n === 1 ? '' : 's'}`;
}

const CommitMenuItem: React.FC<{
  icon: React.ReactNode;
  label: string;
  hint?: string;
  warn?: boolean;
  disabled?: boolean;
  onClick: () => void;
}> = ({ icon, label, hint, warn, disabled, onClick }) => (
  <button
    onClick={onClick}
    disabled={disabled}
    className="w-full flex items-start gap-2 px-3 py-1.5 text-left hover:bg-[var(--bg-hover)] disabled:opacity-40 disabled:hover:bg-transparent cursor-pointer disabled:cursor-default"
  >
    <span className="mt-[1px] text-[var(--text-muted)]">{icon}</span>
    <span className="min-w-0">
      <span className="block text-[var(--text-primary)]">{label}</span>
      {hint && (
        <span
          className={`block text-[11px] truncate ${
            warn ? 'text-[var(--modified-fg)]' : 'text-[var(--text-dim)]'
          }`}
        >
          {hint}
        </span>
      )}
    </span>
  </button>
);

const GRAPH_ROW_H = 24;
const GRAPH_LANE_W = 12;
const GRAPH_MAX_LANES = 6;
import { GitActionModal, type GitModalState } from './GitActionModal';
import { FileIcon } from './FileIcon';
import { PublishDialog, type PublishMode } from './PublishDialog';

interface SourceControlPaneProps {
  gitStatus: GitStatusRes | null;
  selectedPath: string | null;
  selectedCommitKey: string | null;
  workspacePath: string;
  onOpenDiff: (path: string, staged?: boolean) => void;
  onOpenFile: (path: string, line?: number) => void;
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
  onOpenGraphTab: (selectedHash?: string) => void;
  onRefreshGit: () => Promise<void>;
  onToast: (msg: string, isError?: boolean, action?: ToastAction) => void;
  /** path -> pulse key for files an agent just changed. */
  recentPaths?: Record<string, number>;
}

export const SourceControlPane: React.FC<SourceControlPaneProps> = ({
  gitStatus,
  selectedPath,
  selectedCommitKey,
  workspacePath,
  onOpenDiff,
  onOpenFile,
  onOpenCommitDiff,
  onOpenCompareDiff,
  onOpenGraphTab,
  onRefreshGit,
  onToast,
  recentPaths,
}) => {
  const [commitMsg, setCommitMsg] = useState('');
  const [amendMode, setAmendMode] = useState(false);
  const [showCommitMenu, setShowCommitMenu] = useState(false);
  const [busy, setBusy] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [showMoreMenu, setShowMoreMenu] = useState(false);
  const [publishDialog, setPublishDialog] = useState<PublishMode | null>(null);
  const moreMenuRef = useRef<HTMLDivElement>(null);

  // Branch picker
  const [branchInfo, setBranchInfo] = useState<BranchesRes>({
    current: '',
    branches: [],
    remote_branches: [],
  });
  const [amendPrefill, setAmendPrefill] = useState('');
  const commitInputRef = useRef<HTMLTextAreaElement>(null);
  const commitMenuRef = useRef<HTMLDivElement>(null);

  // Section layout & view modes
  const [treeView, setTreeView] = useState<boolean>(() => {
    try {
      return localStorage.getItem('vireo-scm-tree') !== 'false';
    } catch {
      return true;
    }
  });
  const [collapsedDirs, setCollapsedDirs] = useState<Record<string, boolean>>({});
  const [changesOpen, setChangesOpen] = useState(true);
  const [stagedSubOpen, setStagedSubOpen] = useState(true);
  const [unstagedSubOpen, setUnstagedSubOpen] = useState(true);
  const [graphOpen, setGraphOpen] = useState(true);
  const [stashesOpen, setStashesOpen] = useState(false);
  const [checkpointsOpen, setCheckpointsOpen] = useState(false);
  const [changesRatio, setChangesRatio] = useState(0.46); // split ratio between Changes & Graph
  const [cleanRatio, setCleanRatio] = useState(0.22); // same, when there is nothing to change
  const splitContainerRef = useRef<HTMLDivElement>(null);

  // Changes filter
  const [showChangesFilter, setShowChangesFilter] = useState(false);
  const [changesFilter, setChangesFilter] = useState('');

  // Commit Graph state
  const [commits, setCommits] = useState<CommitLogItem[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [graphLoading, setGraphLoading] = useState(false);
  const [allBranches, setAllBranches] = useState(true);
  const [showGraphSearch, setShowGraphSearch] = useState(false);
  const [graphSearch, setGraphSearch] = useState('');
  const [expandedCommitHash, setExpandedCommitHash] = useState<string | null>(null);
  const [commitDetails, setCommitDetails] = useState<Record<string, CommitDetailRes>>({});
  const [loadingDetailHash, setLoadingDetailHash] = useState<string | null>(null);
  const [compareDrawer, setCompareDrawer] = useState<{
    commit: CommitLogItem;
    files: CommitFileChange[];
  } | null>(null);
  const [copiedHash, setCopiedHash] = useState<string | null>(null);

  // Context menu & modal
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    items: ContextMenuItem[];
  } | null>(null);
  const [modal, setModal] = useState<GitModalState>(null);

  const remoteHost = useMemo(
    () => parseRemoteUrl(gitStatus?.remote_url || ''),
    [gitStatus?.remote_url]
  );

  useEffect(() => {
    try {
      localStorage.setItem('vireo-scm-tree', String(treeView));
    } catch {
      // ignore
    }
  }, [treeView]);

  const fetchBranches = useCallback(() => {
    api
      .gitBranches()
      .then(setBranchInfo)
      .catch(() => setBranchInfo({ current: '', branches: [], remote_branches: [] }));
  }, []);

  const appendInFlight = useRef(false);
  const fetchLog = useCallback(
    async (opts?: { append?: boolean; skip?: number }) => {
      if (opts?.append) {
        if (appendInFlight.current) return;
        appendInFlight.current = true;
      }
      setGraphLoading(true);
      try {
        const res = await api.gitLog({
          limit: 60,
          skip: opts?.skip ?? 0,
          all: allBranches,
          search: graphSearch.trim() || undefined,
        });
        setCommits((prev) => (opts?.append ? appendUniqueCommits(prev, res.commits) : res.commits));
        setHasMore(res.has_more);
      } catch {
        // ignore transient error
      } finally {
        if (opts?.append) appendInFlight.current = false;
        setGraphLoading(false);
      }
    },
    [allBranches, graphSearch]
  );

  useEffect(() => {
    fetchBranches();
  }, [fetchBranches, workspacePath, gitStatus?.branch, gitStatus?.head_hash]);

  useEffect(() => {
    const t = window.setTimeout(() => {
      fetchLog({ append: false, skip: 0 });
    }, graphSearch ? 200 : 0);
    return () => window.clearTimeout(t);
  }, [fetchLog, workspacePath, gitStatus?.branch, gitStatus?.head_hash, graphSearch]);

  const runAction = useCallback(
    async (fn: () => Promise<void>) => {
      setBusy(true);
      try {
        await fn();
        setModal(null);
        await onRefreshGit();
        fetchBranches();
        fetchLog({ append: false, skip: 0 });
      } catch (e: unknown) {
        onToast(e instanceof Error ? e.message : String(e), true);
      } finally {
        setBusy(false);
      }
    },
    [onRefreshGit, fetchBranches, fetchLog, onToast]
  );

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

  const loadCommitDetail = useCallback(
    async (hash: string, force = false) => {
      if (!force && commitDetails[hash]) return commitDetails[hash];
      setLoadingDetailHash(hash);
      try {
        const d = await api.gitCommitDetail(hash);
        setCommitDetails((prev) => ({ ...prev, [hash]: d }));
        return d;
      } catch (e: unknown) {
        onToast(e instanceof Error ? e.message : 'Failed to load commit details', true);
        return null;
      } finally {
        setLoadingDetailHash((cur) => (cur === hash ? null : cur));
      }
    },
    [commitDetails, onToast]
  );

  const toggleCommitExpand = (commit: CommitLogItem) => {
    if (expandedCommitHash === commit.hash) {
      setExpandedCommitHash(null);
    } else {
      setExpandedCommitHash(commit.hash);
      loadCommitDetail(commit.hash);
    }
  };

  // Filtered files
  const allFiles = gitStatus?.files || [];
  const filteredFiles = useMemo(() => {
    const q = changesFilter.trim().toLowerCase();
    if (!q) return allFiles;
    return allFiles.filter((f) => f.path.toLowerCase().includes(q));
  }, [allFiles, changesFilter]);

  const stagedFiles = useMemo(() => filteredFiles.filter((f) => f.staged), [filteredFiles]);
  const unstagedFiles = useMemo(
    () => filteredFiles.filter((f) => f.unstaged || f.untracked),
    [filteredFiles]
  );
  const stagedTree = useMemo(() => buildCompactFileTree(stagedFiles), [stagedFiles]);
  const unstagedTree = useMemo(() => buildCompactFileTree(unstagedFiles), [unstagedFiles]);
  const totalChanged = allFiles.length;
  const allStagedCount = useMemo(() => allFiles.filter((f) => f.staged).length, [allFiles]);
  const isDetached = branchInfo.detached ?? gitStatus?.branch === 'HEAD';
  const hasRemote = (branchInfo.remotes?.length ?? 0) > 0 || !!gitStatus?.remote_url;
  // HEAD is already on the remote: rewriting it (amend / undo) will need a force push.
  const headPushed =
    !!branchInfo.upstream && !branchInfo.upstream_gone && (gitStatus?.ahead ?? 0) === 0;
  const canCommit = amendMode ? !!gitStatus?.head_hash : totalChanged > 0;

  useEffect(() => {
    if (!showCommitMenu) return;
    const onDown = (e: MouseEvent) => {
      if (!commitMenuRef.current?.contains(e.target as Node)) setShowCommitMenu(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setShowCommitMenu(false);
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [showCommitMenu]);

  useEffect(() => {
    if (!showMoreMenu) return;
    const onDown = (e: MouseEvent) => {
      if (!moreMenuRef.current?.contains(e.target as Node)) setShowMoreMenu(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setShowMoreMenu(false);
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [showMoreMenu]);

  const folderName = useMemo(
    () => workspacePath.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || 'my-project',
    [workspacePath]
  );
  const hasCommits = !!gitStatus?.head_hash;

  const handleInitRepo = () =>
    runAction(async () => {
      const res = await api.gitInit();
      if (!res.ok) throw new Error(res.stderr || res.stdout || 'Failed to initialize repository');
      onToast('Initialized empty Git repository');
    });

  const handlePublishGithub = (name: string, isPrivate: boolean, description: string) =>
    runAction(async () => {
      const res = await api.gitPublishGithub(name, isPrivate, description);
      if (!res.ok) throw new Error((res.stderr || res.stdout || 'Publish failed').trim());
      setPublishDialog(null);
      const url = res.stdout.match(/https:\/\/github\.com\/\S+/)?.[0];
      onToast(url ? `Published to ${url}` : `Published ${name} to GitHub`);
    });

  const handleAddRemote = (url: string) =>
    runAction(async () => {
      const res = await api.gitRemoteAdd(url);
      if (!res.ok) throw new Error((res.stderr || res.stdout || 'Failed to add remote').trim());
      setPublishDialog(null);
      onToast('Remote added. Use Publish Branch to push.');
    });

  const graphFiltered = graphSearch.trim().length > 0;
  const graphLayout = useMemo(
    () =>
      computeGitGraphLayout(commits, {
        workingTree: totalChanged > 0 && !graphFiltered,
        currentBranch: gitStatus?.branch,
        flat: graphFiltered,
      }),
    [commits, totalChanged, graphFiltered, gitStatus?.branch]
  );
  const graphColW = graphColumnWidth(graphLayout.maxLanes, GRAPH_LANE_W, GRAPH_MAX_LANES);
  const upstreamName = useMemo(() => {
    const up = commits.find((c) => c.hash === graphLayout.upstreamHash);
    const branch = gitStatus?.branch;
    return up?.refs.find(
      (r) => r.kind === 'remote' && branch && r.name.slice(r.name.indexOf('/') + 1) === branch
    )?.name;
  }, [commits, graphLayout.upstreamHash, gitStatus?.branch]);

  const graphScrollRef = useRef<HTMLDivElement>(null);
  const onGraphScroll = (e: React.UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    if (hasMore && !graphLoading && el.scrollTop + el.clientHeight > el.scrollHeight - 160) {
      fetchLog({ append: true, skip: commits.length });
    }
  };
  const revealHead = () => {
    const head = graphLayout.headHash;
    const el = head
      ? graphScrollRef.current?.querySelector<HTMLElement>(`[data-hash="${head}"]`)
      : null;
    if (el) el.scrollIntoView({ block: 'center', behavior: 'smooth' });
    else onToast('HEAD is not in the loaded history', true);
  };

  const commitHover = useCommitHover({
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

  const handleGenerateMessage = async () => {
    if (generating) return;
    setGenerating(true);
    try {
      const res = await api.gitGenerateMessage(amendMode);
      if (res.ok) {
        setCommitMsg(res.message);
        commitInputRef.current?.focus();
      } else if (res.code === 'copilot_missing' || res.code === 'copilot_unauthenticated') {
        onToast(`Set up GitHub Copilot CLI to generate messages. ${res.error}`, true);
      } else {
        onToast(res.error || 'Could not generate a commit message', true);
      }
    } catch (e) {
      onToast(e instanceof Error ? e.message : 'Could not generate a commit message', true);
    } finally {
      setGenerating(false);
    }
  };

  const handleCommit = (andPush: boolean, forceAmend?: boolean) => {
    const useAmend = forceAmend ?? amendMode;
    if (!commitMsg.trim()) {
      setShowCommitMenu(false);
      commitInputRef.current?.focus();
      onToast('Write a commit message first.', true);
      return;
    }
    setShowCommitMenu(false);
    runAction(async () => {
      // Staged counts come from ALL files, never the filtered Changes view.
      const stageAll = allStagedCount === 0 && !useAmend;
      const res = await api.gitCommit(commitMsg.trim(), stageAll, useAmend);
      if (!res.ok) {
        throw new Error(res.stderr || res.stdout || 'Commit failed');
      }
      setCommitMsg('');
      setAmendMode(false);
      if (andPush) {
        const pushRes = await api.gitPush(false);
        if (!pushRes.ok) {
          if (pushRes.stderr.includes('has no upstream')) {
            const retry = await api.gitPush(true);
            if (!retry.ok) throw new Error(retry.stderr || 'Push failed');
          } else if (isPushRejected(pushRes.stderr || pushRes.stdout)) {
            onToast('Committed, but push rejected: the remote has commits you don’t have.', true, {
              label: 'Pull (rebase) & push',
              run: () =>
                void runAction(async () => {
                  await pullThenPush('rebase');
                  onToast('Rebased onto remote and pushed');
                }),
            });
            return;
          } else {
            throw new Error(pushRes.stderr || 'Push failed');
          }
        }
        onToast('Committed and pushed to remote!');
      } else {
        onToast(useAmend ? 'Amended last commit!' : 'Committed successfully!');
      }
    });
  };

  /** Full message (subject + body) of the checked-out commit — not row 0 of the graph. */
  const loadHeadMessage = async (): Promise<string> => {
    if (!gitStatus?.head_hash) return '';
    try {
      const d = await api.gitCommitDetail(gitStatus.head_hash);
      return d.body ? `${d.subject}\n\n${d.body}` : d.subject;
    } catch {
      return commits.find((c) => c.is_head)?.message ?? '';
    }
  };

  const toggleAmend = async () => {
    setShowCommitMenu(false);
    if (amendMode) {
      setAmendMode(false);
      if (commitMsg === amendPrefill) setCommitMsg('');
      return;
    }
    setAmendMode(true);
    if (!commitMsg.trim()) {
      const msg = await loadHeadMessage();
      setAmendPrefill(msg);
      setCommitMsg(msg);
    }
    commitInputRef.current?.focus();
  };

  const handleUndoLastCommit = () => {
    setShowCommitMenu(false);
    runAction(async () => {
      const msg = await loadHeadMessage();
      const res = await api.gitReset('HEAD~1', 'soft');
      if (!res.ok) throw new Error(res.stderr || 'Undo last commit failed');
      if (msg && !commitMsg.trim()) setCommitMsg(msg);
      onToast('Undid last commit (changes kept staged).');
    });
  };

  const startSplitDrag = (e: React.MouseEvent) => {
    e.preventDefault();
    const container = splitContainerRef.current;
    if (!container) return;
    const rect = container.getBoundingClientRect();
    const onMove = (ev: MouseEvent) => {
      const rel = (ev.clientY - rect.top) / Math.max(1, rect.height);
      if (totalChanged > 0) setChangesRatio(Math.max(0.18, Math.min(0.82, rel)));
      else setCleanRatio(Math.max(0.1, Math.min(0.82, rel)));
    };
    const onUp = () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  };

  const openFileContextMenu = (
    e: React.MouseEvent,
    f: GitFileStatus,
    isStagedSection: boolean
  ) => {
    e.preventDefault();
    e.stopPropagation();
    const items: ContextMenuItem[] = [
      {
        id: 'open-diff',
        label: 'Open Changes (Diff)',
        icon: <GitCompare size={13.5} />,
        onClick: () => onOpenDiff(f.path, isStagedSection),
      },
      {
        id: 'open-file',
        label: 'Open File in Editor',
        icon: <FileCode size={13.5} />,
        onClick: () => onOpenFile(f.path),
      },
      {
        id: 'stage-toggle',
        label: isStagedSection ? 'Unstage Changes' : 'Stage Changes',
        icon: isStagedSection ? <Minus size={13.5} /> : <Plus size={13.5} />,
        dividerBefore: true,
        onClick: () =>
          runAction(async () => {
            if (isStagedSection) await api.gitUnstage({ paths: [f.path] });
            else await api.gitStage({ paths: [f.path] });
          }),
      },
      {
        id: 'copy-path',
        label: 'Copy Relative Path',
        icon: <Copy size={13.5} />,
        dividerBefore: true,
        onClick: () => copyText(f.path, 'relative path'),
      },
    ];
    if (!isStagedSection) {
      items.push({
        id: 'discard',
        label: f.untracked ? 'Delete Untracked File...' : 'Discard Changes...',
        icon: <RotateCcw size={13.5} />,
        danger: true,
        dividerBefore: true,
        onClick: () => setModal({ kind: 'discard-file', path: f.path, untracked: f.untracked }),
      });
    }
    setContextMenu({ x: e.clientX, y: e.clientY, items });
  };

  const openCommitContextMenu = (e: React.MouseEvent, commit: CommitLogItem) => {
    e.preventDefault();
    e.stopPropagation();
    const items = buildCommitContextMenuItems(commit, remoteHost, {
      onInspect: (c) => {
        setExpandedCommitHash(c.hash);
        loadCommitDetail(c.hash);
      },
      onCompareWithWorkingTree: async (c) => {
        try {
          const files = await api.gitCompareFiles(c.hash);
          setCompareDrawer({ commit: c, files });
          if (files.length === 0) {
            onToast(`Working tree matches ${c.short_hash}`);
          }
        } catch (err: unknown) {
          onToast(err instanceof Error ? err.message : 'Compare failed', true);
        }
      },
      onCopyFullHash: (c) => copyText(c.hash, `SHA (${c.short_hash})`, c.hash),
      onCopyShortHash: (c) => copyText(c.short_hash, `short SHA (${c.short_hash})`),
      onCopyMessage: async (c) => {
        const d = await loadCommitDetail(c.hash);
        const full = d?.body ? `${d.subject}\n\n${d.body}` : c.message;
        copyText(full, 'commit message');
      },
      onCopySummary: (c) =>
        copyText(`${c.short_hash} - ${c.message} (${c.author}, ${c.date})`, 'commit summary'),
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
      onReset: (c) =>
        setModal({ kind: 'reset', commit: c, currentBranch: gitStatus?.branch || 'HEAD' }),
      onCheckoutCommit: (c) => setModal({ kind: 'checkout-commit', commit: c }),
      onCreateBranch: (c) => setModal({ kind: 'create-branch', commit: c }),
      onCreateTag: (c) => setModal({ kind: 'create-tag', commit: c }),
      onMergeCommit: (c) =>
        setModal({ kind: 'merge-commit', commit: c, currentBranch: gitStatus?.branch || 'HEAD' }),
    });
    setContextMenu({ x: e.clientX, y: e.clientY, items });
  };

  const renderFileRow = (
    f: GitFileStatus,
    isStagedSection: boolean,
    depth = 0,
    showDir = true
  ) => {
    const fileName = f.path.split('/').pop() || f.path;
    const dirPart = f.path.includes('/') ? f.path.slice(0, f.path.lastIndexOf('/')) : '';
    const badge = f.untracked
      ? 'U'
      : isStagedSection
      ? f.index_status
      : f.worktree_status;
    const color = fileStatusColor(badge);
    const isSelected = selectedPath === f.path;

    return (
      <div
        key={`${isStagedSection ? 's' : 'u'}-${f.path}`}
        onClick={() => onOpenDiff(f.path, isStagedSection)}
        onContextMenu={(e) => openFileContextMenu(e, f, isStagedSection)}
        style={{ paddingLeft: `${10 + depth * 14}px` }}
        title={f.path}
        className={`group relative flex items-center justify-between pr-2 py-[5px] text-[12.5px] cursor-pointer gap-1.5 transition-colors ${
          isSelected
            ? 'bg-[var(--bg-active)] font-medium'
            : 'hover:bg-[var(--bg-hover)]'
        } ${recentPaths?.[f.path] !== undefined ? `row-pulse-${recentPaths[f.path] % 2}` : ''}`}
      >
        {isSelected && (
          <span className="absolute left-0 top-0.5 bottom-0.5 w-[2.5px] bg-[var(--accent)] rounded-r" />
        )}

        <div className="min-w-0 flex-1 flex items-center gap-1.5">
          <FileIcon path={f.path} size={14} />
          <span
            className={`truncate ${badge === 'D' ? 'line-through opacity-75' : ''}`}
            style={{ color }}
          >
            {fileName}
          </span>
          {showDir && dirPart && (
            <span className="truncate text-[11px] text-[var(--text-dim)]">{dirPart}</span>
          )}
        </div>

        {/* Stats (idle) / Hover Actions (VS Code style) + Status Badge */}
        <div className="flex items-center gap-1 shrink-0">
          <div className="flex items-center gap-1 group-hover:hidden font-mono text-[10.5px] tabular-nums">
            {f.additions > 0 && (
              <span className="text-[var(--added-fg)]">+{f.additions}</span>
            )}
            {f.deletions > 0 && (
              <span className="text-[var(--removed-fg)]">-{f.deletions}</span>
            )}
          </div>

          <div className="hidden group-hover:flex items-center gap-0.5">
            <button
              onClick={(e) => {
                e.stopPropagation();
                onOpenFile(f.path);
              }}
              className="p-0.5 hover:bg-[var(--bg-elevated)] text-[var(--text-muted)] hover:text-[var(--text-primary)] rounded cursor-pointer"
              title="Open File"
            >
              <FileCode size={12.5} />
            </button>
            {isStagedSection ? (
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  runAction(async () => {
                    await api.gitUnstage({ paths: [f.path] });
                  });
                }}
                className="p-0.5 hover:bg-[var(--bg-elevated)] text-[var(--text-muted)] hover:text-[var(--text-primary)] rounded cursor-pointer"
                title="Unstage Changes"
              >
                <Minus size={12.5} />
              </button>
            ) : (
              <>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    setModal({ kind: 'discard-file', path: f.path, untracked: f.untracked });
                  }}
                  className="p-0.5 hover:bg-[var(--bg-elevated)] text-[var(--removed-fg)] rounded cursor-pointer"
                  title={f.untracked ? 'Delete Untracked File' : 'Discard Changes'}
                >
                  <RotateCcw size={12} />
                </button>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    runAction(async () => {
                      await api.gitStage({ paths: [f.path] });
                    });
                  }}
                  className="p-0.5 hover:bg-[var(--bg-elevated)] text-[var(--added-fg)] rounded cursor-pointer"
                  title="Stage Changes"
                >
                  <Plus size={12.5} />
                </button>
              </>
            )}
          </div>

          <span
            className="font-mono text-[11px] font-bold w-4 text-center shrink-0"
            style={{ color }}
            title={
              badge === 'M'
                ? 'Modified'
                : badge === 'A'
                ? 'Added'
                : badge === 'U'
                ? 'Untracked'
                : badge === 'D'
                ? 'Deleted'
                : badge
            }
          >
            {badge}
          </span>
        </div>
      </div>
    );
  };

  const renderFileTree = (
    nodes: Array<FileTreeNode<GitFileStatus>>,
    isStagedSection: boolean,
    depth = 0
  ): React.ReactNode =>
    nodes.map((node) => {
      if (node.kind === 'folder') {
        const key = `${isStagedSection ? 's' : 'u'}-${node.path}`;
        const collapsed = !!collapsedDirs[key];
        return (
          <div key={`dir-${key}`}>
            <div
              onClick={() =>
                setCollapsedDirs((prev) => ({ ...prev, [key]: !prev[key] }))
              }
              style={{ paddingLeft: `${8 + depth * 14}px` }}
              className="flex items-center gap-1.5 pr-2 py-1 text-[12px] text-[var(--text-muted)] hover:bg-[var(--bg-hover)] cursor-pointer select-none"
            >
              {collapsed ? (
                <ChevronRight size={13} className="shrink-0 opacity-75" />
              ) : (
                <ChevronDown size={13} className="shrink-0 opacity-75" />
              )}
              {collapsed ? (
                <Folder size={13} className="shrink-0 text-[var(--accent)] opacity-85" />
              ) : (
                <FolderOpen size={13} className="shrink-0 text-[var(--accent)] opacity-85" />
              )}
              <span className="truncate">{node.name}</span>
              <span className="ml-auto text-[10px] text-[var(--text-dim)] tabular-nums">
                {node.fileCount}
              </span>
            </div>
            {!collapsed && renderFileTree(node.children, isStagedSection, depth + 1)}
          </div>
        );
      }
      return renderFileRow(node.item, isStagedSection, depth, false);
    });

  // Clean tree: hide the commit box entirely (VS Code style); the BranchBar shows the next sync step.
  const showCommitBox = totalChanged > 0 || amendMode || commitMsg.trim().length > 0;
  const firstLineLen = commitMsg.split('\n')[0].length;
  const commitBtnLabel = amendMode
    ? 'Amend Last Commit'
    : allStagedCount > 0
      ? `Commit ${plural(allStagedCount, 'staged file')}`
      : totalChanged > 0
        ? `Commit all ${plural(totalChanged, 'change')}`
        : 'Nothing to commit';
  const unstagedLeftOut = allFiles.filter((f) => f.unstaged || f.untracked).length;
  const commitScopeHint = amendMode
    ? allStagedCount > 0
      ? `Adds ${plural(allStagedCount, 'staged file')} to the last commit`
      : 'Only the message will change'
    : allStagedCount > 0 && unstagedLeftOut > 0
      ? `${plural(unstagedLeftOut, 'unstaged change')} won’t be included`
      : '';

  return (
    <div className="flex-1 flex flex-col min-h-0 overflow-hidden select-none">
      <PublishDialog
        open={publishDialog !== null}
        initialMode={publishDialog ?? 'github'}
        defaultName={folderName}
        isRepo={!!gitStatus?.is_repo}
        hasCommits={hasCommits}
        busy={busy}
        onClose={() => setPublishDialog(null)}
        onPublishGithub={handlePublishGithub}
        onAddRemote={handleAddRemote}
      />
      {/* Top Header */}
      <div className="flex items-center justify-between h-10 px-3 border-b border-[var(--border)] shrink-0">
        <span className="text-[11px] font-semibold uppercase tracking-wider text-[var(--text-muted)]">
          Source Control
        </span>
        <div className="flex items-center gap-0.5">
          <button
            onClick={() => setTreeView((v) => !v)}
            className="p-1.5 rounded-md hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
            title={treeView ? 'View Changes as Flat List' : 'View Changes as Compact Tree'}
          >
            {treeView ? <List size={14} /> : <FolderTree size={14} />}
          </button>
          <button
            onClick={() => onOpenGraphTab()}
            className="p-1.5 rounded-md hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
            title="Open Full Git Commit Graph"
          >
            <GitFork size={14} />
          </button>
          <button
            onClick={() =>
              runAction(async () => {
                await api.gitCheckpoint('Manual snapshot');
                onToast('Checkpoint saved!');
              })
            }
            disabled={busy || !gitStatus?.is_repo}
            className="p-1.5 rounded-md hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] disabled:opacity-40 cursor-pointer"
            title="Save a checkpoint (snapshot of all files, without committing)"
          >
            <BookmarkPlus size={14} />
          </button>
          <button
            onClick={() => {
              onRefreshGit();
              fetchBranches();
              fetchLog({ append: false, skip: 0 });
            }}
            className="p-1.5 rounded-md hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
            title="Refresh Git status & graph"
          >
            <RefreshCw size={13.5} className={graphLoading ? 'animate-spin' : ''} />
          </button>
          {gitStatus?.is_repo && (
            <div ref={moreMenuRef} className="relative">
              <button
                onClick={() => setShowMoreMenu((v) => !v)}
                className={`p-1.5 rounded-md hover:bg-[var(--bg-hover)] cursor-pointer ${
                  showMoreMenu
                    ? 'text-[var(--text-primary)] bg-[var(--bg-hover)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                }`}
                title="More actions"
                aria-label="More actions"
              >
                <MoreHorizontal size={14} />
              </button>
              {showMoreMenu && (
                <div className="absolute right-0 top-full mt-1 w-60 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-[0_12px_32px_-8px_rgba(0,0,0,0.6)] z-40 py-1 text-[12px]">
                  {!hasRemote && (
                    <>
                      <CommitMenuItem
                        icon={<CloudUpload size={13} />}
                        label="Publish to GitHub…"
                        hint="Create a private or public repository"
                        onClick={() => {
                          setShowMoreMenu(false);
                          setPublishDialog('github');
                        }}
                      />
                      <CommitMenuItem
                        icon={<GitBranch size={13} />}
                        label="Add Remote…"
                        hint="Use an existing repository URL"
                        onClick={() => {
                          setShowMoreMenu(false);
                          setPublishDialog('remote');
                        }}
                      />
                      <div className="my-1 border-t border-[var(--border)]" />
                    </>
                  )}
                  <CommitMenuItem
                    icon={<Pencil size={13} />}
                    label={amendMode ? 'Stop Amending' : 'Amend Last Commit'}
                    hint={headPushed && !amendMode ? 'Already pushed — will need force push' : undefined}
                    warn={!amendMode && headPushed}
                    disabled={!gitStatus.head_hash}
                    onClick={() => {
                      setShowMoreMenu(false);
                      toggleAmend();
                    }}
                  />
                  <CommitMenuItem
                    icon={<Undo2 size={13} />}
                    label="Undo Last Commit"
                    hint={headPushed ? 'Already pushed — will need force push' : 'Keeps its changes staged'}
                    warn={headPushed}
                    disabled={!gitStatus.head_hash}
                    onClick={() => {
                      setShowMoreMenu(false);
                      handleUndoLastCommit();
                    }}
                  />
                  <div className="my-1 border-t border-[var(--border)]" />
                  <CommitMenuItem
                    icon={<Archive size={13} />}
                    label="Stash All Changes"
                    hint="Includes untracked files"
                    disabled={totalChanged === 0}
                    onClick={() => {
                      setShowMoreMenu(false);
                      runAction(async () => {
                        const res = await api.gitStash('push', {
                          message: `On ${isDetached ? 'detached HEAD' : gitStatus.branch}`,
                        });
                        if (!res.ok) throw new Error(res.stderr || 'Stash failed');
                        onToast('Stashed working changes');
                      });
                    }}
                  />
                  <CommitMenuItem
                    icon={<History size={13} />}
                    label="Pop Latest Stash"
                    disabled={(gitStatus.stashes?.length ?? 0) === 0}
                    onClick={() => {
                      setShowMoreMenu(false);
                      runAction(async () => {
                        const res = await api.gitStash('pop');
                        if (!res.ok) throw new Error(res.stderr || 'Stash pop failed');
                        onToast('Popped latest stash');
                      });
                    }}
                  />
                </div>
              )}
            </div>
          )}
        </div>
      </div>

      {!gitStatus?.is_repo ? (
        <div className="p-3.5 space-y-3">
          <p className="text-[12px] text-[var(--text-muted)] leading-relaxed">
            The folder currently open doesn’t have a Git repository. Initialize one to enable source control.
          </p>
          <button
            onClick={handleInitRepo}
            disabled={busy}
            className="w-full h-[28px] flex items-center justify-center gap-1.5 text-[12.5px] font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:brightness-110 disabled:opacity-50 cursor-pointer disabled:cursor-not-allowed"
          >
            <GitBranch size={13} />
            Initialize Repository
          </button>
          <button
            onClick={() => setPublishDialog('github')}
            disabled={busy}
            className="w-full h-[28px] flex items-center justify-center gap-1.5 text-[12.5px] font-medium rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] disabled:opacity-50 cursor-pointer disabled:cursor-not-allowed"
          >
            <CloudUpload size={13} />
            Publish to GitHub
          </button>
        </div>
      ) : (
        <>
          {/* In-progress Git Operation / Conflict Banner */}
          {gitStatus.state && (
            <div className="px-2.5 py-2 bg-amber-500/15 border-b border-amber-500/35 shrink-0 space-y-1.5">
              <div className="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wide text-amber-400">
                <AlertTriangle size={13} className="shrink-0" />
                <span>{gitStatus.state} in progress</span>
              </div>
              <div className="flex gap-1.5">
                <button
                  disabled={busy}
                  onClick={() =>
                    runAction(async () => {
                      const res = await api.gitOperation(gitStatus.state!, 'continue');
                      if (!res.ok) throw new Error(res.stderr || 'Continue failed');
                      onToast(`Continued ${gitStatus.state}`);
                    })
                  }
                  className="flex-1 py-1 text-[11px] font-medium rounded bg-amber-500 text-black hover:opacity-90 cursor-pointer"
                >
                  Continue
                </button>
                <button
                  disabled={busy}
                  onClick={() =>
                    runAction(async () => {
                      const res = await api.gitOperation(gitStatus.state!, 'abort');
                      if (!res.ok) throw new Error(res.stderr || 'Abort failed');
                      onToast(`Aborted ${gitStatus.state}`);
                    })
                  }
                  className="flex-1 py-1 text-[11px] font-medium rounded border border-amber-500/50 text-amber-300 hover:bg-amber-500/20 cursor-pointer"
                >
                  Abort
                </button>
              </div>
            </div>
          )}

          {/* Branch Bar + Commit Box */}
          <div className="px-2.5 pt-2 pb-2.5 border-b border-[var(--border)] shrink-0">
            <BranchBar
              gitStatus={gitStatus}
              branchInfo={branchInfo}
              busy={busy}
              uncommittedCount={totalChanged}
              runAction={runAction}
              onToast={onToast}
              onRefreshBranches={fetchBranches}
              onOpenPublish={setPublishDialog}
              largeAction={!showCommitBox}
            />

            {showCommitBox && (
            <>
            {/* Amend mode banner: makes the mode obvious and easy to leave */}
            {amendMode && (
              <div
                className={`mt-1.5 flex items-center gap-1.5 px-2 h-[24px] rounded-t-md border border-b-0 text-[11px] ${
                  headPushed
                    ? 'border-[var(--modified-fg)]/50 bg-[var(--modified-fg)]/10 text-[var(--modified-fg)]'
                    : 'border-[var(--accent)]/50 bg-[var(--accent)]/10 text-[var(--accent-hover)]'
                }`}
              >
                {headPushed ? <AlertTriangle size={11} className="shrink-0" /> : <Pencil size={11} className="shrink-0" />}
                <span className="truncate">
                  {headPushed ? 'Amending a pushed commit — needs force push' : 'Amending the last commit'}
                </span>
                <button
                  onClick={() => {
                    setAmendMode(false);
                    if (commitMsg === amendPrefill) setCommitMsg('');
                  }}
                  className="ml-auto shrink-0 p-0.5 rounded hover:bg-[var(--bg-hover)] cursor-pointer"
                  aria-label="Stop amending"
                  title="Stop amending"
                >
                  <X size={11} />
                </button>
              </div>
            )}

            {/* Commit Message Box */}
            <div className={`relative ${amendMode ? '' : 'mt-1.5'}`}>
              <textarea
                ref={commitInputRef}
                value={commitMsg}
                onChange={(e) => setCommitMsg(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
                    e.preventDefault();
                    handleCommit(false);
                  }
                }}
                rows={commitMsg.includes('\n') ? 4 : 2}
                aria-label="Commit message"
                className={`block w-full pl-2.5 pr-8 py-1.5 text-[12.5px] bg-[var(--bg-input)] border text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none resize-none leading-relaxed ${
                  amendMode
                    ? `rounded-b-md ${headPushed ? 'border-[var(--modified-fg)]/50' : 'border-[var(--accent)]/50'} focus:border-[var(--accent)]`
                    : 'rounded-md border-[var(--border)] focus:border-[var(--accent)]'
                }`}
              />
              {!commitMsg && (
                <div className="absolute left-[11px] right-8 top-[7px] truncate pointer-events-none text-[12.5px] leading-relaxed text-[var(--text-dim)]">
                  {isDetached
                    ? 'Message (Ctrl+Enter to commit on detached HEAD)'
                    : `Message (Ctrl+Enter to commit on “${gitStatus.branch}”)`}
                </div>
              )}
              <button
                onClick={handleGenerateMessage}
                disabled={generating || busy || !canCommit}
                className="absolute right-1.5 top-1.5 p-1 rounded text-[var(--text-dim)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-[var(--text-dim)] cursor-pointer disabled:cursor-not-allowed"
                aria-label="Generate commit message"
                title={generating ? 'Generating…' : 'Generate commit message with GitHub Copilot CLI'}
              >
                {generating ? <Loader2 size={13} className="animate-spin" /> : <Sparkles size={13} />}
              </button>
              {firstLineLen > 50 && (
                <span
                  className={`absolute right-2 bottom-1.5 text-[10px] font-mono tabular-nums pointer-events-none ${
                    firstLineLen > 72 ? 'text-[var(--removed-fg)]' : 'text-[var(--text-dim)]'
                  }`}
                  title="Keep the first line (summary) under 72 characters"
                >
                  {firstLineLen}/72
                </span>
              )}
            </div>

            {/* What will be committed */}
            {commitScopeHint && (
              <div className="mt-1 px-0.5 text-[11px] text-[var(--text-dim)] truncate" title={commitScopeHint}>
                {commitScopeHint}
              </div>
            )}

            {/* Split Commit Button */}
            <div className="relative flex items-stretch h-[28px] mt-1.5" ref={commitMenuRef}>
              <button
                onClick={() => handleCommit(false)}
                disabled={busy || !canCommit}
                title={
                  !canCommit
                    ? 'Nothing to commit'
                    : !commitMsg.trim()
                      ? 'Write a commit message first'
                      : `${commitBtnLabel} (Ctrl+Enter)`
                }
                className="flex-1 min-w-0 px-2.5 text-[12.5px] font-medium rounded-l-md bg-[var(--accent)] text-[var(--accent-fg)] hover:brightness-110 disabled:opacity-40 disabled:hover:brightness-100 cursor-pointer disabled:cursor-not-allowed flex items-center justify-center gap-1.5"
              >
                {amendMode ? <Pencil size={12.5} className="shrink-0" /> : <Check size={13.5} className="shrink-0" />}
                <span className="truncate">{commitBtnLabel}</span>
              </button>
              <button
                onClick={() => setShowCommitMenu((v) => !v)}
                disabled={busy}
                className="px-1.5 rounded-r-md bg-[var(--accent)] text-[var(--accent-fg)] shadow-[inset_1px_0_0_rgba(0,0,0,0.22)] hover:brightness-110 disabled:opacity-40 cursor-pointer flex items-center justify-center"
                aria-label="More commit actions"
                title="More commit actions"
              >
                <ChevronDown size={13} />
              </button>

              {showCommitMenu && (
                <div className="absolute right-0 top-full mt-1 w-60 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-[0_12px_32px_-8px_rgba(0,0,0,0.6)] z-40 py-1 text-[12px]">
                  <CommitMenuItem
                    icon={<ArrowUp size={13} />}
                    label={hasRemote ? 'Commit & Push' : 'Commit & Push (no remote)'}
                    disabled={!canCommit || !hasRemote || isDetached}
                    onClick={() => handleCommit(true)}
                  />
                  <CommitMenuItem
                    icon={<Pencil size={13} />}
                    label={amendMode ? 'Stop Amending' : 'Amend Last Commit'}
                    hint={
                      amendMode
                        ? undefined
                        : headPushed
                          ? 'Already pushed — will need force push'
                          : 'Edit the message or add staged changes'
                    }
                    warn={!amendMode && headPushed}
                    disabled={!gitStatus.head_hash}
                    onClick={toggleAmend}
                  />
                  <CommitMenuItem
                    icon={<Undo2 size={13} />}
                    label="Undo Last Commit"
                    hint={
                      headPushed
                        ? 'Already pushed — will need force push'
                        : 'Keeps its changes staged'
                    }
                    warn={headPushed}
                    disabled={!gitStatus.head_hash}
                    onClick={handleUndoLastCommit}
                  />
                  <div className="my-1 border-t border-[var(--border)]" />
                  <CommitMenuItem
                    icon={<Archive size={13} />}
                    label="Stash All Changes"
                    hint="Includes untracked files"
                    disabled={totalChanged === 0}
                    onClick={() => {
                      setShowCommitMenu(false);
                      runAction(async () => {
                        const res = await api.gitStash('push', {
                          message: `On ${isDetached ? 'detached HEAD' : gitStatus.branch}`,
                        });
                        if (!res.ok) throw new Error(res.stderr || 'Stash failed');
                        onToast('Stashed working changes');
                      });
                    }}
                  />
                  {(gitStatus.stashes?.length ?? 0) > 0 && (
                    <CommitMenuItem
                      icon={<History size={13} />}
                      label="Pop Latest Stash"
                      onClick={() => {
                        setShowCommitMenu(false);
                        runAction(async () => {
                          const res = await api.gitStash('pop');
                          if (!res.ok) throw new Error(res.stderr || 'Stash pop failed');
                          onToast('Popped latest stash');
                        });
                      }}
                    />
                  )}
                </div>
              )}
            </div>
            </>
            )}
          </div>

          {/* Resizable Split between CHANGES and GRAPH (VS Code Accordion style) */}
          <div ref={splitContainerRef} className="flex-1 flex flex-col min-h-0 overflow-hidden">
            {/* SECTION 1: CHANGES (omitted entirely when the tree is clean) */}
            {totalChanged > 0 && (
            <div
              style={
                changesOpen && graphOpen && totalChanged > 0
                  ? { flex: `0 0 ${Math.round(changesRatio * 100)}%` }
                  : changesOpen && totalChanged > 0
                  ? { flex: '1 1 auto' }
                  : { flex: '0 0 auto' }
              }
              className="flex flex-col min-h-0 overflow-hidden"
            >
              {/* Changes Accordion Header */}
              <div
                onClick={() => setChangesOpen((v) => !v)}
                className="group flex items-center justify-between h-7 px-2 bg-[var(--bg-sidebar)] border-b border-[var(--border)]/70 hover:bg-[var(--bg-hover)]/60 cursor-pointer shrink-0"
              >
                <div className="flex items-center gap-1 min-w-0">
                  {changesOpen ? (
                    <ChevronDown size={13} className="text-[var(--text-muted)] shrink-0" />
                  ) : (
                    <ChevronRight size={13} className="text-[var(--text-muted)] shrink-0" />
                  )}
                  <span className="text-[11px] font-bold uppercase tracking-wider text-[var(--text-muted)]">
                    Changes
                  </span>
                  {totalChanged > 0 && (
                    <span className="px-1.5 py-[1px] rounded-full text-[10px] font-semibold bg-[var(--bg-elevated)] text-[var(--text-primary)] tabular-nums">
                      {totalChanged}
                    </span>
                  )}
                  {(gitStatus.total_additions > 0 || gitStatus.total_deletions > 0) && (
                    <span className="font-mono text-[10.5px] tabular-nums ml-1">
                      <span className="text-[var(--added-fg)]">+{gitStatus.total_additions}</span>{' '}
                      <span className="text-[var(--removed-fg)]">-{gitStatus.total_deletions}</span>
                    </span>
                  )}
                </div>

                <div
                  className="flex items-center gap-0.5"
                  onClick={(e) => e.stopPropagation()}
                >
                  <button
                    onClick={() => {
                      setShowChangesFilter((v) => !v);
                      if (showChangesFilter) setChangesFilter('');
                    }}
                    className={`p-1 rounded hover:bg-[var(--bg-elevated)] cursor-pointer ${
                      showChangesFilter
                        ? 'text-[var(--accent)]'
                        : 'text-[var(--text-muted)] opacity-0 group-hover:opacity-100'
                    }`}
                    title="Filter changed files"
                  >
                    <Search size={12} />
                  </button>
                  {unstagedFiles.length > 0 && (
                    <>
                      <button
                        onClick={() =>
                          setModal({
                            kind: 'discard-all',
                            count: unstagedFiles.length,
                            hasUntracked: unstagedFiles.some((f) => f.untracked),
                          })
                        }
                        className="p-1 rounded hover:bg-[var(--bg-elevated)] text-[var(--removed-fg)] opacity-0 group-hover:opacity-100 cursor-pointer"
                        title="Discard All Unstaged Changes"
                      >
                        <RotateCcw size={12} />
                      </button>
                      <button
                        onClick={() =>
                          runAction(async () => {
                            await api.gitStage({ all: true });
                          })
                        }
                        className="p-1 rounded hover:bg-[var(--bg-elevated)] text-[var(--text-muted)] hover:text-[var(--added-fg)] cursor-pointer"
                        title="Stage All Changes"
                      >
                        <Plus size={12.5} />
                      </button>
                    </>
                  )}
                </div>
              </div>

              {changesOpen && (
                <div className="flex-1 flex flex-col min-h-0 overflow-hidden">
                  {showChangesFilter && (
                    <div className="px-2 py-1.5 border-b border-[var(--border)]/60 shrink-0">
                      <div className="relative flex items-center">
                        <Search size={11.5} className="absolute left-2 text-[var(--text-dim)]" />
                        <input
                          value={changesFilter}
                          onChange={(e) => setChangesFilter(e.target.value)}
                          placeholder="Filter files..."
                          autoFocus
                          className="w-full pl-6 pr-5 py-1 text-[11.5px] rounded bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] outline-none focus:border-[var(--accent)]"
                        />
                        {changesFilter && (
                          <button
                            onClick={() => setChangesFilter('')}
                            className="absolute right-1.5 text-[var(--text-dim)] hover:text-[var(--text-primary)] cursor-pointer"
                          >
                            <X size={11} />
                          </button>
                        )}
                      </div>
                    </div>
                  )}

                  <div className="flex-1 overflow-y-auto py-1">
                    {totalChanged === 0 ? null : (
                      <>
                        {/* Staged Changes Sub-section */}
                        {stagedFiles.length > 0 && (
                          <div className="mb-1">
                            <div
                              onClick={() => setStagedSubOpen((v) => !v)}
                              className="group flex items-center justify-between px-2 py-1 text-[11px] font-semibold text-[var(--text-muted)] hover:bg-[var(--bg-hover)]/50 cursor-pointer"
                            >
                              <div className="flex items-center gap-1">
                                {stagedSubOpen ? (
                                  <ChevronDown size={12} />
                                ) : (
                                  <ChevronRight size={12} />
                                )}
                                <span>Staged Changes</span>
                                <span className="px-1.5 rounded-full bg-[var(--added-bg)] text-[var(--added-fg)] text-[10px] tabular-nums">
                                  {stagedFiles.length}
                                </span>
                              </div>
                              <button
                                onClick={(e) => {
                                  e.stopPropagation();
                                  runAction(async () => {
                                    await api.gitUnstage({ all: true });
                                  });
                                }}
                                className="p-0.5 rounded hover:bg-[var(--bg-elevated)] hover:text-[var(--text-primary)] cursor-pointer"
                                title="Unstage All Changes"
                              >
                                <Minus size={12} />
                              </button>
                            </div>
                            {stagedSubOpen &&
                              (treeView
                                ? renderFileTree(stagedTree, true, 0)
                                : stagedFiles.map((f) => renderFileRow(f, true, 0, true)))}
                          </div>
                        )}

                        {/* Unstaged / Untracked Changes Sub-section */}
                        {unstagedFiles.length > 0 && (
                          <div>
                            {stagedFiles.length > 0 && (
                              <div
                                onClick={() => setUnstagedSubOpen((v) => !v)}
                                className="group flex items-center justify-between px-2 py-1 text-[11px] font-semibold text-[var(--text-muted)] hover:bg-[var(--bg-hover)]/50 cursor-pointer"
                              >
                                <div className="flex items-center gap-1">
                                  {unstagedSubOpen ? (
                                    <ChevronDown size={12} />
                                  ) : (
                                    <ChevronRight size={12} />
                                  )}
                                  <span>Changes</span>
                                  <span className="px-1.5 rounded-full bg-[var(--bg-elevated)] text-[var(--text-primary)] text-[10px] tabular-nums">
                                    {unstagedFiles.length}
                                  </span>
                                </div>
                                <button
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    runAction(async () => {
                                      await api.gitStage({ all: true });
                                    });
                                  }}
                                  className="p-0.5 rounded hover:bg-[var(--bg-elevated)] hover:text-[var(--added-fg)] cursor-pointer"
                                  title="Stage All Changes"
                                >
                                  <Plus size={12} />
                                </button>
                              </div>
                            )}
                            {unstagedSubOpen &&
                              (treeView
                                ? renderFileTree(unstagedTree, false, 0)
                                : unstagedFiles.map((f) => renderFileRow(f, false, 0, true)))}
                          </div>
                        )}
                      </>
                    )}
                  </div>
                </div>
              )}
            </div>
            )}

            {/* Clean tree: a quiet placeholder keeps the graph docked at the bottom and resizable */}
            {totalChanged === 0 && (
              <div
                style={graphOpen ? { flex: `0 0 ${Math.round(cleanRatio * 100)}%` } : { flex: '1 1 auto' }}
                className="flex flex-col items-center justify-center gap-2.5 min-h-0 overflow-hidden px-4 text-center"
              >
                <svg width="44" height="44" viewBox="0 0 44 44" fill="none" aria-hidden="true" className="text-[var(--added-fg)]">
                  <circle cx="22" cy="22" r="21" stroke="currentColor" strokeOpacity="0.12" />
                  <circle cx="22" cy="22" r="15.5" fill="currentColor" fillOpacity="0.1" stroke="currentColor" strokeOpacity="0.35" />
                  <path d="M15.5 22.5l4.5 4.5 8.5-9.5" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
                </svg>
                <span className="flex flex-col items-center gap-0.5 leading-none">
                  <span className="text-[12.5px] font-medium text-[var(--text-primary)]">No changes</span>
                  <span className="text-[11px] text-[var(--text-dim)]">Working tree clean</span>
                </span>
              </div>
            )}

            {/* Draggable Splitter between Changes/placeholder & Graph */}
            {graphOpen && (totalChanged === 0 || changesOpen) && (
              <div
                onMouseDown={startSplitDrag}
                className="h-[3px] cursor-row-resize bg-[var(--border)]/70 hover:bg-[var(--accent)] shrink-0 transition-colors"
                title="Drag to resize Changes and Graph sections"
              />
            )}

            {/* SECTION 2: GRAPH (COMMIT HISTORY) */}
            <div
              style={
                graphOpen
                  ? { flex: '1 1 0%' }
                  : { flex: '0 0 auto' }
              }
              className="flex flex-col min-h-0 overflow-hidden"
            >
              {/* Graph Accordion Header */}
              <div
                onClick={() => setGraphOpen((v) => !v)}
                className="group flex items-center justify-between h-7 px-2 bg-[var(--bg-sidebar)] border-b border-[var(--border)]/70 hover:bg-[var(--bg-hover)]/60 cursor-pointer shrink-0"
              >
                <div className="flex items-center gap-1 min-w-0">
                  {graphOpen ? (
                    <ChevronDown size={13} className="text-[var(--text-muted)] shrink-0" />
                  ) : (
                    <ChevronRight size={13} className="text-[var(--text-muted)] shrink-0" />
                  )}
                  <span className="text-[11px] font-bold uppercase tracking-wider text-[var(--text-muted)]">
                    Graph
                  </span>
                  <span className="text-[10px] text-[var(--text-dim)] truncate ml-0.5">
                    {allBranches ? 'All' : gitStatus.branch}
                  </span>
                </div>

                <div
                  className="flex items-center gap-0.5"
                  onClick={(e) => e.stopPropagation()}
                >
                  <button
                    onClick={() => {
                      setShowGraphSearch((v) => !v);
                      if (showGraphSearch) setGraphSearch('');
                    }}
                    className={`p-1 rounded hover:bg-[var(--bg-elevated)] cursor-pointer ${
                      showGraphSearch
                        ? 'text-[var(--accent)]'
                        : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                    }`}
                    title="Search Commits"
                  >
                    <Search size={12} />
                  </button>
                  <button
                    onClick={() => setAllBranches((v) => !v)}
                    className={`p-1 rounded hover:bg-[var(--bg-elevated)] cursor-pointer ${
                      allBranches
                        ? 'text-[var(--accent)]'
                        : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                    }`}
                    title={
                      allBranches
                        ? 'Showing all branches (click for current branch only)'
                        : 'Showing current branch only (click for all branches)'
                    }
                  >
                    <GitFork size={12} />
                  </button>
                  <button
                    onClick={() => {
                      if (!graphOpen) setGraphOpen(true);
                      requestAnimationFrame(revealHead);
                    }}
                    className="p-1 rounded hover:bg-[var(--bg-elevated)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
                    title="Go to current commit (HEAD)"
                  >
                    <LocateFixed size={12} />
                  </button>
                  <button
                    onClick={() => onOpenGraphTab(expandedCommitHash || undefined)}
                    className="p-1 rounded hover:bg-[var(--bg-elevated)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
                    title="Expand Full Git Graph in Editor"
                  >
                    <Maximize2 size={11.5} />
                  </button>
                </div>
              </div>

              {graphOpen && (
                <div className="flex-1 flex flex-col min-h-0 overflow-hidden">
                  {showGraphSearch && (
                    <div className="px-2 py-1.5 border-b border-[var(--border)]/60 shrink-0">
                      <div className="relative flex items-center">
                        <Search size={11.5} className="absolute left-2 text-[var(--text-dim)]" />
                        <input
                          value={graphSearch}
                          onChange={(e) => setGraphSearch(e.target.value)}
                          placeholder="Search message, author, SHA..."
                          autoFocus
                          className="w-full pl-6 pr-5 py-1 text-[11.5px] rounded bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] outline-none focus:border-[var(--accent)]"
                        />
                        {graphSearch && (
                          <button
                            onClick={() => setGraphSearch('')}
                            className="absolute right-1.5 text-[var(--text-dim)] hover:text-[var(--text-primary)] cursor-pointer"
                          >
                            <X size={11} />
                          </button>
                        )}
                      </div>
                    </div>
                  )}

                  {/* Compare with Working Tree inline banner if active */}
                  {compareDrawer && (
                    <div className="border-b border-[var(--accent)]/40 bg-[var(--bg-elevated)]/70 shrink-0 max-h-40 flex flex-col">
                      <div className="flex items-center justify-between px-2.5 py-1 text-[11px] border-b border-[var(--border)]/60">
                        <span className="font-medium text-[var(--accent)] truncate">
                          Compare {compareDrawer.commit.short_hash} → Working Tree ({compareDrawer.files.length})
                        </span>
                        <button
                          onClick={() => setCompareDrawer(null)}
                          className="p-0.5 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer"
                        >
                          <X size={11.5} />
                        </button>
                      </div>
                      <div className="overflow-y-auto py-0.5">
                        {compareDrawer.files.length === 0 ? (
                          <div className="px-2.5 py-2 text-[11px] text-[var(--text-dim)]">
                            No differences from working tree.
                          </div>
                        ) : (
                          compareDrawer.files.map((f) => (
                            <div
                              key={f.path}
                              onClick={() =>
                                onOpenCompareDiff(
                                  {
                                    hash: compareDrawer.commit.hash,
                                    short_hash: compareDrawer.commit.short_hash,
                                    subject: compareDrawer.commit.message,
                                  },
                                  f,
                                  compareDrawer.files
                                )
                              }
                              className="flex items-center justify-between gap-1.5 px-2.5 py-1 text-[11.5px] hover:bg-[var(--bg-hover)] cursor-pointer"
                            >
                              <span
                                className="truncate"
                                style={{ color: fileStatusColor(f.status) }}
                              >
                                {f.path.split('/').pop()}
                              </span>
                              <span
                                className="font-mono text-[10.5px] font-bold"
                                style={{ color: fileStatusColor(f.status) }}
                              >
                                {f.status}
                              </span>
                            </div>
                          ))
                        )}
                      </div>
                    </div>
                  )}

                  {/* Scrollable Commit Graph Rows */}
                  <div
                    ref={graphScrollRef}
                    onScroll={onGraphScroll}
                    className="@container flex-1 overflow-y-auto py-0.5"
                  >
                    {graphLayout.rows.map((row) => {
                      const c = row.commit;

                      if (row.kind === 'working-tree') {
                        return (
                          <div
                            key={WORKING_TREE_ID}
                            onClick={() => setChangesOpen(true)}
                            className="flex items-center h-[24px] pr-2 cursor-pointer [--graph-row-bg:var(--bg-sidebar)] hover:[--graph-row-bg:var(--bg-hover)] hover:bg-[var(--bg-hover)]"
                          >
                            <CommitGraphSvg
                              row={row}
                              maxLanes={graphLayout.maxLanes}
                              laneWidth={GRAPH_LANE_W}
                              rowHeight={GRAPH_ROW_H}
                              maxVisibleLanes={GRAPH_MAX_LANES}
                            />
                            <span className="pl-0.5 text-[12.5px] italic text-[var(--modified-fg)] truncate">
                              Uncommitted Changes
                            </span>
                            <span className="ml-auto pl-2 shrink-0 text-[11px] tabular-nums text-[var(--text-dim)]">
                              {totalChanged} file{totalChanged === 1 ? '' : 's'}
                            </span>
                          </div>
                        );
                      }

                      const isExpanded = expandedCommitHash === c.hash;
                      const detail = commitDetails[c.hash];
                      const isLoadingDetail = loadingDetailHash === c.hash;

                      return (
                        <div key={c.hash} data-hash={c.hash}>
                          <div
                            {...commitHover.rowProps({
                              commit: c,
                              color: row.circleColor,
                              sync: row.sync,
                              isMerge: row.isMerge,
                            })}
                            onClick={() => toggleCommitExpand(c)}
                            onContextMenu={(e) => {
                              commitHover.hide();
                              openCommitContextMenu(e, c);
                            }}
                            className={`group relative flex items-center h-[24px] cursor-pointer ${
                              isExpanded
                                ? '[--graph-row-bg:var(--bg-active)] bg-[var(--bg-active)]'
                                : '[--graph-row-bg:var(--bg-sidebar)] hover:[--graph-row-bg:var(--bg-hover)] hover:bg-[var(--bg-hover)]'
                            }`}
                          >
                            <CommitGraphSvg
                              row={row}
                              maxLanes={graphLayout.maxLanes}
                              laneWidth={GRAPH_LANE_W}
                              rowHeight={GRAPH_ROW_H}
                              maxVisibleLanes={GRAPH_MAX_LANES}
                              fitRow
                            />

                            <div className="min-w-0 flex-1 flex items-center gap-1.5 pl-0.5 pr-2">
                              {row.sync && <SyncMark dir={row.sync} size={13} />}
                              <span
                                className={`truncate min-w-[40px] text-[12.5px] ${
                                  c.is_head
                                    ? 'font-semibold text-[var(--text-primary)]'
                                    : row.sync === 'incoming'
                                      ? 'text-[var(--text-muted)]'
                                      : 'text-[var(--text-primary)]'
                                }`}
                              >
                                {c.message}
                              </span>
                              <span className="hidden @[300px]:inline truncate min-w-0 shrink-[4] text-[11.5px] text-[var(--text-dim)]">
                                {c.author}
                              </span>
                              {c.refs.length > 0 && (
                                <span className="ml-auto pl-1 shrink-0 flex items-center">
                                  <RefBadges refs={c.refs} color={row.circleColor} maxWidth={96} tooltips={false} />
                                </span>
                              )}
                            </div>

                            {/* Hover quick actions overlay the row end (no reflow) */}
                            <div
                              className="absolute right-0 inset-y-0 hidden group-hover:flex items-center gap-0.5 pl-6 pr-1"
                              style={{
                                background:
                                  'linear-gradient(to right, transparent, var(--graph-row-bg) 20px)',
                              }}
                            >
                              <button
                                onClick={(e) => {
                                  e.stopPropagation();
                                  copyText(c.hash, `SHA (${c.short_hash})`, c.hash);
                                }}
                                className="p-1 rounded hover:bg-[var(--bg-elevated)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
                                aria-label="Copy commit SHA"
                              >
                                {copiedHash === c.hash ? (
                                  <Check size={12} className="text-[var(--added-fg)]" />
                                ) : (
                                  <Copy size={12} />
                                )}
                              </button>
                              <button
                                onClick={(e) => {
                                  commitHover.hide();
                                  openCommitContextMenu(e, c);
                                }}
                                className="p-1 rounded hover:bg-[var(--bg-elevated)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
                                aria-label="More commit actions"
                              >
                                <MoreHorizontal size={13} />
                              </button>
                            </div>
                          </div>

                          {/* Inline expanded commit: rails continue down the left edge */}
                          {isExpanded && (
                            <div
                              className="relative bg-[var(--bg-hover)]/25"
                              style={{ paddingLeft: graphColW }}
                            >
                              <GraphRails
                                lanes={row.output}
                                laneWidth={GRAPH_LANE_W}
                                maxVisibleLanes={GRAPH_MAX_LANES}
                              />
                              <div className="pl-1 pr-2 py-1.5 space-y-1.5">
                                <div className="flex flex-wrap items-center gap-1">
                                  <button
                                    onClick={() => setModal({ kind: 'cherry-pick', commit: c })}
                                    className="h-[22px] px-1.5 text-[11px] rounded bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] border border-[var(--border)] text-[var(--text-primary)] flex items-center gap-1 cursor-pointer"
                                  >
                                    <GitCommitHorizontal size={11} className="text-[var(--accent)]" />
                                    Cherry-Pick
                                  </button>
                                  <button
                                    onClick={() => setModal({ kind: 'revert', commit: c })}
                                    className="h-[22px] px-1.5 text-[11px] rounded bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] border border-[var(--border)] text-[var(--text-primary)] flex items-center gap-1 cursor-pointer"
                                  >
                                    <Undo2 size={11} />
                                    Revert
                                  </button>
                                  {remoteHost && (
                                    <button
                                      onClick={async () => {
                                        const url = remoteHost.commitUrl(c.hash);
                                        try {
                                          await api.openExternalUrl(url);
                                        } catch {
                                          window.open(url, '_blank', 'noopener');
                                        }
                                      }}
                                      className="h-[22px] px-1.5 text-[11px] rounded bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] border border-[var(--border)] text-[var(--accent-hover)] flex items-center gap-1 cursor-pointer"
                                    >
                                      <ExternalLink size={11} />
                                      {remoteHost.label}
                                    </button>
                                  )}
                                  <button
                                    onClick={(e) => openCommitContextMenu(e, c)}
                                    className="h-[22px] px-1.5 rounded bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] border border-[var(--border)] text-[var(--text-muted)] flex items-center cursor-pointer"
                                    aria-label="All commit actions"
                                  >
                                    <MoreHorizontal size={12} />
                                  </button>
                                  {detail && (
                                    <span className="ml-auto font-mono text-[10.5px] tabular-nums">
                                      <span className="text-[var(--added-fg)]">+{detail.total_additions}</span>{' '}
                                      <span className="text-[var(--removed-fg)]">−{detail.total_deletions}</span>
                                    </span>
                                  )}
                                </div>

                                {isLoadingDetail && !detail && (
                                  <div className="space-y-1 py-0.5">
                                    {[0, 1, 2].map((i) => (
                                      <div
                                        key={i}
                                        className="h-3 rounded bg-[var(--bg-hover)] animate-pulse"
                                        style={{ width: `${70 - i * 15}%` }}
                                      />
                                    ))}
                                  </div>
                                )}

                                {detail && detail.files.length === 0 && (
                                  <div className="text-[11px] text-[var(--text-dim)] py-0.5">
                                    No file changes.
                                  </div>
                                )}

                                {detail && detail.files.length > 0 && (
                                  <div className="-mx-1">
                                    {detail.files.map((f) => {
                                      const fName = f.path.split('/').pop() || f.path;
                                      const fDir = f.path.includes('/')
                                        ? f.path.slice(0, f.path.lastIndexOf('/'))
                                        : '';
                                      const isFileActive =
                                        selectedCommitKey === `commit:${c.hash}:${f.path}`;
                                      return (
                                        <div
                                          key={f.path}
                                          onClick={() =>
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
                                            )
                                          }
                                          title={f.old_path ? `${f.old_path} → ${f.path}` : f.path}
                                          className={`flex items-center justify-between gap-1.5 h-[22px] px-1 rounded text-[12px] cursor-pointer ${
                                            isFileActive
                                              ? 'bg-[var(--bg-active)]'
                                              : 'hover:bg-[var(--bg-hover)]'
                                          }`}
                                        >
                                          <div className="flex items-center gap-1.5 min-w-0">
                                            <FileIcon path={f.path} size={13} />
                                            <span
                                              className={`truncate shrink-0 max-w-[70%] ${
                                                f.status === 'D' ? 'line-through opacity-75' : ''
                                              }`}
                                              style={{ color: fileStatusColor(f.status) }}
                                            >
                                              {fName}
                                            </span>
                                            {fDir && (
                                              <span className="truncate text-[11px] text-[var(--text-dim)]">
                                                {fDir}
                                              </span>
                                            )}
                                          </div>
                                          <span
                                            className="w-3 text-center font-mono text-[10.5px] font-bold shrink-0"
                                            style={{ color: fileStatusColor(f.status) }}
                                          >
                                            {f.status}
                                          </span>
                                        </div>
                                      );
                                    })}
                                  </div>
                                )}
                              </div>
                            </div>
                          )}
                        </div>
                      );
                    })}

                    {commits.length === 0 && !graphLoading && (
                      <div className="px-3 py-4 text-center text-xs text-[var(--text-dim)]">
                        {graphSearch ? 'No matching commits.' : 'No commits yet.'}
                      </div>
                    )}

                    {hasMore && (
                      <div className="px-2 py-1.5 flex justify-center">
                        <button
                          disabled={graphLoading}
                          onClick={() => fetchLog({ append: true, skip: commits.length })}
                          className="w-full h-6 text-[11px] rounded text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
                        >
                          {graphLoading ? 'Loading…' : 'Load more commits'}
                        </button>
                      </div>
                    )}
                  </div>
                </div>
              )}
            </div>
          </div>

          {/* Compact Collapsible Bottom Sections: STASHES & CHECKPOINTS */}
          {((gitStatus.stashes?.length ?? 0) > 0 || gitStatus.checkpoints.length > 0) && (
            <div className="border-t border-[var(--border)] shrink-0">
              {/* Stashes */}
              {(gitStatus.stashes?.length ?? 0) > 0 && (
                <div>
                  <div
                    onClick={() => setStashesOpen((v) => !v)}
                    className="flex items-center justify-between h-7 px-2 hover:bg-[var(--bg-hover)]/60 cursor-pointer"
                  >
                    <div className="flex items-center gap-1 text-[11px] font-bold uppercase tracking-wider text-[var(--text-muted)]">
                      {stashesOpen ? <ChevronDown size={12.5} /> : <ChevronRight size={12.5} />}
                      <Archive size={11.5} />
                      <span>Stashes</span>
                      <span className="px-1.5 rounded-full bg-[var(--bg-elevated)] text-[var(--text-primary)] text-[10px] tabular-nums">
                        {gitStatus.stashes.length}
                      </span>
                    </div>
                  </div>
                  {stashesOpen && (
                    <div className="max-h-28 overflow-y-auto pb-1">
                      {gitStatus.stashes.map((st) => (
                        <div
                          key={st.index}
                          className="group flex items-center justify-between gap-1 px-2.5 py-1 text-[11.5px] hover:bg-[var(--bg-hover)]"
                        >
                          <div className="min-w-0">
                            <div className="truncate text-[var(--text-primary)]">{st.message}</div>
                            <div className="text-[10px] text-[var(--text-dim)] font-mono">
                              {st.index} · {st.date}
                            </div>
                          </div>
                          <div className="hidden group-hover:flex items-center gap-1 shrink-0">
                            <button
                              onClick={() =>
                                runAction(async () => {
                                  const res = await api.gitStash('pop', { index: st.index });
                                  if (!res.ok) throw new Error(res.stderr || 'Stash pop failed');
                                  onToast(`Popped ${st.index}`);
                                })
                              }
                              className="px-1.5 py-0.5 text-[10px] rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--accent)] hover:text-[var(--accent-fg)] cursor-pointer"
                            >
                              Pop
                            </button>
                            <button
                              onClick={() =>
                                runAction(async () => {
                                  const res = await api.gitStash('drop', { index: st.index });
                                  if (!res.ok) throw new Error(res.stderr || 'Stash drop failed');
                                  onToast(`Dropped ${st.index}`);
                                })
                              }
                              className="p-0.5 rounded hover:text-[var(--removed-fg)] cursor-pointer"
                              title="Drop stash"
                            >
                              <Trash2 size={11} />
                            </button>
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              )}

              {/* Checkpoints */}
              {gitStatus.checkpoints.length > 0 && (
                <div className="border-t border-[var(--border)]/60">
                  <div
                    onClick={() => setCheckpointsOpen((v) => !v)}
                    className="flex items-center justify-between h-7 px-2 hover:bg-[var(--bg-hover)]/60 cursor-pointer"
                  >
                    <div className="flex items-center gap-1 text-[11px] font-bold uppercase tracking-wider text-[var(--text-muted)]">
                      {checkpointsOpen ? (
                        <ChevronDown size={12.5} />
                      ) : (
                        <ChevronRight size={12.5} />
                      )}
                      <History size={11.5} />
                      <span>Checkpoints</span>
                      <span className="px-1.5 rounded-full bg-[var(--bg-elevated)] text-[var(--text-primary)] text-[10px] tabular-nums">
                        {gitStatus.checkpoints.length}
                      </span>
                    </div>
                  </div>
                  {checkpointsOpen && (
                    <div className="max-h-32 overflow-y-auto pb-1">
                      {gitStatus.checkpoints.map((cp) => (
                        <div
                          key={cp.id}
                          className="flex items-center justify-between px-2.5 py-1 text-xs hover:bg-[var(--bg-hover)]"
                        >
                          <div className="min-w-0">
                            <div className="truncate text-[var(--text-primary)]">
                              {cp.kind === 'turn' ? `${cp.label} · ${cp.files?.length ?? 0} file${(cp.files?.length ?? 0) === 1 ? '' : 's'}` : cp.label}
                            </div>
                            <div className="text-[10px] text-[var(--text-dim)]">{cp.created_at}</div>
                          </div>
                          <button
                            onClick={() =>
                              runAction(async () => {
                                const res = await api.gitRollback(cp.id);
                                if (!res.ok) throw new Error(res.stderr || 'Rollback failed');
                                onToast(
                                  cp.kind === 'manual'
                                    ? `Restored checkpoint (${cp.created_at}); previous state saved as a checkpoint`
                                    : `Reverted ${res.restored} file${res.restored === 1 ? '' : 's'}`,
                                  false,
                                  cp.kind === 'manual' || res.restored === 0
                                    ? undefined
                                    : { label: 'Redo', run: () => void api.gitRedo(cp.id).then(() => onToast('Re-applied')) }
                                );
                              })
                            }
                            className="px-1.5 py-0.5 text-[10px] rounded border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--accent)] hover:text-[var(--accent-fg)] cursor-pointer"
                          >
                            Restore
                          </button>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              )}
            </div>
          )}
        </>
      )}

      {/* Floating Context Menu */}
      {commitHover.card}

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
          runAction(async () => {
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
          runAction(async () => {
            const res = await api.gitRevert(hash, noCommit);
            if (!res.ok) throw new Error(res.stderr || res.stdout || 'Revert failed');
            onToast(`Reverted ${hash.slice(0, 7)}`);
          })
        }
        onConfirmReset={(hash, mode) =>
          runAction(async () => {
            const res = await api.gitReset(hash, mode);
            if (!res.ok) throw new Error(res.stderr || 'Reset failed');
            onToast(`Reset branch (${mode}) to ${hash.slice(0, 7)}`);
          })
        }
        onConfirmCreateBranch={(name, startPoint) =>
          runAction(async () => {
            const res = await api.gitCheckout(name, true, startPoint);
            if (!res.ok) throw new Error(res.stderr || 'Branch creation failed');
            onToast(`Created and switched to branch ${name}`);
          })
        }
        onConfirmCreateTag={(name, target, message) =>
          runAction(async () => {
            const res = await api.gitCreateTag(name, target, message || undefined);
            if (!res.ok) throw new Error(res.stderr || 'Tag creation failed');
            onToast(`Created tag ${name}`);
          })
        }
        onConfirmCheckoutCommit={(hash) =>
          runAction(async () => {
            const res = await api.gitCheckout(hash, false);
            if (!res.ok) throw new Error(res.stderr || 'Checkout failed');
            onToast(`Checked out ${hash.slice(0, 7)} (detached HEAD)`);
          })
        }
        onConfirmMergeCommit={(hash) =>
          runAction(async () => {
            const res = await api.gitMerge(hash);
            if (!res.ok) throw new Error(res.stderr || res.stdout || 'Merge failed');
            onToast(`Merged ${hash.slice(0, 7)}`);
          })
        }
        onConfirmDiscardFile={(path, untracked) =>
          runAction(async () => {
            const res = await api.gitDiscard(path, untracked);
            if (!res.ok) throw new Error(res.stderr || 'Discard failed');
            onToast(untracked ? `Deleted ${path}` : `Discarded changes in ${path}`);
          })
        }
        onConfirmDiscardAll={(includeUntracked) =>
          runAction(async () => {
            const res = await api.gitDiscardAll(includeUntracked);
            if (!res.ok) throw new Error(res.stderr || 'Discard all failed');
            onToast('Discarded all unstaged changes');
          })
        }
      />
    </div>
  );
};
