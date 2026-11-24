import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertTriangle,
  Bot,
  ChevronRight,
  Code2,
  ExternalLink,
  Eye,
  FileText,
  Pencil,
  Plus,
  RefreshCw,
  Rocket,
  ShieldCheck,
  Sparkles,
  Tag,
  XCircle,
} from 'lucide-react';
import { api } from '../../api';
import type {
  ReleaseActivity,
  ReleaseOutcome,
  ReleaseReport,
  ReleaseStatus,
  ReleaseTag,
} from '../../types';
import type { LiteTheme } from '../../themes';
import { FileIcon } from '../FileIcon';
import { MarkdownMessage } from '../MarkdownView';
import { activityTitle } from './ActivityFeed';
import { ReviewDialog } from './ReviewDialog';
import { CheckList, Panel, Spinner, TONE, Toggle, btnGhost, btnPrimary, btnQuiet, isLock, splitPath, tint } from './ui';

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

type Notes = { path: string; markdown: string; draft: boolean; commits?: number; tagExists?: boolean; warnings?: string[] } | null;
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
  const [composing, setComposing] = useState(false);
  const [withTests, setWithTests] = useState(true);
  const [report, setReport] = useState<ReleaseReport | null>(null);
  const changelog = false;
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
      const exists = status?.releases.some((r) => r.tag === tag) ?? false;
      const warnings = exists ? [`Tag ${tag} already exists. Choose a newer version for a new release.`] : undefined;
      setNotes({ path: notesPath, markdown: f.content, draft: true, tagExists: exists, warnings });
    } catch {
      try {
        const r = await api.releaseNotes({ version: target });
        setNotes({ path: notesPath, markdown: r.markdown, draft: false, commits: r.commits, tagExists: r.tag_exists, warnings: r.warnings });
      } catch {
        setNotes(null);
      }
    }
  }, [notesPath, status?.releases, tag, target, targetValid]);

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

  const runChecks = () =>
    run('checks', () => api.releasePreflight({ version: target, skip_tests: !withTests }), setReport);

  const saveNotes = () =>
    run(
      'notes',
      () => api.releaseNotes({ version: target, write: true }),
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
  const canAct = targetValid && !busy;
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

  return (
    <div className="flex-1 min-h-0 overflow-y-auto bg-[var(--bg-base)] select-text">
      <div className="mx-auto w-full max-w-[860px] px-8 py-8">
        {/* Header */}
        <header className="flex items-center gap-3 pb-4 border-b border-[var(--border)]">
          <div className="min-w-0 flex-1">
            <h1 className="text-[20px] font-semibold tracking-tight text-[var(--text-primary)]">Releases</h1>
            <p className="text-[12.5px] text-[var(--text-muted)] truncate">
              {status.repo ?? 'This repository'} · {status.version} on {status.branch ?? 'detached HEAD'}
              {status.commits_since > 0 && ` · ${status.commits_since} unreleased commit${status.commits_since === 1 ? '' : 's'}`}
            </p>
          </div>
          <button className={btnQuiet} onClick={() => void Promise.all([load(), loadNotes(), pollActivity()])} title="Refresh">
            <RefreshCw size={13} />
          </button>
          {status.repo && (
            <button className={btnQuiet} onClick={() => openUrl(`https://github.com/${status.repo}/releases`)} title="Open on GitHub">
              <ExternalLink size={13} /> GitHub
            </button>
          )}
          <button className={btnPrimary} onClick={() => setComposing((v) => !v)}>
            <Plus size={13} /> {composing ? 'Close' : 'New release'}
          </button>
        </header>

        {/* Live banners: what an agent (or the terminal) is doing right now */}
        {running && (
          <Banner tone="accent" icon={<Spinner size={15} />}>
            <b>{running.source === 'agent' ? 'An agent is' : running.source === 'cli' ? 'The terminal is' : 'Vireo is'} releasing {running.tag}.</b>{' '}
            The page updates as each step finishes.
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
            <b>An agent prepared {agentPlan.tag}.</b> Nothing has changed yet. Review the plan, then confirm here or tell the agent to go ahead.
          </Banner>
        )}
        {status.drift.length > 0 && (
          <Banner tone="warn" icon={<AlertTriangle size={15} />}>
            Version files disagree ({status.drift.join(', ')}). Releasing aligns them all to {targetValid ? target : 'the new version'}.
          </Banner>
        )}

        {/* Composer */}
        {composing && (
          <Panel className="mt-5 overflow-hidden">
            <div className="px-5 pt-4 pb-4">
              <div className="flex items-baseline justify-between gap-3">
                <h2 className="text-[13px] font-semibold text-[var(--text-primary)]">New release</h2>
                <span className="text-[11.5px] text-[var(--text-muted)]">{mode.title}</span>
              </div>
              <p className="mt-0.5 text-[12px] text-[var(--text-muted)]">{mode.text}</p>
              {status.mode === 'ci' && status.ci_notes_warning && (
                <p className="mt-2 flex items-start gap-1.5 text-[12px] leading-snug" style={{ color: TONE.warn }}>
                  <AlertTriangle size={13} className="mt-0.5 shrink-0" />
                  <span>{status.ci_notes_warning}</span>
                </p>
              )}

              <div className="mt-4 flex flex-wrap items-center gap-3">
                <div className="inline-flex rounded-md border border-[var(--border)] overflow-hidden divide-x divide-[var(--border)]">
                  {BUMPS.map((b) => {
                    const sel = bump === b.key;
                    return (
                      <button
                        key={b.key}
                        onClick={() => setBump(b.key)}
                        title={b.hint}
                        className={`h-8 px-3 text-[12.5px] cursor-pointer transition-colors ${
                          sel ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium' : 'bg-[var(--bg-base)] text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                        }`}
                      >
                        {b.label} <span className="font-mono text-[11.5px] opacity-70">{status.bumps[b.key]}</span>
                      </button>
                    );
                  })}
                </div>
                <input
                  value={custom}
                  onFocus={() => setBump('custom')}
                  onChange={(e) => {
                    setCustom(e.target.value);
                    setBump('custom');
                  }}
                  placeholder="custom"
                  spellCheck={false}
                  className="w-[130px] h-8 px-2.5 rounded-md bg-[var(--bg-input)] border text-[12.5px] font-mono text-[var(--text-primary)] outline-none focus:border-[var(--accent)]"
                  style={{ borderColor: bump === 'custom' ? 'var(--accent)' : 'var(--border)' }}
                />
              </div>
              <p className="mt-2 text-[11.5px] text-[var(--text-muted)]">
                {bump === 'custom' && custom.trim() && !targetValid ? (
                  <span style={{ color: TONE.fail }}>Use MAJOR.MINOR.PATCH, e.g. 1.2.0</span>
                ) : (
                  <>
                    {tag ? <span className="font-mono text-[var(--text-primary)]">{tag}</span> : 'Choose a version'} · {status.suggested.reason}
                  </>
                )}
              </p>

              {/* Notes */}
              <div className="mt-4 rounded-md border border-[var(--border)] overflow-hidden">
                <div className="flex items-center gap-1 px-2 h-9 border-b border-[var(--border)] bg-[var(--bg-base)]">
                  <span className="px-1 text-[12px] font-medium text-[var(--text-primary)]">Notes</span>
                  <span className="text-[11.5px] text-[var(--text-dim)]">{notes ? (notes.draft ? 'draft saved' : 'auto preview') : ''}</span>
                  <div className="ml-auto flex items-center gap-1">
                    <Seg on={notesView === 'preview'} onClick={() => setNotesView('preview')} icon={<Eye size={12} />}>
                      Preview
                    </Seg>
                    <Seg on={notesView === 'source'} onClick={() => setNotesView('source')} icon={<Code2 size={12} />}>
                      Markdown
                    </Seg>
                  </div>
                </div>
                {notes?.warnings && notes.warnings.filter((w) => w !== status.ci_notes_warning).length > 0 && (
                  <div
                    className="px-4 py-2 border-b border-[var(--border)] flex flex-col gap-1 text-[11.5px] leading-snug"
                    style={{ background: tint('warn', 8), color: TONE.warn }}
                  >
                    {notes.warnings
                      .filter((w) => w !== status.ci_notes_warning)
                      .map((w, i) => (
                        <div key={i} className="flex items-start gap-1.5">
                          <AlertTriangle size={12} className="mt-0.5 shrink-0" />
                          <span>{w}</span>
                        </div>
                      ))}
                  </div>
                )}
                <div className="max-h-[320px] overflow-y-auto px-5 py-4">
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
                <div className="flex items-center gap-1.5 px-2 h-10 border-t border-[var(--border)] bg-[var(--bg-base)]">
                  <button className={btnQuiet} disabled={!canAct} onClick={askNotes} title="Sends instructions to the agent panel">
                    <Sparkles size={13} /> Write with agent
                  </button>
                  {notes?.draft ? (
                    <button className={btnQuiet} onClick={() => onOpenFile(notes.path)}>
                      <Pencil size={13} /> Edit
                    </button>
                  ) : (
                    <button className={btnQuiet} disabled={!canAct} onClick={saveNotes}>
                      {busy === 'notes' ? <Spinner /> : <FileText size={13} />} Save draft &amp; edit
                    </button>
                  )}
                </div>
              </div>

              {/* Version files */}
              {versionFiles.length > 0 && (
                <details className="mt-3 group">
                  <summary className="cursor-pointer text-[12px] text-[var(--text-muted)] hover:text-[var(--text-primary)] select-none">
                    {selectedFiles} of {versionFiles.length} version file{versionFiles.length === 1 ? '' : 's'} will change
                  </summary>
                  <ul className="mt-2 flex flex-col">
                    {versionFiles.map((m) => {
                      const off = skipped.has(m.path);
                      const { name } = splitPath(m.path);
                      return (
                        <li key={m.path} className="flex items-center gap-2 py-1 text-[12.5px]" style={{ opacity: off ? 0.5 : 1 }}>
                          <input
                            type="checkbox"
                            checked={!off}
                            onChange={() => toggleFile(m.path)}
                            className="cursor-pointer"
                            style={{ accentColor: 'var(--accent)' }}
                            aria-label={`Include ${m.path}`}
                          />
                          <FileIcon path={m.path} size={14} />
                          <button className="text-[var(--text-primary)] hover:underline cursor-pointer truncate" onClick={() => onOpenFile(m.path)} title={m.path}>
                            {name}
                          </button>
                          <span className="ml-auto font-mono text-[11.5px] text-[var(--text-muted)] whitespace-nowrap">
                            {m.version ?? '—'}
                            {!off && targetValid && m.version != null && m.version !== target && <span style={{ color: TONE.ok }}> → {target}</span>}
                          </span>
                        </li>
                      );
                    })}
                  </ul>
                </details>
              )}

              {/* Readiness */}
              {status.dirty > 0 && !report && (
                <p className="mt-3 text-[12px]" style={{ color: TONE.warn }}>
                  {status.dirty} uncommitted file{status.dirty === 1 ? '' : 's'} will block this release.{' '}
                  <button className="underline cursor-pointer" onClick={onShowChanges}>
                    Open Source Control
                  </button>
                </p>
              )}
              {report && (
                <div className="mt-3 rounded-md border border-[var(--border)] px-4 py-3">
                  <CheckList checks={report.checks} dense />
                </div>
              )}
            </div>

            <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-5 py-3 border-t border-[var(--border)] bg-[var(--bg-base)]">
              <Toggle checked={withTests} onChange={setWithTests} label="Build & tests" />
              {status.mode === 'gh' && <Toggle checked={goPublic} onChange={setGoPublic} label="Publish now" hint="" />}
              <ReadinessLine counts={counts} dirty={status.dirty} />
              <div className="ml-auto flex items-center gap-1.5">
                <button className={btnGhost} disabled={!canAct} onClick={runChecks}>
                  {busy === 'checks' ? <Spinner /> : <ShieldCheck size={13} />} Run checks
                </button>
                <button className={btnGhost} disabled={!canAct} onClick={askShip} title="Sends step-by-step instructions to the agent panel">
                  <Bot size={13} /> Let an agent ship it
                </button>
                <button className={btnPrimary} disabled={!canAct} onClick={reviewPlan}>
                  {busy === 'plan' ? <Spinner /> : <Rocket size={13} />} Review release
                </button>
              </div>
            </div>
          </Panel>
        )}

        {/* Release list */}
        <section className="mt-6">
          {status.releases.length === 0 ? (
            <div className="py-12 text-center text-[12.5px] text-[var(--text-muted)]">
              <Tag size={22} className="mx-auto mb-2 text-[var(--text-dim)]" />
              No releases yet.
            </div>
          ) : (
            <ul>
              {status.releases.map((r, i) => (
                <ReleaseRow key={r.tag} release={r} latest={i === 0 && !r.prerelease} repo={status.repo} theme={theme} onOpenUrl={openUrl} />
              ))}
            </ul>
          )}
        </section>
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
      ? (['warn', `Passed with ${counts.warn} warning${counts.warn === 1 ? '' : 's'}`] as const)
      : (['ok', 'All checks passed'] as const)
    : dirty > 0
    ? (['warn', 'Uncommitted changes'] as const)
    : (['neutral', 'Checks not run'] as const);
  return (
    <div className="flex items-center gap-2 text-[12px]" style={{ color: TONE[tone] }}>
      <span className="w-2 h-2 rounded-full shrink-0" style={{ background: TONE[tone] }} />
      {text}
    </div>
  );
}

function ReleaseRow({
  release: r,
  latest,
  repo,
  theme,
  onOpenUrl,
}: {
  release: ReleaseTag;
  latest: boolean;
  repo: string | null;
  theme: LiteTheme;
  onOpenUrl: (url: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [body, setBody] = useState<{ markdown: string; source: string } | null>(null);
  const [failed, setFailed] = useState(false);

  const toggle = () => {
    const next = !open;
    setOpen(next);
    if (next && !body) {
      setFailed(false);
      api
        .releaseBody(r.tag)
        .then(setBody)
        .catch(() => setFailed(true));
    }
  };

  return (
    <li className="border-b border-[var(--border)] last:border-b-0">
      <div
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={toggle}
        onKeyDown={(e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), toggle())}
        className="flex gap-6 py-4 cursor-pointer hover:bg-[var(--bg-hover)] rounded-md px-2 -mx-2"
      >
        <div className="w-[130px] shrink-0 text-[12px] text-[var(--text-muted)] leading-6" title={r.date ?? undefined}>
          {r.date ? new Date(r.date).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' }) : ''}
          {r.commit && <div className="font-mono text-[11px] text-[var(--text-dim)]">{r.commit.slice(0, 7)}</div>}
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 leading-6">
            <ChevronRight size={14} className="shrink-0 text-[var(--text-dim)] transition-transform" style={{ transform: open ? 'rotate(90deg)' : undefined }} />
            <span className="font-mono text-[15px] font-semibold text-[var(--text-primary)]">{r.tag}</span>
            {latest && (
              <span className="text-[11px] px-1.5 rounded-full border leading-[18px]" style={{ color: TONE.ok, borderColor: tint('ok', 40) }}>
                Latest
              </span>
            )}
            {r.prerelease && (
              <span className="text-[11px] px-1.5 rounded-full border leading-[18px]" style={{ color: TONE.warn, borderColor: tint('warn', 40) }}>
                Pre-release
              </span>
            )}
            {repo && (
              <button
                className="ml-auto p-1 rounded text-[var(--text-dim)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-active)] cursor-pointer"
                title="Open on GitHub"
                onClick={(e) => {
                  e.stopPropagation();
                  onOpenUrl(`https://github.com/${repo}/releases/tag/${r.tag}`);
                }}
              >
                <ExternalLink size={13} />
              </button>
            )}
          </div>
        </div>
      </div>
      {open && (
        <div className="mb-4 ml-[154px] rounded-md border border-[var(--border)] px-5 py-4 select-text">
          {failed ? (
            <span className="text-[12px]" style={{ color: TONE.fail }}>
              Could not load the release notes.
            </span>
          ) : !body ? (
            <span className="flex items-center gap-2 text-[12px] text-[var(--text-muted)]">
              <Spinner /> Loading release notes…
            </span>
          ) : body.markdown ? (
            <MarkdownMessage content={body.markdown} theme={theme} variant="document" />
          ) : (
            <span className="text-[12px] text-[var(--text-muted)]">No release notes for {r.tag}.</span>
          )}
        </div>
      )}
    </li>
  );
}
