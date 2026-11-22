import React, { useEffect, useState } from 'react';
import {
  AlertTriangle,
  GitBranchPlus,
  GitCommitHorizontal,
  GitMerge,
  History,
  RotateCcw,
  Tag,
  Undo2,
  X,
} from 'lucide-react';
import type { CommitLogItem } from '../types';

export type GitModalState =
  | { kind: 'cherry-pick'; commit: CommitLogItem }
  | { kind: 'revert'; commit: CommitLogItem }
  | { kind: 'reset'; commit: CommitLogItem; currentBranch: string }
  | { kind: 'create-branch'; commit: CommitLogItem }
  | { kind: 'create-tag'; commit: CommitLogItem }
  | { kind: 'checkout-commit'; commit: CommitLogItem }
  | { kind: 'merge-commit'; commit: CommitLogItem; currentBranch: string }
  | { kind: 'discard-file'; path: string; untracked: boolean }
  | { kind: 'discard-all'; count: number; hasUntracked: boolean }
  | null;

interface GitActionModalProps {
  modal: GitModalState;
  busy?: boolean;
  onClose: () => void;
  onConfirmCherryPick: (hash: string, noCommit: boolean) => void;
  onConfirmRevert: (hash: string, noCommit: boolean) => void;
  onConfirmReset: (hash: string, mode: 'soft' | 'mixed' | 'hard') => void;
  onConfirmCreateBranch: (name: string, startPoint: string) => void;
  onConfirmCreateTag: (name: string, target: string, message: string) => void;
  onConfirmCheckoutCommit: (hash: string) => void;
  onConfirmMergeCommit: (hash: string) => void;
  onConfirmDiscardFile: (path: string, untracked: boolean) => void;
  onConfirmDiscardAll: (includeUntracked: boolean) => void;
}

export const GitActionModal: React.FC<GitActionModalProps> = ({
  modal,
  busy = false,
  onClose,
  onConfirmCherryPick,
  onConfirmRevert,
  onConfirmReset,
  onConfirmCreateBranch,
  onConfirmCreateTag,
  onConfirmCheckoutCommit,
  onConfirmMergeCommit,
  onConfirmDiscardFile,
  onConfirmDiscardAll,
}) => {
  const [noCommit, setNoCommit] = useState(false);
  const [resetMode, setResetMode] = useState<'soft' | 'mixed' | 'hard'>('mixed');
  const [branchName, setBranchName] = useState('');
  const [tagName, setTagName] = useState('');
  const [tagMessage, setTagMessage] = useState('');
  const [includeUntracked, setIncludeUntracked] = useState(false);

  useEffect(() => {
    setNoCommit(false);
    setResetMode('mixed');
    setBranchName('');
    setTagName('');
    setTagMessage('');
    setIncludeUntracked(false);
  }, [modal]);

  useEffect(() => {
    if (!modal) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [modal, onClose]);

  if (!modal) return null;

  const commitSummaryPill = (c: CommitLogItem) => (
    <div className="px-3 py-2 rounded-md bg-[var(--bg-input)] border border-[var(--border)] text-xs space-y-0.5">
      <div className="flex items-center gap-2">
        <span className="font-mono font-semibold text-[var(--accent)]">{c.short_hash}</span>
        <span className="truncate font-medium text-[var(--text-primary)]">{c.message}</span>
      </div>
      <div className="text-[11px] text-[var(--text-dim)]">
        {c.author} · {c.date}
      </div>
    </div>
  );

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/55 backdrop-blur-[1px] p-4"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="w-full max-w-[440px] rounded-xl border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl overflow-hidden text-[var(--text-primary)]">
        {/* Cherry-pick */}
        {modal.kind === 'cherry-pick' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              onConfirmCherryPick(modal.commit.hash, noCommit);
            }}
          >
            <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border)]">
              <div className="flex items-center gap-2 font-semibold text-sm">
                <GitCommitHorizontal size={16} className="text-[var(--accent)]" />
                Cherry-Pick Commit
              </div>
              <button type="button" onClick={onClose} className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer">
                <X size={15} />
              </button>
            </div>
            <div className="p-4 space-y-3 text-xs">
              <p className="text-[var(--text-muted)]">
                Apply the changes from this commit onto your current branch:
              </p>
              {commitSummaryPill(modal.commit)}
              <label className="flex items-center gap-2 pt-1 cursor-pointer select-none">
                <input
                  type="checkbox"
                  checked={noCommit}
                  onChange={(e) => setNoCommit(e.target.checked)}
                  className="accent-[var(--accent)]"
                />
                <span className="text-[var(--text-primary)]">
                  Apply changes without committing (<code className="font-mono text-[11px] text-[var(--text-muted)]">-n</code>)
                </span>
              </label>
            </div>
            <div className="flex justify-end gap-2 px-4 py-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)]/50">
              <button type="button" onClick={onClose} className="px-3 py-1.5 text-xs rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] cursor-pointer">
                Cancel
              </button>
              <button
                type="submit"
                disabled={busy}
                className="px-3.5 py-1.5 text-xs font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:opacity-90 disabled:opacity-40 cursor-pointer"
              >
                Cherry-Pick
              </button>
            </div>
          </form>
        )}

        {/* Revert */}
        {modal.kind === 'revert' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              onConfirmRevert(modal.commit.hash, noCommit);
            }}
          >
            <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border)]">
              <div className="flex items-center gap-2 font-semibold text-sm">
                <Undo2 size={16} className="text-[var(--modified-fg)]" />
                Revert Commit
              </div>
              <button type="button" onClick={onClose} className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer">
                <X size={15} />
              </button>
            </div>
            <div className="p-4 space-y-3 text-xs">
              <p className="text-[var(--text-muted)]">
                Create a new commit that safely reverses the changes introduced by:
              </p>
              {commitSummaryPill(modal.commit)}
              <label className="flex items-center gap-2 pt-1 cursor-pointer select-none">
                <input
                  type="checkbox"
                  checked={noCommit}
                  onChange={(e) => setNoCommit(e.target.checked)}
                  className="accent-[var(--accent)]"
                />
                <span className="text-[var(--text-primary)]">
                  Stage revert changes without committing (<code className="font-mono text-[11px] text-[var(--text-muted)]">-n</code>)
                </span>
              </label>
            </div>
            <div className="flex justify-end gap-2 px-4 py-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)]/50">
              <button type="button" onClick={onClose} className="px-3 py-1.5 text-xs rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] cursor-pointer">
                Cancel
              </button>
              <button
                type="submit"
                disabled={busy}
                className="px-3.5 py-1.5 text-xs font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:opacity-90 disabled:opacity-40 cursor-pointer"
              >
                Revert Commit
              </button>
            </div>
          </form>
        )}

        {/* Reset */}
        {modal.kind === 'reset' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              onConfirmReset(modal.commit.hash, resetMode);
            }}
          >
            <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border)]">
              <div className="flex items-center gap-2 font-semibold text-sm">
                <RotateCcw size={16} className="text-[var(--removed-fg)]" />
                Reset Branch to Commit
              </div>
              <button type="button" onClick={onClose} className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer">
                <X size={15} />
              </button>
            </div>
            <div className="p-4 space-y-3 text-xs">
              <p className="text-[var(--text-muted)]">
                Reset <code className="px-1 py-0.5 rounded bg-[var(--bg-input)] text-[var(--text-primary)] font-mono">{modal.currentBranch || 'HEAD'}</code> to:
              </p>
              {commitSummaryPill(modal.commit)}
              <div className="space-y-1.5 pt-1">
                {(
                  [
                    {
                      id: 'soft',
                      title: 'Soft (--soft)',
                      desc: 'Keep all subsequent changes staged in the index.',
                    },
                    {
                      id: 'mixed',
                      title: 'Mixed (--mixed)',
                      desc: 'Keep subsequent changes in your working directory (unstaged).',
                    },
                    {
                      id: 'hard',
                      title: 'Hard (--hard)',
                      desc: 'Permanently discard all subsequent commits and uncommitted changes.',
                    },
                  ] as const
                ).map((opt) => (
                  <label
                    key={opt.id}
                    className={`flex items-start gap-2.5 p-2.5 rounded-lg border cursor-pointer transition-colors ${
                      resetMode === opt.id
                        ? opt.id === 'hard'
                          ? 'border-[var(--removed-fg)] bg-[var(--removed-bg)]'
                          : 'border-[var(--accent)] bg-[var(--bg-active)]/50'
                        : 'border-[var(--border)] hover:bg-[var(--bg-hover)]'
                    }`}
                  >
                    <input
                      type="radio"
                      name="reset-mode"
                      checked={resetMode === opt.id}
                      onChange={() => setResetMode(opt.id)}
                      className="mt-0.5 accent-[var(--accent)]"
                    />
                    <div>
                      <div
                        className={`font-medium ${
                          opt.id === 'hard' ? 'text-[var(--removed-fg)]' : 'text-[var(--text-primary)]'
                        }`}
                      >
                        {opt.title}
                      </div>
                      <div className="text-[11px] text-[var(--text-muted)] mt-0.5">{opt.desc}</div>
                    </div>
                  </label>
                ))}
              </div>
              {resetMode === 'hard' && (
                <div className="flex items-start gap-2 p-2.5 rounded-md bg-[var(--removed-bg)] border border-[var(--removed-fg)]/40 text-[11px] text-[var(--removed-fg)]">
                  <AlertTriangle size={14} className="shrink-0 mt-0.5" />
                  <span>Hard reset cannot be easily undone for uncommitted work. Consider saving a checkpoint first.</span>
                </div>
              )}
            </div>
            <div className="flex justify-end gap-2 px-4 py-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)]/50">
              <button type="button" onClick={onClose} className="px-3 py-1.5 text-xs rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] cursor-pointer">
                Cancel
              </button>
              <button
                type="submit"
                disabled={busy}
                className={`px-3.5 py-1.5 text-xs font-medium rounded-md hover:opacity-90 disabled:opacity-40 cursor-pointer ${
                  resetMode === 'hard'
                    ? 'bg-[var(--danger)] text-white'
                    : 'bg-[var(--accent)] text-[var(--accent-fg)]'
                }`}
              >
                Reset ({resetMode})
              </button>
            </div>
          </form>
        )}

        {/* Create Branch */}
        {modal.kind === 'create-branch' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              if (!branchName.trim()) return;
              onConfirmCreateBranch(branchName.trim(), modal.commit.hash);
            }}
          >
            <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border)]">
              <div className="flex items-center gap-2 font-semibold text-sm">
                <GitBranchPlus size={16} className="text-[var(--accent)]" />
                Create Branch from Commit
              </div>
              <button type="button" onClick={onClose} className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer">
                <X size={15} />
              </button>
            </div>
            <div className="p-4 space-y-3 text-xs">
              {commitSummaryPill(modal.commit)}
              <div className="space-y-1">
                <label className="text-[11px] font-medium text-[var(--text-muted)]">
                  New branch name
                </label>
                <input
                  value={branchName}
                  onChange={(e) => setBranchName(e.target.value)}
                  placeholder="e.g. feat/diff-panel"
                  autoFocus
                  className="w-full px-2.5 py-2 text-xs rounded-md bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] outline-none focus:border-[var(--accent)] font-mono"
                />
              </div>
            </div>
            <div className="flex justify-end gap-2 px-4 py-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)]/50">
              <button type="button" onClick={onClose} className="px-3 py-1.5 text-xs rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] cursor-pointer">
                Cancel
              </button>
              <button
                type="submit"
                disabled={busy || !branchName.trim()}
                className="px-3.5 py-1.5 text-xs font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:opacity-90 disabled:opacity-40 cursor-pointer"
              >
                Create & Checkout
              </button>
            </div>
          </form>
        )}

        {/* Create Tag */}
        {modal.kind === 'create-tag' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              if (!tagName.trim()) return;
              onConfirmCreateTag(tagName.trim(), modal.commit.hash, tagMessage.trim());
            }}
          >
            <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border)]">
              <div className="flex items-center gap-2 font-semibold text-sm">
                <Tag size={16} className="text-[var(--modified-fg)]" />
                Create Tag at Commit
              </div>
              <button type="button" onClick={onClose} className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer">
                <X size={15} />
              </button>
            </div>
            <div className="p-4 space-y-3 text-xs">
              {commitSummaryPill(modal.commit)}
              <div className="space-y-1">
                <label className="text-[11px] font-medium text-[var(--text-muted)]">Tag name</label>
                <input
                  value={tagName}
                  onChange={(e) => setTagName(e.target.value)}
                  placeholder="e.g. v1.2.0"
                  autoFocus
                  className="w-full px-2.5 py-2 text-xs rounded-md bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] outline-none focus:border-[var(--accent)] font-mono"
                />
              </div>
              <div className="space-y-1">
                <label className="text-[11px] font-medium text-[var(--text-muted)]">
                  Annotation message (optional)
                </label>
                <input
                  value={tagMessage}
                  onChange={(e) => setTagMessage(e.target.value)}
                  placeholder="Leave blank for lightweight tag"
                  className="w-full px-2.5 py-2 text-xs rounded-md bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] outline-none focus:border-[var(--accent)]"
                />
              </div>
            </div>
            <div className="flex justify-end gap-2 px-4 py-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)]/50">
              <button type="button" onClick={onClose} className="px-3 py-1.5 text-xs rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] cursor-pointer">
                Cancel
              </button>
              <button
                type="submit"
                disabled={busy || !tagName.trim()}
                className="px-3.5 py-1.5 text-xs font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:opacity-90 disabled:opacity-40 cursor-pointer"
              >
                Create Tag
              </button>
            </div>
          </form>
        )}

        {/* Checkout Commit (Detached HEAD) */}
        {modal.kind === 'checkout-commit' && (
          <div>
            <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border)]">
              <div className="flex items-center gap-2 font-semibold text-sm">
                <History size={16} className="text-[var(--accent)]" />
                Checkout Commit
              </div>
              <button onClick={onClose} className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer">
                <X size={15} />
              </button>
            </div>
            <div className="p-4 space-y-3 text-xs">
              <p className="text-[var(--text-muted)]">
                Switch your working tree to this commit in a <strong>detached HEAD</strong> state:
              </p>
              {commitSummaryPill(modal.commit)}
            </div>
            <div className="flex justify-end gap-2 px-4 py-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)]/50">
              <button onClick={onClose} className="px-3 py-1.5 text-xs rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] cursor-pointer">
                Cancel
              </button>
              <button
                disabled={busy}
                onClick={() => onConfirmCheckoutCommit(modal.commit.hash)}
                className="px-3.5 py-1.5 text-xs font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:opacity-90 disabled:opacity-40 cursor-pointer"
              >
                Checkout
              </button>
            </div>
          </div>
        )}

        {/* Merge Commit */}
        {modal.kind === 'merge-commit' && (
          <div>
            <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border)]">
              <div className="flex items-center gap-2 font-semibold text-sm">
                <GitMerge size={16} className="text-[var(--accent)]" />
                Merge into Current Branch
              </div>
              <button onClick={onClose} className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer">
                <X size={15} />
              </button>
            </div>
            <div className="p-4 space-y-3 text-xs">
              <p className="text-[var(--text-muted)]">
                Merge commit <code className="font-mono text-[var(--text-primary)]">{modal.commit.short_hash}</code> into{' '}
                <code className="px-1 py-0.5 rounded bg-[var(--bg-input)] text-[var(--text-primary)] font-mono">{modal.currentBranch || 'HEAD'}</code>:
              </p>
              {commitSummaryPill(modal.commit)}
            </div>
            <div className="flex justify-end gap-2 px-4 py-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)]/50">
              <button onClick={onClose} className="px-3 py-1.5 text-xs rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] cursor-pointer">
                Cancel
              </button>
              <button
                disabled={busy}
                onClick={() => onConfirmMergeCommit(modal.commit.hash)}
                className="px-3.5 py-1.5 text-xs font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:opacity-90 disabled:opacity-40 cursor-pointer"
              >
                Merge
              </button>
            </div>
          </div>
        )}

        {/* Discard Single File */}
        {modal.kind === 'discard-file' && (
          <div>
            <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border)]">
              <div className="flex items-center gap-2 font-semibold text-sm text-[var(--removed-fg)]">
                <AlertTriangle size={16} />
                {modal.untracked ? 'Delete Untracked File' : 'Discard Changes'}
              </div>
              <button onClick={onClose} className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer">
                <X size={15} />
              </button>
            </div>
            <div className="p-4 space-y-2.5 text-xs">
              <p className="text-[var(--text-muted)]">
                {modal.untracked
                  ? 'Are you sure you want to permanently delete this untracked file?'
                  : 'Are you sure you want to discard all local changes to this file? This cannot be undone.'}
              </p>
              <div className="px-3 py-2 rounded-md bg-[var(--bg-input)] border border-[var(--border)] font-mono text-[var(--text-primary)] break-all">
                {modal.path}
              </div>
            </div>
            <div className="flex justify-end gap-2 px-4 py-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)]/50">
              <button onClick={onClose} className="px-3 py-1.5 text-xs rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] cursor-pointer">
                Cancel
              </button>
              <button
                disabled={busy}
                onClick={() => onConfirmDiscardFile(modal.path, modal.untracked)}
                className="px-3.5 py-1.5 text-xs font-medium rounded-md bg-[var(--danger)] text-white hover:opacity-90 disabled:opacity-40 cursor-pointer"
              >
                {modal.untracked ? 'Delete File' : 'Discard Changes'}
              </button>
            </div>
          </div>
        )}

        {/* Discard All */}
        {modal.kind === 'discard-all' && (
          <div>
            <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border)]">
              <div className="flex items-center gap-2 font-semibold text-sm text-[var(--removed-fg)]">
                <AlertTriangle size={16} />
                Discard All Changes
              </div>
              <button onClick={onClose} className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer">
                <X size={15} />
              </button>
            </div>
            <div className="p-4 space-y-3 text-xs">
              <p className="text-[var(--text-muted)]">
                Discard unstaged changes across <strong>{modal.count}</strong> file{modal.count === 1 ? '' : 's'}? This cannot be undone.
              </p>
              {modal.hasUntracked && (
                <label className="flex items-center gap-2 cursor-pointer select-none">
                  <input
                    type="checkbox"
                    checked={includeUntracked}
                    onChange={(e) => setIncludeUntracked(e.target.checked)}
                    className="accent-[var(--danger)]"
                  />
                  <span className="text-[var(--text-primary)]">
                    Also delete untracked files (<code className="font-mono text-[11px] text-[var(--text-muted)]">git clean -fd</code>)
                  </span>
                </label>
              )}
            </div>
            <div className="flex justify-end gap-2 px-4 py-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)]/50">
              <button onClick={onClose} className="px-3 py-1.5 text-xs rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] cursor-pointer">
                Cancel
              </button>
              <button
                disabled={busy}
                onClick={() => onConfirmDiscardAll(includeUntracked)}
                className="px-3.5 py-1.5 text-xs font-medium rounded-md bg-[var(--danger)] text-white hover:opacity-90 disabled:opacity-40 cursor-pointer"
              >
                Discard All
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
};
