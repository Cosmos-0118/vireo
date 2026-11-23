import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { visibleInterval } from '../utils/visibleInterval';
import {
  AlertTriangle,
  ArrowDown,
  ArrowUp,
  Check,
  ChevronDown,
  Cloud,
  CloudOff,
  CloudUpload,
  CornerDownLeft,
  GitBranch,
  GitCommitHorizontal,
  Link2,
  Loader2,
  Plus,
  RefreshCw,
  Search,
  Trash2,
} from 'lucide-react';
import { api } from '../api';
import type { BranchesRes, BranchInfo, GitStatusRes } from '../types';
import { SyncCounts } from './GitBadges';

/* ------------------------------------------------------------------------- */
/* Helpers                                                                   */
/* ------------------------------------------------------------------------- */

function shortAgo(unixSeconds: number | null | undefined, now: number): string {
  if (!unixSeconds) return '';
  const s = Math.max(0, Math.round(now / 1000 - unixSeconds));
  if (s < 45) return 'just now';
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.round(h / 24);
  if (d < 30) return `${d}d ago`;
  const mo = Math.round(d / 30);
  if (mo < 12) return `${mo}mo ago`;
  return `${Math.round(mo / 12)}y ago`;
}

function compactAgo(unixSeconds: number, now: number): string {
  return shortAgo(unixSeconds, now).replace(' ago', '').replace('just now', 'now');
}

function useNow(intervalMs = 30_000) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    return visibleInterval(() => setNow(Date.now()), intervalMs);
  }, [intervalMs]);
  return now;
}

type SyncKind =
  | 'no-remote'
  | 'detached'
  | 'blocked'
  | 'publish'
  | 'gone'
  | 'diverged'
  | 'pull'
  | 'push'
  | 'fetch';

interface SyncInfo {
  kind: SyncKind;
  ahead: number;
  behind: number;
  upstream: string;
  remote: string;
}

function computeSync(status: GitStatusRes, info: BranchesRes): SyncInfo {
  const remotes = info.remotes ?? (status.remote_url ? ['origin'] : []);
  const remote = remotes.includes('origin') ? 'origin' : remotes[0] || 'origin';
  const ahead = status.ahead;
  const behind = status.behind;
  const upstream = info.upstream ?? '';
  const base = { ahead, behind, upstream, remote };
  const detached = info.detached ?? status.branch === 'HEAD';
  if (detached) return { ...base, kind: 'detached' };
  if (remotes.length === 0 && !status.remote_url) return { ...base, kind: 'no-remote' };
  if (status.state) return { ...base, kind: 'blocked' };
  // Older backends don't report upstream; fall back to ahead/behind only.
  const knowsUpstream = info.upstream !== undefined;
  if (knowsUpstream && info.upstream_gone) return { ...base, kind: 'gone' };
  if (knowsUpstream && !upstream) return { ...base, kind: 'publish' };
  if (ahead > 0 && behind > 0) return { ...base, kind: 'diverged' };
  if (behind > 0) return { ...base, kind: 'pull' };
  if (ahead > 0) return { ...base, kind: 'push' };
  return { ...base, kind: 'fetch' };
}

function plural(n: number, word: string) {
  return `${n} ${word}${n === 1 ? '' : 's'}`;
}

/* ------------------------------------------------------------------------- */
/* Branch bar                                                                */
/* ------------------------------------------------------------------------- */

interface BranchBarProps {
  gitStatus: GitStatusRes;
  branchInfo: BranchesRes;
  busy: boolean;
  uncommittedCount: number;
  runAction: (fn: () => Promise<void>) => Promise<void>;
  onToast: (msg: string, isError?: boolean) => void;
  onRefreshBranches: () => void;
  /** Opens the "publish to GitHub / add remote" dialog. */
  onOpenPublish: (mode: 'github' | 'remote') => void;
  /** Working tree is clean: show the next sync step as a full-width button (VS Code style). */
  largeAction?: boolean;
}

export const BranchBar: React.FC<BranchBarProps> = ({
  gitStatus,
  branchInfo,
  busy,
  uncommittedCount,
  runAction,
  onToast,
  onRefreshBranches,
  onOpenPublish,
  largeAction = false,
}) => {
  const now = useNow();
  const [pickerOpen, setPickerOpen] = useState(false);
  const [syncMenuOpen, setSyncMenuOpen] = useState(false);
  const [running, setRunning] = useState<string | null>(null);
  const branchBtnRef = useRef<HTMLButtonElement>(null);
  const syncMenuRef = useRef<HTMLDivElement>(null);

  const sync = computeSync(gitStatus, branchInfo);
  const branch = gitStatus.branch;
  const detached = sync.kind === 'detached';
  const upstreamLabel = sync.upstream || `${sync.remote}/${branch}`;

  useEffect(() => {
    if (!syncMenuOpen) return;
    const onDown = (e: MouseEvent) => {
      if (syncMenuRef.current && !syncMenuRef.current.contains(e.target as Node)) {
        setSyncMenuOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setSyncMenuOpen(false);
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [syncMenuOpen]);

  const act = useCallback(
    (label: string, fn: () => Promise<void>) => {
      setSyncMenuOpen(false);
      setRunning(label);
      return runAction(fn).finally(() => setRunning(null));
    },
    [runAction]
  );

  const doFetch = () =>
    act('Fetching', async () => {
      const res = await api.gitFetch();
      if (!res.ok) throw new Error(res.stderr || 'Fetch failed');
      onToast('Fetched from remote');
    });

  const doPull = (mode: 'merge' | 'rebase' | 'ff-only') =>
    act('Pulling', async () => {
      const res = await api.gitPull(mode);
      if (!res.ok) {
        const msg = res.stderr || res.stdout || 'Pull failed';
        if (mode === 'ff-only' && /not possible to fast-forward|diverg/i.test(msg)) {
          throw new Error('Your branch and the remote have both changed. Use Pull (merge) or Pull (rebase).');
        }
        throw new Error(msg);
      }
      onToast(mode === 'rebase' ? 'Pulled and rebased' : 'Pulled latest changes');
    });

  const doPush = () =>
    act('Pushing', async () => {
      const res = await api.gitPush(false);
      if (!res.ok) {
        const msg = res.stderr || res.stdout || 'Push failed';
        if (/rejected|fetch first|non-fast-forward/i.test(msg)) {
          throw new Error('Push rejected: the remote has commits you don’t have. Pull first, then push.');
        }
        throw new Error(msg);
      }
      onToast(`Pushed to ${upstreamLabel}`);
    });

  const doPublish = () =>
    act('Publishing', async () => {
      const res = await api.gitPush(true);
      if (!res.ok) throw new Error(res.stderr || res.stdout || 'Publish failed');
      onToast(`Published ${branch} to ${sync.remote}`);
    });

  const doSync = (mode: 'merge' | 'rebase') =>
    act('Syncing', async () => {
      const pull = await api.gitPull(mode);
      if (!pull.ok) throw new Error(pull.stderr || pull.stdout || 'Pull failed');
      const push = await api.gitPush(false);
      if (!push.ok) throw new Error(push.stderr || push.stdout || 'Push failed');
      onToast(mode === 'rebase' ? 'Rebased onto remote and pushed' : 'Merged remote changes and pushed');
    });

  /* Primary contextual action ------------------------------------------ */
  let primary: {
    label: string;
    icon: React.ReactNode;
    title: string;
    onClick?: () => void;
    emphasis: boolean;
  } | null = null;

  const isLarge =
    largeAction &&
    (sync.kind === 'no-remote' ||
      sync.kind === 'publish' ||
      sync.kind === 'gone' ||
      sync.kind === 'diverged' ||
      sync.kind === 'pull' ||
      sync.kind === 'push');

  switch (sync.kind) {
    case 'no-remote':
      primary = {
        label: isLarge ? 'Publish Branch' : 'Publish',
        icon: <CloudUpload size={13} />,
        title: 'Publish this repository to GitHub or add a remote',
        onClick: () => onOpenPublish('github'),
        emphasis: true,
      };
      break;
    case 'publish':
      primary = {
        label: isLarge ? 'Publish Branch' : 'Publish',
        icon: <CloudUpload size={13} />,
        title: `Push ${branch} to ${sync.remote} and track it`,
        onClick: doPublish,
        emphasis: true,
      };
      break;
    case 'gone':
      primary = {
        label: isLarge ? 'Publish Branch' : 'Publish',
        icon: <CloudUpload size={13} />,
        title: `${upstreamLabel} was deleted on the remote. Publish ${branch} again`,
        onClick: doPublish,
        emphasis: true,
      };
      break;
    case 'diverged':
      primary = {
        label: isLarge ? 'Sync Changes' : 'Sync',
        icon: <RefreshCw size={12.5} />,
        title: `${plural(sync.behind, 'commit')} to pull and ${plural(sync.ahead, 'commit')} to push. Choose how to combine them.`,
        onClick: () => setSyncMenuOpen((v) => !v),
        emphasis: true,
      };
      break;
    case 'pull':
      primary = {
        label: 'Pull',
        icon: <ArrowDown size={13} />,
        title: `Pull ${plural(sync.behind, 'commit')} from ${upstreamLabel}`,
        onClick: () => doPull('ff-only'),
        emphasis: true,
      };
      break;
    case 'push':
      primary = {
        label: 'Push',
        icon: <ArrowUp size={13} />,
        title: `Push ${plural(sync.ahead, 'commit')} to ${upstreamLabel}`,
        onClick: doPush,
        emphasis: true,
      };
      break;
    case 'fetch':
    case 'blocked':
      primary = {
        label: 'Fetch',
        icon: <RefreshCw size={12.5} />,
        title: 'Check the remote for new commits',
        onClick: doFetch,
        emphasis: false,
      };
      break;
    default:
      primary = null;
  }

  const syncDisabled = busy || sync.kind === 'blocked';
  const counts =
    sync.kind === 'diverged' || sync.kind === 'pull' || sync.kind === 'push' ? (
      // With a visible "Push"/"Pull" label + icon, the badge is just the number;
      // otherwise (compact, or Sync with both directions) it carries the arrows.
      <SyncCounts
        ahead={sync.ahead}
        behind={sync.behind}
        variant="badge"
        arrows={!isLarge || sync.kind === 'diverged'}
      />
    ) : null;

  /* Status line ---------------------------------------------------------- */
  const fetched = branchInfo.last_fetched ? `fetched ${shortAgo(branchInfo.last_fetched, now)}` : '';
  let statusIcon: React.ReactNode = <Cloud size={11} className="shrink-0" />;
  let statusText: React.ReactNode;
  let statusTone = 'text-[var(--text-dim)]';
  switch (sync.kind) {
    case 'no-remote':
      // The Publish button already says it; keep the panel quiet like VS Code.
      statusText = null;
      break;
    case 'detached':
      statusIcon = <AlertTriangle size={11} className="shrink-0" />;
      statusText = 'Not on a branch — create one to keep new commits';
      statusTone = 'text-[var(--modified-fg)]';
      break;
    case 'blocked':
      statusIcon = <AlertTriangle size={11} className="shrink-0" />;
      statusText = `Finish or abort the ${gitStatus.state} first`;
      statusTone = 'text-[var(--modified-fg)]';
      break;
    case 'publish':
      statusIcon = <CloudOff size={11} className="shrink-0" />;
      statusText = `Not published to ${sync.remote} yet`;
      break;
    case 'gone':
      statusIcon = <AlertTriangle size={11} className="shrink-0" />;
      statusText = `${upstreamLabel} was deleted on the remote`;
      statusTone = 'text-[var(--modified-fg)]';
      break;
    case 'diverged':
      statusText = (
        <>
          <span className="text-[var(--modified-fg)]">Diverged</span> from {upstreamLabel}
        </>
      );
      break;
    case 'pull':
      statusText = `${sync.behind} to pull from ${upstreamLabel}`;
      break;
    case 'push':
      statusText = `${sync.ahead} to push to ${upstreamLabel}`;
      break;
    default:
      statusIcon = (
        <span className="flex items-center justify-center w-[11px] shrink-0">
          <span className="w-[6px] h-[6px] rounded-full bg-[var(--added-fg)] shadow-[0_0_0_3px_color-mix(in_srgb,var(--added-fg)_16%,transparent)]" />
        </span>
      );
      statusText = (
        <>
          <span className="text-[var(--text-muted)]">Up to date</span>
          {sync.upstream && (
            <span className="text-[var(--text-dim)]">
              {' · '}
              <span className="font-mono text-[10.5px]">{upstreamLabel}</span>
            </span>
          )}
        </>
      );
  }

  return (
    <div className="@container relative">
      <div className="flex flex-wrap items-stretch gap-1">
        {/* Branch switcher button */}
        <button
          ref={branchBtnRef}
          onClick={() => {
            setPickerOpen((v) => !v);
            setSyncMenuOpen(false);
            onRefreshBranches();
          }}
          disabled={!!gitStatus.state}
          aria-haspopup="listbox"
          aria-expanded={pickerOpen}
          className={`flex items-center gap-1.5 min-w-0 flex-1 h-[28px] px-2 rounded-md border text-[12.5px] font-medium cursor-pointer transition-colors disabled:opacity-50 disabled:cursor-not-allowed ${
            pickerOpen
              ? 'bg-[var(--bg-hover)] border-[var(--accent)]'
              : 'bg-[var(--bg-elevated)] border-[var(--border)] hover:bg-[var(--bg-hover)]'
          }`}
          title={detached ? 'Switch to a branch' : `Current branch: ${branch} — click to switch or create`}
        >
          {detached ? (
            <GitCommitHorizontal size={13} className="shrink-0 text-[var(--modified-fg)]" />
          ) : (
            <GitBranch size={13} className="shrink-0 text-[var(--accent)]" />
          )}
          <span className="truncate">
            {detached ? (
              <>
                <span className="text-[var(--text-muted)] font-normal">Detached at </span>
                <span className="font-mono">{branchInfo.head_short || gitStatus.head_hash.slice(0, 7)}</span>
              </>
            ) : (
              branch
            )}
          </span>
          <ChevronDown
            size={12}
            className={`ml-auto shrink-0 opacity-60 transition-transform ${pickerOpen ? 'rotate-180' : ''}`}
          />
        </button>

        {/* Contextual sync button (+ menu) */}
        {primary && (
          <div
            ref={syncMenuRef}
            className={`relative flex items-stretch h-[28px] ${isLarge ? 'w-full mt-0.5' : 'shrink-0'}`}
          >
            <button
              onClick={primary.onClick}
              disabled={syncDisabled}
              title={primary.title}
              className={`flex items-center gap-1.5 pl-2 pr-1.5 rounded-l-md border text-[12px] font-medium cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed ${
                isLarge ? 'flex-1 min-w-0 justify-center' : ''
              } ${
                primary.emphasis
                  ? 'bg-[var(--accent)] border-[var(--accent)] text-[var(--accent-fg)] hover:brightness-110'
                  : 'bg-[var(--bg-elevated)] border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
              }`}
            >
              {running ? (
                <Loader2 size={12.5} className="animate-spin" />
              ) : counts && !isLarge ? null : (
                primary.icon
              )}
              <span className={!running && counts && !isLarge ? 'hidden @[280px]:inline' : undefined}>
                {running ?? primary.label}
              </span>
              {!running && counts}
            </button>
            <button
              onClick={() => setSyncMenuOpen((v) => !v)}
              disabled={syncDisabled}
              aria-label="More sync actions"
              className={`flex items-center justify-center w-[24px] rounded-r-md border border-l-0 cursor-pointer disabled:opacity-50 ${
                primary.emphasis
                  ? 'bg-[var(--accent)] border-[var(--accent)] text-[var(--accent-fg)] hover:brightness-110 shadow-[inset_1px_0_0_rgba(0,0,0,0.2)]'
                  : 'bg-[var(--bg-elevated)] border-[var(--border)] text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
              }`}
            >
              <ChevronDown size={12} />
            </button>

            {syncMenuOpen && (
              <div className="absolute right-0 top-full mt-1 w-64 z-50 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-[0_12px_32px_-8px_rgba(0,0,0,0.6)] py-1 text-[12px]">
                {sync.kind === 'diverged' && (
                  <>
                    <div className="px-3 pt-1 pb-1.5 text-[11px] text-[var(--text-muted)] leading-snug">
                      You have {plural(sync.ahead, 'local commit')} and {upstreamLabel} has{' '}
                      {plural(sync.behind, 'new commit')}.
                    </div>
                    <SyncMenuItem
                      icon={<RefreshCw size={13} />}
                      label="Merge, then push"
                      hint="Keeps both histories with a merge commit"
                      onClick={() => doSync('merge')}
                    />
                    <SyncMenuItem
                      icon={<RefreshCw size={13} />}
                      label="Rebase, then push"
                      hint="Replays your commits on top of the remote"
                      onClick={() => doSync('rebase')}
                    />
                    <div className="my-1 border-t border-[var(--border)]" />
                  </>
                )}
                {sync.kind === 'no-remote' && (
                  <>
                    <SyncMenuItem
                      icon={<CloudUpload size={13} />}
                      label="Publish to GitHub…"
                      hint="Create a private or public repository"
                      onClick={() => {
                        setSyncMenuOpen(false);
                        onOpenPublish('github');
                      }}
                    />
                    <SyncMenuItem
                      icon={<Link2 size={13} />}
                      label="Add remote URL…"
                      hint="Use a repository that already exists"
                      onClick={() => {
                        setSyncMenuOpen(false);
                        onOpenPublish('remote');
                      }}
                    />
                  </>
                )}
                {sync.kind !== 'no-remote' && (
                  <>
                    <SyncMenuItem
                      icon={<RefreshCw size={13} />}
                      label="Fetch"
                      hint={fetched ? `Last ${fetched}` : 'Check the remote for new commits'}
                      onClick={doFetch}
                    />
                    <SyncMenuItem
                      icon={<ArrowDown size={13} />}
                      label="Pull (merge)"
                      disabled={sync.kind === 'publish'}
                      onClick={() => doPull('merge')}
                    />
                    <SyncMenuItem
                      icon={<ArrowDown size={13} />}
                      label="Pull (rebase)"
                      disabled={sync.kind === 'publish'}
                      onClick={() => doPull('rebase')}
                    />
                  </>
                )}
                {sync.kind === 'no-remote' ? null : sync.kind === 'publish' || sync.kind === 'gone' ? (
                  <SyncMenuItem
                    icon={<CloudUpload size={13} />}
                    label="Publish branch"
                    hint={`Push and track ${sync.remote}/${branch}`}
                    onClick={doPublish}
                  />
                ) : (
                  <SyncMenuItem
                    icon={<ArrowUp size={13} />}
                    label="Push"
                    disabled={sync.ahead === 0}
                    onClick={doPush}
                  />
                )}
              </div>
            )}
          </div>
        )}
      </div>

      {/* One-line status: where this branch stands relative to its remote */}
      {statusText && !isLarge && (
        <div
          className={`flex items-center gap-1.5 h-[20px] mt-1 px-0.5 text-[11px] ${statusTone}`}
          title={fetched ? `Last ${fetched}` : undefined}
        >
          {statusIcon}
          <span className="truncate min-w-0">{statusText}</span>
          {fetched && (
            <span className="hidden @[320px]:inline ml-auto pl-2 shrink-0 text-[var(--text-dim)]">{fetched}</span>
          )}
        </div>
      )}

      {pickerOpen && (
        <BranchPicker
          gitStatus={gitStatus}
          branchInfo={branchInfo}
          uncommittedCount={uncommittedCount}
          now={now}
          runAction={runAction}
          onToast={onToast}
          onClose={() => {
            setPickerOpen(false);
            branchBtnRef.current?.focus();
          }}
          anchorRef={branchBtnRef}
        />
      )}
    </div>
  );
};

const SyncMenuItem: React.FC<{
  icon: React.ReactNode;
  label: string;
  hint?: string;
  disabled?: boolean;
  onClick: () => void;
}> = ({ icon, label, hint, disabled, onClick }) => (
  <button
    onClick={onClick}
    disabled={disabled}
    className="w-full flex items-start gap-2 px-3 py-1.5 text-left hover:bg-[var(--bg-hover)] disabled:opacity-40 disabled:hover:bg-transparent cursor-pointer disabled:cursor-default"
  >
    <span className="mt-[1px] text-[var(--text-muted)]">{icon}</span>
    <span className="min-w-0">
      <span className="block text-[var(--text-primary)]">{label}</span>
      {hint && <span className="block text-[11px] leading-snug text-[var(--text-dim)]">{hint}</span>}
    </span>
  </button>
);

/* ------------------------------------------------------------------------- */
/* Branch picker                                                             */
/* ------------------------------------------------------------------------- */

type PickerItem =
  | { type: 'create'; name: string }
  | { type: 'local'; b: BranchInfo }
  | { type: 'remote'; b: BranchInfo };

type Pending =
  | { kind: 'switch'; item: PickerItem; target: string }
  | { kind: 'delete'; name: string; unmerged?: boolean }
  | null;

const SECTION_LIMIT = 50;

interface BranchPickerProps {
  gitStatus: GitStatusRes;
  branchInfo: BranchesRes;
  uncommittedCount: number;
  now: number;
  runAction: (fn: () => Promise<void>) => Promise<void>;
  onToast: (msg: string, isError?: boolean) => void;
  onClose: () => void;
  anchorRef: React.RefObject<HTMLButtonElement | null>;
}

const BranchPicker: React.FC<BranchPickerProps> = ({
  gitStatus,
  branchInfo,
  uncommittedCount,
  now,
  runAction,
  onToast,
  onClose,
  anchorRef,
}) => {
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const [pending, setPending] = useState<Pending>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const current = gitStatus.branch;
  const detached = branchInfo.detached ?? current === 'HEAD';

  // Prefer rich details; fall back to plain name lists from older backends.
  const { locals, remotes } = useMemo(() => {
    const details: BranchInfo[] =
      branchInfo.details ??
      [
        ...branchInfo.branches.map((name) => ({ name, kind: 'local' as const })),
        ...branchInfo.remote_branches.map((name) => ({ name, kind: 'remote' as const })),
      ].map((b) => ({
        ...b,
        is_current: b.kind === 'local' && b.name === current,
        upstream: '',
        ahead: 0,
        behind: 0,
        upstream_gone: false,
        timestamp: 0,
        relative_date: '',
        subject: '',
      }));
    const locals = details.filter((b) => b.kind === 'local');
    const tracked = new Set(locals.map((b) => b.upstream).filter(Boolean));
    const localNames = new Set(locals.map((b) => b.name));
    const remotes = details.filter(
      (b) =>
        b.kind === 'remote' &&
        !tracked.has(b.name) &&
        !localNames.has(b.name.slice(b.name.indexOf('/') + 1))
    );
    // Current branch first, then most recent commit first.
    locals.sort((a, b) => Number(b.is_current) - Number(a.is_current) || b.timestamp - a.timestamp);
    return { locals, remotes };
  }, [branchInfo, current]);

  const q = query.trim();
  const ql = q.toLowerCase();
  const matchedLocals = locals.filter((b) => b.name.toLowerCase().includes(ql));
  const matchedRemotes = remotes.filter((b) => b.name.toLowerCase().includes(ql));
  const exact = locals.some((b) => b.name === q);
  const validName = q.length > 0 && !/[\s~^:?*[\\]|\.\.|@\{|^[-/.]|[/.]$|\.lock$/.test(q);

  const items: PickerItem[] = [];
  if (q && !exact) items.push({ type: 'create', name: q });
  const shownLocals = matchedLocals.slice(0, SECTION_LIMIT);
  const shownRemotes = matchedRemotes.slice(0, SECTION_LIMIT);
  shownLocals.forEach((b) => items.push({ type: 'local', b }));
  shownRemotes.forEach((b) => items.push({ type: 'remote', b }));

  useEffect(() => setActive(0), [query]);

  useEffect(() => {
    inputRef.current?.focus();
  }, [pending]);

  // Close on outside click.
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (rootRef.current?.contains(t) || anchorRef.current?.contains(t)) return;
      onClose();
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [onClose, anchorRef]);

  useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>(`[data-idx="${active}"]`)
      ?.scrollIntoView({ block: 'nearest' });
  }, [active]);

  const checkoutTarget = (item: PickerItem): (() => Promise<void>) => {
    if (item.type === 'create') {
      return async () => {
        const res = await api.gitCheckout(item.name, true);
        if (!res.ok) throw new Error(res.stderr || 'Could not create branch');
        onToast(`Created and switched to ${item.name}`);
      };
    }
    if (item.type === 'local') {
      return async () => {
        const res = await api.gitCheckout(item.b.name, false);
        if (!res.ok) throw new Error(res.stderr || 'Checkout failed');
        onToast(`Switched to ${item.b.name}`);
      };
    }
    const rb = item.b.name;
    const localName = rb.slice(rb.indexOf('/') + 1);
    return async () => {
      let res = await api.gitCheckout(localName, false);
      if (!res.ok) res = await api.gitCheckout(localName, true, rb);
      if (!res.ok) throw new Error(res.stderr || 'Checkout failed');
      onToast(`Switched to ${localName} (tracking ${rb})`);
    };
  };

  const targetName = (item: PickerItem) =>
    item.type === 'create'
      ? item.name
      : item.type === 'local'
        ? item.b.name
        : item.b.name.slice(item.b.name.indexOf('/') + 1);

  const choose = (item: PickerItem) => {
    if (item.type === 'create' && !validName) {
      onToast(`"${item.name}" is not a valid branch name`, true);
      return;
    }
    if (item.type === 'local' && item.b.is_current) {
      onClose();
      return;
    }
    // Creating a branch always carries the changes along; switching asks.
    if (uncommittedCount > 0 && item.type !== 'create') {
      setPending({ kind: 'switch', item, target: targetName(item) });
      return;
    }
    onClose();
    runAction(checkoutTarget(item));
  };

  const switchLeavingChanges = (item: PickerItem) => {
    onClose();
    const from = detached ? 'detached HEAD' : current;
    runAction(async () => {
      const st = await api.gitStash('push', { message: `Left on ${from} (switched to ${targetName(item)})` });
      if (!st.ok) throw new Error(st.stderr || 'Could not stash changes');
      try {
        await checkoutTarget(item)();
      } catch (e) {
        await api.gitStash('pop');
        throw e;
      }
      onToast(`Your changes were stashed on ${from}`);
    });
  };

  const deleteBranch = (name: string, force: boolean) => {
    runAction(async () => {
      const res = await api.gitDeleteBranch(name, force);
      if (!res.ok) {
        if (!force && /not fully merged/i.test(res.stderr)) {
          setPending({ kind: 'delete', name, unmerged: true });
          return;
        }
        throw new Error(res.stderr || 'Delete failed');
      }
      setPending(null);
      onToast(`Deleted branch ${name}`);
    });
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      if (pending) setPending(null);
      else onClose();
      return;
    }
    if (pending) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setActive((i) => Math.min(items.length - 1, i + 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setActive((i) => Math.max(0, i - 1));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const it = items[active];
      if (it) choose(it);
    }
  };

  const base = detached ? (branchInfo.head_short || 'HEAD') : current;
  let idx = -1;

  return (
    <div
      ref={rootRef}
      role="dialog"
      aria-label="Switch branch"
      onKeyDown={onKeyDown}
      className="absolute left-0 right-0 top-[32px] z-50 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-[0_16px_40px_-10px_rgba(0,0,0,0.65),0_2px_8px_rgba(0,0,0,0.3)] overflow-hidden"
    >
      {/* Search / create */}
      <div className="flex items-center gap-1.5 px-2.5 h-9 border-b border-[var(--border)]">
        <Search size={13} className="shrink-0 text-[var(--text-dim)]" />
        <input
          ref={inputRef}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Find or create a branch…"
          spellCheck={false}
          className="flex-1 min-w-0 bg-transparent text-[12.5px] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none"
        />
      </div>

      {pending?.kind === 'switch' ? (
        <div className="p-3 space-y-2.5">
          <div className="text-[12px] leading-snug">
            You have <span className="font-semibold">{plural(uncommittedCount, 'uncommitted change')}</span>.
            What should happen to {uncommittedCount === 1 ? 'it' : 'them'}?
          </div>
          <button
            onClick={() => {
              const it = pending.item;
              onClose();
              runAction(checkoutTarget(it));
            }}
            className="w-full text-left px-2.5 py-2 rounded-md border border-[var(--accent)] bg-[var(--accent)]/10 hover:bg-[var(--accent)]/18 cursor-pointer"
          >
            <span className="block text-[12px] font-medium text-[var(--text-primary)]">
              Bring them to <span className="font-mono">{pending.target}</span>
            </span>
            <span className="block text-[11px] text-[var(--text-muted)]">
              Keep working on them after switching
            </span>
          </button>
          <button
            onClick={() => switchLeavingChanges(pending.item)}
            className="w-full text-left px-2.5 py-2 rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] cursor-pointer"
          >
            <span className="block text-[12px] font-medium text-[var(--text-primary)]">
              Leave them on <span className="font-mono">{detached ? 'this commit' : current}</span>
            </span>
            <span className="block text-[11px] text-[var(--text-muted)]">
              Stashed — restore later from Stashes
            </span>
          </button>
          <button
            onClick={() => setPending(null)}
            className="w-full h-7 text-[12px] rounded-md text-[var(--text-muted)] hover:bg-[var(--bg-hover)] cursor-pointer"
          >
            Cancel
          </button>
        </div>
      ) : (
        <>
          <div ref={listRef} role="listbox" className="max-h-[320px] overflow-y-auto py-1">
            {items.length === 0 && !q && (
              <div className="px-3 py-3 text-[12px] text-[var(--text-dim)]">No branches yet.</div>
            )}

            {items[0]?.type === 'create' &&
              (() => {
                idx++;
                const i = idx;
                const it = items[0];
                return (
                  <button
                    data-idx={i}
                    onMouseEnter={() => setActive(i)}
                    onClick={() => choose(it)}
                    className={`w-full flex items-center gap-2 px-2.5 h-8 text-left text-[12.5px] cursor-pointer ${
                      active === i ? 'bg-[var(--accent)]/15' : ''
                    }`}
                  >
                    <Plus size={13} className="shrink-0 text-[var(--accent)]" />
                    <span className="truncate">
                      Create branch <span className="font-semibold">{it.name}</span>
                      <span className="text-[var(--text-dim)]"> from {base}</span>
                    </span>
                    {!validName && (
                      <span className="ml-auto shrink-0 text-[10.5px] text-[var(--removed-fg)]">invalid name</span>
                    )}
                  </button>
                );
              })()}

            {shownLocals.length > 0 && <SectionLabel>Local branches</SectionLabel>}
            {shownLocals.map((b) => {
              idx++;
              const i = idx;
              const confirming = pending?.kind === 'delete' && pending.name === b.name;
              if (confirming) {
                return (
                  <div
                    key={`l:${b.name}`}
                    data-idx={i}
                    className="mx-1.5 my-0.5 px-2 py-1.5 rounded-md bg-[var(--removed-fg)]/10 border border-[var(--removed-fg)]/40 text-[12px]"
                  >
                    <div className="mb-1.5 leading-snug">
                      {pending.unmerged ? (
                        <>
                          <span className="font-mono">{b.name}</span> has commits that aren&rsquo;t merged
                          anywhere. Delete them too?
                        </>
                      ) : (
                        <>
                          Delete <span className="font-mono">{b.name}</span>?
                        </>
                      )}
                    </div>
                    <div className="flex gap-1.5">
                      <button
                        onClick={() => deleteBranch(b.name, !!pending.unmerged)}
                        className="h-6 px-2 rounded bg-[var(--removed-fg)] text-white text-[11.5px] font-medium cursor-pointer"
                      >
                        {pending.unmerged ? 'Delete anyway' : 'Delete'}
                      </button>
                      <button
                        onClick={() => setPending(null)}
                        className="h-6 px-2 rounded text-[11.5px] text-[var(--text-muted)] hover:bg-[var(--bg-hover)] cursor-pointer"
                      >
                        Cancel
                      </button>
                    </div>
                  </div>
                );
              }
              return (
                <BranchRow
                  key={`l:${b.name}`}
                  idx={i}
                  active={active === i}
                  b={b}
                  now={now}
                  isDefault={b.name === branchInfo.default_branch}
                  hasRemote={(branchInfo.remotes?.length ?? 0) > 0 || !!gitStatus.remote_url}
                  onHover={() => setActive(i)}
                  onChoose={() => choose({ type: 'local', b })}
                  onDelete={
                    b.is_current ? undefined : () => setPending({ kind: 'delete', name: b.name })
                  }
                />
              );
            })}
            {matchedLocals.length > SECTION_LIMIT && (
              <MoreHint n={matchedLocals.length - SECTION_LIMIT} />
            )}

            {shownRemotes.length > 0 && <SectionLabel>Remote only</SectionLabel>}
            {shownRemotes.map((b) => {
              idx++;
              const i = idx;
              return (
                <BranchRow
                  key={`r:${b.name}`}
                  idx={i}
                  active={active === i}
                  b={b}
                  now={now}
                  remote
                  onHover={() => setActive(i)}
                  onChoose={() => choose({ type: 'remote', b })}
                />
              );
            })}
            {matchedRemotes.length > SECTION_LIMIT && (
              <MoreHint n={matchedRemotes.length - SECTION_LIMIT} />
            )}
          </div>

          <div className="flex items-center gap-3 px-2.5 h-7 border-t border-[var(--border)] text-[10.5px] text-[var(--text-dim)]">
            <span>↑↓ to move</span>
            <span className="flex items-center gap-0.5">
              <CornerDownLeft size={10} /> to {items[active]?.type === 'create' ? 'create' : 'switch'}
            </span>
            <span>Esc to close</span>
          </div>
        </>
      )}
    </div>
  );
};

const SectionLabel: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div className="px-2.5 pt-2 pb-1 text-[10px] font-semibold uppercase tracking-wider text-[var(--text-dim)]">
    {children}
  </div>
);

const MoreHint: React.FC<{ n: number }> = ({ n }) => (
  <div className="px-2.5 py-1 text-[11px] text-[var(--text-dim)]">
    {n} more — type to filter
  </div>
);

const BranchRow: React.FC<{
  idx: number;
  active: boolean;
  b: BranchInfo;
  now: number;
  remote?: boolean;
  isDefault?: boolean;
  hasRemote?: boolean;
  onHover: () => void;
  onChoose: () => void;
  onDelete?: () => void;
}> = ({ idx, active, b, now, remote, isDefault, hasRemote, onHover, onChoose, onDelete }) => {
  const title = [
    b.name,
    b.subject && `Last commit: ${b.subject}`,
    b.relative_date && `(${b.relative_date})`,
    b.upstream && `Tracks ${b.upstream}`,
  ]
    .filter(Boolean)
    .join('\n');
  return (
    <div
      data-idx={idx}
      role="option"
      aria-selected={active}
      onMouseEnter={onHover}
      onClick={onChoose}
      title={title}
      className={`group relative flex items-center gap-2 px-2.5 h-[30px] cursor-pointer ${
        active ? 'bg-[var(--accent)]/15' : ''
      }`}
    >
      {b.is_current ? (
        <Check size={13} className="shrink-0 text-[var(--accent)]" />
      ) : remote ? (
        <Cloud size={13} className="shrink-0 text-[var(--text-dim)]" />
      ) : (
        <GitBranch size={13} className="shrink-0 text-[var(--text-dim)]" />
      )}
      <span
        className={`truncate text-[12.5px] ${
          b.is_current ? 'font-semibold text-[var(--text-primary)]' : remote ? 'text-[var(--text-muted)]' : 'text-[var(--text-primary)]'
        }`}
      >
        {b.name}
      </span>
      {isDefault && (
        <span className="shrink-0 px-1 h-[15px] leading-[15px] rounded text-[9.5px] font-medium text-[var(--text-muted)] bg-[var(--bg-hover)]">
          default
        </span>
      )}

      <span className="ml-auto pl-1 flex items-center gap-1.5 shrink-0 text-[10.5px] tabular-nums text-[var(--text-dim)] group-hover:invisible">
        {!remote && b.upstream_gone && <span className="text-[var(--modified-fg)]">remote deleted</span>}
        {!remote && !b.upstream && hasRemote && !b.upstream_gone && <span>local only</span>}
        <SyncCounts ahead={b.ahead} behind={b.behind} className="text-[var(--text-muted)]" />
        {b.timestamp > 0 && <span>{compactAgo(b.timestamp, now)}</span>}
      </span>

      {onDelete && (
        <button
          onClick={(e) => {
            e.stopPropagation();
            onDelete();
          }}
          aria-label={`Delete branch ${b.name}`}
          className="absolute right-1.5 hidden group-hover:flex items-center justify-center w-6 h-6 rounded text-[var(--text-muted)] hover:text-[var(--removed-fg)] hover:bg-[var(--bg-hover)] cursor-pointer"
        >
          <Trash2 size={12} />
        </button>
      )}
    </div>
  );
};
