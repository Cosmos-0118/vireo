import React from 'react';
import {
  Files,
  GitBranch,
  Search,
} from 'lucide-react';
import type { CommitFileChange, GitStatusRes } from '../types';
import { FileExplorer } from './FileExplorer';
import { SearchPane } from './SearchPane';
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
  onOpenFile: (path: string, line?: number, col?: number, colEnd?: number) => void;
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
  fsVersion: number;
  onAskAgent: (text: string) => void;
}

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
  fsVersion,
  onAskAgent,
}) => {
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

        <div className={activeTab === 'explorer' ? 'flex-1 flex flex-col min-h-0' : 'hidden'}>
          <FileExplorer
            active={activeTab === 'explorer'}
            gitStatus={gitStatus}
            selectedPath={selectedPath}
            workspacePath={workspacePath}
            fsVersion={fsVersion}
            onOpenFile={(p) => onOpenFile(p)}
            onOpenDiff={onOpenDiff}
            onAskAgent={onAskAgent}
            onRefreshGit={onRefreshGit}
            onToast={onToast}
          />
        </div>

        <div className={activeTab === 'search' ? 'flex-1 flex flex-col min-h-0' : 'hidden'}>
          <SearchPane
            active={activeTab === 'search'}
            gitStatus={gitStatus}
            fsVersion={fsVersion}
            onOpenFile={onOpenFile}
            onOpenDiff={onOpenDiff}
            onAskAgent={onAskAgent}
            onRefreshGit={onRefreshGit}
            onToast={onToast}
          />
        </div>
      </div>
    </div>
  );
};
