import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertTriangle,
  Check,
  ChevronDown,
  ChevronRight,
  CircleDashed,
  ExternalLink,
  FileText,
  Loader2,
  RefreshCw,
  Rocket,
  ShieldCheck,
  Sparkles,
  Tag,
  X,
} from 'lucide-react';
import { api } from '../api';
import type {
  ReleaseCheck,
  ReleaseFileChange,
  ReleaseOutcome,
  ReleaseReport,
  ReleaseStatus,
} from '../types';
import { FileIcon } from './FileIcon';

interface ReleasePaneProps {
  active: boolean;
  fsVersion: number;
  onOpenFile: (path: string, line?: number, col?: number, colEnd?: number) => void;
  onAskAgent: (text: string) => void;
  onRefreshGit: () => Promise<void>;
  onToast: (msg: string, isError?: boolean) => void;
}

type BumpKey = 'patch' | 'minor' | 'major' | 'prerelease' | 'custom';
const BUMPS: { key: Exclude<BumpKey, 'custom'>; label: string }[] = [
  { key: 'patch', label: 'Patch' },
  { key: 'minor', label: 'Minor' },
  { key: 'major', label: 'Major' },
  { key: 'prerelease', label: 'Pre-release' },
];

const MODE_TEXT: Record<ReleaseStatus['mode'], string> = {
  ci: 'Pushing the tag triggers CI to build and publish',
  gh: 'Creates a draft GitHub release with `gh`',
  local: 'Commit and tag only (no GitHub remote detected)',
};

function StatusIcon({ status }: { status: ReleaseCheck['status'] }) {
  if (status === 'ok') return <Check size={13} className="text-[var(--added-fg)] shrink-0 mt-0.5" />;
  if (status === 'fail') return <X size={13} className="text-[var(--removed-fg)] shrink-0 mt-0.5" />;
  if (status === 'warn') return <AlertTriangle size={13} className="text-[var(--modified-fg)] shrink-0 mt-0.5" />;
  return <CircleDashed size={13} className="text-[var(--text-dim)] shrink-0 mt-0.5" />;
}

function CheckList({ checks }: { checks: ReleaseCheck[] }) {
  return (
    <ul className="flex flex-col gap-1">
      {checks.map((c) => (
        <li key={c.id + c.label} className="flex gap-1.5 text-[12px] leading-snug">
          <StatusIcon status={c.status} />
          <span className="min-w-0">
            <span className="text-[var(--text-primary)]">{c.label}</span>
            {c.detail && <span className="text-[var(--text-muted)]"> — {c.detail}</span>}
          </span>
        </li>
      ))}
    </ul>
  );
}

function Section({
  title,
  children,
  action,
}: {
  title: string;
  children: React.ReactNode;
  action?: React.ReactNode;
}) {
  return (
    <section className="px-3 py-2.5 border-b border-[var(--border)]">
      <div className="flex items-center justify-between mb-1.5">
        <h3 className="text-[11px] font-semibold uppercase tracking-wider text-[var(--text-muted)]">{title}</h3>
        {action}
      </div>
      {children}
    </section>
  );
}

const btn =
  'inline-flex items-center justify-center gap-1.5 rounded-md px-2.5 py-1 text-[12px] font-medium cursor-pointer transition-colors disabled:opacity-40 disabled:cursor-default';
const btnGhost = `${btn} border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)]`;
const btnPrimary = `${btn} bg-[var(--accent)] text-[var(--accent-fg)] hover:bg-[var(--accent-hover)]`;

export const ReleasePane: React.FC<ReleasePaneProps> = ({
  active,
  fsVersion,
  onOpenFile,
  onAskAgent,
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
  const [notesInfo, setNotesInfo] = useState<string | null>(null);
  const [goPublic, setGoPublic] = useState(false);
  const [plan, setPlan] = useState<ReleaseOutcome | null>(null);
  const [outcome, setOutcome] = useState<ReleaseOutcome | null>(null);
  const [verify, setVerify] = useState<ReleaseReport | null>(null);
  const [showFiles, setShowFiles] = useState(true);
  const loaded = useRef(false);

  const load = useCallback(async () => {
    try {
      const s = await api.releaseStatus();
      setStatus(s);
      setError(null);
      // First load: preselect the suggested bump.
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
    const t = setTimeout(load, loaded.current ? 800 : 0);
    loaded.current = true;
    return () => clearTimeout(t);
  }, [active, fsVersion, load]);

  const target = useMemo(() => {
    if (!status || !bump) return '';
    return bump === 'custom' ? custom.trim() : status.bumps[bump] ?? '';
  }, [status, bump, custom]);

  const only = useMemo(() => {
    if (!status) return [];
    const files = status.manifests.filter((m) => !/lock\.json$|Cargo\.lock$/.test(m.path));
    return skipped.size === 0 ? [] : files.filter((m) => !skipped.has(m.path)).map((m) => m.path);
  }, [status, skipped]);

  // Any input change invalidates what was computed for the previous choice.
  useEffect(() => {
    setDiff(null);
    setReport(null);
    setPlan(null);
  }, [target, skipped]);

  const run = async <T,>(name: string, fn: () => Promise<T>, done: (v: T) => void) => {
    setBusy(name);
    try {
      done(await fn());
    } catch (e: unknown) {
      onToast(e instanceof Error ? e.message : String(e), true);
    } finally {
      setBusy(null);
    }
  };

  const previewBump = () =>
    run('diff', () => api.releaseBump(target, { dry_run: true, only }), (r) => setDiff(r.files));

  const runChecks = () =>
    run('checks', () => api.releasePreflight({ version: target, skip_tests: !withTests }), setReport);

  const writeNotes = async (): Promise<string | null> => {
    try {
      const r = await api.releaseNotes({ version: target, write: true, changelog });
      setNotesInfo(`${r.commits} commit(s) · ${r.messages.join(' · ')}`);
      void load();
      return r.written;
    } catch (e: unknown) {
      onToast(e instanceof Error ? e.message : String(e), true);
      return null;
    }
  };

  const generateNotes = () =>
    run('notes', writeNotes, (p) => {
      if (p) onOpenFile(p);
    });

  const polish = () =>
    run('polish', writeNotes, (p) => {
      if (!p) return;
      onOpenFile(p);
      onAskAgent(
        `Polish the release notes for ${status?.tag_prefix ?? 'v'}${target} in \`${p}\`. ` +
          'Keep every statement grounded in the commit list already in that file (do not invent or drop changes), ' +
          'add a 2-3 sentence **Highlights** summary at the top, keep breaking changes prominent, then save the file. ' +
          'Do not tag, push or publish anything.'
      );
    });

  const reviewPlan = () =>
    run(
      'plan',
      () => api.releasePublish({ version: target, dry_run: true, only, publish: goPublic }),
      (p) => {
        setOutcome(null);
        setVerify(null);
        setPlan(p);
      }
    );

  const release = () =>
    run(
      'release',
      () => api.releasePublish({ version: target, skip_tests: !withTests, only, publish: goPublic }),
      async (o) => {
        setPlan(null);
        setOutcome(o);
        if (o.ok) onToast(`Released ${o.tag}`);
        else onToast('Release stopped: see the pane for details', true);
        if (o.ok) {
          // The released version is the new baseline: let the next suggestion be recomputed.
          setBump(null);
          setCustom('');
        }
        await onRefreshGit();
        void load();
      }
    );

  const runVerify = () =>
    run('verify', () => api.releaseVerify({ version: outcome?.version ?? target }), setVerify);

  const toggleFile = (p: string) =>
    setSkipped((cur) => {
      const n = new Set(cur);
      if (n.has(p)) n.delete(p);
      else n.add(p);
      return n;
    });

  if (!status) {
    return (
      <div className="flex-1 flex items-center justify-center text-[12px] text-[var(--text-muted)] p-4 text-center">
        {error ? (
          <div className="flex flex-col items-center gap-2">
            <span className="text-[var(--removed-fg)]">{error}</span>
            <button className={btnGhost} onClick={load}>
              <RefreshCw size={13} /> Retry
            </button>
          </div>
        ) : (
          <Loader2 size={16} className="animate-spin" />
        )}
      </div>
    );
  }

  const versionFiles = status.manifests.filter((m) => !/lock\.json$|Cargo\.lock$/.test(m.path));
  const lockFiles = status.manifests.length - versionFiles.length;
  const canAct = !!target && !busy;
  const steps = outcome ?? plan;

  return (
    <div className="flex-1 min-h-0 flex flex-col overflow-hidden">
      <div className="h-9 px-3 flex items-center justify-between border-b border-[var(--border)] shrink-0">
        <span className="text-[11px] font-semibold uppercase tracking-wider text-[var(--text-muted)]">Releases</span>
        <button
          className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
          title="Refresh"
          onClick={load}
        >
          <RefreshCw size={13} />
        </button>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto select-text">
        {/* Current version */}
        <Section title="Current version">
          <div className="flex items-center gap-2">
            <Tag size={15} className="text-[var(--accent)]" />
            <span className="text-[15px] font-semibold text-[var(--text-primary)]">{status.version}</span>
            <span className="text-[11px] text-[var(--text-muted)]">
              {status.latest_tag ? `latest tag ${status.latest_tag}` : 'no release tag yet'}
            </span>
          </div>
          <div className="mt-1 text-[11px] text-[var(--text-muted)]">
            {status.commits_since} commit(s) since {status.latest_stable ?? 'start'}
            {status.breaking > 0 && <span className="text-[var(--removed-fg)]"> · {status.breaking} breaking</span>}
            {status.feats > 0 && <span> · {status.feats} feat</span>}
            {status.fixes > 0 && <span> · {status.fixes} fix</span>}
            {status.branch && <span> · on {status.branch}</span>}
          </div>
          {status.stacks.length > 0 && (
            <div className="mt-1.5 flex flex-wrap gap-1">
              {status.stacks.map((s) => (
                <span key={s} className="px-1.5 py-px rounded bg-[var(--bg-elevated)] text-[10px] text-[var(--text-muted)]">
                  {s}
                </span>
              ))}
            </div>
          )}
          {status.drift.length > 0 && (
            <div className="mt-1.5 flex gap-1.5 text-[11px] text-[var(--modified-fg)]">
              <AlertTriangle size={12} className="shrink-0 mt-0.5" />
              Version files disagree ({status.drift.join(', ')}); the bump aligns them.
            </div>
          )}
        </Section>

        {/* Version bump */}
        <Section title="Next version">
          <div className="flex flex-wrap gap-1.5">
            {BUMPS.map((b) => (
              <button
                key={b.key}
                onClick={() => setBump(b.key)}
                className={`${btn} border ${
                  bump === b.key
                    ? 'border-[var(--accent)] bg-[var(--bg-active)] text-[var(--text-primary)]'
                    : 'border-[var(--border)] text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                }`}
                title={status.suggested.reason}
              >
                {b.label} <span className="font-mono text-[11px]">{status.bumps[b.key]}</span>
                {status.suggested.version === status.bumps[b.key] && (
                  <span className="text-[9px] uppercase text-[var(--accent)]">suggested</span>
                )}
              </button>
            ))}
          </div>
          <input
            value={custom}
            onFocus={() => setBump('custom')}
            onChange={(e) => {
              setCustom(e.target.value);
              setBump('custom');
            }}
            placeholder="Custom, e.g. 1.0.0-rc.1"
            spellCheck={false}
            className={`mt-2 w-full px-2 py-1 rounded-md bg-[var(--bg-input)] border text-[12px] font-mono text-[var(--text-primary)] outline-none focus:border-[var(--accent)] ${
              bump === 'custom' ? 'border-[var(--accent)]' : 'border-[var(--border)]'
            }`}
          />
          <p className="mt-1 text-[11px] text-[var(--text-dim)]">{status.suggested.reason}</p>
        </Section>

        {/* Version files */}
        <Section
          title={`Version files (${versionFiles.length}${lockFiles ? ` + ${lockFiles} lock` : ''})`}
          action={
            <button
              className="p-0.5 text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
              onClick={() => setShowFiles((v) => !v)}
            >
              {showFiles ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
            </button>
          }
        >
          {versionFiles.length === 0 && (
            <p className="text-[11px] text-[var(--text-muted)]">
              No version files found: only the tag is created. Add custom ones in <code>.vireo/release.json</code>.
            </p>
          )}
          {showFiles &&
            versionFiles.map((m) => (
              <label key={m.path} className="flex items-center gap-1.5 py-0.5 text-[12px] cursor-pointer">
                <input type="checkbox" checked={!skipped.has(m.path)} onChange={() => toggleFile(m.path)} />
                <FileIcon path={m.path} size={13} />
                <button
                  type="button"
                  className="truncate text-left text-[var(--text-primary)] hover:underline cursor-pointer"
                  title={m.note ?? m.path}
                  onClick={(e) => {
                    e.preventDefault();
                    onOpenFile(m.path);
                  }}
                >
                  {m.path}
                </button>
                <span
                  className={`ml-auto font-mono text-[11px] shrink-0 ${
                    m.version && m.version !== status.version ? 'text-[var(--modified-fg)]' : 'text-[var(--text-muted)]'
                  }`}
                >
                  {m.version ?? '—'}
                </span>
              </label>
            ))}
          {versionFiles.length > 0 && (
            <button className={`${btnGhost} mt-2`} disabled={!canAct} onClick={previewBump}>
              {busy === 'diff' ? <Loader2 size={13} className="animate-spin" /> : null} Preview changes
            </button>
          )}
          {diff && (
            <div className="mt-2 flex flex-col gap-1.5">
              {diff.length === 0 && <p className="text-[11px] text-[var(--text-muted)]">Everything already at {target}.</p>}
              {diff.map((f) => (
                <div key={f.path} className="rounded border border-[var(--border)] overflow-hidden">
                  <div className="px-2 py-0.5 bg-[var(--bg-elevated)] text-[11px] text-[var(--text-muted)] truncate">
                    {f.path}
                  </div>
                  {f.changes.map((c) => (
                    <div key={c.line} className="font-mono text-[11px] leading-snug">
                      <div className="px-2 bg-[var(--removed-bg)] text-[var(--removed-fg)] truncate">
                        {c.line}: − {c.before}
                      </div>
                      <div className="px-2 bg-[var(--added-bg)] text-[var(--added-fg)] truncate">
                        {c.line}: + {c.after}
                      </div>
                    </div>
                  ))}
                </div>
              ))}
            </div>
          )}
        </Section>

        {/* Preflight */}
        <Section title="Readiness">
          <div className="flex items-center gap-2">
            <button className={btnGhost} disabled={!canAct} onClick={runChecks}>
              {busy === 'checks' ? <Loader2 size={13} className="animate-spin" /> : <ShieldCheck size={13} />} Run checks
            </button>
            <label className="flex items-center gap-1 text-[11px] text-[var(--text-muted)] cursor-pointer">
              <input type="checkbox" checked={withTests} onChange={(e) => setWithTests(e.target.checked)} />
              include build &amp; tests
            </label>
          </div>
          {report && (
            <div className="mt-2">
              <CheckList checks={report.checks} />
            </div>
          )}
        </Section>

        {/* Notes */}
        <Section title="Release notes">
          <div className="flex flex-wrap items-center gap-1.5">
            <button className={btnGhost} disabled={!canAct} onClick={generateNotes}>
              {busy === 'notes' ? <Loader2 size={13} className="animate-spin" /> : <FileText size={13} />} Generate &amp; edit
            </button>
            <button className={btnGhost} disabled={!canAct} onClick={polish} title="Writes the draft, then asks the agent to polish it">
              {busy === 'polish' ? <Loader2 size={13} className="animate-spin" /> : <Sparkles size={13} />} Polish with agent
            </button>
          </div>
          <label className="mt-1.5 flex items-center gap-1 text-[11px] text-[var(--text-muted)] cursor-pointer">
            <input type="checkbox" checked={changelog} onChange={(e) => setChangelog(e.target.checked)} />
            also prepend to CHANGELOG.md
          </label>
          <p className="mt-1 text-[11px] text-[var(--text-dim)]">
            {status.notes_exists
              ? `Using ${status.notes_path} as the release body.`
              : `Without a draft, notes are generated from commits at release time.`}
            {notesInfo && <span> {notesInfo}</span>}
          </p>
        </Section>

        {/* Results */}
        {steps && (
          <Section title={outcome ? (outcome.ok ? 'Released' : 'Release stopped') : 'Plan (nothing changed yet)'}>
            <CheckList
              checks={[
                ...steps.preflight.filter((c) => c.status === 'warn' || c.status === 'fail'),
                ...steps.steps,
              ]}
            />
            {!steps.ok && !outcome && (
              <p className="mt-1.5 text-[11px] text-[var(--removed-fg)]">Fix the failed checks before releasing.</p>
            )}
            {outcome?.url && (
              <button className={`${btnGhost} mt-2`} onClick={() => api.openExternalUrl(outcome.url!)}>
                <ExternalLink size={13} /> Open draft release
              </button>
            )}
            {outcome && (
              <button className={`${btnGhost} mt-2 ml-1.5`} disabled={!!busy} onClick={runVerify}>
                {busy === 'verify' ? <Loader2 size={13} className="animate-spin" /> : <ShieldCheck size={13} />} Verify
              </button>
            )}
            {verify && (
              <div className="mt-2">
                <CheckList checks={verify.checks} />
              </div>
            )}
          </Section>
        )}
      </div>

      {/* Footer actions */}
      <div className="shrink-0 border-t border-[var(--border)] px-3 py-2.5 flex flex-col gap-2 bg-[var(--bg-sidebar)]">
        <p className="text-[11px] text-[var(--text-muted)]">{MODE_TEXT[status.mode]}</p>
        {status.mode === 'gh' && (
          <label className="flex items-center gap-1 text-[11px] text-[var(--text-muted)] cursor-pointer">
            <input type="checkbox" checked={goPublic} onChange={(e) => setGoPublic(e.target.checked)} />
            publish immediately (otherwise saved as a draft)
          </label>
        )}
        <div className="flex gap-1.5">
          {!plan || outcome ? (
            <button className={`${btnPrimary} flex-1`} disabled={!canAct} onClick={reviewPlan}>
              {busy === 'plan' ? <Loader2 size={13} className="animate-spin" /> : <Rocket size={13} />}
              {target ? `Review release ${status.tag_prefix}${target}` : 'Pick a version'}
            </button>
          ) : (
            <>
              <button className={btnGhost} disabled={!!busy} onClick={() => setPlan(null)}>
                Cancel
              </button>
              <button className={`${btnPrimary} flex-1`} disabled={!plan.ok || !!busy} onClick={release}>
                {busy === 'release' ? <Loader2 size={13} className="animate-spin" /> : <Rocket size={13} />}
                Confirm &amp; release {plan.tag}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
};
