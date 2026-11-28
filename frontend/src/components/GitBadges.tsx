import React from 'react';
import { ArrowDown, ArrowUp, ChevronDown, ChevronUp, Cloud, GitBranch, Tag } from 'lucide-react';
import type { GitRef } from '../types';

const SYNC_OUT = 'var(--accent-hover)';
const SYNC_IN = 'color-mix(in srgb, #a78bfa 75%, var(--text-primary))';

/**
 * Row marker for a commit that is not pushed (outgoing) or not pulled (incoming).
 * A bare tinted arrow, like GitHub Desktop / Tower; no box, so it reads as a glyph
 * next to the message rather than another badge competing with ref labels.
 */
export const SyncMark: React.FC<{ dir: 'outgoing' | 'incoming'; size?: number }> = React.memo(({ dir, size = 14 }) => {
  const Icon = dir === 'outgoing' ? ChevronUp : ChevronDown;
  return (
    <span
      role="img"
      aria-label={dir === 'outgoing' ? 'Not pushed' : 'Incoming, not pulled'}
      className="inline-flex items-center justify-center shrink-0"
      style={{ width: size, height: size, color: dir === 'outgoing' ? SYNC_OUT : SYNC_IN }}
    >
      <Icon size={size - 1} strokeWidth={2.25} />
    </span>
  );
});

/**
 * Ahead/behind counts ("↓2 ↑1").
 * - `plain`: inherits the surrounding text colour, no chrome (lists, status lines).
 * - `badge`: a single rounded counter for use on a filled button.
 * `arrows={false}` drops the direction glyphs when the button icon already shows it.
 */
export const SyncCounts: React.FC<{
  ahead: number;
  behind: number;
  className?: string;
  variant?: 'plain' | 'badge';
  arrows?: boolean;
}> = ({ ahead, behind, className = '', variant = 'plain', arrows = true }) => {
  if (ahead <= 0 && behind <= 0) return null;
  const part = (n: number, Icon: typeof ArrowUp) => (
    <span className="inline-flex items-center gap-[1px]">
      {arrows && <Icon size={variant === 'badge' ? 10 : 11} strokeWidth={2.5} className="shrink-0" />}
      {n}
    </span>
  );
  const shell =
    variant === 'badge'
      ? 'h-[16px] min-w-[16px] justify-center px-[5px] rounded-full bg-white/20 text-[10.5px] font-semibold'
      : 'text-[11px] font-medium';
  return (
    <span className={`inline-flex items-center gap-1.5 leading-none tabular-nums shrink-0 ${shell} ${className}`}>
      {behind > 0 && part(behind, ArrowDown)}
      {ahead > 0 && part(ahead, ArrowUp)}
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

const TAG_COLOR = '#d9a441';

/** Neutral chip surface shared by every non-current ref: reads on dark and light themes alike. */
const QUIET_BG = 'color-mix(in srgb, var(--text-primary) 7%, transparent)';
const QUIET_RING = 'inset 0 0 0 1px color-mix(in srgb, var(--text-primary) 13%, transparent)';

/** Icon tint: lane colour for branches, amber for tags, dim for remotes. */
function iconColor(kind: GitRef['kind'], lane: string): string {
  if (kind === 'tag') return TAG_COLOR;
  if (kind === 'remote') return 'var(--text-dim)';
  return lane;
}

const RefIcon: React.FC<{ kind: GitRef['kind']; size: number; color?: string; strokeWidth?: number }> = ({
  kind,
  size,
  color,
  strokeWidth = 2.25,
}) => {
  const Icon = kind === 'tag' ? Tag : kind === 'remote' ? Cloud : GitBranch;
  return <Icon size={size} strokeWidth={strokeWidth} color={color} className="shrink-0" />;
};

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

/**
 * A single ref label. The checked-out branch is solid in its lane colour; every
 * other ref is a quiet neutral chip (hairline ring, readable text) where only the
 * icon carries colour, so a row of refs never turns into a rainbow.
 */
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
  const remote = d.ref.kind === 'remote';
  return (
    <span
      title={tooltip ? refTitle(d) : undefined}
      className="inline-flex items-center gap-[4px] h-[18px] pl-[5px] pr-[6px] rounded-[5px] text-[11px] font-medium tracking-[0.005em] shrink-0 min-w-0"
      style={{
        maxWidth,
        color: solid ? '#fff' : remote ? 'var(--text-muted)' : 'var(--text-primary)',
        background: solid ? color : QUIET_BG,
        fontWeight: solid ? 600 : 500,
        boxShadow: solid
          ? 'inset 0 1px 0 rgba(255,255,255,0.22), inset 0 0 0 1px rgba(0,0,0,0.12)'
          : QUIET_RING,
      }}
    >
      <RefIcon kind={d.ref.kind} size={11} color={solid ? '#fff' : iconColor(d.ref.kind, color)} />
      {/* Explicit line box: `truncate` clips overflow, so leading-none would cut descenders (g, p, y). */}
      <span className="truncate leading-[16px]">{d.ref.name}</span>
      {d.syncedWith.length > 0 && (
        <Cloud
          size={11}
          strokeWidth={2.25}
          className="shrink-0"
          color={solid ? 'rgba(255,255,255,0.75)' : 'var(--text-dim)'}
        />
      )}
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
}> = React.memo(({ refs, color, labelled = 1, maxWidth, tooltips = true }) => {
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
          className="inline-flex items-center gap-[3px] h-[18px] px-[5px] rounded-[5px] text-[10.5px] font-medium leading-none shrink-0 text-[var(--text-muted)]"
          style={{ background: QUIET_BG, boxShadow: QUIET_RING }}
        >
          <RefIcon kind={kind} size={11} color={iconColor(kind, color)} />
          {list.length > 1 && <span className="tabular-nums">{list.length}</span>}
        </span>
      ))}
    </span>
  );
});

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
