import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { ArrowDown, ArrowUp, Check, Copy, ExternalLink, FileDiff, GitMerge } from 'lucide-react';
import { api } from '../api';
import type { CommitDetailRes, CommitLogItem } from '../types';
import type { RemoteHostInfo, SyncState } from '../utils/gitGraph';
import { groupRefs, RefLabel } from './GitBadges';

/* ------------------------------------------------------------------------- */
/* Silent, shared commit-detail cache (no toasts, no loading flags elsewhere) */
/* ------------------------------------------------------------------------- */

const detailCache = new Map<string, CommitDetailRes>();
const inflight = new Map<string, Promise<CommitDetailRes | null>>();

export function clearCommitDetailCache() {
  detailCache.clear();
  inflight.clear();
}

function loadDetailSilently(hash: string): Promise<CommitDetailRes | null> {
  const cached = detailCache.get(hash);
  if (cached) return Promise.resolve(cached);
  let p = inflight.get(hash);
  if (!p) {
    p = api
      .gitCommitDetail(hash)
      .then((d) => {
        detailCache.set(hash, d);
        // small LRU: commit details are only re-read while hovering recent rows
        while (detailCache.size > 60) detailCache.delete(detailCache.keys().next().value as string);
        return d;
      })
      .catch(() => null)
      .finally(() => inflight.delete(hash));
    inflight.set(hash, p);
  }
  return p;
}

/* ------------------------------------------------------------------------- */

const SHOW_DELAY = 500;
const HIDE_GRACE = 160;
const CARD_WIDTH = 360;
const GAP = 6;
const MARGIN = 8;

export interface CommitHoverTarget {
  commit: CommitLogItem;
  color: string;
  sync: SyncState;
  isMerge: boolean;
}

interface HoverState extends CommitHoverTarget {
  rect: DOMRect;
  pointerX: number;
}

function avatarHue(seed: string): number {
  let h = 0;
  for (let i = 0; i < seed.length; i++) h = (h * 31 + seed.charCodeAt(i)) >>> 0;
  return h % 360;
}

function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return '?';
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

function formatAbsolute(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  return d.toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  });
}

interface CardProps {
  state: HoverState;
  remoteHost: RemoteHostInfo | null;
  upstreamName?: string;
  onCopy: (text: string, label: string) => void;
  onOpenRemote: (url: string) => void;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
  cardRef: React.RefObject<HTMLDivElement | null>;
}

const CommitHoverCard: React.FC<CardProps> = ({
  state,
  remoteHost,
  upstreamName,
  onCopy,
  onOpenRemote,
  onMouseEnter,
  onMouseLeave,
  cardRef,
}) => {
  const { commit, color, sync, isMerge, rect, pointerX } = state;
  const [detail, setDetail] = useState<CommitDetailRes | null>(
    () => detailCache.get(commit.hash) ?? null
  );
  const [failed, setFailed] = useState(false);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let alive = true;
    setDetail(detailCache.get(commit.hash) ?? null);
    setFailed(false);
    setCopied(false);
    loadDetailSilently(commit.hash).then((d) => {
      if (!alive) return;
      if (d) setDetail(d);
      else setFailed(true);
    });
    return () => {
      alive = false;
    };
  }, [commit.hash]);

  // Position: beside the row (right, else left); for wide rows, below/above the pointer.
  useLayoutEffect(() => {
    const el = cardRef.current;
    if (!el) return;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    let left: number;
    let top: number;
    if (rect.right + GAP + w <= vw - MARGIN) {
      left = rect.right + GAP;
      top = rect.top - 6;
    } else if (rect.left - GAP - w >= MARGIN) {
      left = rect.left - GAP - w;
      top = rect.top - 6;
    } else {
      left = pointerX - 28;
      top = rect.bottom + GAP;
      if (top + h > vh - MARGIN) top = rect.top - GAP - h;
    }
    left = Math.max(MARGIN, Math.min(left, vw - w - MARGIN));
    top = Math.max(MARGIN, Math.min(top, vh - h - MARGIN));
    setPos((prev) => (prev && prev.left === left && prev.top === top ? prev : { left, top }));
  }, [rect, pointerX, detail, failed, cardRef]);

  const refs = groupRefs(commit.refs);
  const subject = detail?.subject ?? commit.message;
  const body = detail?.body?.trim();
  const hue = avatarHue(commit.author_email || commit.author);
  const absolute = formatAbsolute(commit.iso_date);

  return (
    <div
      ref={cardRef}
      role="tooltip"
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      onContextMenu={(e) => e.preventDefault()}
      className="vireo-hover-card fixed z-[1000] rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] text-[var(--text-primary)] shadow-[0_12px_32px_-8px_rgba(0,0,0,0.6),0_2px_6px_rgba(0,0,0,0.35)] select-text overflow-hidden"
      style={{
        width: CARD_WIDTH,
        maxWidth: `calc(100vw - ${MARGIN * 2}px)`,
        left: pos?.left ?? -9999,
        top: pos?.top ?? -9999,
        visibility: pos ? 'visible' : 'hidden',
      }}
    >
      {/* Author */}
      <div className="flex items-center gap-2.5 px-3 pt-3 pb-2">
        <div
          className="w-7 h-7 rounded-full flex items-center justify-center text-[10.5px] font-semibold shrink-0"
          style={{
            background: `hsl(${hue} 45% 28%)`,
            color: `hsl(${hue} 80% 85%)`,
          }}
          aria-hidden="true"
        >
          {initials(commit.author)}
        </div>
        <div className="min-w-0 flex-1 leading-tight">
          <div className="flex items-baseline gap-1.5 min-w-0">
            <span className="text-[12.5px] font-semibold truncate">{commit.author}</span>
            {commit.author_email && (
              <span className="text-[11px] text-[var(--text-dim)] truncate">
                {commit.author_email}
              </span>
            )}
          </div>
          <div className="text-[11px] text-[var(--text-muted)] mt-0.5 truncate" title={commit.iso_date}>
            {commit.date}
            {absolute && <span className="text-[var(--text-dim)]"> · {absolute}</span>}
          </div>
        </div>
      </div>

      {/* Message */}
      <div className="px-3 pb-2.5">
        <div className="text-[12.5px] font-medium leading-snug break-words">{subject}</div>
        {body && (
          <div className="mt-1.5 text-[11.5px] leading-relaxed text-[var(--text-muted)] whitespace-pre-wrap break-words max-h-36 overflow-y-auto">
            {body}
          </div>
        )}
      </div>

      {/* Refs + sync state */}
      {(refs.length > 0 || sync || isMerge) && (
        <div className="flex flex-wrap items-center gap-1 px-3 pb-2.5">
          {refs.map((d) => (
            <RefLabel key={`${d.ref.kind}:${d.ref.name}`} d={d} color={color} maxWidth={220} />
          ))}
          {isMerge && (
            <span className="inline-flex items-center gap-1 h-[17px] px-1.5 rounded-[4px] text-[10.5px] text-[var(--text-muted)] bg-[var(--bg-hover)]">
              <GitMerge size={10} />
              Merge
            </span>
          )}
          {sync === 'outgoing' && (
            <span className="inline-flex items-center gap-1 h-[17px] px-1.5 rounded-[4px] text-[10.5px] text-[var(--accent-hover)] bg-[var(--accent)]/12">
              <ArrowUp size={10} />
              Not pushed{upstreamName ? ` to ${upstreamName}` : ''}
            </span>
          )}
          {sync === 'incoming' && (
            <span className="inline-flex items-center gap-1 h-[17px] px-1.5 rounded-[4px] text-[10.5px] text-[color-mix(in_srgb,#a78bfa_70%,var(--text-primary))] bg-violet-400/12">
              <ArrowDown size={10} />
              Incoming{upstreamName ? ` from ${upstreamName}` : ''} · not pulled
            </span>
          )}
        </div>
      )}

      {/* Footer: stats + hash + links */}
      <div className="flex items-center gap-2 px-3 h-8 border-t border-[var(--border)] bg-[var(--bg-panel)]/60 text-[11px]">
        <span className="flex items-center gap-1.5 text-[var(--text-muted)] tabular-nums min-w-0">
          <FileDiff size={11.5} className="shrink-0 text-[var(--text-dim)]" />
          {detail ? (
            <>
              <span>
                {detail.files.length} file{detail.files.length === 1 ? '' : 's'}
              </span>
              <span className="font-mono text-[var(--added-fg)]">+{detail.total_additions}</span>
              <span className="font-mono text-[var(--removed-fg)]">−{detail.total_deletions}</span>
            </>
          ) : failed ? (
            <span className="text-[var(--text-dim)]">Stats unavailable</span>
          ) : (
            <span className="inline-block w-24 h-2.5 rounded bg-[var(--bg-hover)] animate-pulse" />
          )}
        </span>
        <span className="ml-auto flex items-center gap-0.5 shrink-0">
          <button
            onClick={() => {
              onCopy(commit.hash, `SHA (${commit.short_hash})`);
              setCopied(true);
            }}
            className="flex items-center gap-1 h-6 px-1.5 rounded font-mono text-[11px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
            title={`Copy full SHA ${commit.hash}`}
          >
            {copied ? <Check size={11} className="text-[var(--added-fg)]" /> : <Copy size={11} />}
            {commit.short_hash}
          </button>
          {remoteHost && (
            <button
              onClick={() => onOpenRemote(remoteHost.commitUrl(commit.hash))}
              className="flex items-center gap-1 h-6 px-1.5 rounded text-[11px] text-[var(--accent-hover)] hover:bg-[var(--bg-hover)] cursor-pointer"
              title={`Open commit on ${remoteHost.label}`}
            >
              <ExternalLink size={11} />
              {remoteHost.label}
            </button>
          )}
        </span>
      </div>
    </div>
  );
};

/**
 * Hover-intent controller for commit rows: ~500 ms before the first card,
 * instant switching between rows while a card is open, and a short grace
 * period so the pointer can travel into the card to copy or click links.
 */
export function useCommitHover(opts: {
  remoteHost: RemoteHostInfo | null;
  upstreamName?: string;
  onCopy: (text: string, label: string) => void;
  onOpenRemote: (url: string) => void;
  /** Suppress the card (e.g. while a context menu or modal is open) */
  disabled?: boolean;
}) {
  const [state, setState] = useState<HoverState | null>(null);
  const showTimer = useRef<number | null>(null);
  const hideTimer = useRef<number | null>(null);
  const openRef = useRef(false);
  const cardRef = useRef<HTMLDivElement | null>(null);
  openRef.current = state !== null;

  const clearTimers = () => {
    if (showTimer.current) window.clearTimeout(showTimer.current);
    if (hideTimer.current) window.clearTimeout(hideTimer.current);
    showTimer.current = hideTimer.current = null;
  };

  const hide = useCallback(() => {
    clearTimers();
    setState(null);
  }, []);

  const scheduleHide = useCallback(() => {
    if (showTimer.current) window.clearTimeout(showTimer.current);
    showTimer.current = null;
    if (hideTimer.current) window.clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => setState(null), HIDE_GRACE);
  }, []);

  const cancelHide = useCallback(() => {
    if (hideTimer.current) window.clearTimeout(hideTimer.current);
    hideTimer.current = null;
  }, []);

  const rowProps = useCallback(
    (target: CommitHoverTarget) => ({
      onMouseEnter: (e: React.MouseEvent<HTMLElement>) => {
        if (opts.disabled) return;
        const el = e.currentTarget;
        const pointerX = e.clientX;
        cancelHide();
        if (showTimer.current) window.clearTimeout(showTimer.current);
        const open = () => {
          if (!el.isConnected) return;
          setState({ ...target, rect: el.getBoundingClientRect(), pointerX });
        };
        if (openRef.current) open();
        else showTimer.current = window.setTimeout(open, SHOW_DELAY);
      },
      onMouseLeave: scheduleHide,
      onMouseDown: hide,
    }),
    [opts.disabled, cancelHide, scheduleHide, hide]
  );

  useEffect(() => {
    if (opts.disabled) hide();
  }, [opts.disabled, hide]);

  useEffect(() => {
    if (!state) return;
    const onScroll = (e: Event) => {
      if (cardRef.current && e.target instanceof Node && cardRef.current.contains(e.target)) return;
      hide();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') hide();
    };
    window.addEventListener('scroll', onScroll, true);
    window.addEventListener('keydown', onKey);
    window.addEventListener('blur', hide);
    window.addEventListener('resize', hide);
    return () => {
      window.removeEventListener('scroll', onScroll, true);
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('blur', hide);
      window.removeEventListener('resize', hide);
    };
  }, [state, hide]);

  useEffect(() => clearTimers, []);

  const card = state
    ? createPortal(
        <CommitHoverCard
          key={state.commit.hash}
          state={state}
          remoteHost={opts.remoteHost}
          upstreamName={opts.upstreamName}
          onCopy={opts.onCopy}
          onOpenRemote={(url) => {
            hide();
            opts.onOpenRemote(url);
          }}
          onMouseEnter={cancelHide}
          onMouseLeave={scheduleHide}
          cardRef={cardRef}
        />,
        document.body
      )
    : null;

  return { rowProps, hide, card };
}
