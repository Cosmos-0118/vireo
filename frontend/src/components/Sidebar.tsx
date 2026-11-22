import React, { useEffect, useState } from 'react';
import {
  ArrowDown,
  ArrowUp,
  ChevronDown,
  ChevronRight,
  FileCode,
  Files,
  Folder,
  FolderOpen,
  GitBranch,
  GitCommitHorizontal,
  History,
  Minus,
  Plus,
  RefreshCw,
  RotateCcw,
  Save,
  Search,
} from 'lucide-react';
import { api } from '../api';
import type { CommitLogItem, GitFileStatus, GitStatusRes, SearchMatch, TreeEntry } from '../types';

export type SidebarTab = 'changes' | 'explorer' | 'search';

interface SidebarProps {
  activeTab: SidebarTab;
  onTabChange: (t: SidebarTab) => void;
  gitStatus: GitStatusRes | null;
  selectedPath: string | null;
  workspacePath: string;
  onOpenDiff: (path: string, staged?: boolean) => void;
  onOpenFile: (path: string, line?: number) => void;
  onRefreshGit: () => Promise<void>;
  onToast: (msg: string, isError?: boolean) => void;
}

const TreeNode: React.FC<{
  entry: TreeEntry;
  depth: number;
  selectedPath: string | null;
  modifiedSet: Set<string>;
  onOpenFile: (path: string) => void;
}> = ({ entry, depth, selectedPath, modifiedSet, onOpenFile }) => {
  const [open, setOpen] = useState(false);
  const [children, setChildren] = useState<TreeEntry[] | null>(null);

  const toggle = async () => {
    if (!entry.is_dir) {
      onOpenFile(entry.path);
      return;
    }
    if (!open && children === null) {
      try {
        const res = await api.getTree(entry.path);
        setChildren(res);
      } catch {
        setChildren([]);
      }
    }
    setOpen(!open);
  };

  const isModified = modifiedSet.has(entry.path);
  const isSelected = selectedPath === entry.path;

  return (
    <div>
      <div
        onClick={toggle}
        style={{ paddingLeft: `${8 + depth * 14}px` }}
        className={`flex items-center gap-1.5 py-1 pr-2 text-xs cursor-pointer select-none ${
          isSelected
            ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
            : 'hover:bg-[var(--bg-hover)] text-[var(--text-primary)]'
        }`}
      >
        {entry.is_dir ? (
          <>
            {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
            {open ? (
              <FolderOpen size={14} className="text-[var(--accent)] shrink-0" />
            ) : (
              <Folder size={14} className="text-[var(--accent)] shrink-0" />
            )}
          </>
        ) : (
          <>
            <span className="w-[13px] shrink-0" />
            <FileCode size={13} className="text-[var(--text-muted)] shrink-0" />
          </>
        )}
        <span
          className="truncate"
          style={isModified ? { color: 'var(--modified-fg)' } : undefined}
        >
          {entry.name}
        </span>
      </div>
      {open &&
        children?.map((c) => (
          <TreeNode
            key={c.path}
            entry={c}
            depth={depth + 1}
            selectedPath={selectedPath}
            modifiedSet={modifiedSet}
            onOpenFile={onOpenFile}
          />
        ))}
    </div>
  );
};

export const Sidebar: React.FC<SidebarProps> = ({
  activeTab,
  onTabChange,
  gitStatus,
  selectedPath,
  workspacePath,
  onOpenDiff,
  onOpenFile,
  onRefreshGit,
  onToast,
}) => {
  const [commitMsg, setCommitMsg] = useState('');
  const [busy, setBusy] = useState(false);
  const [rootTree, setRootTree] = useState<TreeEntry[]>([]);
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<SearchMatch[]>([]);
  const [searching, setSearching] = useState(false);
  const [branches, setBranches] = useState<string[]>([]);
  const [showBranchMenu, setShowBranchMenu] = useState(false);
  const [newBranchName, setNewBranchName] = useState('');
  const [logs, setLogs] = useState<CommitLogItem[]>([]);
  const [showLogs, setShowLogs] = useState(false);

  useEffect(() => {
    api.getTree('').then(setRootTree).catch(() => setRootTree([]));
    api.gitBranches().then((r) => setBranches(r.branches)).catch(() => setBranches([]));
    api.gitLog().then(setLogs).catch(() => setLogs([]));
  }, [workspacePath, gitStatus?.branch]);

  const modifiedSet = new Set((gitStatus?.files || []).map((f) => f.path));
  const stagedFiles = (gitStatus?.files || []).filter((f) => f.staged);
  const unstagedFiles = (gitStatus?.files || []).filter((f) => f.unstaged || f.untracked);

  const runAction = async (fn: () => Promise<void>) => {
    setBusy(true);
    try {
      await fn();
      await onRefreshGit();
      api.gitLog().then(setLogs).catch(() => {});
    } catch (e: unknown) {
      onToast(e instanceof Error ? e.message : String(e), true);
    } finally {
      setBusy(false);
    }
  };

  const handleCommit = (andPush: boolean) => {
    if (!commitMsg.trim()) {
      onToast('Enter a commit message first.', true);
      return;
    }
    runAction(async () => {
      const stageAll = stagedFiles.length === 0;
      const res = await api.gitCommit(commitMsg.trim(), stageAll);
      if (!res.ok) {
        throw new Error(res.stderr || res.stdout || 'Commit failed');
      }
      setCommitMsg('');
      if (andPush) {
        const pushRes = await api.gitPush(false);
        if (!pushRes.ok) {
          // Try with -u origin <branch> if no upstream set
          if (pushRes.stderr.includes('has no upstream')) {
            const retry = await api.gitPush(true);
            if (!retry.ok) throw new Error(retry.stderr || 'Push failed');
          } else {
            throw new Error(pushRes.stderr || 'Push failed');
          }
        }
        onToast('Committed and pushed to remote!');
      } else {
        onToast('Committed successfully!');
      }
    });
  };

  const handlePush = () =>
    runAction(async () => {
      const res = await api.gitPush(false);
      if (!res.ok) {
        if (res.stderr.includes('has no upstream')) {
          const retry = await api.gitPush(true);
          if (!retry.ok) throw new Error(retry.stderr || 'Push failed');
        } else {
          throw new Error(res.stderr || res.stdout || 'Push failed');
        }
      }
      onToast('Pushed to remote!');
    });

  const handlePull = () =>
    runAction(async () => {
      const res = await api.gitPull();
      if (!res.ok) throw new Error(res.stderr || 'Pull failed');
      onToast('Pulled latest changes.');
    });

  const handleSearch = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!searchQuery.trim()) return;
    setSearching(true);
    try {
      const res = await api.searchFiles(searchQuery.trim());
      setSearchResults(res);
    } finally {
      setSearching(false);
    }
  };

  const renderFileRow = (f: GitFileStatus, isStagedSection: boolean) => {
    const fileName = f.path.split('/').pop() || f.path;
    const dirPart = f.path.includes('/') ? f.path.slice(0, f.path.lastIndexOf('/')) : '';
    const badge = f.untracked
      ? 'U'
      : isStagedSection
      ? f.index_status
      : f.worktree_status;
    const badgeColor =
      badge === 'A' || badge === 'U'
        ? 'var(--added-fg)'
        : badge === 'D'
        ? 'var(--removed-fg)'
        : 'var(--modified-fg)';

    return (
      <div
        key={`${isStagedSection ? 's' : 'u'}-${f.path}`}
        onClick={() => onOpenDiff(f.path, isStagedSection)}
        className={`group flex items-center justify-between px-2.5 py-1 text-xs cursor-pointer gap-1.5 ${
          selectedPath === f.path
            ? 'bg-[var(--bg-active)]'
            : 'hover:bg-[var(--bg-hover)]'
        }`}
      >
        <div className="min-w-0 flex-1 flex items-center gap-1.5">
          <span
            className="font-mono text-[11px] font-bold w-3 shrink-0"
            style={{ color: badgeColor }}
          >
            {badge}
          </span>
          <span className="truncate text-[var(--text-primary)]">{fileName}</span>
          {dirPart && (
            <span className="truncate text-[11px] text-[var(--text-dim)]">{dirPart}</span>
          )}
        </div>

        <div className="flex items-center gap-1 shrink-0">
          <span className="font-mono text-[10px] text-[var(--added-fg)] group-hover:hidden">
            {f.additions > 0 ? `+${f.additions}` : ''}
          </span>
          <span className="font-mono text-[10px] text-[var(--removed-fg)] group-hover:hidden">
            {f.deletions > 0 ? `-${f.deletions}` : ''}
          </span>

          <div className="hidden group-hover:flex items-center gap-0.5">
            {isStagedSection ? (
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  runAction(async () => {
                    await api.gitUnstage({ paths: [f.path] });
                  });
                }}
                className="p-0.5 hover:bg-[var(--bg-elevated)] rounded cursor-pointer"
                title="Unstage"
              >
                <Minus size={12} />
              </button>
            ) : (
              <>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    runAction(async () => {
                      await api.gitStage({ paths: [f.path] });
                    });
                  }}
                  className="p-0.5 hover:bg-[var(--bg-elevated)] rounded cursor-pointer"
                  title="Stage"
                >
                  <Plus size={12} />
                </button>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    runAction(async () => {
                      await api.gitDiscard(f.path, f.untracked);
                    });
                  }}
                  className="p-0.5 hover:bg-[var(--bg-elevated)] text-[var(--removed-fg)] rounded cursor-pointer"
                  title="Discard changes"
                >
                  <RotateCcw size={12} />
                </button>
              </>
            )}
          </div>
        </div>
      </div>
    );
  };

  const totalChanged = gitStatus?.files.length ?? 0;

  return (
    <div className="flex h-full bg-[var(--bg-sidebar)] border-r border-[var(--border)] select-none">
      {/* Slim Activity Bar */}
      <div className="w-11 bg-[var(--bg-activity)] border-r border-[var(--border)] flex flex-col items-center py-2 gap-2 shrink-0">
        <button
          onClick={() => onTabChange('changes')}
          className={`relative p-2 rounded cursor-pointer ${
            activeTab === 'changes'
              ? 'text-[var(--text-primary)] bg-[var(--bg-active)]'
              : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
          }`}
          title="Source Control & Agent Diffs"
        >
          <GitBranch size={18} />
          {totalChanged > 0 && (
            <span className="absolute -top-0.5 -right-0.5 min-w-[16px] h-4 px-1 rounded-full bg-[var(--accent)] text-[var(--accent-fg)] text-[10px] font-bold flex items-center justify-center">
              {totalChanged}
            </span>
          )}
        </button>

        <button
          onClick={() => onTabChange('explorer')}
          className={`p-2 rounded cursor-pointer ${
            activeTab === 'explorer'
              ? 'text-[var(--text-primary)] bg-[var(--bg-active)]'
              : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
          }`}
          title="File Explorer"
        >
          <Files size={18} />
        </button>

        <button
          onClick={() => onTabChange('search')}
          className={`p-2 rounded cursor-pointer ${
            activeTab === 'search'
              ? 'text-[var(--text-primary)] bg-[var(--bg-active)]'
              : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
          }`}
          title="Search in Files (Ctrl+Shift+F)"
        >
          <Search size={18} />
        </button>
      </div>

      {/* Sidebar Pane Content */}
      <div className="flex-1 min-w-0 flex flex-col overflow-hidden">
        {activeTab === 'changes' && (
          <div className="flex-1 flex flex-col min-h-0 overflow-y-auto">
            {/* Header */}
            <div className="flex items-center justify-between px-3 py-2 border-b border-[var(--border)] shrink-0">
              <span className="text-[11px] font-semibold uppercase tracking-wider text-[var(--text-muted)]">
                Changes & Git
              </span>
              <div className="flex items-center gap-1">
                <button
                  onClick={() =>
                    runAction(async () => {
                      await api.gitCheckpoint('Manual snapshot');
                      onToast('Checkpoint saved!');
                    })
                  }
                  className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
                  title="Save instant checkpoint"
                >
                  <Save size={13} />
                </button>
                <button
                  onClick={() => onRefreshGit()}
                  className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
                  title="Refresh Git status"
                >
                  <RefreshCw size={13} />
                </button>
              </div>
            </div>

            {!gitStatus?.is_repo ? (
              <div className="p-3 text-xs text-[var(--text-muted)]">
                This folder is not a Git repository. Open a Git repo or run{' '}
                <code className="px-1 py-0.5 bg-[var(--bg-elevated)] rounded">git init</code> in the
                terminal.
              </div>
            ) : (
              <>
                {/* Branch + Sync Bar */}
                <div className="px-2.5 py-2 border-b border-[var(--border)] space-y-2">
                  <div className="relative flex items-center justify-between gap-1">
                    <button
                      onClick={() => setShowBranchMenu(!showBranchMenu)}
                      className="flex items-center gap-1.5 text-xs font-medium px-2 py-1 rounded bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-primary)] min-w-0 flex-1 cursor-pointer"
                    >
                      <GitBranch size={13} className="text-[var(--accent)] shrink-0" />
                      <span className="truncate">{gitStatus.branch || 'HEAD'}</span>
                      <ChevronDown size={12} className="ml-auto shrink-0" />
                    </button>

                    <button
                      onClick={handlePull}
                      disabled={busy}
                      className="px-2 py-1 text-xs rounded bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-primary)] flex items-center gap-0.5 cursor-pointer"
                      title="Git Pull"
                    >
                      <ArrowDown size={12} />
                      {gitStatus.behind > 0 && <span>{gitStatus.behind}</span>}
                    </button>
                    <button
                      onClick={handlePush}
                      disabled={busy}
                      className="px-2 py-1 text-xs rounded bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-primary)] flex items-center gap-0.5 cursor-pointer"
                      title="Git Push"
                    >
                      <ArrowUp size={12} />
                      {gitStatus.ahead > 0 && <span>{gitStatus.ahead}</span>}
                    </button>
                  </div>

                  {showBranchMenu && (
                    <div className="p-2 rounded bg-[var(--bg-elevated)] border border-[var(--border)] space-y-1.5">
                      <div className="max-h-32 overflow-y-auto space-y-0.5">
                        {branches.map((b) => (
                          <button
                            key={b}
                            onClick={() => {
                              setShowBranchMenu(false);
                              runAction(async () => {
                                const res = await api.gitCheckout(b, false);
                                if (!res.ok) throw new Error(res.stderr || 'Checkout failed');
                                onToast(`Switched to ${b}`);
                              });
                            }}
                            className={`w-full text-left px-2 py-1 text-xs rounded cursor-pointer ${
                              b === gitStatus.branch
                                ? 'bg-[var(--accent)] text-[var(--accent-fg)]'
                                : 'hover:bg-[var(--bg-hover)] text-[var(--text-primary)]'
                            }`}
                          >
                            {b}
                          </button>
                        ))}
                      </div>
                      <form
                        onSubmit={(e) => {
                          e.preventDefault();
                          if (!newBranchName.trim()) return;
                          setShowBranchMenu(false);
                          runAction(async () => {
                            const res = await api.gitCheckout(newBranchName.trim(), true);
                            if (!res.ok) throw new Error(res.stderr || 'Branch creation failed');
                            setNewBranchName('');
                            onToast(`Created branch ${newBranchName.trim()}`);
                          });
                        }}
                        className="flex gap-1"
                      >
                        <input
                          value={newBranchName}
                          onChange={(e) => setNewBranchName(e.target.value)}
                          placeholder="New branch..."
                          className="flex-1 min-w-0 px-2 py-1 text-xs rounded bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] outline-none"
                        />
                        <button
                          type="submit"
                          className="px-2 py-1 text-xs rounded bg-[var(--accent)] text-[var(--accent-fg)] cursor-pointer"
                        >
                          +
                        </button>
                      </form>
                    </div>
                  )}

                  {/* Commit Box */}
                  <textarea
                    value={commitMsg}
                    onChange={(e) => setCommitMsg(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
                        e.preventDefault();
                        handleCommit(false);
                      }
                    }}
                    rows={2}
                    placeholder="Commit message (Ctrl+Enter to commit)"
                    className="w-full px-2 py-1.5 text-xs rounded bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none focus:border-[var(--accent)] resize-none"
                  />
                  <div className="flex gap-1.5">
                    <button
                      onClick={() => handleCommit(false)}
                      disabled={busy || totalChanged === 0}
                      className="flex-1 py-1.5 text-xs font-medium rounded bg-[var(--accent)] text-[var(--accent-fg)] hover:opacity-90 disabled:opacity-40 cursor-pointer flex items-center justify-center gap-1"
                    >
                      <GitCommitHorizontal size={13} />
                      Commit
                    </button>
                    <button
                      onClick={() => handleCommit(true)}
                      disabled={busy || totalChanged === 0}
                      className="flex-1 py-1.5 text-xs font-medium rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-primary)] disabled:opacity-40 cursor-pointer flex items-center justify-center gap-1"
                    >
                      <ArrowUp size={13} />
                      Commit & Push
                    </button>
                  </div>
                </div>

                {/* Staged Files */}
                {stagedFiles.length > 0 && (
                  <div className="border-b border-[var(--border)] pb-1">
                    <div className="flex items-center justify-between px-2.5 py-1.5 text-[11px] font-semibold text-[var(--text-muted)]">
                      <span>STAGED ({stagedFiles.length})</span>
                      <button
                        onClick={() =>
                          runAction(async () => {
                            await api.gitUnstage({ all: true });
                          })
                        }
                        className="hover:text-[var(--text-primary)] cursor-pointer"
                        title="Unstage all"
                      >
                        <Minus size={12} />
                      </button>
                    </div>
                    {stagedFiles.map((f) => renderFileRow(f, true))}
                  </div>
                )}

                {/* Unstaged / Untracked Files */}
                <div className="border-b border-[var(--border)] pb-1">
                  <div className="flex items-center justify-between px-2.5 py-1.5 text-[11px] font-semibold text-[var(--text-muted)]">
                    <span>
                      CHANGES ({unstagedFiles.length}){' '}
                      {(gitStatus.total_additions > 0 || gitStatus.total_deletions > 0) && (
                        <span className="font-mono ml-1">
                          <span className="text-[var(--added-fg)]">+{gitStatus.total_additions}</span>{' '}
                          <span className="text-[var(--removed-fg)]">-{gitStatus.total_deletions}</span>
                        </span>
                      )}
                    </span>
                    {unstagedFiles.length > 0 && (
                      <button
                        onClick={() =>
                          runAction(async () => {
                            await api.gitStage({ all: true });
                          })
                        }
                        className="hover:text-[var(--text-primary)] cursor-pointer"
                        title="Stage all"
                      >
                        <Plus size={12} />
                      </button>
                    )}
                  </div>
                  {unstagedFiles.length === 0 && stagedFiles.length === 0 ? (
                    <div className="px-3 py-3 text-xs text-[var(--text-dim)]">
                      Working tree clean. Edits made by agents appear here live.
                    </div>
                  ) : (
                    unstagedFiles.map((f) => renderFileRow(f, false))
                  )}
                </div>

                {/* Checkpoints */}
                {gitStatus.checkpoints.length > 0 && (
                  <div className="border-b border-[var(--border)] pb-1">
                    <div className="px-2.5 py-1.5 text-[11px] font-semibold text-[var(--text-muted)] flex items-center gap-1">
                      <History size={12} />
                      CHECKPOINTS ({gitStatus.checkpoints.length})
                    </div>
                    {gitStatus.checkpoints.map((cp) => (
                      <div
                        key={cp.id}
                        className="flex items-center justify-between px-2.5 py-1 text-xs hover:bg-[var(--bg-hover)]"
                      >
                        <div className="min-w-0">
                          <div className="truncate text-[var(--text-primary)]">{cp.label}</div>
                          <div className="text-[10px] text-[var(--text-dim)]">{cp.created_at}</div>
                        </div>
                        <button
                          onClick={() =>
                            runAction(async () => {
                              const res = await api.gitRollback(cp.id);
                              if (!res.ok) throw new Error(res.stderr || 'Rollback failed');
                              onToast(`Rolled back to checkpoint (${cp.created_at})`);
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

                {/* Recent Commits */}
                <div>
                  <button
                    onClick={() => setShowLogs(!showLogs)}
                    className="w-full flex items-center gap-1 px-2.5 py-1.5 text-[11px] font-semibold text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
                  >
                    {showLogs ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
                    RECENT COMMITS
                  </button>
                  {showLogs &&
                    logs.slice(0, 10).map((c) => (
                      <div
                        key={c.hash}
                        className="px-2.5 py-1 text-xs border-b border-[var(--border)]/40"
                      >
                        <div className="truncate text-[var(--text-primary)]">{c.message}</div>
                        <div className="text-[10px] text-[var(--text-dim)] flex justify-between">
                          <span className="font-mono">{c.short_hash}</span>
                          <span>{c.date}</span>
                        </div>
                      </div>
                    ))}
                </div>
              </>
            )}
          </div>
        )}

        {activeTab === 'explorer' && (
          <div className="flex-1 flex flex-col min-h-0">
            <div className="flex items-center justify-between px-3 py-2 border-b border-[var(--border)] shrink-0">
              <span className="text-[11px] font-semibold uppercase tracking-wider text-[var(--text-muted)]">
                Explorer
              </span>
              <button
                onClick={() => api.getTree('').then(setRootTree).catch(() => {})}
                className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer"
                title="Refresh files"
              >
                <RefreshCw size={12} />
              </button>
            </div>
            <div className="flex-1 overflow-y-auto py-1">
              {rootTree.map((entry) => (
                <TreeNode
                  key={entry.path}
                  entry={entry}
                  depth={0}
                  selectedPath={selectedPath}
                  modifiedSet={modifiedSet}
                  onOpenFile={(p) => onOpenFile(p)}
                />
              ))}
            </div>
          </div>
        )}

        {activeTab === 'search' && (
          <div className="flex-1 flex flex-col min-h-0">
            <div className="p-2 border-b border-[var(--border)]">
              <form onSubmit={handleSearch} className="flex gap-1">
                <input
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  placeholder="Search with ripgrep..."
                  className="flex-1 min-w-0 px-2 py-1 text-xs rounded bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] outline-none focus:border-[var(--accent)]"
                  autoFocus
                />
                <button
                  type="submit"
                  className="px-2 py-1 text-xs rounded bg-[var(--accent)] text-[var(--accent-fg)] cursor-pointer"
                >
                  Go
                </button>
              </form>
            </div>
            <div className="flex-1 overflow-y-auto">
              {searching ? (
                <div className="p-3 text-xs text-[var(--text-muted)]">Searching...</div>
              ) : searchResults.length === 0 ? (
                <div className="p-3 text-xs text-[var(--text-dim)]">
                  Enter a query to search across your repo.
                </div>
              ) : (
                searchResults.map((m, idx) => (
                  <div
                    key={`${m.path}-${m.line}-${idx}`}
                    onClick={() => onOpenFile(m.path, m.line)}
                    className="px-2.5 py-1.5 border-b border-[var(--border)]/40 hover:bg-[var(--bg-hover)] cursor-pointer"
                  >
                    <div className="flex items-center justify-between text-[11px] font-mono">
                      <span className="truncate text-[var(--accent)]">{m.path}</span>
                      <span className="text-[var(--text-dim)] shrink-0">:{m.line}</span>
                    </div>
                    <div className="text-xs font-mono truncate text-[var(--text-primary)]">
                      {m.text.trim()}
                    </div>
                  </div>
                ))
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
};
