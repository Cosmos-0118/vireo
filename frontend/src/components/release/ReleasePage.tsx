import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertTriangle,
  ArrowRight,
  Bot,
  Code2,
  ExternalLink,
  Eye,
  FileText,
  GitBranch,
  GitCommitHorizontal,
  Pencil,
  RefreshCw,
  Rocket,
  ShieldCheck,
  Sparkles,
  Star,
  Tag,
  XCircle,
} from 'lucide-react';
import { api } from '../../api';
import type {
  ReleaseActivity,
  ReleaseFileChange,
  ReleaseOutcome,
  ReleaseReport,
  ReleaseStatus,
} from '../../types';
import type { LiteTheme } from '../../themes';
import { FileIcon } from '../FileIcon';
import { MarkdownMessage } from '../MarkdownView';
import { ActivityFeed, activityTitle } from './ActivityFeed';
import { ReviewDialog } from './ReviewDialog';
import {
  Badge,
  CheckList,
  Panel,
  Spinner,
  Step,
  TONE,
  Toggle,
  btnGhost,
  btnPrimary,
  btnQuiet,
  isLock,
  relTime,
  splitPath,
  tint,
  type StepState,
} from './ui';

interface ReleasePageProps {
  /** The Release tab is the visible tab (the page stays mounted while other tabs are shown). */
  active: boolean;
  fsVersion: number;
  theme: LiteTheme;
  onOpenFile: (path: string) => void;
  onAskAgent: (text: string) => void;
  onShowChanges: () => void;
  onRefreshGit: () => Promise<void>;
  onToast: (msg: string, isError?: boolean) => void;
}

type BumpKey = 'patch' | 'minor' | 'major' | 'prerelease' | 'custom';
const BUMPS: { key: Exclude<BumpKey, 'custom'>; label: string; hint: string }[] = [
  { key: 'patch', label: 'Patch', hint: 'Bug fixes only' },
  { key: 'minor', label: 'Minor', hint: 'New features' },
  { key: 'major', label: 'Major', hint: 'Breaking changes' },
  { key: 'prerelease', label: 'Pre-release', hint: 'Release candidate' },
];

const MODE: Record<ReleaseStatus['mode'], { title: string; text: string; finish: string }> = {
  ci: {
    title: 'CI release',
    text: 'Pushing the tag starts your release workflow, which builds and publishes.',
    finish: 'Your CI workflow builds and publishes the release',
  },
  gh: {
    title: 'GitHub release',
    text: 'Creates a GitHub release with the GitHub CLI (draft unless you publish now).',
    finish: 'Create the GitHub release',
  },
  local: {
    title: 'Local release',
    text: 'No GitHub remote: the commit and tag stay on this machine.',
    finish: 'Nothing is pushed (no remote)',
  },
};

const SEMVER = /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$/;
const POLL_MS = 3000;

type Notes = { path: string; markdown: string; draft: boolean; commits?: number } | null;
type Dialog = { plan: ReleaseOutcome | null; outcome: ReleaseOutcome | null; verify: ReleaseReport | null; fromAgent?: boolean };

export const ReleasePage: React.FC<ReleasePageProps> = ({
  active,
  fsVersion,
  theme,
  onOpenFile,
  onAskAgent,
  onShowChanges,
  onRefreshGit,
  onToast,
}) => {
  const [status, setStatus] = useState<ReleaseStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [bump, setBump] = useState<BumpKey | null>(null);
  const [custom, setCustom] = useState('');
  const [skipped, setSkipped] = useState<Set<string>>(new Set());
  const [diff, setDiff] = useState<ReleaseFileChange[] | null>(null);
  const [withTests, setWithTests] = useState(true);
  const [report, setReport] = useState<ReleaseReport | null>(null);
  const [changelog, setChangelog] = useState(false);
  const [goPublic, setGoPublic] = useState(false);
  const [notes, setNotes] = useState<Notes>(null);
  const [notesView, setNotesView] = useState<'preview' | 'source'>('preview');
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [activity, setActivity] = useState<ReleaseActivity[]>([]);
  const [, setNow] = useState(0); // re-render relative times
  const mountedAt = useRef(Date.now());
  const lastSeen = useRef<string>('');

  // ---- Data -------------------------------------------------------------------------------

  const load = useCallback(async () => {
    try {
      const s = await api.releaseStatus();
      setStatus(s);
      setError(null);
      setBump((cur) => {
        if (cur) return cur;
        const hit = BUMPS.find((b) => s.bumps[b.key] === s.suggested.version);
        return hit ? hit.key : 'custom';
      });
      setCustom((cur) => cur || s.suggested.version || '');
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    const t = setTimeout(load, status ? 600 : 0);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, fsVersion, load]);

  const target = useMemo(() => {
    if (!status || !bump) return '';
    return bump === 'custom' ? custom.trim() : status.bumps[bump] ?? '';
  }, [status, bump, custom]);
  const targetValid = SEMVER.test(target);
  const tag = targetValid && status ? `${status.tag_prefix}${target}` : '';

  const versionFiles = useMemo(() => (status ? status.manifests.filter((m) => !isLock(m.path)) : []), [status]);
  const only = useMemo(
    () => (skipped.size === 0 ? [] : versionFiles.filter((m) => !skipped.has(m.path)).map((m) => m.path)),
    [versionFiles, skipped]
  );

  // A different version or file selection invalidates what was computed for the old one.
  useEffect(() => {
    setDiff(null);
    setReport(null);
  }, [target, skipped]);

  const notesPath = useMemo(() => {
    if (!status || !tag) return '';
    const dir = status.notes_path.slice(0, status.notes_path.lastIndexOf('/') + 1);
    return `${dir}${tag}.md`;
  }, [status, tag]);

  // Notes: the saved draft when there is one, else a preview generated from commits (not saved).
  const loadNotes = useCallback(async () => {
    if (!notesPath || !targetValid) {
      setNotes(null);
      return;
    }
    try {
      const f = await api.readFile(notesPath);
      setNotes({ path: notesPath, markdown: f.content, draft: true });
    } catch {
      try {
        const r = await api.releaseNotes({ version: target });
        setNotes({ path: notesPath, markdown: r.markdown, draft: false, commits: r.commits });
      } catch {
        setNotes(null);
      }
    }
  }, [notesPath, target, targetValid]);

  useEffect(() => {
    if (!active) return;
    const t = setTimeout(loadNotes, 250);
    return () => clearTimeout(t);
  }, [active, fsVersion, loadNotes]);

  // Activity: poll while visible so release steps run by an agent show up live. Callbacks are
  // read through a ref so the poller never re-subscribes when a parent re-renders.
  const live = useRef({ load, loadNotes, onRefreshGit, onToast });
  live.current = { load, loadNotes, onRefreshGit, onToast };
  const pollActivity = useCallback(async () => {
    const { load, loadNotes, onRefreshGit, onToast } = live.current;
    try {
      const { items } = await api.releaseActivity();
      setActivity(items);
      const head = items[0];
      const key = head ? `${head.id}:${head.phase}` : '';
      if (head && key !== lastSeen.current) {
        const first = lastSeen.current === '';
        lastSeen.current = key;
        if (!first && head.source !== 'ui') {
          void load();
          void loadNotes();
          if (head.ts >= mountedAt.current && head.phase === 'done') {
            onToast(`${head.source === 'agent' ? 'Agent' : 'Terminal'}: ${activityTitle(head)}`, head.ok === false);
          }
          if (head.action === 'publish' && !head.dry_run && head.phase === 'done') void onRefreshGit();
        }
      }
    } catch {
      /* keep the last list */
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void pollActivity();
    const t = setInterval(() => {
      void pollActivity();
      setNow((n) => n + 1);
    }, POLL_MS);
    return () => clearInterval(t);
  }, [active, pollActivity]);

  // ---- Actions ----------------------------------------------------------------------------

  const run = async <T,>(name: string, fn: () => Promise<T>, done: (v: T) => void | Promise<void>) => {
    setBusy(name);
    try {
      await done(await fn());
    } catch (e: unknown) {
      onToast(e instanceof Error ? e.message : String(e), true);
    } finally {
      setBusy(null);
      void pollActivity();
    }
  };

  const previewBump = () =>
    run('diff', () => api.releaseBump(target, { dry_run: true, only }), (r) => setDiff(r.files));

  const runChecks = () =>
    run('checks', () => api.releasePreflight({ version: target, skip_tests: !withTests }), setReport);

  const saveNotes = () =>
    run(
      'notes',
      () => api.releaseNotes({ version: target, write: true, changelog }),
      async (r) => {
        await loadNotes();
        if (r.written) onOpenFile(r.written);
      }
    );

  const reviewPlan = () =>
    run(
      'plan',
      () => api.releasePublish({ version: target, dry_run: true, only, publish: goPublic, skip_tests: !withTests }),
      (p) => setDialog({ plan: p, outcome: null, verify: null })
    );

  const release = (version: string, files: string[]) =>
    run(
      'release',
      () => api.releasePublish({ version, skip_tests: !withTests, only: files, publish: goPublic }),
      async (o) => {
        setDialog((d) => ({ plan: d?.plan ?? null, outcome: o, verify: null, fromAgent: d?.fromAgent }));
        onToast(o.ok ? `Released ${o.tag}` : 'Release stopped. See the dialog for the failed step.', !o.ok);
        if (o.ok) {
          setBump(null); // the released version is the new baseline: recompute the suggestion
          setCustom('');
        }
        await onRefreshGit();
        void load();
      }
    );

  const confirmPlan = () => {
    const p = dialog?.plan;
    if (!p) return;
    // An agent's plan carries its own file list; ours uses the current selection.
    const files = dialog?.fromAgent
      ? (() => {
          const planned = p.files.map((f) => f.path).filter((x) => !isLock(x));
          return planned.length === versionFiles.length ? [] : planned;
        })()
      : only;
    void release(p.version, files);
  };

  const runVerify = (version: string) =>
    run('verify', () => api.releaseVerify({ version }), (v) => setDialog((d) => (d ? { ...d, verify: v } : d)));

  const openActivity = (e: ReleaseActivity) => {
    if (!e.detail) return;
    if ('steps' in e.detail) {
      const o = e.detail as ReleaseOutcome;
      setDialog(o.dry_run ? { plan: o, outcome: null, verify: null, fromAgent: e.source !== 'ui' } : { plan: null, outcome: o, verify: null });
    } else {
      const r = e.detail as ReleaseReport;
      setDialog({
        plan: null,
        outcome: { ok: r.ok, dry_run: true, version: r.version, tag: r.tag, mode: status?.mode ?? '', preflight: [], steps: r.checks, files: [], notes_path: null, url: null },
        verify: null,
      });
    }
  };

  const toggleFile = (p: string) =>
    setSkipped((cur) => {
      const n = new Set(cur);
      if (n.has(p)) n.delete(p);
      else n.add(p);
      return n;
    });

  const openUrl = (u: string) => void api.openExternalUrl(u);

  // ---- Agent hand-off ---------------------------------------------------------------------

  const tool = 'the `release` MCP tool (or `vx release` in the terminal)';
  const askNotes = () =>
    onAskAgent(
      `Write the release notes for ${tag}. Use ${tool}: run \`notes\` with version ${target} and write: true to save the draft at \`${notesPath}\`, ` +
        'then edit that file: add a 2-3 sentence **Highlights** summary at the top, keep every statement grounded in the listed commits ' +
        '(do not invent or drop changes), and keep breaking changes prominent. Do not bump, tag, push or publish.'
    );
  const askShip = () =>
    onAskAgent(
      `Prepare and ship release ${tag} using ${tool}. I am watching progress on the Releases page.\n` +
        `1. \`status\` to confirm the version and version files.\n` +
        `2. If \`${notesPath}\` does not exist, run \`notes\` (version ${target}, write: true) and polish it with a short Highlights section.\n` +
        `3. \`preflight\` (version ${target}). If a check fails, tell me why and stop; do not work around it.\n` +
        `4. \`publish\` with dry_run: true and show me the plan.\n` +
        `5. Only after I confirm: \`publish\` (version ${target}${status?.mode === 'gh' && goPublic ? ', publish: true' : ''}), then \`verify\`.`
    );

  // ---- Render -----------------------------------------------------------------------------

  if (!status) {
    return (
      <div className="flex-1 flex items-center justify-center text-[12.5px] text-[var(--text-muted)] p-6 text-center">
        {error ? (
          <div className="flex flex-col items-center gap-3 max-w-[420px]">
            <XCircle size={24} style={{ color: TONE.fail }} />
            <span className="break-words">{error}</span>
            <button className={btnGhost} onClick={load}>
              <RefreshCw size={13} /> Retry
            </button>
          </div>
        ) : (
          <span className="flex items-center gap-2">
            <Spinner /> Reading release state…
          </span>
        )}
      </div>
    );
  }

  const mode = MODE[status.mode];
  const selectedFiles = versionFiles.length - skipped.size;
  const lockFiles = status.manifests.length - versionFiles.length;
  const canAct = targetValid && !busy;
  const lastRelease = status.releases[0];
  const counts = report
    ? {
        ok: report.checks.filter((c) => c.status === 'ok').length,
        bad: report.checks.filter((c) => c.status === 'fail').length,
        warn: report.checks.filter((c) => c.status === 'warn').length,
      }
    : null;

  const running = activity.find((e) => e.phase === 'running' && Date.now() - e.ts < 30 * 60_000);
  const agentPlan = activity.find(
    (e) =>
      e.source !== 'ui' &&
      e.action === 'publish' &&
      e.dry_run &&
      e.ok &&
      e.ts >= Date.now() - 30 * 60_000 &&
      // Not yet superseded by a real release of the same tag.
      !activity.some((x) => x.action === 'publish' && !x.dry_run && x.tag === e.tag && x.ts > e.ts)
  );

  const versionState: StepState = targetValid ? 'done' : 'todo';
  const filesState: StepState = versionFiles.length === 0 ? 'warn' : selectedFiles === 0 ? 'warn' : 'done';
  const notesState: StepState = notes?.draft ? 'done' : 'todo';
  const checksState: StepState =
    busy === 'checks' ? 'running' : !counts ? 'todo' : counts.bad ? 'fail' : counts.warn ? 'warn' : 'done';

  return (
    <div className="flex-1 min-h-0 overflow-y-auto bg-[var(--bg-base)] select-text">
      <div className="mx-auto w-full max-w-[1180px] px-8 py-7">
        {/* Header */}
        <header className="flex flex-wrap items-center gap-4">
          <span
            className="w-10 h-10 rounded-xl flex items-center justify-center shrink-0"
            style={{ background: tint('accent', 16), color: TONE.accent }}
          >
            <Rocket size={19} />
          </span>
          <div className="min-w-0 flex-1">
            <h1 className="text-[18px] font-semibold tracking-tight text-[var(--text-primary)]">Releases</h1>
            <p className="text-[12.5px] text-[var(--text-muted)] truncate">
              {status.repo ?? 'This repository'} · {mode.text}
            </p>
          </div>
          <div className="flex items-center gap-1.5">
            <button className={btnQuiet} onClick={() => void Promise.all([load(), loadNotes(), pollActivity()])} title="Refresh">
              <RefreshCw size={13} /> Refresh
            </button>
            {status.repo && (
              <button className={btnGhost} onClick={() => openUrl(`https://github.com/${status.repo}/releases`)}>
                <ExternalLink size={13} /> GitHub releases
              </button>
            )}
          </div>
        </header>

        {/* Live banners: what an agent (or the terminal) is doing right now */}
        {running && (
          <Banner tone="accent" icon={<Spinner size={15} />}>
            <b>{running.source === 'agent' ? 'An agent is' : running.source === 'cli' ? 'The terminal is' : 'Vireo is'} releasing {running.tag}.</b>{' '}
            Steps appear in Activity as they finish.
          </Banner>
        )}
        {!running && agentPlan && !dialog && (
          <Banner
            tone="accent"
            icon={<Bot size={15} />}
            action={
              <button className={btnPrimary} onClick={() => openActivity(agentPlan)}>
                Review plan
              </button>
            }
          >
            <b>An agent prepared release {agentPlan.tag}.</b> Nothing has changed yet. Review the plan, then confirm here or tell the agent to go ahead.
          </Banner>
        )}

        {/* Summary strip */}
        <Panel className="mt-5 grid grid-cols-1 md:grid-cols-4 divide-y md:divide-y-0 md:divide-x divide-[var(--border)] overflow-hidden">
          <Cell label="Current">
            <div className="font-mono text-[20px] font-semibold text-[var(--text-primary)] leading-tight">{status.version}</div>
            <div className="mt-1 text-[11.5px] text-[var(--text-muted)]">
              {lastRelease ? (
                <>
                  {lastRelease.tag}
                  {lastRelease.date && <> · {relTime(Date.parse(lastRelease.date))}</>}
                </>
              ) : (
                'Never released'
              )}
            </div>
          </Cell>
          <Cell label="Next" highlight>
            <div className="flex items-center gap-2 font-mono text-[20px] font-semibold leading-tight" style={{ color: targetValid ? TONE.accent : 'var(--text-dim)' }}>
              <ArrowRight size={16} className="text-[var(--text-dim)]" />
              {targetValid ? target : '—'}
            </div>
            <div className="mt-1 text-[11.5px] text-[var(--text-muted)] flex items-center gap-1">
              <Tag size={11} /> {tag || 'Choose a version below'}
            </div>
          </Cell>
          <Cell label={`Changes since ${status.latest_stable ?? 'the start'}`}>
            <div className="flex items-center gap-1.5 text-[20px] font-semibold text-[var(--text-primary)] leading-tight">
              <GitCommitHorizontal size={17} className="text-[var(--text-dim)]" />
              {status.commits_since}
              <span className="text-[12px] font-normal text-[var(--text-muted)]">commit{status.commits_since === 1 ? '' : 's'}</span>
            </div>
            <div className="mt-1.5 flex flex-wrap gap-1">
              {status.feats > 0 && <Badge tone="ok">{status.feats} feature{status.feats === 1 ? '' : 's'}</Badge>}
              {status.fixes > 0 && <Badge tone="warn">{status.fixes} fix{status.fixes === 1 ? '' : 'es'}</Badge>}
              {status.breaking > 0 && <Badge tone="fail">{status.breaking} breaking</Badge>}
              {status.feats + status.fixes + status.breaking === 0 && <Badge>no conventional commits</Badge>}
            </div>
          </Cell>
          <Cell label="Branch">
            <div className="flex items-center gap-1.5 text-[15px] font-semibold text-[var(--text-primary)] leading-tight">
              <GitBranch size={15} className="text-[var(--text-dim)]" />
              {status.branch ?? 'detached'}
            </div>
            <div className="mt-1.5">
              {status.dirty > 0 ? (
                <button onClick={onShowChanges} className="cursor-pointer" title="Open Source Control">
                  <Badge tone="warn">
                    {status.dirty} uncommitted file{status.dirty === 1 ? '' : 's'}
                  </Badge>
                </button>
              ) : (
                <Badge tone="ok">clean</Badge>
              )}
            </div>
          </Cell>
        </Panel>

        {status.drift.length > 0 && (
          <Banner tone="warn" icon={<AlertTriangle size={15} />}>
            Version files disagree ({status.drift.join(', ')}). Releasing aligns them all to {targetValid ? target : 'the new version'}.
          </Banner>
        )}

        {/* Body */}
        <div className="mt-7 grid grid-cols-1 lg:grid-cols-[minmax(0,1fr)_340px] gap-8 items-start">
          <div className="min-w-0">
            {/* 1. Version */}
            <Step
              n={1}
              id="release-version"
              title="Choose the version"
              subtitle={
                <span className="inline-flex items-start gap-1">
                  <Star size={11} fill="currentColor" className="shrink-0 mt-[3px]" style={{ color: TONE.warn }} />
                  {status.suggested.reason}
                </span>
              }
              state={versionState}
            >
              <div className="grid grid-cols-2 xl:grid-cols-4 gap-2">
                {BUMPS.map((b) => {
                  const sel = bump === b.key;
                  const suggested = status.suggested.version === status.bumps[b.key];
                  return (
                    <button
                      key={b.key}
                      onClick={() => setBump(b.key)}
                      className="relative text-left rounded-lg border px-3 py-2.5 cursor-pointer transition-colors hover:bg-[var(--bg-hover)]"
                      style={{
                        borderColor: sel ? 'var(--accent)' : 'var(--border)',
                        background: sel ? tint('accent', 10) : 'var(--bg-elevated)',
                        boxShadow: sel ? `0 0 0 1px ${tint('accent', 45)}` : 'none',
                      }}
                    >
                      <div className="flex items-center justify-between gap-1">
                        <span className="text-[12px] font-medium text-[var(--text-primary)]">{b.label}</span>
                        {suggested && (
                          <span className="inline-flex items-center gap-0.5 text-[10.5px]" style={{ color: TONE.warn }}>
                            <Star size={10} fill="currentColor" /> Suggested
                          </span>
                        )}
                      </div>
                      <div className="mt-1 font-mono text-[15px] font-semibold text-[var(--text-primary)]">{status.bumps[b.key]}</div>
                      <div className="text-[11px] text-[var(--text-muted)]">{b.hint}</div>
                    </button>
                  );
                })}
              </div>
              <div className="mt-2 flex items-center gap-2">
                <span className="text-[12px] text-[var(--text-muted)] shrink-0">or custom</span>
                <input
                  value={custom}
                  onFocus={() => setBump('custom')}
                  onChange={(e) => {
                    setCustom(e.target.value);
                    setBump('custom');
                  }}
                  placeholder="e.g. 1.0.0-rc.1"
                  spellCheck={false}
                  className="flex-1 max-w-[260px] h-8 px-2.5 rounded-md bg-[var(--bg-input)] border text-[12.5px] font-mono text-[var(--text-primary)] outline-none focus:border-[var(--accent)]"
                  style={{ borderColor: bump === 'custom' ? 'var(--accent)' : 'var(--border)' }}
                />
                {bump === 'custom' && custom.trim() && !targetValid && (
                  <span className="text-[11.5px]" style={{ color: TONE.fail }}>
                    Use MAJOR.MINOR.PATCH, e.g. 1.2.0
                  </span>
                )}
              </div>
            </Step>

            {/* 2. Version files */}
            <Step
              n={2}
              id="release-files"
              title="Version files"
              subtitle={
                versionFiles.length === 0
                  ? 'No version files found: only the tag is created.'
                  : `These files get the new version${lockFiles ? `; ${lockFiles} lockfile${lockFiles === 1 ? '' : 's'} follow automatically` : ''}.`
              }
              state={filesState}
              badge={versionFiles.length > 0 ? <Badge>{selectedFiles}/{versionFiles.length} selected</Badge> : undefined}
              actions={
                versionFiles.length > 0 && (
                  <button className={btnGhost} disabled={!canAct || selectedFiles === 0} onClick={previewBump}>
                    {busy === 'diff' ? <Spinner /> : <Eye size={13} />} {diff ? 'Refresh diff' : 'Preview diff'}
                  </button>
                )
              }
            >
              {versionFiles.length === 0 ? (
                <p className="text-[12px] text-[var(--text-muted)]">
                  Add custom version files in <code className="font-mono">.vireo/release.json</code>.
                </p>
              ) : (
                <Panel className="overflow-hidden">
                  <table className="w-full text-[12.5px]">
                    <thead>
                      <tr className="text-[11px] uppercase tracking-wider text-[var(--text-dim)] border-b border-[var(--border)]">
                        <th className="w-8 py-2 pl-3" />
                        <th className="py-2 text-left font-medium">File</th>
                        <th className="py-2 text-left font-medium hidden md:table-cell">Stack</th>
                        <th className="py-2 pr-3 text-right font-medium">Version</th>
                      </tr>
                    </thead>
                    <tbody>
                      {versionFiles.map((m) => {
                        const off = skipped.has(m.path);
                        const stale = !!m.version && m.version !== status.version;
                        const { dir, name } = splitPath(m.path);
                        return (
                          <tr key={m.path} className="border-t border-[var(--border)] first:border-t-0 hover:bg-[var(--bg-hover)]" style={{ opacity: off ? 0.5 : 1 }}>
                            <td className="py-2 pl-3">
                              <input
                                type="checkbox"
                                checked={!off}
                                onChange={() => toggleFile(m.path)}
                                className="cursor-pointer"
                                style={{ accentColor: 'var(--accent)' }}
                                aria-label={`Include ${m.path}`}
                              />
                            </td>
                            <td className="py-2">
                              <button className="flex items-center gap-2 min-w-0 text-left cursor-pointer" onClick={() => onOpenFile(m.path)} title={m.note ?? `Open ${m.path}`}>
                                <FileIcon path={m.path} size={14} />
                                <span className="text-[var(--text-primary)] hover:underline">{name}</span>
                                {dir && <span className="text-[11px] text-[var(--text-dim)] truncate">{dir}</span>}
                              </button>
                            </td>
                            <td className="py-2 text-[var(--text-muted)] hidden md:table-cell">{m.stack}</td>
                            <td className="py-2 pr-3 text-right font-mono text-[12px] whitespace-nowrap">
                              <span style={{ color: stale ? TONE.warn : 'var(--text-muted)' }} title={stale ? `Differs from ${status.version}` : undefined}>
                                {m.version ?? '—'}
                              </span>
                              {!off && targetValid && m.version !== target && (
                                <>
                                  <ArrowRight size={11} className="inline mx-1 text-[var(--text-dim)]" />
                                  <span style={{ color: TONE.ok }}>{target}</span>
                                </>
                              )}
                            </td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </Panel>
              )}
              {diff && <DiffList diff={diff} target={target} />}
            </Step>

            {/* 3. Notes */}
            <Step
              n={3}
              id="release-notes"
              title="Release notes"
              subtitle={
                notes?.draft ? (
                  <>
                    Draft saved at <span className="font-mono text-[var(--text-primary)]">{notes.path}</span>. It becomes the release body.
                  </>
                ) : (
                  'Preview generated from commits. Save it as a draft to edit, or let an agent write it. Without a draft, this text is used.'
                )
              }
              state={notesState}
              badge={notes ? <Badge tone={notes.draft ? 'ok' : 'neutral'}>{notes.draft ? 'draft saved' : 'auto preview'}</Badge> : undefined}
              actions={
                <>
                  <button className={btnGhost} disabled={!canAct} onClick={askNotes} title="Sends instructions to the agent panel">
                    <Sparkles size={13} /> Write with agent
                  </button>
                  {notes?.draft ? (
                    <button className={btnGhost} onClick={() => onOpenFile(notes.path)}>
                      <Pencil size={13} /> Edit
                    </button>
                  ) : (
                    <button className={btnGhost} disabled={!canAct} onClick={saveNotes}>
                      {busy === 'notes' ? <Spinner /> : <FileText size={13} />} Save draft &amp; edit
                    </button>
                  )}
                </>
              }
            >
              <Panel className="overflow-hidden">
                <div className="flex items-center gap-1 px-2 h-9 border-b border-[var(--border)]">
                  <Seg on={notesView === 'preview'} onClick={() => setNotesView('preview')} icon={<Eye size={12} />}>
                    Preview
                  </Seg>
                  <Seg on={notesView === 'source'} onClick={() => setNotesView('source')} icon={<Code2 size={12} />}>
                    Markdown
                  </Seg>
                  {!notes?.draft && (
                    <div className="ml-auto pr-1">
                      <Toggle checked={changelog} onChange={setChangelog} label="Also add to CHANGELOG.md when saving" />
                    </div>
                  )}
                </div>
                <div className="max-h-[440px] overflow-y-auto px-5 py-4">
                  {!notes ? (
                    <span className="flex items-center gap-2 text-[12px] text-[var(--text-muted)]">
                      {targetValid ? (
                        <>
                          <Spinner /> Generating preview…
                        </>
                      ) : (
                        'Choose a valid version to preview notes.'
                      )}
                    </span>
                  ) : notesView === 'preview' ? (
                    <MarkdownMessage content={notes.markdown} theme={theme} variant="document" filePath={notes.path} />
                  ) : (
                    <pre className="font-mono text-[12px] leading-relaxed whitespace-pre-wrap text-[var(--text-primary)]">{notes.markdown}</pre>
                  )}
                </div>
              </Panel>
            </Step>

            {/* 4. Readiness */}
            <Step
              n={4}
              id="release-checks"
              title="Readiness checks"
              subtitle="Clean working tree, branch in sync with origin, free tag, version files and (optionally) build & tests."
              state={checksState}
              last
              badge={
                counts &&
                (counts.bad > 0 ? (
                  <Badge tone="fail">{counts.bad} failed</Badge>
                ) : counts.warn > 0 ? (
                  <Badge tone="warn">
                    {counts.warn} warning{counts.warn === 1 ? '' : 's'}
                  </Badge>
                ) : (
                  <Badge tone="ok">all {counts.ok} passed</Badge>
                ))
              }
              actions={
                <button className={btnGhost} disabled={!canAct} onClick={runChecks}>
                  {busy === 'checks' ? <Spinner /> : <ShieldCheck size={13} />} {report ? 'Run again' : 'Run checks'}
                </button>
              }
            >
              {!report && status.dirty > 0 && (
                <Panel className="px-4 py-3 mb-3">
                  <div className="flex items-start gap-2.5">
                    <AlertTriangle size={15} className="shrink-0 mt-px" style={{ color: TONE.warn }} />
                    <div className="min-w-0 flex-1 text-[12.5px]">
                      <div className="text-[var(--text-primary)]">
                        {status.dirty} uncommitted file{status.dirty === 1 ? '' : 's'} will block this release.
                      </div>
                      <div className="text-[11.5px] text-[var(--text-muted)]">Commit or stash them first. The release commit should only contain the version bump.</div>
                      <ul className="mt-1.5 flex flex-wrap gap-x-3 gap-y-0.5">
                        {status.dirty_files.map((f) => (
                          <li key={f}>
                            <button className="font-mono text-[11.5px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:underline cursor-pointer" onClick={() => onOpenFile(f)}>
                              {f}
                            </button>
                          </li>
                        ))}
                      </ul>
                    </div>
                    <button className={btnGhost} onClick={onShowChanges}>
                      Source Control
                    </button>
                  </div>
                </Panel>
              )}
              {report ? (
                <Panel className="px-4 py-3">
                  <CheckList checks={report.checks} />
                </Panel>
              ) : (
                <p className="text-[12px] text-[var(--text-muted)]">
                  Not run yet. They also run automatically when you review the release.
                </p>
              )}
              <div className="mt-3">
                <Toggle checked={withTests} onChange={setWithTests} label="Include build & tests" hint="Slower, but catches broken builds before the tag is pushed." />
              </div>
            </Step>
          </div>

          {/* Right rail */}
          <aside className="flex flex-col gap-4 lg:sticky lg:top-4">
            <Panel className="overflow-hidden">
              <div className="px-4 pt-4 pb-3 border-b border-[var(--border)]">
                <div className="flex items-center justify-between">
                  <h2 className="text-[14px] font-semibold text-[var(--text-primary)]">{tag ? `Ship ${tag}` : 'Ship'}</h2>
                  <Badge tone="accent">{mode.title}</Badge>
                </div>
                <p className="mt-1 text-[11.5px] text-[var(--text-muted)] leading-snug">When you confirm, Vireo will:</p>
                <ol className="mt-2 flex flex-col gap-1.5 text-[12px] text-[var(--text-primary)]">
                  {[
                    'Run the readiness checks',
                    versionFiles.length > 0 ? `Set ${selectedFiles} version file${selectedFiles === 1 ? '' : 's'} to ${targetValid ? target : 'the new version'}` : 'Skip version files (none found)',
                    `Commit and tag ${tag || 'the release'}`,
                    status.mode === 'local' ? 'Keep it local (no remote)' : 'Push the commit and tag to origin',
                    mode.finish,
                  ].map((t, i) => (
                    <li key={i} className="flex gap-2">
                      <span className="w-[18px] h-[18px] rounded-full shrink-0 flex items-center justify-center text-[10.5px] font-semibold bg-[var(--bg-active)] text-[var(--text-muted)]">
                        {i + 1}
                      </span>
                      <span className="leading-[18px]">{t}</span>
                    </li>
                  ))}
                </ol>
                {status.mode === 'ci' && status.ci_workflows.length > 0 && (
                  <p className="mt-2 text-[11px] text-[var(--text-dim)] truncate" title={status.ci_workflows.join(', ')}>
                    Workflow: {status.ci_workflows.join(', ')}
                  </p>
                )}
              </div>
              <div className="px-4 py-3 flex flex-col gap-3">
                {status.mode === 'gh' && (
                  <Toggle checked={goPublic} onChange={setGoPublic} label="Publish immediately" hint="Off: the GitHub release is created as a draft." />
                )}
                <ReadinessLine counts={counts} dirty={status.dirty} />
                <button className={`${btnPrimary} h-9 w-full`} disabled={!canAct} onClick={reviewPlan}>
                  {busy === 'plan' ? <Spinner /> : <Rocket size={14} />}
                  {targetValid ? `Review release ${tag}` : 'Choose a valid version'}
                </button>
                <button className={`${btnGhost} w-full`} disabled={!canAct} onClick={askShip} title="Sends step-by-step instructions to the agent panel">
                  <Bot size={13} /> Let an agent ship it
                </button>
                <p className="text-[11px] text-[var(--text-dim)] leading-snug">
                  Review is a dry run. Nothing changes until you confirm. Agent progress shows in Activity below.
                </p>
              </div>
            </Panel>

            <Panel className="overflow-hidden">
              <div className="flex items-center justify-between px-4 h-10 border-b border-[var(--border)]">
                <h2 className="text-[13px] font-semibold text-[var(--text-primary)]">Activity</h2>
                <span className="inline-flex items-center gap-1.5 text-[11px] text-[var(--text-dim)]" title="Updates every few seconds">
                  <span className="w-1.5 h-1.5 rounded-full animate-pulse" style={{ background: TONE.ok }} /> Live
                </span>
              </div>
              <div className="max-h-[360px] overflow-y-auto">
                <ActivityFeed items={activity} onOpen={openActivity} onOpenFile={onOpenFile} onOpenUrl={openUrl} />
              </div>
            </Panel>

            {status.releases.length > 0 && (
              <Panel className="overflow-hidden">
                <div className="px-4 h-10 flex items-center border-b border-[var(--border)]">
                  <h2 className="text-[13px] font-semibold text-[var(--text-primary)]">Past releases</h2>
                </div>
                <ul>
                  {status.releases.map((r, i) => (
                    <li key={r.tag} className="flex items-center gap-2.5 px-4 py-2 border-t border-[var(--border)] first:border-t-0 text-[12.5px]">
                      <Tag size={13} className="shrink-0 text-[var(--text-dim)]" />
                      <span className="font-mono text-[var(--text-primary)]">{r.tag}</span>
                      {i === 0 && <Badge tone="ok">latest</Badge>}
                      {r.prerelease && <Badge tone="warn">pre</Badge>}
                      <span className="ml-auto text-[11px] text-[var(--text-dim)]" title={r.date ?? undefined}>
                        {r.date ? relTime(Date.parse(r.date)) : ''}
                      </span>
                      {status.repo && (
                        <button
                          className="p-1 rounded text-[var(--text-dim)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
                          title="Open on GitHub"
                          onClick={() => openUrl(`https://github.com/${status.repo}/releases/tag/${r.tag}`)}
                        >
                          <ExternalLink size={12} />
                        </button>
                      )}
                    </li>
                  ))}
                </ul>
              </Panel>
            )}
          </aside>
        </div>
      </div>

      {dialog && (
        <ReviewDialog
          plan={dialog.plan}
          outcome={dialog.outcome}
          verify={dialog.verify}
          busy={busy}
          modeLabel={mode.title}
          fromAgent={dialog.fromAgent}
          onConfirm={confirmPlan}
          onVerify={() => void runVerify((dialog.outcome ?? dialog.plan)!.version)}
          onOpenUrl={openUrl}
          onClose={() => setDialog(null)}
        />
      )}
    </div>
  );
};

function Cell({ label, children, highlight }: { label: string; children: React.ReactNode; highlight?: boolean }) {
  return (
    <div className="px-4 py-3.5 min-w-0" style={highlight ? { background: `linear-gradient(135deg, ${tint('accent', 10)}, transparent 75%)` } : undefined}>
      <div className="mb-1 text-[11px] font-medium uppercase tracking-wider text-[var(--text-dim)] truncate">{label}</div>
      {children}
    </div>
  );
}

function Banner({
  tone,
  icon,
  action,
  children,
}: {
  tone: 'accent' | 'warn';
  icon: React.ReactNode;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div
      className="mt-4 flex items-center gap-3 rounded-lg px-4 py-2.5 text-[12.5px] text-[var(--text-primary)]"
      style={{ background: tint(tone, 10), border: `1px solid ${tint(tone, 30)}` }}
    >
      <span className="shrink-0" style={{ color: TONE[tone] }}>
        {icon}
      </span>
      <span className="min-w-0 flex-1 leading-snug">{children}</span>
      {action}
    </div>
  );
}

function Seg({ on, onClick, icon, children }: { on: boolean; onClick: () => void; icon: React.ReactNode; children: React.ReactNode }) {
  return (
    <button
      onClick={onClick}
      className={`inline-flex items-center gap-1.5 h-6 px-2 rounded text-[12px] cursor-pointer ${
        on ? 'bg-[var(--bg-active)] text-[var(--text-primary)]' : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
      }`}
    >
      {icon}
      {children}
    </button>
  );
}

function ReadinessLine({ counts, dirty }: { counts: { ok: number; bad: number; warn: number } | null; dirty: number }) {
  const [tone, text] = counts
    ? counts.bad
      ? (['fail', `${counts.bad} check${counts.bad === 1 ? '' : 's'} failing`] as const)
      : counts.warn
      ? (['warn', `Checks passed with ${counts.warn} warning${counts.warn === 1 ? '' : 's'}`] as const)
      : (['ok', 'All checks passed'] as const)
    : dirty > 0
    ? (['warn', `${dirty} uncommitted file${dirty === 1 ? '' : 's'} will block release`] as const)
    : (['neutral', 'Checks not run yet'] as const);
  return (
    <div className="flex items-center gap-2 text-[12px]" style={{ color: TONE[tone] }}>
      <span className="w-2 h-2 rounded-full shrink-0" style={{ background: TONE[tone] }} />
      {text}
    </div>
  );
}

function DiffList({ diff, target }: { diff: ReleaseFileChange[]; target: string }) {
  if (diff.length === 0) return <p className="mt-3 text-[12px] text-[var(--text-muted)]">Everything is already at {target}.</p>;
  return (
    <div className="mt-3 flex flex-col gap-2">
      {diff.map((f) => (
        <Panel key={f.path} className="overflow-hidden">
          <div className="px-3 py-1.5 border-b border-[var(--border)] text-[11.5px] text-[var(--text-muted)] flex items-center gap-2">
            <FileIcon path={f.path} size={13} />
            {f.path}
          </div>
          {f.changes.map((c) => (
            <div key={c.line} className="font-mono text-[11.5px] leading-relaxed">
              <div className="px-3 bg-[var(--removed-bg)] text-[var(--removed-fg)] truncate">
                <span className="inline-block w-10 opacity-60">{c.line}</span>− {c.before.trim()}
              </div>
              <div className="px-3 bg-[var(--added-bg)] text-[var(--added-fg)] truncate">
                <span className="inline-block w-10 opacity-60">{c.line}</span>+ {c.after.trim()}
              </div>
            </div>
          ))}
        </Panel>
      ))}
    </div>
  );
}
