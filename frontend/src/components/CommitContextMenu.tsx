import React, { useEffect, useLayoutEffect, useRef, useState } from 'react';
import {
  Clipboard,
  Copy,
  ExternalLink,
  Eye,
  FileDiff,
  GitBranchPlus,
  GitCommitHorizontal,
  GitCompare,
  GitMerge,
  History,
  Link2,
  RotateCcw,
  Tag,
  Undo2,
} from 'lucide-react';
import type { CommitLogItem } from '../types';
import type { RemoteHostInfo } from '../utils/gitGraph';

export interface ContextMenuItem {
  id: string;
  label: string;
  hint?: string;
  icon?: React.ReactNode;
  danger?: boolean;
  disabled?: boolean;
  dividerBefore?: boolean;
  onClick: () => void;
}

interface FloatingContextMenuProps {
  x: number;
  y: number;
  items: ContextMenuItem[];
  onClose: () => void;
}

export const FloatingContextMenu: React.FC<FloatingContextMenuProps> = ({
  x,
  y,
  items,
  onClose,
}) => {
  const menuRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: x, top: y });

  useLayoutEffect(() => {
    const el = menuRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const pad = 8;
    const left = Math.max(pad, Math.min(x, window.innerWidth - rect.width - pad));
    const top = Math.max(pad, Math.min(y, window.innerHeight - rect.height - pad));
    setPos({ left, top });
  }, [x, y, items.length]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    const onMouseDown = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        onClose();
      }
    };
    const onScroll = () => onClose();
    window.addEventListener('keydown', onKey);
    window.addEventListener('mousedown', onMouseDown, true);
    window.addEventListener('scroll', onScroll, true);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('mousedown', onMouseDown, true);
      window.removeEventListener('scroll', onScroll, true);
    };
  }, [onClose]);

  return (
    <div
      ref={menuRef}
      style={{ left: pos.left, top: pos.top }}
      className="fixed z-50 min-w-[235px] max-w-[290px] py-1.5 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl select-none"
      onContextMenu={(e) => e.preventDefault()}
    >
      {items.map((item) => (
        <React.Fragment key={item.id}>
          {item.dividerBefore && (
            <div className="my-1 border-t border-[var(--border)]" />
          )}
          <button
            disabled={item.disabled}
            onClick={() => {
              onClose();
              item.onClick();
            }}
            className={`w-full flex items-center gap-2.5 px-3 py-1.5 text-[12.5px] text-left transition-colors cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed ${
              item.danger
                ? 'text-[var(--removed-fg)] hover:bg-[var(--removed-bg)]'
                : 'text-[var(--text-primary)] hover:bg-[var(--accent)] hover:text-[var(--accent-fg)]'
            }`}
          >
            <span className="w-4 h-4 flex items-center justify-center shrink-0 opacity-85">
              {item.icon}
            </span>
            <span className="flex-1 truncate">{item.label}</span>
            {item.hint && (
              <span className="text-[10.5px] font-mono opacity-60 shrink-0">
                {item.hint}
              </span>
            )}
          </button>
        </React.Fragment>
      ))}
    </div>
  );
};

export interface CommitActionHandlers {
  onInspect: (commit: CommitLogItem) => void;
  onCompareWithWorkingTree: (commit: CommitLogItem) => void;
  onCopyFullHash: (commit: CommitLogItem) => void;
  onCopyShortHash: (commit: CommitLogItem) => void;
  onCopyMessage: (commit: CommitLogItem) => void;
  onCopySummary: (commit: CommitLogItem) => void;
  onCopyPatch: (commit: CommitLogItem) => void;
  onCopyRemoteUrl?: (commit: CommitLogItem, url: string) => void;
  onOpenRemoteUrl?: (commit: CommitLogItem, url: string) => void;
  onCherryPick: (commit: CommitLogItem) => void;
  onRevert: (commit: CommitLogItem) => void;
  onReset: (commit: CommitLogItem) => void;
  onCheckoutCommit: (commit: CommitLogItem) => void;
  onCreateBranch: (commit: CommitLogItem) => void;
  onCreateTag: (commit: CommitLogItem) => void;
  onMergeCommit: (commit: CommitLogItem) => void;
}

export function buildCommitContextMenuItems(
  commit: CommitLogItem,
  remoteHost: RemoteHostInfo | null,
  handlers: CommitActionHandlers
): ContextMenuItem[] {
  const remoteCommitUrl = remoteHost ? remoteHost.commitUrl(commit.hash) : null;

  const items: ContextMenuItem[] = [
    {
      id: 'inspect',
      label: 'View Commit Details & Files',
      icon: <Eye size={13.5} />,
      onClick: () => handlers.onInspect(commit),
    },
    {
      id: 'compare-wt',
      label: 'Compare with Working Tree',
      icon: <GitCompare size={13.5} />,
      onClick: () => handlers.onCompareWithWorkingTree(commit),
    },
  ];

  if (remoteHost && remoteCommitUrl && handlers.onOpenRemoteUrl) {
    items.push({
      id: 'open-remote',
      label: `Open Commit on ${remoteHost.label}`,
      icon: <ExternalLink size={13.5} />,
      onClick: () => handlers.onOpenRemoteUrl!(commit, remoteCommitUrl),
    });
  }

  // Copy section
  items.push(
    {
      id: 'copy-full-sha',
      label: 'Copy Commit ID (Full SHA)',
      hint: commit.short_hash,
      icon: <Copy size={13.5} />,
      dividerBefore: true,
      onClick: () => handlers.onCopyFullHash(commit),
    },
    {
      id: 'copy-short-sha',
      label: 'Copy Short Commit ID',
      icon: <Clipboard size={13.5} />,
      onClick: () => handlers.onCopyShortHash(commit),
    },
    {
      id: 'copy-message',
      label: 'Copy Commit Message',
      icon: <GitCommitHorizontal size={13.5} />,
      onClick: () => handlers.onCopyMessage(commit),
    },
    {
      id: 'copy-summary',
      label: 'Copy Commit Summary',
      icon: <Clipboard size={13.5} />,
      onClick: () => handlers.onCopySummary(commit),
    },
    {
      id: 'copy-patch',
      label: 'Copy Commit Patch (.diff)',
      icon: <FileDiff size={13.5} />,
      onClick: () => handlers.onCopyPatch(commit),
    }
  );

  if (remoteHost && remoteCommitUrl && handlers.onCopyRemoteUrl) {
    items.push({
      id: 'copy-remote-url',
      label: `Copy ${remoteHost.label} Commit URL`,
      icon: <Link2 size={13.5} />,
      onClick: () => handlers.onCopyRemoteUrl!(commit, remoteCommitUrl),
    });
  }

  // Branch / Tag / Checkout section
  items.push(
    {
      id: 'create-branch',
      label: 'Create Branch from Commit...',
      icon: <GitBranchPlus size={13.5} />,
      dividerBefore: true,
      onClick: () => handlers.onCreateBranch(commit),
    },
    {
      id: 'create-tag',
      label: 'Create Tag at Commit...',
      icon: <Tag size={13.5} />,
      onClick: () => handlers.onCreateTag(commit),
    },
    {
      id: 'checkout-commit',
      label: 'Checkout Commit (Detached HEAD)',
      icon: <History size={13.5} />,
      disabled: commit.is_head,
      onClick: () => handlers.onCheckoutCommit(commit),
    },
    {
      id: 'merge-commit',
      label: 'Merge into Current Branch...',
      icon: <GitMerge size={13.5} />,
      disabled: commit.is_head,
      onClick: () => handlers.onMergeCommit(commit),
    }
  );

  // History actions section (Cherry-pick, Revert, Reset)
  items.push(
    {
      id: 'cherry-pick',
      label: 'Cherry-Pick Commit...',
      icon: <GitCommitHorizontal size={13.5} />,
      dividerBefore: true,
      onClick: () => handlers.onCherryPick(commit),
    },
    {
      id: 'revert',
      label: 'Revert Commit...',
      icon: <Undo2 size={13.5} />,
      onClick: () => handlers.onRevert(commit),
    },
    {
      id: 'reset',
      label: 'Reset Current Branch to Here...',
      icon: <RotateCcw size={13.5} />,
      danger: true,
      onClick: () => handlers.onReset(commit),
    }
  );

  return items;
}
