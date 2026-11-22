import React from 'react';
import { GitBranch, Globe, Tag } from 'lucide-react';
import type { GitRef } from '../types';

export const RefPill: React.FC<{ r: GitRef; compact?: boolean }> = ({ r, compact }) => {
  const base = compact
    ? 'inline-flex items-center gap-0.5 px-1.5 py-[1px] rounded text-[10px] font-medium leading-tight shrink-0 max-w-[115px]'
    : 'inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[10.5px] font-medium leading-tight shrink-0 max-w-[150px]';

  if (r.kind === 'tag') {
    return (
      <span
        className={`${base} bg-amber-500/15 text-amber-400 border border-amber-500/35`}
        title={`Tag: ${r.name}`}
      >
        <Tag size={compact ? 9 : 10} className="shrink-0" />
        <span className="truncate">{r.name}</span>
      </span>
    );
  }
  if (r.kind === 'remote') {
    return (
      <span
        className={`${base} bg-[var(--bg-elevated)] text-[var(--text-muted)] border border-[var(--border)]`}
        title={`Remote branch: ${r.name}`}
      >
        <Globe size={compact ? 9 : 10} className="shrink-0 opacity-75" />
        <span className="truncate">{r.name}</span>
      </span>
    );
  }
  if (r.is_current) {
    return (
      <span
        className={`${base} bg-[var(--accent)] text-[var(--accent-fg)] shadow-xs`}
        title={`Current branch: ${r.name}`}
      >
        <GitBranch size={compact ? 9 : 10} className="shrink-0" />
        <span className="truncate">{r.name}</span>
      </span>
    );
  }
  return (
    <span
      className={`${base} bg-emerald-500/15 text-emerald-400 border border-emerald-500/35`}
      title={`Local branch: ${r.name}`}
    >
      <GitBranch size={compact ? 9 : 10} className="shrink-0" />
      <span className="truncate">{r.name}</span>
    </span>
  );
};

export function fileStatusColor(status: string): string {
  switch (status) {
    case 'A':
    case 'U':
    case '?':
      return 'var(--added-fg)';
    case 'D':
      return 'var(--removed-fg)';
    case 'R':
    case 'C':
      return 'var(--accent)';
    default:
      return 'var(--modified-fg)';
  }
}
