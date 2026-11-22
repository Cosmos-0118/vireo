import React, { useEffect, useState } from 'react';
import {
  ChevronDown,
  ChevronRight,
  FileCode,
  Files,
  Folder,
  FolderOpen,
  GitBranch,
  RefreshCw,
  Search,
} from 'lucide-react';
import { api } from '../api';
import type { CommitFileChange, GitStatusRes, SearchMatch, TreeEntry } from '../types';
import { SourceControlPane } from './SourceControlPane';

export type SidebarTab = 'changes' | 'explorer' | 'search';

interface SidebarProps {
  activeTab: SidebarTab;
  onTabChange: (t: SidebarTab) => void;
  gitStatus: GitStatusRes | null;
  selectedPath: string | null;
  selectedTabId: string | null;
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
        style={{ paddingLeft: `${10 + depth * 15}px` }}
        className={`mx-1.5 flex items-center gap-2 py-1.5 pr-2.5 rounded-md text-[13px] cursor-pointer select-none transition-colors ${
          isSelected
            ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium'
            : 'hover:bg-[var(--bg-hover)] text-[var(--text-primary)]'
        }`}
      >
        {entry.is_dir ? (
          <>
            {open ? (
              <ChevronDown size={14} className="shrink-0 text-[var(--text-muted)]" />
            ) : (
              <ChevronRight size={14} className="shrink-0 text-[var(--text-muted)]" />
            )}
            {open ? (
              <FolderOpen size={15} className="text-[var(--accent)] shrink-0" />
            ) : (
              <Folder size={15} className="text-[var(--accent)] shrink-0" />
            )}
          </>
        ) : (
          <>
            <span className="w-[14px] shrink-0" />
            <FileCode size={15} className="text-[var(--text-muted)] shrink-0" />
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
  selectedTabId,
  workspacePath,
  onOpenDiff,
  onOpenFile,
  onOpenCommitDiff,
  onOpenCompareDiff,
  onOpenGraphTab,
  onRefreshGit,
  onToast,
}) => {
  const [rootTree, setRootTree] = useState<TreeEntry[]>([]);
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<SearchMatch[]>([]);
  const [searching, setSearching] = useState(false);

  useEffect(() => {
    api.getTree('').then(setRootTree).catch(() => setRootTree([]));
  }, [workspacePath, gitStatus?.branch]);

  const modifiedSet = new Set((gitStatus?.files || []).map((f) => f.path));

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

  const totalChanged = gitStatus?.files.length ?? 0;

  return (
    <div className="flex h-full bg-[var(--bg-sidebar)] border-r border-[var(--border)] select-none">
      {/* Slim Activity Bar */}
      <div className="w-12 bg-[var(--bg-activity)] border-r border-[var(--border)] flex flex-col items-center py-2.5 gap-2 shrink-0">
        <button
          onClick={() => onTabChange('changes')}
          className={`relative w-9 h-9 flex items-center justify-center rounded-lg cursor-pointer transition-colors ${
            activeTab === 'changes'
              ? 'text-[var(--text-primary)] bg-[var(--bg-active)]'
              : 'text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
          }`}
          title="Source Control & Git Graph (Ctrl+Shift+G)"
        >
          {activeTab === 'changes' && (
            <span className="absolute -left-1.5 top-2 bottom-2 w-[3px] rounded-r bg-[var(--accent)]" />
          )}
          <GitBranch size={19} />
          {totalChanged > 0 && (
            <span className="absolute -bottom-0.5 -right-0.5 min-w-[17px] h-[17px] px-1 rounded-full bg-[var(--accent)] text-[var(--accent-fg)] text-[10px] font-bold leading-none flex items-center justify-center ring-2 ring-[var(--bg-activity)] tabular-nums">
              {totalChanged > 99 ? '99+' : totalChanged}
            </span>
          )}
        </button>

        <button
          onClick={() => onTabChange('explorer')}
          className={`relative w-9 h-9 flex items-center justify-center rounded-lg cursor-pointer transition-colors ${
            activeTab === 'explorer'
              ? 'text-[var(--text-primary)] bg-[var(--bg-active)]'
              : 'text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
          }`}
          title="File Explorer"
        >
          {activeTab === 'explorer' && (
            <span className="absolute -left-1.5 top-2 bottom-2 w-[3px] rounded-r bg-[var(--accent)]" />
          )}
          <Files size={19} />
        </button>

        <button
          onClick={() => onTabChange('search')}
          className={`relative w-9 h-9 flex items-center justify-center rounded-lg cursor-pointer transition-colors ${
            activeTab === 'search'
              ? 'text-[var(--text-primary)] bg-[var(--bg-active)]'
              : 'text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
          }`}
          title="Search in Files (Ctrl+Shift+F)"
        >
          {activeTab === 'search' && (
            <span className="absolute -left-1.5 top-2 bottom-2 w-[3px] rounded-r bg-[var(--accent)]" />
          )}
          <Search size={19} />
        </button>
      </div>

      {/* Sidebar Pane Content */}
      <div className="flex-1 min-w-0 flex flex-col overflow-hidden">
        {activeTab === 'changes' && (
          <SourceControlPane
            gitStatus={gitStatus}
            selectedPath={selectedPath}
            selectedCommitKey={selectedTabId}
            workspacePath={workspacePath}
            onOpenDiff={onOpenDiff}
            onOpenFile={onOpenFile}
            onOpenCommitDiff={onOpenCommitDiff}
            onOpenCompareDiff={onOpenCompareDiff}
            onOpenGraphTab={onOpenGraphTab}
            onRefreshGit={onRefreshGit}
            onToast={onToast}
          />
        )}

        {activeTab === 'explorer' && (
          <div className="flex-1 flex flex-col min-h-0">
            <div className="flex items-center justify-between h-10 px-3.5 border-b border-[var(--border)] shrink-0">
              <span className="text-[11.5px] font-semibold uppercase tracking-wider text-[var(--text-muted)]">
                Explorer
              </span>
              <button
                onClick={() => api.getTree('').then(setRootTree).catch(() => {})}
                className="p-1.5 rounded-md hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
                title="Refresh files"
              >
                <RefreshCw size={14} />
              </button>
            </div>
            <div className="flex-1 overflow-y-auto py-1.5 space-y-0.5">
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
