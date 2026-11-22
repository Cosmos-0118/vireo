import React from 'react';
import { ArrowDown, ArrowUp, Cloud, GitBranch, Tag } from 'lucide-react';
import type { GitRef } from '../types';

const SYNC_OUT = 'var(--accent-hover)';
const SYNC_IN = 'color-mix(in srgb, #a78bfa 75%, var(--text-primary))';

/** Row marker for a commit that is not pushed (outgoing) or not pulled (incoming). */
export const SyncMark: React.FC<{ dir: 'outgoing' | 'incoming'; size?: number }> = ({ dir, size = 14 }) => {
  const color = dir === 'outgoing' ? SYNC_OUT : SYNC_IN;
  const Icon = dir === 'outgoing' ? ArrowUp : ArrowDown;
  return (
    <span
      role="img"
      aria-label={dir === 'outgoing' ? 'Not pushed' : 'Incoming, not pulled'}
      className="inline-flex items-center justify-center shrink-0 rounded-[4px] relative top-px"
      style={{
        width: size,
        height: size,
        color,
        background: `color-mix(in srgb, ${color} 16%, transparent)`,
        boxShadow: `inset 0 0 0 1px color-mix(in srgb, ${color} 34%, transparent)`,
      }}
    >
      <Icon size={size - 5} strokeWidth={2.75} />
    </span>
  );
};

/** Compact ahead/behind counts ("↓2 ↑1") that inherit the surrounding text colour. */
export const SyncCounts: React.FC<{ ahead: number; behind: number; className?: string }> = ({
  ahead,
  behind,
  className = '',
}) => {
  if (ahead <= 0 && behind <= 0) return null;
  const chip =
    'inline-flex items-center gap-px h-[15px] pl-[2px] pr-[4px] rounded-[4px] bg-current/12 font-mono text-[10.5px] font-medium leading-none tabular-nums';
  return (
    <span className={`inline-flex items-center gap-1 ${className}`}>
      {behind > 0 && (
        <span className={chip}>
          <ArrowDown size={10} strokeWidth={2.75} />
          {behind}
        </span>
      )}
      {ahead > 0 && (
        <span className={chip}>
          <ArrowUp size={10} strokeWidth={2.75} />
          {ahead}
        </span>
      )}
    </span>
  );
};

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
        <Cloud size={compact ? 9 : 10} className="shrink-0 opacity-75" />
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

interface DisplayRef {
  ref: GitRef;
  /** Remote-tracking refs pointing at the same commit as this local branch */
  syncedWith: string[];
}

const KIND_ORDER: Record<GitRef['kind'], number> = { head: 0, local: 1, remote: 2, tag: 3 };

/** Folds `origin/main` into `main` when both point at the same commit and orders refs. */
export function groupRefs(refs: GitRef[]): DisplayRef[] {
  const locals = new Set(refs.filter((r) => r.kind === 'local').map((r) => r.name));
  const out: DisplayRef[] = [];
  const synced = new Map<string, string[]>();
  for (const r of refs) {
    if (r.kind === 'remote') {
      const short = r.name.slice(r.name.indexOf('/') + 1);
      if (locals.has(short)) {
        synced.set(short, [...(synced.get(short) || []), r.name]);
        continue;
      }
    }
    out.push({ ref: r, syncedWith: [] });
  }
  for (const d of out) {
    if (d.ref.kind === 'local') d.syncedWith = synced.get(d.ref.name) || [];
  }
  return out.sort((a, b) => {
    if (a.ref.is_current !== b.ref.is_current) return a.ref.is_current ? -1 : 1;
    return KIND_ORDER[a.ref.kind] - KIND_ORDER[b.ref.kind];
  });
}

const RefIcon: React.FC<{ kind: GitRef['kind']; size: number }> = ({ kind, size }) =>
  kind === 'tag' ? (
    <Tag size={size} className="shrink-0" />
  ) : kind === 'remote' ? (
    <Cloud size={size} className="shrink-0" />
  ) : (
    <GitBranch size={size} className="shrink-0" />
  );

function refTitle(d: DisplayRef): string {
  const label =
    d.ref.kind === 'tag'
      ? 'Tag'
      : d.ref.kind === 'remote'
        ? 'Remote branch'
        : d.ref.is_current
          ? 'Current branch'
          : 'Branch';
  const sync = d.syncedWith.length ? ` · in sync with ${d.syncedWith.join(', ')}` : '';
  return `${label}: ${d.ref.name}${sync}`;
}

/** A single ref label tinted with the lane colour (solid for the checked-out branch). */
export const RefLabel: React.FC<{
  d: DisplayRef;
  color: string;
  maxWidth?: number;
  tooltip?: boolean;
}> = ({
  d,
  color,
  maxWidth = 140,
  tooltip = true,
}) => {
  const solid = d.ref.is_current;
  return (
    <span
      title={tooltip ? refTitle(d) : undefined}
      className="inline-flex items-center gap-[3px] h-[17px] px-[5px] rounded-[4px] text-[10.5px] font-medium leading-none shrink-0 min-w-0"
      style={{
        maxWidth,
        color: solid ? '#fff' : color,
        background: solid ? color : `color-mix(in srgb, ${color} 16%, transparent)`,
        boxShadow: solid ? undefined : `inset 0 0 0 1px color-mix(in srgb, ${color} 38%, transparent)`,
      }}
    >
      <RefIcon kind={d.ref.kind} size={10} />
      <span className="truncate">{d.ref.name}</span>
      {d.syncedWith.length > 0 && <Cloud size={10} className="shrink-0 opacity-80" />}
    </span>
  );
};

/**
 * Compact ref badges: the most relevant ref as a labelled pill, the rest
 * collapsed into icon+count chips (VS Code "filter" style). Full names are in
 * each chip's title and in the commit hover card.
 */
export const RefBadges: React.FC<{
  refs: GitRef[];
  color: string;
  /** Number of refs rendered with a full text label */
  labelled?: number;
  maxWidth?: number;
  /** Native title tooltips; disable inside rows that already show a hover card */
  tooltips?: boolean;
}> = ({ refs, color, labelled = 1, maxWidth, tooltips = true }) => {
  if (refs.length === 0) return null;
  const grouped = groupRefs(refs);
  const shown = grouped.slice(0, labelled);
  const rest = grouped.slice(labelled);
  const buckets = new Map<GitRef['kind'], DisplayRef[]>();
  for (const d of rest) {
    const k = d.ref.kind === 'head' ? 'local' : d.ref.kind;
    buckets.set(k, [...(buckets.get(k) || []), d]);
  }
  return (
    <span className="inline-flex items-center gap-1 min-w-0 overflow-hidden">
      {shown.map((d) => (
        <RefLabel
          key={`${d.ref.kind}:${d.ref.name}`}
          d={d}
          color={color}
          maxWidth={maxWidth}
          tooltip={tooltips}
        />
      ))}
      {[...buckets.entries()].map(([kind, list]) => (
        <span
          key={kind}
          title={tooltips ? list.map(refTitle).join('\n') : undefined}
          className="inline-flex items-center gap-[2px] h-[17px] px-1 rounded-[4px] text-[10px] font-medium leading-none shrink-0 text-[var(--text-muted)] bg-[var(--bg-elevated)] shadow-[inset_0_0_0_1px_var(--border)]"
        >
          <RefIcon kind={kind} size={10} />
          {list.length > 1 && <span className="tabular-nums">{list.length}</span>}
        </span>
      ))}
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
